use alloc::{string::ToString, vec::Vec};
use core::ops::Range;

use anyhow::{bail, Result};
use object::{elf, Endian as _, Object as _, ObjectSection as _, ObjectSymbol as _};
use rabbitizer::{
    abi::Abi,
    operands::{ValuedOperand, IU16},
    registers_meta::Register,
    IsaExtension, IsaVersion, Vram,
};

use crate::{
    arch::Arch,
    diff::{display::InstructionPart, DiffObjConfig, MipsAbi, MipsInstrCategory},
    obj::{
        InstructionArg, InstructionArgValue, InstructionRef, Relocation, RelocationFlags,
        ResolvedInstructionRef, ResolvedRelocation, ScannedInstruction,
    },
};

#[derive(Debug)]
pub struct ArchMips {
    pub endianness: object::Endianness,
    pub abi: Abi,
    pub isa_extension: Option<IsaExtension>,
    pub ri_gp_value: i32,
}

const EF_MIPS_ABI: u32 = 0x0000F000;
const EF_MIPS_MACH: u32 = 0x00FF0000;

const EF_MIPS_MACH_ALLEGREX: u32 = 0x00840000;
const EF_MIPS_MACH_5900: u32 = 0x00920000;

const R_MIPS15_S3: u32 = 119;

impl ArchMips {
    pub fn new(object: &object::File) -> Result<Self> {
        let mut abi = Abi::O32;
        let mut isa_extension = None;
        match object.flags() {
            object::FileFlags::None => {}
            object::FileFlags::Elf { e_flags, .. } => {
                abi = match e_flags & EF_MIPS_ABI {
                    elf::EF_MIPS_ABI_O32 | elf::EF_MIPS_ABI_O64 => Abi::O32,
                    elf::EF_MIPS_ABI_EABI32 | elf::EF_MIPS_ABI_EABI64 => Abi::N32,
                    _ => {
                        if e_flags & elf::EF_MIPS_ABI2 != 0 {
                            Abi::N32
                        } else {
                            Abi::O32
                        }
                    }
                };
                isa_extension = match e_flags & EF_MIPS_MACH {
                    EF_MIPS_MACH_ALLEGREX => Some(IsaExtension::R4000ALLEGREX),
                    EF_MIPS_MACH_5900 => Some(IsaExtension::R5900EE),
                    _ => None,
                };
            }
            _ => bail!("Unsupported MIPS file flags"),
        }

        // Parse the ri_gp_value stored in .reginfo to be able to correctly
        // calculate R_MIPS_GPREL16 relocations later. The value is stored
        // 0x14 bytes into .reginfo (on 32 bit platforms)
        let ri_gp_value = object
            .section_by_name(".reginfo")
            .and_then(|section| section.data().ok())
            .and_then(|data| data.get(0x14..0x18))
            .and_then(|s| s.try_into().ok())
            .map(|bytes| object.endianness().read_i32_bytes(bytes))
            .unwrap_or(0);

        Ok(Self { endianness: object.endianness(), abi, isa_extension, ri_gp_value })
    }

    fn instruction_flags(&self, diff_config: &DiffObjConfig) -> rabbitizer::InstructionFlags {
        let isa_extension = match diff_config.mips_instr_category {
            MipsInstrCategory::Auto => self.isa_extension,
            MipsInstrCategory::Cpu => None,
            MipsInstrCategory::Rsp => Some(IsaExtension::RSP),
            MipsInstrCategory::R3000gte => Some(IsaExtension::R3000GTE),
            MipsInstrCategory::R4000allegrex => Some(IsaExtension::R4000ALLEGREX),
            MipsInstrCategory::R5900 => Some(IsaExtension::R5900EE),
        };
        match isa_extension {
            Some(extension) => rabbitizer::InstructionFlags::new_extension(extension),
            None => rabbitizer::InstructionFlags::new_isa(IsaVersion::MIPS_III, None),
        }
        .with_abi(match diff_config.mips_abi {
            MipsAbi::Auto => self.abi,
            MipsAbi::O32 => Abi::O32,
            MipsAbi::N32 => Abi::N32,
            MipsAbi::N64 => Abi::N64,
        })
    }

    fn instruction_display_flags(
        &self,
        _diff_config: &DiffObjConfig,
    ) -> rabbitizer::InstructionDisplayFlags {
        rabbitizer::InstructionDisplayFlags::default().with_unknown_instr_comment(false)
    }

    fn parse_ins_ref(
        &self,
        ins_ref: InstructionRef,
        code: &[u8],
        diff_config: &DiffObjConfig,
    ) -> Result<rabbitizer::Instruction> {
        Ok(rabbitizer::Instruction::new(
            self.endianness.read_u32_bytes(code.try_into()?),
            Vram::new(ins_ref.address as u32),
            self.instruction_flags(diff_config),
        ))
    }
}

impl Arch for ArchMips {
    fn scan_instructions(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        diff_config: &DiffObjConfig,
    ) -> Result<Vec<ScannedInstruction>> {
        let instruction_flags = self.instruction_flags(diff_config);
        let start_address = address;
        let mut ops = Vec::<ScannedInstruction>::with_capacity(code.len() / 4);
        let mut cur_addr = start_address as u32;
        for chunk in code.chunks_exact(4) {
            let code = self.endianness.read_u32_bytes(chunk.try_into()?);
            let vram = Vram::new(cur_addr);
            let instruction = rabbitizer::Instruction::new(code, vram, instruction_flags);
            let opcode = instruction.opcode() as u16;
            let branch_dest =
                instruction.get_branch_offset_generic().map(|o| (vram + o).inner() as u64);
            ops.push(ScannedInstruction {
                ins_ref: InstructionRef { address, size: 4, opcode },
                branch_dest,
            });
            cur_addr += 4;
        }
        Ok(ops)
    }

    fn display_instruction(
        &self,
        resolved: ResolvedInstructionRef,
        diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        let instruction = self.parse_ins_ref(resolved.ins_ref, resolved.code, diff_config)?;
        let display_flags = self.instruction_display_flags(diff_config);
        let opcode = instruction.opcode();
        cb(InstructionPart::opcode(opcode.name(), opcode as u16))?;
        let start_address = resolved.symbol.address;
        let function_range = start_address..start_address + resolved.symbol.size;
        push_args(
            &instruction,
            resolved.relocation,
            function_range,
            resolved.section_index,
            &display_flags,
            cb,
        )?;
        Ok(())
    }

    fn implcit_addend(
        &self,
        file: &object::File<'_>,
        section: &object::Section,
        address: u64,
        reloc: &object::Relocation,
        flags: RelocationFlags,
    ) -> Result<i64> {
        let data = section.data()?;
        let code = data[address as usize..address as usize + 4].try_into()?;
        let addend = self.endianness.read_u32_bytes(code);
        Ok(match flags {
            RelocationFlags::Elf(elf::R_MIPS_32) => addend as i64,
            RelocationFlags::Elf(elf::R_MIPS_26) => ((addend & 0x03FFFFFF) << 2) as i64,
            RelocationFlags::Elf(elf::R_MIPS_HI16) => ((addend & 0x0000FFFF) << 16) as i32 as i64,
            RelocationFlags::Elf(elf::R_MIPS_LO16 | elf::R_MIPS_GOT16 | elf::R_MIPS_CALL16) => {
                (addend & 0x0000FFFF) as i16 as i64
            }
            RelocationFlags::Elf(elf::R_MIPS_GPREL16 | elf::R_MIPS_LITERAL) => {
                let object::RelocationTarget::Symbol(idx) = reloc.target() else {
                    bail!("Unsupported R_MIPS_GPREL16 relocation against a non-symbol");
                };
                let sym = file.symbol_by_index(idx)?;

                // if the symbol we are relocating against is in a local section we need to add
                // the ri_gp_value from .reginfo to the addend.
                if sym.section().index().is_some() {
                    ((addend & 0x0000FFFF) as i16 as i64) + self.ri_gp_value as i64
                } else {
                    (addend & 0x0000FFFF) as i16 as i64
                }
            }
            RelocationFlags::Elf(elf::R_MIPS_PC16) => 0, // PC-relative relocation
            RelocationFlags::Elf(R_MIPS15_S3) => ((addend & 0x001FFFC0) >> 3) as i64,
            flags => bail!("Unsupported MIPS implicit relocation {flags:?}"),
        })
    }

    fn reloc_name(&self, flags: RelocationFlags) -> Option<&'static str> {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                elf::R_MIPS_NONE => Some("R_MIPS_NONE"),
                elf::R_MIPS_16 => Some("R_MIPS_16"),
                elf::R_MIPS_32 => Some("R_MIPS_32"),
                elf::R_MIPS_26 => Some("R_MIPS_26"),
                elf::R_MIPS_HI16 => Some("R_MIPS_HI16"),
                elf::R_MIPS_LO16 => Some("R_MIPS_LO16"),
                elf::R_MIPS_GPREL16 => Some("R_MIPS_GPREL16"),
                elf::R_MIPS_LITERAL => Some("R_MIPS_LITERAL"),
                elf::R_MIPS_GOT16 => Some("R_MIPS_GOT16"),
                elf::R_MIPS_PC16 => Some("R_MIPS_PC16"),
                elf::R_MIPS_CALL16 => Some("R_MIPS_CALL16"),
                R_MIPS15_S3 => Some("R_MIPS15_S3"),
                _ => None,
            },
            _ => None,
        }
    }

    fn data_reloc_size(&self, flags: RelocationFlags) -> usize {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                elf::R_MIPS_16 => 2,
                elf::R_MIPS_32 => 4,
                _ => 1,
            },
            _ => 1,
        }
    }
}

fn push_args(
    instruction: &rabbitizer::Instruction,
    relocation: Option<ResolvedRelocation>,
    function_range: Range<u64>,
    section_index: usize,
    display_flags: &rabbitizer::InstructionDisplayFlags,
    mut arg_cb: impl FnMut(InstructionPart) -> Result<()>,
) -> Result<()> {
    let operands = instruction.valued_operands_iter();
    for (idx, op) in operands.enumerate() {
        if idx > 0 {
            arg_cb(InstructionPart::separator())?;
        }

        match op {
            ValuedOperand::core_immediate(imm) => {
                if let Some(resolved) = relocation {
                    push_reloc(resolved.relocation, &mut arg_cb)?;
                } else {
                    arg_cb(match imm {
                        IU16::Integer(s) => InstructionPart::signed(s),
                        IU16::Unsigned(u) => InstructionPart::unsigned(u),
                    })?;
                }
            }
            ValuedOperand::core_label(..) | ValuedOperand::core_branch_target_label(..) => {
                if let Some(resolved) = relocation {
                    // If the relocation target is within the current function, we can
                    // convert it into a relative branch target. Note that we check
                    // target_address > start_address instead of >= so that recursive
                    // tail calls are not considered branch targets.
                    let target_address =
                        resolved.symbol.address.checked_add_signed(resolved.relocation.addend);
                    if resolved.symbol.section == Some(section_index)
                        && target_address.is_some_and(|addr| {
                            addr > function_range.start && addr < function_range.end
                        })
                    {
                        // TODO move this logic up a level
                        let target_address = target_address.unwrap();
                        arg_cb(InstructionPart::branch_dest(target_address))?;
                    } else {
                        push_reloc(resolved.relocation, &mut arg_cb)?;
                    }
                } else if let Some(branch_dest) = instruction
                    .get_branch_offset_generic()
                    .map(|o| (instruction.vram() + o).inner() as u64)
                {
                    arg_cb(InstructionPart::branch_dest(branch_dest))?;
                } else {
                    arg_cb(InstructionPart::opaque(
                        op.display(instruction, display_flags, None::<&str>).to_string(),
                    ))?;
                }
            }
            ValuedOperand::core_immediate_base(imm, base) => {
                if let Some(resolved) = relocation {
                    push_reloc(resolved.relocation, &mut arg_cb)?;
                } else {
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(match imm {
                        IU16::Integer(s) => InstructionArgValue::Signed(s as i64),
                        IU16::Unsigned(u) => InstructionArgValue::Unsigned(u as u64),
                    })))?;
                }
                arg_cb(InstructionPart::basic("("))?;
                arg_cb(InstructionPart::opaque(
                    base.either_name(instruction.flags().abi(), display_flags.named_gpr()),
                ))?;
                arg_cb(InstructionPart::basic(")"))?;
            }
            // ValuedOperand::r5900_immediate15(..) => match relocation {
            //     Some(resolved)
            //         if resolved.relocation.flags == RelocationFlags::Elf(R_MIPS15_S3) =>
            //     {
            //         push_reloc(&resolved.relocation, &mut arg_cb)?;
            //     }
            //     _ => {
            //         arg_cb(InstructionPart::opaque(op.disassemble(&instruction, None)))?;
            //     }
            // },
            _ => {
                arg_cb(InstructionPart::opaque(
                    op.display(instruction, display_flags, None::<&str>).to_string(),
                ))?;
            }
        }
    }
    Ok(())
}

fn push_reloc(
    reloc: &Relocation,
    mut arg_cb: impl FnMut(InstructionPart) -> Result<()>,
) -> Result<()> {
    match reloc.flags {
        RelocationFlags::Elf(r_type) => match r_type {
            elf::R_MIPS_HI16 => {
                arg_cb(InstructionPart::basic("%hi("))?;
                arg_cb(InstructionPart::reloc())?;
                arg_cb(InstructionPart::basic(")"))?;
            }
            elf::R_MIPS_LO16 => {
                arg_cb(InstructionPart::basic("%lo("))?;
                arg_cb(InstructionPart::reloc())?;
                arg_cb(InstructionPart::basic(")"))?;
            }
            elf::R_MIPS_GOT16 => {
                arg_cb(InstructionPart::basic("%got("))?;
                arg_cb(InstructionPart::reloc())?;
                arg_cb(InstructionPart::basic(")"))?;
            }
            elf::R_MIPS_CALL16 => {
                arg_cb(InstructionPart::basic("%call16("))?;
                arg_cb(InstructionPart::reloc())?;
                arg_cb(InstructionPart::basic(")"))?;
            }
            elf::R_MIPS_GPREL16 => {
                arg_cb(InstructionPart::basic("%gp_rel("))?;
                arg_cb(InstructionPart::reloc())?;
                arg_cb(InstructionPart::basic(")"))?;
            }
            elf::R_MIPS_32
            | elf::R_MIPS_26
            | elf::R_MIPS_LITERAL
            | elf::R_MIPS_PC16
            | R_MIPS15_S3 => {
                arg_cb(InstructionPart::reloc())?;
            }
            _ => bail!("Unsupported ELF MIPS relocation type {r_type}"),
        },
        flags => panic!("Unsupported MIPS relocation flags {flags:?}"),
    }
    Ok(())
}

use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::{String, ToString},
    vec::Vec,
};

use anyhow::{Result, bail};
use object::{Endian as _, Object as _, ObjectSection as _, ObjectSymbol as _, elf};
use rabbitizer::{
    IsaExtension, IsaVersion, Vram, abi::Abi, operands::ValuedOperand, registers_meta::Register,
};

use crate::{
    arch::{Arch, RelocationOverride, RelocationOverrideTarget},
    diff::{DiffObjConfig, MipsAbi, MipsInstrCategory, display::InstructionPart},
    obj::{
        InstructionArg, InstructionArgValue, InstructionRef, Relocation, RelocationFlags,
        ResolvedInstructionRef, ResolvedRelocation, Section, Symbol, SymbolFlag, SymbolFlagSet,
    },
};

#[derive(Debug)]
pub struct ArchMips {
    pub endianness: object::Endianness,
    pub abi: Abi,
    pub isa_extension: Option<IsaExtension>,
    pub ri_gp_value: i32,
    pub paired_relocations: Vec<BTreeMap<u64, i64>>,
    pub ignored_symbols: BTreeSet<usize>,
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
        // 0x14 bytes into .reginfo (on 32-bit platforms)
        let endianness = object.endianness();
        let ri_gp_value = object
            .section_by_name(".reginfo")
            .and_then(|section| section.data().ok())
            .and_then(|data| data.get(0x14..0x18))
            .and_then(|s| s.try_into().ok())
            .map(|bytes| endianness.read_i32_bytes(bytes))
            .unwrap_or(0);

        // Parse all relocations to pair R_MIPS_HI16 and R_MIPS_LO16. Since the instructions only
        // have 16-bit immediate fields, the 32-bit addend is split across the two relocations.
        // R_MIPS_LO16 relocations without an immediately preceding R_MIPS_HI16 use the last seen
        // R_MIPS_HI16 addend.
        // See https://refspecs.linuxfoundation.org/elf/mipsabi.pdf pages 4-17 and 4-18
        let mut paired_relocations = Vec::with_capacity(object.sections().count() + 1);
        for obj_section in object.sections() {
            let data = obj_section.data()?;
            let mut last_hi = None;
            let mut last_hi_addend = 0;
            let mut addends = BTreeMap::new();
            for (addr, reloc) in obj_section.relocations() {
                if !reloc.has_implicit_addend() {
                    continue;
                }
                match reloc.flags() {
                    object::RelocationFlags::Elf { r_type: elf::R_MIPS_HI16 } => {
                        let code = data[addr as usize..addr as usize + 4].try_into()?;
                        let addend = ((endianness.read_u32_bytes(code) & 0x0000FFFF) << 16) as i32;
                        last_hi = Some(addr);
                        last_hi_addend = addend;
                    }
                    object::RelocationFlags::Elf { r_type: elf::R_MIPS_LO16 } => {
                        let code = data[addr as usize..addr as usize + 4].try_into()?;
                        let addend = (endianness.read_u32_bytes(code) & 0x0000FFFF) as i16 as i32;
                        let full_addend = (last_hi_addend + addend) as i64;
                        if let Some(hi_addr) = last_hi.take() {
                            addends.insert(hi_addr, full_addend);
                        }
                        addends.insert(addr, full_addend);
                    }
                    _ => {
                        last_hi = None;
                    }
                }
            }
            let section_index = obj_section.index().0;
            if section_index >= paired_relocations.len() {
                paired_relocations.resize_with(section_index + 1, BTreeMap::new);
            }
            paired_relocations[section_index] = addends;
        }

        let mut ignored_symbols = BTreeSet::new();
        for obj_symbol in object.symbols() {
            let Ok(name) = obj_symbol.name() else { continue };
            if let Some(prefix) = name.strip_suffix(".NON_MATCHING") {
                ignored_symbols.insert(obj_symbol.index().0);
                if let Some(target_symbol) = object.symbol_by_name(prefix) {
                    ignored_symbols.insert(target_symbol.index().0);
                }
            }
        }

        Ok(Self {
            endianness,
            abi,
            isa_extension,
            ri_gp_value,
            paired_relocations,
            ignored_symbols,
        })
    }

    fn default_instruction_flags(&self) -> rabbitizer::InstructionFlags {
        match self.isa_extension {
            Some(extension) => rabbitizer::InstructionFlags::new_extension(extension),
            None => rabbitizer::InstructionFlags::new(IsaVersion::MIPS_III),
        }
        .with_abi(self.abi)
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
            None => rabbitizer::InstructionFlags::new(IsaVersion::MIPS_III),
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
        diff_config: &DiffObjConfig,
    ) -> rabbitizer::InstructionDisplayFlags {
        rabbitizer::InstructionDisplayFlags::default()
            .with_unknown_instr_comment(false)
            .with_use_dollar(diff_config.mips_register_prefix)
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
    fn scan_instructions_internal(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        _relocations: &[Relocation],
        diff_config: &DiffObjConfig,
    ) -> Result<Vec<InstructionRef>> {
        let instruction_flags = self.instruction_flags(diff_config);
        let mut ops = Vec::<InstructionRef>::with_capacity(code.len() / 4);
        let mut cur_addr = address as u32;
        for chunk in code.chunks_exact(4) {
            let code = self.endianness.read_u32_bytes(chunk.try_into()?);
            let instruction =
                rabbitizer::Instruction::new(code, Vram::new(cur_addr), instruction_flags);
            let opcode = instruction.opcode() as u16;
            let branch_dest = instruction.get_branch_vram_generic().map(|v| v.inner() as u64);
            ops.push(InstructionRef { address: cur_addr as u64, size: 4, opcode, branch_dest });
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
        push_args(&instruction, resolved.relocation, &display_flags, cb)?;
        Ok(())
    }

    fn relocation_override(
        &self,
        file: &object::File<'_>,
        section: &object::Section,
        address: u64,
        relocation: &object::Relocation,
    ) -> Result<Option<RelocationOverride>> {
        match relocation.flags() {
            // Handle ELF implicit relocations
            object::RelocationFlags::Elf { r_type } => {
                if relocation.has_implicit_addend() {
                    // Check for paired R_MIPS_HI16 and R_MIPS_LO16 relocations.
                    if let elf::R_MIPS_HI16 | elf::R_MIPS_LO16 = r_type {
                        if let Some(addend) = self
                            .paired_relocations
                            .get(section.index().0)
                            .and_then(|m| m.get(&address).copied())
                        {
                            return Ok(Some(RelocationOverride {
                                target: RelocationOverrideTarget::Keep,
                                addend,
                            }));
                        }
                    }

                    let data = section.data()?;
                    let code = self
                        .endianness
                        .read_u32_bytes(data[address as usize..address as usize + 4].try_into()?);
                    let addend = match r_type {
                        elf::R_MIPS_32 => code as i64,
                        elf::R_MIPS_26 => ((code & 0x03FFFFFF) << 2) as i64,
                        elf::R_MIPS_HI16 => ((code & 0x0000FFFF) << 16) as i32 as i64,
                        elf::R_MIPS_LO16 | elf::R_MIPS_GOT16 | elf::R_MIPS_CALL16 => {
                            (code & 0x0000FFFF) as i16 as i64
                        }
                        elf::R_MIPS_GPREL16 | elf::R_MIPS_LITERAL => {
                            let object::RelocationTarget::Symbol(idx) = relocation.target() else {
                                bail!("Unsupported R_MIPS_GPREL16 relocation against a non-symbol");
                            };
                            let sym = file.symbol_by_index(idx)?;

                            // if the symbol we are relocating against is in a local section we need to add
                            // the ri_gp_value from .reginfo to the addend.
                            if sym.section().index().is_some() {
                                ((code & 0x0000FFFF) as i16 as i64) + self.ri_gp_value as i64
                            } else {
                                (code & 0x0000FFFF) as i16 as i64
                            }
                        }
                        elf::R_MIPS_PC16 => 0, // PC-relative relocation
                        R_MIPS15_S3 => ((code & 0x001FFFC0) >> 3) as i64,
                        flags => bail!("Unsupported MIPS implicit relocation {flags:?}"),
                    };
                    Ok(Some(RelocationOverride { target: RelocationOverrideTarget::Keep, addend }))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn demangle(&self, name: &str) -> Option<String> {
        cpp_demangle::Symbol::new(name)
            .ok()
            .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
            .or_else(|| cwdemangle::demangle(name, &cwdemangle::DemangleOptions::default()))
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

    fn extra_symbol_flags(&self, symbol: &object::Symbol) -> SymbolFlagSet {
        let mut flags = SymbolFlagSet::default();
        if self.ignored_symbols.contains(&symbol.index().0) {
            flags |= SymbolFlag::Ignored;
        }
        flags
    }

    fn infer_function_size(
        &self,
        symbol: &Symbol,
        section: &Section,
        next_address: u64,
    ) -> Result<u64> {
        // Trim any trailing 4-byte zeroes from the end (nops)
        let mut new_address = next_address;
        while new_address >= symbol.address + 4
            && let Some(data) = section.data_range(new_address - 4, 4)
            && data == [0u8; 4]
        {
            new_address -= 4;
        }
        // Check if the last instruction has a delay slot, if so, include the delay slot nop
        if new_address + 4 <= next_address
            && new_address >= symbol.address + 4
            && let Some(data) = section.data_range(new_address - 4, 4)
            && let instruction = rabbitizer::Instruction::new(
                self.endianness.read_u32_bytes(data.try_into().unwrap()),
                Vram::new((new_address - 4) as u32),
                self.default_instruction_flags(),
            )
            && instruction.opcode().has_delay_slot()
        {
            new_address += 4;
        }
        Ok(new_address.saturating_sub(symbol.address))
    }
}

fn push_args(
    instruction: &rabbitizer::Instruction,
    relocation: Option<ResolvedRelocation>,
    display_flags: &rabbitizer::InstructionDisplayFlags,
    mut arg_cb: impl FnMut(InstructionPart) -> Result<()>,
) -> Result<()> {
    let operands = instruction.valued_operands_iter();
    for (idx, op) in operands.enumerate() {
        if idx > 0 {
            arg_cb(InstructionPart::separator())?;
        }

        match op {
            ValuedOperand::core_imm_i16(imm) => {
                if let Some(resolved) = relocation {
                    push_reloc(resolved.relocation, &mut arg_cb)?;
                } else {
                    arg_cb(InstructionPart::signed(imm))?;
                }
            }
            ValuedOperand::core_imm_u16(imm) => {
                if let Some(resolved) = relocation {
                    push_reloc(resolved.relocation, &mut arg_cb)?;
                } else {
                    arg_cb(InstructionPart::unsigned(imm))?;
                }
            }
            ValuedOperand::core_label(..) | ValuedOperand::core_branch_target_label(..) => {
                if let Some(resolved) = relocation {
                    push_reloc(resolved.relocation, &mut arg_cb)?;
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
            ValuedOperand::core_imm_rs(imm, base) => {
                if let Some(resolved) = relocation {
                    push_reloc(resolved.relocation, &mut arg_cb)?;
                } else {
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Signed(imm as i64),
                    )))?;
                }
                arg_cb(InstructionPart::basic("("))?;
                arg_cb(InstructionPart::opaque(base.either_name(
                    instruction.flags().abi(),
                    display_flags.named_gpr(),
                    !display_flags.use_dollar(),
                )))?;
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

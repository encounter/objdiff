use std::{borrow::Cow, collections::BTreeMap, sync::Mutex};

use anyhow::{anyhow, bail, Result};
use object::{
    elf, Endian, Endianness, File, FileFlags, Object, ObjectSection, ObjectSymbol, Relocation,
    RelocationFlags, RelocationTarget,
};
use rabbitizer::{config, Abi, InstrCategory, Instruction, OperandType};

use crate::{
    arch::{ObjArch, ProcessCodeResult},
    diff::{DiffObjConfig, MipsAbi, MipsInstrCategory},
    obj::{ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjSection},
};

static RABBITIZER_MUTEX: Mutex<()> = Mutex::new(());

fn configure_rabbitizer(abi: Abi) {
    unsafe {
        config::RabbitizerConfig_Cfg.reg_names.fpr_abi_names = abi;
    }
}

pub struct ObjArchMips {
    pub endianness: Endianness,
    pub abi: Abi,
    pub instr_category: InstrCategory,
    pub ri_gp_value: i32,
}

const EF_MIPS_ABI: u32 = 0x0000F000;
const EF_MIPS_MACH: u32 = 0x00FF0000;

const EF_MIPS_MACH_ALLEGREX: u32 = 0x00840000;
const EF_MIPS_MACH_5900: u32 = 0x00920000;

const R_MIPS15_S3: u32 = 119;

impl ObjArchMips {
    pub fn new(object: &File) -> Result<Self> {
        let mut abi = Abi::NUMERIC;
        let mut instr_category = InstrCategory::CPU;
        match object.flags() {
            FileFlags::None => {}
            FileFlags::Elf { e_flags, .. } => {
                abi = match e_flags & EF_MIPS_ABI {
                    elf::EF_MIPS_ABI_O32 | elf::EF_MIPS_ABI_O64 => Abi::O32,
                    elf::EF_MIPS_ABI_EABI32 | elf::EF_MIPS_ABI_EABI64 => Abi::N32,
                    _ => {
                        if e_flags & elf::EF_MIPS_ABI2 != 0 {
                            Abi::N32
                        } else {
                            Abi::NUMERIC
                        }
                    }
                };
                instr_category = match e_flags & EF_MIPS_MACH {
                    EF_MIPS_MACH_ALLEGREX => InstrCategory::R4000ALLEGREX,
                    EF_MIPS_MACH_5900 => InstrCategory::R5900,
                    _ => InstrCategory::CPU,
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

        Ok(Self { endianness: object.endianness(), abi, instr_category, ri_gp_value })
    }
}

impl ObjArch for ObjArchMips {
    fn process_code(
        &self,
        address: u64,
        code: &[u8],
        section_index: usize,
        relocations: &[ObjReloc],
        line_info: &BTreeMap<u64, u32>,
        config: &DiffObjConfig,
        _sections: &[ObjSection],
    ) -> Result<ProcessCodeResult> {
        let _guard = RABBITIZER_MUTEX.lock().map_err(|e| anyhow!("Failed to lock mutex: {e}"))?;
        configure_rabbitizer(match config.mips_abi {
            MipsAbi::Auto => self.abi,
            MipsAbi::O32 => Abi::O32,
            MipsAbi::N32 => Abi::N32,
            MipsAbi::N64 => Abi::N64,
        });
        let instr_category = match config.mips_instr_category {
            MipsInstrCategory::Auto => self.instr_category,
            MipsInstrCategory::Cpu => InstrCategory::CPU,
            MipsInstrCategory::Rsp => InstrCategory::RSP,
            MipsInstrCategory::R3000Gte => InstrCategory::R3000GTE,
            MipsInstrCategory::R4000Allegrex => InstrCategory::R4000ALLEGREX,
            MipsInstrCategory::R5900 => InstrCategory::R5900,
        };

        let start_address = address;
        let end_address = address + code.len() as u64;
        let ins_count = code.len() / 4;
        let mut ops = Vec::<u16>::with_capacity(ins_count);
        let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
        let mut cur_addr = start_address as u32;
        for chunk in code.chunks_exact(4) {
            let reloc = relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);
            let code = self.endianness.read_u32_bytes(chunk.try_into()?);
            let instruction = Instruction::new(code, cur_addr, instr_category);

            let formatted = instruction.disassemble(None, 0);
            let op = instruction.unique_id as u16;
            ops.push(op);

            let mnemonic = instruction.opcode_name();
            let is_branch = instruction.is_branch();
            let branch_offset = instruction.branch_offset();
            let mut branch_dest = if is_branch {
                cur_addr.checked_add_signed(branch_offset).map(|a| a as u64)
            } else {
                None
            };

            let operands = instruction.get_operands_slice();
            let mut args = Vec::with_capacity(operands.len() + 1);
            for (idx, op) in operands.iter().enumerate() {
                if idx > 0 {
                    args.push(ObjInsArg::PlainText(config.separator().into()));
                }

                match op {
                    OperandType::cpu_immediate
                    | OperandType::cpu_label
                    | OperandType::cpu_branch_target_label => {
                        if let Some(reloc) = reloc {
                            // If the relocation target is within the current function, we can
                            // convert it into a relative branch target. Note that we check
                            // target_address > start_address instead of >= so that recursive
                            // tail calls are not considered branch targets.
                            let target_address =
                                reloc.target.address.checked_add_signed(reloc.addend);
                            if reloc.target.orig_section_index == Some(section_index)
                                && matches!(target_address, Some(addr) if addr > start_address && addr < end_address)
                            {
                                let target_address = target_address.unwrap();
                                args.push(ObjInsArg::BranchDest(target_address));
                                branch_dest = Some(target_address);
                            } else {
                                push_reloc(&mut args, reloc)?;
                                branch_dest = None;
                            }
                        } else if let Some(branch_dest) = branch_dest {
                            args.push(ObjInsArg::BranchDest(branch_dest));
                        } else {
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                                op.disassemble(&instruction, None).into(),
                            )));
                        }
                    }
                    OperandType::cpu_immediate_base => {
                        if let Some(reloc) = reloc {
                            push_reloc(&mut args, reloc)?;
                        } else {
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                                OperandType::cpu_immediate.disassemble(&instruction, None).into(),
                            )));
                        }
                        args.push(ObjInsArg::PlainText("(".into()));
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                            OperandType::cpu_rs.disassemble(&instruction, None).into(),
                        )));
                        args.push(ObjInsArg::PlainText(")".into()));
                    }
                    // OperandType::r5900_immediate15 => match reloc {
                    //     Some(reloc)
                    //         if reloc.flags == RelocationFlags::Elf { r_type: R_MIPS15_S3 } =>
                    //     {
                    //         push_reloc(&mut args, reloc)?;
                    //     }
                    //     _ => {
                    //         args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                    //             op.disassemble(&instruction, None).into(),
                    //         )));
                    //     }
                    // },
                    _ => {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                            op.disassemble(&instruction, None).into(),
                        )));
                    }
                }
            }
            let line = line_info.range(..=cur_addr as u64).last().map(|(_, &b)| b);
            insts.push(ObjIns {
                address: cur_addr as u64,
                size: 4,
                op,
                mnemonic: Cow::Borrowed(mnemonic),
                args,
                reloc: reloc.cloned(),
                fake_pool_reloc: None,
                branch_dest,
                line,
                formatted,
                orig: None,
            });
            cur_addr += 4;
        }
        Ok(ProcessCodeResult { ops, insts })
    }

    fn implcit_addend(
        &self,
        file: &File<'_>,
        section: &ObjSection,
        address: u64,
        reloc: &Relocation,
    ) -> Result<i64> {
        let data = section.data[address as usize..address as usize + 4].try_into()?;
        let addend = self.endianness.read_u32_bytes(data);
        Ok(match reloc.flags() {
            RelocationFlags::Elf { r_type: elf::R_MIPS_32 } => addend as i64,
            RelocationFlags::Elf { r_type: elf::R_MIPS_26 } => ((addend & 0x03FFFFFF) << 2) as i64,
            RelocationFlags::Elf { r_type: elf::R_MIPS_HI16 } => {
                ((addend & 0x0000FFFF) << 16) as i32 as i64
            }
            RelocationFlags::Elf {
                r_type: elf::R_MIPS_LO16 | elf::R_MIPS_GOT16 | elf::R_MIPS_CALL16,
            } => (addend & 0x0000FFFF) as i16 as i64,
            RelocationFlags::Elf { r_type: elf::R_MIPS_GPREL16 | elf::R_MIPS_LITERAL } => {
                let RelocationTarget::Symbol(idx) = reloc.target() else {
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
            RelocationFlags::Elf { r_type: elf::R_MIPS_PC16 } => 0, // PC-relative relocation
            RelocationFlags::Elf { r_type: R_MIPS15_S3 } => ((addend & 0x001FFFC0) >> 3) as i64,
            flags => bail!("Unsupported MIPS implicit relocation {flags:?}"),
        })
    }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        match flags {
            RelocationFlags::Elf { r_type } => match r_type {
                elf::R_MIPS_32 => Cow::Borrowed("R_MIPS_32"),
                elf::R_MIPS_26 => Cow::Borrowed("R_MIPS_26"),
                elf::R_MIPS_HI16 => Cow::Borrowed("R_MIPS_HI16"),
                elf::R_MIPS_LO16 => Cow::Borrowed("R_MIPS_LO16"),
                elf::R_MIPS_GPREL16 => Cow::Borrowed("R_MIPS_GPREL16"),
                elf::R_MIPS_LITERAL => Cow::Borrowed("R_MIPS_LITERAL"),
                elf::R_MIPS_GOT16 => Cow::Borrowed("R_MIPS_GOT16"),
                elf::R_MIPS_PC16 => Cow::Borrowed("R_MIPS_PC16"),
                elf::R_MIPS_CALL16 => Cow::Borrowed("R_MIPS_CALL16"),
                R_MIPS15_S3 => Cow::Borrowed("R_MIPS15_S3"),
                _ => Cow::Owned(format!("<{flags:?}>")),
            },
            _ => Cow::Owned(format!("<{flags:?}>")),
        }
    }
}

fn push_reloc(args: &mut Vec<ObjInsArg>, reloc: &ObjReloc) -> Result<()> {
    match reloc.flags {
        RelocationFlags::Elf { r_type } => match r_type {
            elf::R_MIPS_HI16 => {
                args.push(ObjInsArg::PlainText("%hi(".into()));
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText(")".into()));
            }
            elf::R_MIPS_LO16 => {
                args.push(ObjInsArg::PlainText("%lo(".into()));
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText(")".into()));
            }
            elf::R_MIPS_GOT16 => {
                args.push(ObjInsArg::PlainText("%got(".into()));
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText(")".into()));
            }
            elf::R_MIPS_CALL16 => {
                args.push(ObjInsArg::PlainText("%call16(".into()));
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText(")".into()));
            }
            elf::R_MIPS_GPREL16 => {
                args.push(ObjInsArg::PlainText("%gp_rel(".into()));
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText(")".into()));
            }
            elf::R_MIPS_32
            | elf::R_MIPS_26
            | elf::R_MIPS_LITERAL
            | elf::R_MIPS_PC16
            | R_MIPS15_S3 => {
                args.push(ObjInsArg::Reloc);
            }
            _ => bail!("Unsupported ELF MIPS relocation type {r_type}"),
        },
        flags => panic!("Unsupported MIPS relocation flags {flags:?}"),
    }
    Ok(())
}

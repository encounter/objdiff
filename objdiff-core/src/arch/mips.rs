use std::borrow::Cow;

use anyhow::{anyhow, bail, Result};
use object::{elf, Endian, Endianness, File, Object, Relocation, RelocationFlags};
use rabbitizer::{config, Abi, InstrCategory, Instruction, OperandType};

use crate::{
    arch::{ObjArch, ProcessCodeResult},
    diff::DiffObjConfig,
    obj::{ObjInfo, ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjSection, SymbolRef},
};

fn configure_rabbitizer() {
    unsafe {
        config::RabbitizerConfig_Cfg.reg_names.fpr_abi_names = Abi::O32;
    }
}

pub struct ObjArchMips {
    pub endianness: Endianness,
}

impl ObjArchMips {
    pub fn new(object: &File) -> Result<Self> {
        configure_rabbitizer();
        Ok(Self { endianness: object.endianness() })
    }
}

impl ObjArch for ObjArchMips {
    fn process_code(
        &self,
        obj: &ObjInfo,
        symbol_ref: SymbolRef,
        config: &DiffObjConfig,
    ) -> Result<ProcessCodeResult> {
        let (section, symbol) = obj.section_symbol(symbol_ref);
        let section = section.ok_or_else(|| anyhow!("Code symbol section not found"))?;
        let code = &section.data
            [symbol.section_address as usize..(symbol.section_address + symbol.size) as usize];

        let start_address = symbol.address;
        let end_address = symbol.address + symbol.size;
        let ins_count = code.len() / 4;
        let mut ops = Vec::<u16>::with_capacity(ins_count);
        let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
        let mut cur_addr = start_address as u32;
        for chunk in code.chunks_exact(4) {
            let reloc = section.relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);
            let code = self.endianness.read_u32_bytes(chunk.try_into()?);
            let instruction = Instruction::new(code, cur_addr, InstrCategory::CPU);

            let formatted = instruction.disassemble(None, 0);
            let op = instruction.unique_id as u16;
            ops.push(op);

            let mnemonic = instruction.opcode_name().to_string();
            let is_branch = instruction.is_branch();
            let branch_offset = instruction.branch_offset();
            let branch_dest = if is_branch {
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
                        if let Some(branch_dest) = branch_dest {
                            args.push(ObjInsArg::BranchDest(branch_dest));
                        } else if let Some(reloc) = reloc {
                            if matches!(&reloc.target_section, Some(s) if s == ".text")
                                && reloc.target.address > start_address
                                && reloc.target.address < end_address
                            {
                                args.push(ObjInsArg::BranchDest(reloc.target.address));
                            } else {
                                push_reloc(&mut args, reloc)?;
                            }
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
                    _ => {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                            op.disassemble(&instruction, None).into(),
                        )));
                    }
                }
            }
            let line = section.line_info.range(..=cur_addr as u64).last().map(|(_, &b)| b);
            insts.push(ObjIns {
                address: cur_addr as u64,
                size: 4,
                op,
                mnemonic,
                args,
                reloc: reloc.cloned(),
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
        section: &ObjSection,
        address: u64,
        reloc: &Relocation,
    ) -> Result<i64> {
        let data = section.data[address as usize..address as usize + 4].try_into()?;
        let addend = self.endianness.read_u32_bytes(data);
        Ok(match reloc.flags() {
            RelocationFlags::Elf { r_type: elf::R_MIPS_32 } => addend as i64,
            RelocationFlags::Elf { r_type: elf::R_MIPS_HI16 } => {
                ((addend & 0x0000FFFF) << 16) as i32 as i64
            }
            RelocationFlags::Elf {
                r_type:
                    elf::R_MIPS_LO16 | elf::R_MIPS_GOT16 | elf::R_MIPS_CALL16 | elf::R_MIPS_GPREL16,
            } => (addend & 0x0000FFFF) as i16 as i64,
            RelocationFlags::Elf { r_type: elf::R_MIPS_26 } => ((addend & 0x03FFFFFF) << 2) as i64,
            flags => bail!("Unsupported MIPS implicit relocation {flags:?}"),
        })
    }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        match flags {
            RelocationFlags::Elf { r_type } => match r_type {
                elf::R_MIPS_HI16 => Cow::Borrowed("R_MIPS_HI16"),
                elf::R_MIPS_LO16 => Cow::Borrowed("R_MIPS_LO16"),
                elf::R_MIPS_GOT16 => Cow::Borrowed("R_MIPS_GOT16"),
                elf::R_MIPS_CALL16 => Cow::Borrowed("R_MIPS_CALL16"),
                elf::R_MIPS_GPREL16 => Cow::Borrowed("R_MIPS_GPREL16"),
                elf::R_MIPS_32 => Cow::Borrowed("R_MIPS_32"),
                elf::R_MIPS_26 => Cow::Borrowed("R_MIPS_26"),
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
            elf::R_MIPS_32 | elf::R_MIPS_26 => {
                args.push(ObjInsArg::Reloc);
            }
            _ => bail!("Unsupported ELF MIPS relocation type {r_type}"),
        },
        flags => panic!("Unsupported MIPS relocation flags {flags:?}"),
    }
    Ok(())
}

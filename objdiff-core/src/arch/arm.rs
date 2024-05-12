use std::borrow::Cow;

use anyhow::{bail, Result};
use armv5te::arm;
use object::{elf, File, Relocation, RelocationFlags};

use crate::{
    arch::{ObjArch, ProcessCodeResult},
    diff::DiffObjConfig,
    obj::{ObjInfo, ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjSection, SymbolRef},
};

pub struct ObjArchArm {}

impl ObjArchArm {
    pub fn new(_file: &File) -> Result<Self> {
        Ok(Self {})
    }
}

impl ObjArch for ObjArchArm {
    fn process_code(
        &self,
        obj: &ObjInfo,
        symbol_ref: SymbolRef,
        config: &DiffObjConfig,
    ) -> Result<ProcessCodeResult> {
        let (section, symbol) = obj.section_symbol(symbol_ref);
        let code = &section.data
            [symbol.section_address as usize..(symbol.section_address + symbol.size) as usize];

        let ins_count = code.len() / 4;
        let mut ops = Vec::<u16>::with_capacity(ins_count);
        let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
        for (cur_addr, mut ins) in arm::InsIter::new(code, symbol.address as u32) {
            let reloc = section.relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);
            if let Some(reloc) = reloc {
                ins.code = match reloc.flags {
                    RelocationFlags::Elf { r_type: elf::R_ARM_PC24 } => ins.code & !0xffffff,
                    _ => bail!("Unhandled relocation flags {:?}", reloc.flags),
                };
            }

            let parsed_ins = arm::ParsedIns::parse(ins);

            let mut reloc_arg = None;
            if let Some(reloc) = reloc {
                if let RelocationFlags::Elf { r_type: elf::R_ARM_PC24 } = reloc.flags {
                    reloc_arg = parsed_ins
                        .args
                        .iter()
                        .rposition(|a| matches!(a, arm::Argument::BranchDest(_)));
                }
            }

            let mut args = vec![];
            let mut branch_dest = None;
            let mut writeback = false;
            let mut deref = false;
            for (i, arg) in parsed_ins.args_iter().enumerate() {
                // Emit punctuation before separator
                if deref {
                    match arg {
                        arm::Argument::PostOffset(_)
                        | arm::Argument::RegPostOffset(_)
                        | arm::Argument::CoOption(_) => {
                            deref = false;
                            args.push(ObjInsArg::PlainText("]".into()));
                            if writeback {
                                writeback = false;
                                args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("!".into())));
                            }
                        }
                        _ => {}
                    }
                }

                if i > 0 {
                    args.push(ObjInsArg::PlainText(config.separator().into()));
                }

                if reloc_arg == Some(i) {
                    let reloc = reloc.unwrap();
                    push_reloc(&mut args, reloc)?;
                } else {
                    match arg {
                        arm::Argument::RegWb(reg) => {
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                                reg.to_string().into(),
                            )));
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("!".into())));
                        }
                        arm::Argument::RegDeref(reg) => {
                            deref = true;
                            args.push(ObjInsArg::PlainText("[".into()));
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                                reg.to_string().into(),
                            )));
                        }
                        arm::Argument::RegDerefWb(reg) => {
                            deref = true;
                            writeback = true;
                            args.push(ObjInsArg::PlainText("[".into()));
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                                reg.to_string().into(),
                            )));
                        }
                        arm::Argument::RegList(reg_list) => {
                            push_reg_list(reg_list, &mut args, config);
                        }
                        arm::Argument::RegListC(reg_list) => {
                            push_reg_list(reg_list, &mut args, config);
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                                "^".to_string().into(),
                            )));
                        }
                        arm::Argument::UImm(value) | arm::Argument::CoOpcode(value) => {
                            args.push(ObjInsArg::PlainText("#".into()));
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(*value as u64)));
                        }
                        arm::Argument::SImm((value, _))
                        | arm::Argument::Offset((value, _))
                        | arm::Argument::PostOffset((value, _)) => {
                            args.push(ObjInsArg::PlainText("#".into()));
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(*value as i64)));
                        }
                        arm::Argument::BranchDest((value, _)) => {
                            let dest = cur_addr.wrapping_add_signed(*value) as u64;
                            args.push(ObjInsArg::BranchDest(dest));
                            branch_dest = Some(dest);
                        }
                        arm::Argument::CoOption(value) => {
                            args.push(ObjInsArg::PlainText("{".into()));
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(*value as u64)));
                            args.push(ObjInsArg::PlainText("}".into()));
                        }
                        arm::Argument::CoprocNum(value) => {
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                                format!("p{}", value).into(),
                            )));
                        }
                        _ => args
                            .push(ObjInsArg::Arg(ObjInsArgValue::Opaque(arg.to_string().into()))),
                    }
                }
            }
            if deref {
                args.push(ObjInsArg::PlainText("]".into()));
                if writeback {
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("!".into())));
                }
            }

            ops.push(ins.op as u16);
            let line = obj
                .line_info
                .as_ref()
                .and_then(|map| map.range(..=cur_addr as u64).last().map(|(_, &b)| b));
            insts.push(ObjIns {
                address: cur_addr as u64,
                size: 4,
                op: ins.op as u16,
                mnemonic: parsed_ins.mnemonic.to_string(),
                args,
                reloc: reloc.cloned(),
                branch_dest,
                line,
                orig: None,
            })
        }

        Ok(ProcessCodeResult { ops, insts })
    }

    fn implcit_addend(
        &self,
        _section: &ObjSection,
        address: u64,
        reloc: &Relocation,
    ) -> anyhow::Result<i64> {
        bail!("Unsupported ARM implicit relocation {:#x}{:?}", address, reloc.flags())
    }

    fn demangle(&self, name: &str) -> Option<String> {
        cpp_demangle::Symbol::new(name)
            .ok()
            .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
    }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        Cow::Owned(format!("<{flags:?}>"))
    }
}

fn push_reg_list(reg_list: &u32, args: &mut Vec<ObjInsArg>, config: &DiffObjConfig) {
    args.push(ObjInsArg::PlainText("{".into()));
    let mut first = true;
    for i in 0..16 {
        if (reg_list & (1 << i)) != 0 {
            if !first {
                args.push(ObjInsArg::PlainText(config.separator().into()));
            }
            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                arm::Reg::parse(i).to_string().into(),
            )));
            first = false;
        }
    }
    args.push(ObjInsArg::PlainText("}".into()));
}

fn push_reloc(args: &mut Vec<ObjInsArg>, reloc: &ObjReloc) -> Result<()> {
    match reloc.flags {
        RelocationFlags::Elf { r_type } => match r_type {
            elf::R_ARM_PC24 => {
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText("@pc24".into()));
            }
            _ => bail!("Unsupported ELF ARM relocation type {r_type}"),
        },
        flags => bail!("Unsupported ARM relocation kind: {flags:?}"),
    }
    Ok(())
}

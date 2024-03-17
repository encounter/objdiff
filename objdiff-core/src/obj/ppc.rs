use std::collections::BTreeMap;

use anyhow::{bail, Result};
use ppc750cl::{disasm_iter, Argument, SimplifiedIns, GPR};

use crate::{
    diff::{DiffObjConfig, ProcessCodeResult},
    obj::{ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjRelocKind},
};

// Relative relocation, can be Simm, Offset or BranchDest
fn is_relative_arg(arg: &Argument) -> bool {
    matches!(arg, Argument::Simm(_) | Argument::Offset(_) | Argument::BranchDest(_))
}

// Relative or absolute relocation, can be Uimm, Simm or Offset
fn is_rel_abs_arg(arg: &Argument) -> bool {
    matches!(arg, Argument::Uimm(_) | Argument::Simm(_) | Argument::Offset(_))
}

fn is_offset_arg(arg: &Argument) -> bool { matches!(arg, Argument::Offset(_)) }

pub fn process_code(
    config: &DiffObjConfig,
    data: &[u8],
    address: u64,
    relocs: &[ObjReloc],
    line_info: &Option<BTreeMap<u64, u64>>,
) -> Result<ProcessCodeResult> {
    let ins_count = data.len() / 4;
    let mut ops = Vec::<u16>::with_capacity(ins_count);
    let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
    for mut ins in disasm_iter(data, address as u32) {
        let reloc = relocs.iter().find(|r| (r.address as u32 & !3) == ins.addr);
        if let Some(reloc) = reloc {
            // Zero out relocations
            ins.code = match reloc.kind {
                ObjRelocKind::PpcEmbSda21 => ins.code & !0x1FFFFF,
                ObjRelocKind::PpcRel24 => ins.code & !0x3FFFFFC,
                ObjRelocKind::PpcRel14 => ins.code & !0xFFFC,
                ObjRelocKind::PpcAddr16Hi
                | ObjRelocKind::PpcAddr16Ha
                | ObjRelocKind::PpcAddr16Lo => ins.code & !0xFFFF,
                _ => ins.code,
            };
        }
        let simplified = ins.clone().simplified();

        let mut reloc_arg = None;
        if let Some(reloc) = reloc {
            match reloc.kind {
                ObjRelocKind::PpcEmbSda21 => {
                    reloc_arg = Some(1);
                }
                ObjRelocKind::PpcRel24 | ObjRelocKind::PpcRel14 => {
                    reloc_arg = simplified.args.iter().rposition(is_relative_arg);
                }
                ObjRelocKind::PpcAddr16Hi
                | ObjRelocKind::PpcAddr16Ha
                | ObjRelocKind::PpcAddr16Lo => {
                    reloc_arg = simplified.args.iter().rposition(is_rel_abs_arg);
                }
                _ => {}
            }
        }

        let mut args = vec![];
        let mut branch_dest = None;
        let mut writing_offset = false;
        for (idx, arg) in simplified.args.iter().enumerate() {
            if idx > 0 && !writing_offset {
                if config.space_between_args {
                    args.push(ObjInsArg::PlainText(", ".to_string()));
                } else {
                    args.push(ObjInsArg::PlainText(",".to_string()));
                }
            }

            if reloc_arg == Some(idx) {
                let reloc = reloc.unwrap();
                push_reloc(&mut args, reloc)?;
                // For @sda21, we can omit the register argument
                if reloc.kind == ObjRelocKind::PpcEmbSda21
                    // Sanity check: the next argument should be r0
                    && matches!(simplified.args.get(idx + 1), Some(Argument::GPR(GPR(0))))
                {
                    break;
                }
            } else {
                match arg {
                    Argument::Simm(simm) => {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(simm.0 as i64)));
                    }
                    Argument::Uimm(uimm) => {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(uimm.0 as u64)));
                    }
                    Argument::Offset(offset) => {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(offset.0 as i64)));
                    }
                    Argument::BranchDest(dest) => {
                        let dest = ins.addr.wrapping_add_signed(dest.0) as u64;
                        args.push(ObjInsArg::BranchDest(dest));
                        branch_dest = Some(dest);
                    }
                    _ => {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(arg.to_string())));
                    }
                };
            }

            if writing_offset {
                args.push(ObjInsArg::PlainText(")".to_string()));
                writing_offset = false;
            }
            if is_offset_arg(arg) {
                args.push(ObjInsArg::PlainText("(".to_string()));
                writing_offset = true;
            }
        }

        ops.push(simplified.ins.op as u16);
        let line = line_info
            .as_ref()
            .and_then(|map| map.range(..=simplified.ins.addr as u64).last().map(|(_, &b)| b));
        insts.push(ObjIns {
            address: simplified.ins.addr as u64,
            size: 4,
            mnemonic: format!("{}{}", simplified.mnemonic, simplified.suffix),
            args,
            reloc: reloc.cloned(),
            op: ins.op as u16,
            branch_dest,
            line,
            orig: Some(format!("{}", SimplifiedIns::basic_form(ins))),
        });
    }
    Ok(ProcessCodeResult { ops, insts })
}

fn push_reloc(args: &mut Vec<ObjInsArg>, reloc: &ObjReloc) -> Result<()> {
    match reloc.kind {
        ObjRelocKind::PpcAddr16Lo => {
            args.push(ObjInsArg::Reloc);
            args.push(ObjInsArg::PlainText("@l".to_string()));
        }
        ObjRelocKind::PpcAddr16Hi => {
            args.push(ObjInsArg::Reloc);
            args.push(ObjInsArg::PlainText("@h".to_string()));
        }
        ObjRelocKind::PpcAddr16Ha => {
            args.push(ObjInsArg::Reloc);
            args.push(ObjInsArg::PlainText("@ha".to_string()));
        }
        ObjRelocKind::PpcEmbSda21 => {
            args.push(ObjInsArg::Reloc);
            args.push(ObjInsArg::PlainText("@sda21".to_string()));
        }
        ObjRelocKind::PpcRel24 | ObjRelocKind::PpcRel14 => {
            args.push(ObjInsArg::Reloc);
        }
        kind => bail!("Unsupported PPC relocation kind: {:?}", kind),
    };
    Ok(())
}

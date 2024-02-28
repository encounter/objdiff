use std::collections::BTreeMap;

use anyhow::Result;
use ppc750cl::{disasm_iter, Argument, SimplifiedIns};

use crate::{
    diff::ProcessCodeResult,
    obj::{ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjRelocKind},
};

// Relative relocation, can be Simm or BranchOffset
fn is_relative_arg(arg: &ObjInsArg) -> bool {
    matches!(arg, ObjInsArg::Arg(ObjInsArgValue::Signed(_)) | ObjInsArg::BranchOffset(_))
}

// Relative or absolute relocation, can be Uimm, Simm or Offset
fn is_rel_abs_arg(arg: &ObjInsArg) -> bool {
    matches!(
        arg,
        ObjInsArg::Arg(ObjInsArgValue::Signed(_) | ObjInsArgValue::Unsigned(_))
            | ObjInsArg::ArgWithBase(ObjInsArgValue::Signed(_))
    )
}

fn is_offset_arg(arg: &ObjInsArg) -> bool {
    matches!(arg, ObjInsArg::ArgWithBase(ObjInsArgValue::Signed(_)))
}

pub fn process_code(
    data: &[u8],
    address: u64,
    relocs: &[ObjReloc],
    line_info: &Option<BTreeMap<u64, u64>>,
) -> Result<ProcessCodeResult> {
    let ins_count = data.len() / 4;
    let mut ops = Vec::<u8>::with_capacity(ins_count);
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
        let mut args: Vec<ObjInsArg> = simplified
            .args
            .iter()
            .map(|a| match a {
                Argument::Simm(simm) => ObjInsArg::Arg(ObjInsArgValue::Signed(simm.0)),
                Argument::Uimm(uimm) => ObjInsArg::Arg(ObjInsArgValue::Unsigned(uimm.0)),
                Argument::Offset(offset) => {
                    ObjInsArg::ArgWithBase(ObjInsArgValue::Signed(offset.0))
                }
                Argument::BranchDest(dest) => ObjInsArg::BranchOffset(dest.0),
                _ => ObjInsArg::Arg(ObjInsArgValue::Opaque(a.to_string())),
            })
            .collect();
        if let Some(reloc) = reloc {
            match reloc.kind {
                ObjRelocKind::PpcEmbSda21 => {
                    args = vec![args[0].clone(), ObjInsArg::Reloc];
                }
                ObjRelocKind::PpcRel24 | ObjRelocKind::PpcRel14 => {
                    let arg = args
                        .iter_mut()
                        .rfind(|a| is_relative_arg(a))
                        .ok_or_else(|| anyhow::Error::msg("Failed to locate rel arg for reloc"))?;
                    *arg = ObjInsArg::Reloc;
                }
                ObjRelocKind::PpcAddr16Hi
                | ObjRelocKind::PpcAddr16Ha
                | ObjRelocKind::PpcAddr16Lo => {
                    match args.iter_mut().rfind(|a| is_rel_abs_arg(a)) {
                        Some(arg) => {
                            *arg = if is_offset_arg(arg) {
                                ObjInsArg::RelocWithBase
                            } else {
                                ObjInsArg::Reloc
                            };
                        }
                        None => {
                            log::warn!("Failed to locate rel/abs arg for reloc");
                        }
                    };
                }
                _ => {}
            }
        }
        ops.push(simplified.ins.op as u8);
        let line = line_info
            .as_ref()
            .and_then(|map| map.range(..=simplified.ins.addr as u64).last().map(|(_, &b)| b));
        insts.push(ObjIns {
            address: simplified.ins.addr,
            code: simplified.ins.code,
            mnemonic: format!("{}{}", simplified.mnemonic, simplified.suffix),
            args,
            reloc: reloc.cloned(),
            op: ins.op as u8,
            branch_dest: None,
            line,
            orig: Some(format!("{}", SimplifiedIns::basic_form(ins))),
        });
    }
    Ok(ProcessCodeResult { ops, insts })
}

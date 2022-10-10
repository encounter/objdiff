use anyhow::Result;
use ppc750cl::{disasm_iter, Argument};

use crate::obj::{ObjIns, ObjInsArg, ObjReloc, ObjRelocKind};

// Relative relocation, can be Simm or BranchOffset
fn is_relative_arg(arg: &ObjInsArg) -> bool {
    matches!(arg, ObjInsArg::PpcArg(Argument::Simm(_)) | ObjInsArg::BranchOffset(_))
}

// Relative or absolute relocation, can be Uimm, Simm or Offset
fn is_rel_abs_arg(arg: &ObjInsArg) -> bool {
    matches!(arg, ObjInsArg::PpcArg(arg) if matches!(arg, Argument::Uimm(_) | Argument::Simm(_) | Argument::Offset(_)))
}

fn is_offset_arg(arg: &ObjInsArg) -> bool { matches!(arg, ObjInsArg::PpcArg(Argument::Offset(_))) }

pub fn process_code(
    data: &[u8],
    address: u64,
    relocs: &[ObjReloc],
) -> Result<(Vec<u8>, Vec<ObjIns>)> {
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
        let simplified = ins.simplified();
        let mut args: Vec<ObjInsArg> = simplified
            .args
            .iter()
            .map(|a| match a {
                Argument::BranchDest(dest) => ObjInsArg::BranchOffset(dest.0),
                _ => ObjInsArg::PpcArg(a.clone()),
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
                    let arg = args.iter_mut().rfind(|a| is_rel_abs_arg(a)).ok_or_else(|| {
                        anyhow::Error::msg("Failed to locate rel/abs arg for reloc")
                    })?;
                    *arg = if is_offset_arg(arg) {
                        ObjInsArg::RelocWithBase
                    } else {
                        ObjInsArg::Reloc
                    };
                }
                _ => {}
            }
        }
        ops.push(simplified.ins.op as u8);
        insts.push(ObjIns {
            address: simplified.ins.addr,
            code: simplified.ins.code,
            mnemonic: format!("{}{}", simplified.mnemonic, simplified.suffix),
            args,
            reloc: reloc.cloned(),
            op: 0,
            branch_dest: None,
        });
    }
    Ok((ops, insts))
}

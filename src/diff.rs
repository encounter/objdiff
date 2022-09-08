use std::collections::BTreeMap;

use anyhow::Result;
use ppc750cl::{disasm_iter, Argument};

use crate::{
    editops::{editops_find, LevEditType},
    obj::{
        ObjInfo, ObjIns, ObjInsArg, ObjInsArgDiff, ObjInsBranchFrom, ObjInsBranchTo, ObjInsDiff,
        ObjInsDiffKind, ObjReloc, ObjRelocKind, ObjSection, ObjSectionKind, ObjSymbol,
        ObjSymbolFlags,
    },
};

// Relative relocation, can be Simm or BranchDest
fn is_relative_arg(arg: &ObjInsArg) -> bool {
    matches!(arg, ObjInsArg::Arg(arg) if matches!(arg, Argument::Simm(_) | Argument::BranchDest(_)))
}

// Relative or absolute relocation, can be Uimm, Simm or Offset
fn is_rel_abs_arg(arg: &ObjInsArg) -> bool {
    matches!(arg, ObjInsArg::Arg(arg) if matches!(arg, Argument::Uimm(_) | Argument::Simm(_) | Argument::Offset(_)))
}

fn is_offset_arg(arg: &ObjInsArg) -> bool { matches!(arg, ObjInsArg::Arg(Argument::Offset(_))) }

fn process_code(data: &[u8], address: u64, relocs: &[ObjReloc]) -> Result<(Vec<u8>, Vec<ObjIns>)> {
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
        let mut args: Vec<ObjInsArg> =
            simplified.args.iter().map(|a| ObjInsArg::Arg(a.clone())).collect();
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
                    *arg =
                        if is_offset_arg(arg) { ObjInsArg::RelocOffset } else { ObjInsArg::Reloc };
                }
                _ => {}
            }
        }
        ops.push(simplified.ins.op as u8);
        let suffix = simplified.ins.suffix();
        insts.push(ObjIns {
            ins: simplified.ins,
            mnemonic: format!("{}{}", simplified.mnemonic, suffix),
            args,
            reloc: reloc.cloned(),
        });
    }
    Ok((ops, insts))
}

pub fn diff_code(
    left_data: &[u8],
    right_data: &[u8],
    left_symbol: &mut ObjSymbol,
    right_symbol: &mut ObjSymbol,
    left_relocs: &[ObjReloc],
    right_relocs: &[ObjReloc],
) -> Result<()> {
    let left_code =
        &left_data[left_symbol.address as usize..(left_symbol.address + left_symbol.size) as usize];
    let (left_ops, left_insts) = process_code(left_code, left_symbol.address, left_relocs)?;
    let right_code = &right_data
        [right_symbol.address as usize..(right_symbol.address + right_symbol.size) as usize];
    let (right_ops, right_insts) = process_code(right_code, right_symbol.address, right_relocs)?;

    let mut left_diff = Vec::<ObjInsDiff>::new();
    let mut right_diff = Vec::<ObjInsDiff>::new();
    let edit_ops = editops_find(&left_ops, &right_ops);

    {
        let mut op_iter = edit_ops.iter();
        let mut left_iter = left_insts.iter();
        let mut right_iter = right_insts.iter();
        let mut cur_op = op_iter.next();
        let mut cur_left = left_iter.next();
        let mut cur_right = right_iter.next();
        while let Some(op) = cur_op {
            let left_addr = op.first_start as u32 * 4;
            let right_addr = op.second_start as u32 * 4;
            while let (Some(left), Some(right)) = (cur_left, cur_right) {
                if (left.ins.addr - left_symbol.address as u32) < left_addr {
                    left_diff.push(ObjInsDiff { ins: Some(left.clone()), ..ObjInsDiff::default() });
                    right_diff
                        .push(ObjInsDiff { ins: Some(right.clone()), ..ObjInsDiff::default() });
                } else {
                    break;
                }
                cur_left = left_iter.next();
                cur_right = right_iter.next();
            }
            if let (Some(left), Some(right)) = (cur_left, cur_right) {
                if (left.ins.addr - left_symbol.address as u32) != left_addr {
                    return Err(anyhow::Error::msg("Instruction address mismatch (left)"));
                }
                if (right.ins.addr - right_symbol.address as u32) != right_addr {
                    return Err(anyhow::Error::msg("Instruction address mismatch (right)"));
                }
                match op.op_type {
                    LevEditType::Replace => {
                        left_diff
                            .push(ObjInsDiff { ins: Some(left.clone()), ..ObjInsDiff::default() });
                        right_diff
                            .push(ObjInsDiff { ins: Some(right.clone()), ..ObjInsDiff::default() });
                        cur_left = left_iter.next();
                        cur_right = right_iter.next();
                    }
                    LevEditType::Insert => {
                        left_diff.push(ObjInsDiff::default());
                        right_diff
                            .push(ObjInsDiff { ins: Some(right.clone()), ..ObjInsDiff::default() });
                        cur_right = right_iter.next();
                    }
                    LevEditType::Delete => {
                        left_diff
                            .push(ObjInsDiff { ins: Some(left.clone()), ..ObjInsDiff::default() });
                        right_diff.push(ObjInsDiff::default());
                        cur_left = left_iter.next();
                    }
                    LevEditType::Keep => unreachable!(),
                }
            } else {
                break;
            }
            cur_op = op_iter.next();
        }
        // Finalize
        while cur_left.is_some() || cur_right.is_some() {
            left_diff.push(ObjInsDiff { ins: cur_left.cloned(), ..ObjInsDiff::default() });
            right_diff.push(ObjInsDiff { ins: cur_right.cloned(), ..ObjInsDiff::default() });
            cur_left = left_iter.next();
            cur_right = right_iter.next();
        }
    }

    resolve_branches(&mut left_diff);
    resolve_branches(&mut right_diff);

    let mut diff_state = InsDiffState::default();
    for (left, right) in left_diff.iter_mut().zip(right_diff.iter_mut()) {
        let result = compare_ins(left, right, &mut diff_state)?;
        left.kind = result.kind;
        right.kind = result.kind;
        left.arg_diff = result.left_args_diff;
        right.arg_diff = result.right_args_diff;
    }

    let total = left_insts.len();
    let percent = ((total - diff_state.diff_count) as f32 / total as f32) * 100.0;
    left_symbol.match_percent = percent;
    right_symbol.match_percent = percent;

    left_symbol.instructions = left_diff;
    right_symbol.instructions = right_diff;

    Ok(())
}

fn resolve_branches(vec: &mut [ObjInsDiff]) {
    let mut branch_idx = 0usize;
    // Map addresses to indices
    let mut addr_map = BTreeMap::<u32, usize>::new();
    for (i, ins_diff) in vec.iter().enumerate() {
        if let Some(ins) = &ins_diff.ins {
            addr_map.insert(ins.ins.addr, i);
        }
    }
    // Generate branches
    let mut branches = BTreeMap::<usize, ObjInsBranchFrom>::new();
    for (i, ins_diff) in vec.iter_mut().enumerate() {
        if let Some(ins) = &ins_diff.ins {
            if ins.ins.is_blr() || ins.reloc.is_some() {
                continue;
            }
            if let Some(ins_idx) = ins.ins.branch_dest().and_then(|dest| addr_map.get(&dest)) {
                if let Some(branch) = branches.get_mut(ins_idx) {
                    ins_diff.branch_to =
                        Some(ObjInsBranchTo { ins_idx: *ins_idx, branch_idx: branch.branch_idx });
                    branch.ins_idx.push(i);
                } else {
                    ins_diff.branch_to = Some(ObjInsBranchTo { ins_idx: *ins_idx, branch_idx });
                    branches.insert(*ins_idx, ObjInsBranchFrom { ins_idx: vec![i], branch_idx });
                    branch_idx += 1;
                }
            }
        }
    }
    // Store branch from
    for (i, branch) in branches {
        vec[i].branch_from = Some(branch);
    }
}

fn reloc_eq(left_reloc: Option<&ObjReloc>, right_reloc: Option<&ObjReloc>) -> bool {
    if let (Some(left), Some(right)) = (left_reloc, right_reloc) {
        if left.kind != right.kind {
            return false;
        }
        let name_matches = left.target.name == right.target.name;
        match (&left.target_section, &right.target_section) {
            (Some(sl), Some(sr)) => {
                // Match if section and name or address match
                sl == sr && (name_matches || left.target.address == right.target.address)
            }
            (Some(_), None) => false,
            (None, Some(_)) => {
                // Match if possibly stripped weak symbol
                name_matches && right.target.flags.0.contains(ObjSymbolFlags::Weak)
            }
            (None, None) => name_matches,
        }
    } else {
        false
    }
}

fn arg_eq(
    left: &ObjInsArg,
    right: &ObjInsArg,
    left_diff: &ObjInsDiff,
    right_diff: &ObjInsDiff,
) -> bool {
    return match left {
        ObjInsArg::Arg(l) => match right {
            ObjInsArg::Arg(r) => match r {
                Argument::BranchDest(_) => {
                    // Compare dest instruction idx after diffing
                    left_diff.branch_to.as_ref().map(|b| b.ins_idx)
                        == right_diff.branch_to.as_ref().map(|b| b.ins_idx)
                }
                _ => format!("{}", l) == format!("{}", r),
            },
            _ => false,
        },
        ObjInsArg::Reloc => {
            matches!(right, ObjInsArg::Reloc)
                && reloc_eq(
                    left_diff.ins.as_ref().and_then(|i| i.reloc.as_ref()),
                    right_diff.ins.as_ref().and_then(|i| i.reloc.as_ref()),
                )
        }
        ObjInsArg::RelocOffset => {
            matches!(right, ObjInsArg::RelocOffset)
                && reloc_eq(
                    left_diff.ins.as_ref().and_then(|i| i.reloc.as_ref()),
                    right_diff.ins.as_ref().and_then(|i| i.reloc.as_ref()),
                )
        }
    };
}

#[derive(Default)]
struct InsDiffState {
    diff_count: usize,
    left_arg_idx: usize,
    right_arg_idx: usize,
    left_args_idx: BTreeMap<String, usize>,
    right_args_idx: BTreeMap<String, usize>,
}
#[derive(Default)]
struct InsDiffResult {
    kind: ObjInsDiffKind,
    left_args_diff: Vec<Option<ObjInsArgDiff>>,
    right_args_diff: Vec<Option<ObjInsArgDiff>>,
}

fn compare_ins(
    left: &ObjInsDiff,
    right: &ObjInsDiff,
    state: &mut InsDiffState,
) -> Result<InsDiffResult> {
    let mut result = InsDiffResult::default();
    if let (Some(left_ins), Some(right_ins)) = (&left.ins, &right.ins) {
        if left_ins.args.len() != right_ins.args.len() || left_ins.ins.op != right_ins.ins.op {
            // Totally different op
            result.kind = ObjInsDiffKind::Replace;
            state.diff_count += 1;
            return Ok(result);
        }
        if left_ins.mnemonic != right_ins.mnemonic {
            // Same op but different mnemonic, still cmp args
            result.kind = ObjInsDiffKind::OpMismatch;
            state.diff_count += 1;
        }
        for (a, b) in left_ins.args.iter().zip(&right_ins.args) {
            if arg_eq(a, b, left, right) {
                result.left_args_diff.push(None);
                result.right_args_diff.push(None);
            } else {
                if result.kind == ObjInsDiffKind::None {
                    result.kind = ObjInsDiffKind::ArgMismatch;
                    state.diff_count += 1;
                }
                let a_str = match a {
                    ObjInsArg::Arg(arg) => format!("{}", arg),
                    ObjInsArg::Reloc | ObjInsArg::RelocOffset => String::new(),
                };
                let a_diff = if let Some(idx) = state.left_args_idx.get(&a_str) {
                    ObjInsArgDiff { idx: *idx }
                } else {
                    let idx = state.left_arg_idx;
                    state.left_args_idx.insert(a_str, idx);
                    state.left_arg_idx += 1;
                    ObjInsArgDiff { idx }
                };
                let b_str = match b {
                    ObjInsArg::Arg(arg) => format!("{}", arg),
                    ObjInsArg::Reloc | ObjInsArg::RelocOffset => String::new(),
                };
                let b_diff = if let Some(idx) = state.right_args_idx.get(&b_str) {
                    ObjInsArgDiff { idx: *idx }
                } else {
                    let idx = state.right_arg_idx;
                    state.right_args_idx.insert(b_str, idx);
                    state.right_arg_idx += 1;
                    ObjInsArgDiff { idx }
                };
                result.left_args_diff.push(Some(a_diff));
                result.right_args_diff.push(Some(b_diff));
            }
        }
    } else if left.ins.is_some() {
        result.kind = ObjInsDiffKind::Delete;
        state.diff_count += 1;
    } else {
        result.kind = ObjInsDiffKind::Insert;
        state.diff_count += 1;
    }
    Ok(result)
}

fn find_section<'a>(obj: &'a mut ObjInfo, name: &str) -> Option<&'a mut ObjSection> {
    obj.sections.iter_mut().find(|s| s.name == name)
}

fn find_symbol<'a>(symbols: &'a mut [ObjSymbol], name: &str) -> Option<&'a mut ObjSymbol> {
    symbols.iter_mut().find(|s| s.name == name)
}

pub fn diff_objs(left: &mut ObjInfo, right: &mut ObjInfo) -> Result<()> {
    for left_section in &mut left.sections {
        if let Some(right_section) = find_section(right, &left_section.name) {
            for left_symbol in &mut left_section.symbols {
                if let Some(right_symbol) =
                    find_symbol(&mut right_section.symbols, &left_symbol.name)
                {
                    left_symbol.diff_symbol = Some(right_symbol.name.clone());
                    right_symbol.diff_symbol = Some(left_symbol.name.clone());
                    if left_section.kind == ObjSectionKind::Code {
                        diff_code(
                            &left_section.data,
                            &right_section.data,
                            left_symbol,
                            right_symbol,
                            &left_section.relocations,
                            &right_section.relocations,
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

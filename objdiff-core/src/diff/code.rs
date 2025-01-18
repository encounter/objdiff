use std::{cmp::max, collections::BTreeMap};

use anyhow::{anyhow, Result};
use similar::{capture_diff_slices_deadline, Algorithm};

use super::FunctionRelocDiffs;
use crate::{
    arch::ProcessCodeResult,
    diff::{
        DiffObjConfig, ObjInsArgDiff, ObjInsBranchFrom, ObjInsBranchTo, ObjInsDiff, ObjInsDiffKind,
        ObjSymbolDiff,
    },
    obj::{
        ObjInfo, ObjIns, ObjInsArg, ObjReloc, ObjSection, ObjSymbol, ObjSymbolFlags, ObjSymbolKind,
        SymbolRef,
    },
};

pub fn process_code_symbol(
    obj: &ObjInfo,
    symbol_ref: SymbolRef,
    config: &DiffObjConfig,
) -> Result<ProcessCodeResult> {
    let (section, symbol) = obj.section_symbol(symbol_ref);
    let section = section.ok_or_else(|| anyhow!("Code symbol section not found"))?;
    let code = &section.data
        [symbol.section_address as usize..(symbol.section_address + symbol.size) as usize];
    let mut res = obj.arch.process_code(
        symbol.address,
        code,
        section.orig_index,
        &section.relocations,
        &section.line_info,
        config,
    )?;

    for inst in res.insts.iter_mut() {
        if let Some(reloc) = &mut inst.reloc {
            if reloc.target.size == 0 && reloc.target.name.is_empty() {
                // Fake target symbol we added as a placeholder. We need to find the real one.
                if let Some(real_target) =
                    find_symbol_matching_fake_symbol_in_sections(&reloc.target, &obj.sections)
                {
                    reloc.addend = (reloc.target.address - real_target.address) as i64;
                    reloc.target = real_target;
                }
            }
        }
    }

    Ok(res)
}

pub fn no_diff_code(out: &ProcessCodeResult, symbol_ref: SymbolRef) -> Result<ObjSymbolDiff> {
    let mut diff = Vec::<ObjInsDiff>::new();
    for i in &out.insts {
        diff.push(ObjInsDiff {
            ins: Some(i.clone()),
            kind: ObjInsDiffKind::None,
            ..Default::default()
        });
    }
    resolve_branches(&mut diff);
    Ok(ObjSymbolDiff { symbol_ref, target_symbol: None, instructions: diff, match_percent: None })
}

pub fn diff_code(
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    left_out: &ProcessCodeResult,
    right_out: &ProcessCodeResult,
    left_symbol_ref: SymbolRef,
    right_symbol_ref: SymbolRef,
    config: &DiffObjConfig,
) -> Result<(ObjSymbolDiff, ObjSymbolDiff)> {
    let mut left_diff = Vec::<ObjInsDiff>::new();
    let mut right_diff = Vec::<ObjInsDiff>::new();
    diff_instructions(&mut left_diff, &mut right_diff, left_out, right_out)?;

    resolve_branches(&mut left_diff);
    resolve_branches(&mut right_diff);

    let mut diff_state = InsDiffState::default();
    for (left, right) in left_diff.iter_mut().zip(right_diff.iter_mut()) {
        let result = compare_ins(config, left_obj, right_obj, left, right, &mut diff_state)?;
        left.kind = result.kind;
        right.kind = result.kind;
        left.arg_diff = result.left_args_diff;
        right.arg_diff = result.right_args_diff;
    }

    let total = left_out.insts.len().max(right_out.insts.len());
    let percent = if diff_state.diff_count >= total {
        0.0
    } else {
        ((total - diff_state.diff_count) as f32 / total as f32) * 100.0
    };

    Ok((
        ObjSymbolDiff {
            symbol_ref: left_symbol_ref,
            target_symbol: Some(right_symbol_ref),
            instructions: left_diff,
            match_percent: Some(percent),
        },
        ObjSymbolDiff {
            symbol_ref: right_symbol_ref,
            target_symbol: Some(left_symbol_ref),
            instructions: right_diff,
            match_percent: Some(percent),
        },
    ))
}

fn diff_instructions(
    left_diff: &mut Vec<ObjInsDiff>,
    right_diff: &mut Vec<ObjInsDiff>,
    left_code: &ProcessCodeResult,
    right_code: &ProcessCodeResult,
) -> Result<()> {
    let ops =
        capture_diff_slices_deadline(Algorithm::Patience, &left_code.ops, &right_code.ops, None);
    if ops.is_empty() {
        left_diff.extend(
            left_code
                .insts
                .iter()
                .map(|i| ObjInsDiff { ins: Some(i.clone()), ..Default::default() }),
        );
        right_diff.extend(
            right_code
                .insts
                .iter()
                .map(|i| ObjInsDiff { ins: Some(i.clone()), ..Default::default() }),
        );
        return Ok(());
    }

    for op in ops {
        let (_tag, left_range, right_range) = op.as_tag_tuple();
        let len = max(left_range.len(), right_range.len());
        left_diff.extend(
            left_code.insts[left_range.clone()]
                .iter()
                .map(|i| ObjInsDiff { ins: Some(i.clone()), ..Default::default() }),
        );
        right_diff.extend(
            right_code.insts[right_range.clone()]
                .iter()
                .map(|i| ObjInsDiff { ins: Some(i.clone()), ..Default::default() }),
        );
        if left_range.len() < len {
            left_diff.extend((left_range.len()..len).map(|_| ObjInsDiff::default()));
        }
        if right_range.len() < len {
            right_diff.extend((right_range.len()..len).map(|_| ObjInsDiff::default()));
        }
    }

    Ok(())
}

fn resolve_branches(vec: &mut [ObjInsDiff]) {
    let mut branch_idx = 0usize;
    // Map addresses to indices
    let mut addr_map = BTreeMap::<u64, usize>::new();
    for (i, ins_diff) in vec.iter().enumerate() {
        if let Some(ins) = &ins_diff.ins {
            addr_map.insert(ins.address, i);
        }
    }
    // Generate branches
    let mut branches = BTreeMap::<usize, ObjInsBranchFrom>::new();
    for (i, ins_diff) in vec.iter_mut().enumerate() {
        if let Some(ins) = &ins_diff.ins {
            if let Some(ins_idx) = ins.branch_dest.and_then(|a| addr_map.get(&a)) {
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

fn address_eq(left: &ObjReloc, right: &ObjReloc) -> bool {
    left.target.address as i64 + left.addend == right.target.address as i64 + right.addend
}

fn section_name_eq(
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    left_orig_section_index: usize,
    right_orig_section_index: usize,
) -> bool {
    let Some(left_section) =
        left_obj.sections.iter().find(|s| s.orig_index == left_orig_section_index)
    else {
        return false;
    };
    let Some(right_section) =
        right_obj.sections.iter().find(|s| s.orig_index == right_orig_section_index)
    else {
        return false;
    };
    left_section.name == right_section.name
}

fn reloc_eq(
    config: &DiffObjConfig,
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    left_ins: Option<&ObjIns>,
    right_ins: Option<&ObjIns>,
) -> bool {
    let (Some(left_ins), Some(right_ins)) = (left_ins, right_ins) else {
        return false;
    };
    let (Some(left), Some(right)) = (&left_ins.reloc, &right_ins.reloc) else {
        return false;
    };
    if left.flags != right.flags {
        return false;
    }
    if config.function_reloc_diffs == FunctionRelocDiffs::None {
        return true;
    }

    let symbol_name_matches = left.target.name == right.target.name;
    match (&left.target.orig_section_index, &right.target.orig_section_index) {
        (Some(sl), Some(sr)) => {
            // Match if section and name or address match
            section_name_eq(left_obj, right_obj, *sl, *sr)
                && (config.function_reloc_diffs == FunctionRelocDiffs::DataValue
                    || symbol_name_matches
                    || address_eq(left, right))
                && (config.function_reloc_diffs == FunctionRelocDiffs::NameAddress
                    || left.target.kind != ObjSymbolKind::Object
                    || left_obj.arch.display_ins_data(left_ins)
                        == left_obj.arch.display_ins_data(right_ins))
        }
        (Some(_), None) => false,
        (None, Some(_)) => {
            // Match if possibly stripped weak symbol
            symbol_name_matches && right.target.flags.0.contains(ObjSymbolFlags::Weak)
        }
        (None, None) => symbol_name_matches,
    }
}

fn arg_eq(
    config: &DiffObjConfig,
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    left: &ObjInsArg,
    right: &ObjInsArg,
    left_diff: &ObjInsDiff,
    right_diff: &ObjInsDiff,
) -> bool {
    match left {
        ObjInsArg::PlainText(l) => match right {
            ObjInsArg::PlainText(r) => l == r,
            _ => false,
        },
        ObjInsArg::Arg(l) => match right {
            ObjInsArg::Arg(r) => l.loose_eq(r),
            // If relocations are relaxed, match if left is a constant and right is a reloc
            // Useful for instances where the target object is created without relocations
            ObjInsArg::Reloc => config.function_reloc_diffs == FunctionRelocDiffs::None,
            _ => false,
        },
        ObjInsArg::Reloc => {
            matches!(right, ObjInsArg::Reloc)
                && reloc_eq(
                    config,
                    left_obj,
                    right_obj,
                    left_diff.ins.as_ref(),
                    right_diff.ins.as_ref(),
                )
        }
        ObjInsArg::BranchDest(_) => match right {
            // Compare dest instruction idx after diffing
            ObjInsArg::BranchDest(_) => {
                left_diff.branch_to.as_ref().map(|b| b.ins_idx)
                    == right_diff.branch_to.as_ref().map(|b| b.ins_idx)
            }
            // If relocations are relaxed, match if left is a constant and right is a reloc
            // Useful for instances where the target object is created without relocations
            ObjInsArg::Reloc => config.function_reloc_diffs == FunctionRelocDiffs::None,
            _ => false,
        },
    }
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
    config: &DiffObjConfig,
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    left: &ObjInsDiff,
    right: &ObjInsDiff,
    state: &mut InsDiffState,
) -> Result<InsDiffResult> {
    let mut result = InsDiffResult::default();
    if let (Some(left_ins), Some(right_ins)) = (&left.ins, &right.ins) {
        // Count only non-PlainText args
        let left_args_count = left_ins.iter_args().count();
        let right_args_count = right_ins.iter_args().count();
        if left_args_count != right_args_count || left_ins.op != right_ins.op {
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
        for (a, b) in left_ins.iter_args().zip(right_ins.iter_args()) {
            if arg_eq(config, left_obj, right_obj, a, b, left, right) {
                result.left_args_diff.push(None);
                result.right_args_diff.push(None);
            } else {
                if result.kind == ObjInsDiffKind::None {
                    result.kind = ObjInsDiffKind::ArgMismatch;
                    state.diff_count += 1;
                }
                let a_str = match a {
                    ObjInsArg::PlainText(arg) => arg.to_string(),
                    ObjInsArg::Arg(arg) => arg.to_string(),
                    ObjInsArg::Reloc => left_ins
                        .reloc
                        .as_ref()
                        .map_or_else(|| "<unknown>".to_string(), |r| r.target.name.clone()),
                    ObjInsArg::BranchDest(arg) => arg.to_string(),
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
                    ObjInsArg::PlainText(arg) => arg.to_string(),
                    ObjInsArg::Arg(arg) => arg.to_string(),
                    ObjInsArg::Reloc => right_ins
                        .reloc
                        .as_ref()
                        .map_or_else(|| "<unknown>".to_string(), |r| r.target.name.clone()),
                    ObjInsArg::BranchDest(arg) => arg.to_string(),
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

fn find_symbol_matching_fake_symbol_in_sections(
    fake_symbol: &ObjSymbol,
    sections: &[ObjSection],
) -> Option<ObjSymbol> {
    let orig_section_index = fake_symbol.orig_section_index?;
    let section = sections.iter().find(|s| s.orig_index == orig_section_index)?;
    let real_symbol = section
        .symbols
        .iter()
        .find(|s| s.size > 0 && (s.address..s.address + s.size).contains(&fake_symbol.address))?;
    Some(real_symbol.clone())
}

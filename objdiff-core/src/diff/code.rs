use alloc::{
    collections::{BTreeMap, btree_map},
    string::{String, ToString},
    vec,
    vec::Vec,
};

use anyhow::{Context, Result, anyhow, ensure};

use super::{
    DiffObjConfig, FunctionRelocDiffs, InstructionArgDiffIndex, InstructionBranchFrom,
    InstructionBranchTo, InstructionDiffKind, InstructionDiffRow, SymbolDiff,
    display::display_ins_data_literals,
};
use crate::obj::{
    InstructionArg, InstructionArgValue, InstructionRef, Object, ResolvedInstructionRef,
    ResolvedRelocation, ResolvedSymbol, SymbolFlag, SymbolKind,
};

pub fn no_diff_code(
    obj: &Object,
    symbol_index: usize,
    diff_config: &DiffObjConfig,
) -> Result<SymbolDiff> {
    let symbol = &obj.symbols[symbol_index];
    let section_index = symbol.section.ok_or_else(|| anyhow!("Missing section for symbol"))?;
    let section = &obj.sections[section_index];
    let data = section.data_range(symbol.address, symbol.size as usize).ok_or_else(|| {
        anyhow!(
            "Symbol data out of bounds: {:#x}..{:#x}",
            symbol.address,
            symbol.address + symbol.size
        )
    })?;
    let ops = obj.arch.scan_instructions(
        ResolvedSymbol { obj, symbol_index, symbol, section_index, section, data },
        diff_config,
    )?;
    let mut instruction_rows = Vec::<InstructionDiffRow>::new();
    for i in &ops {
        instruction_rows.push(InstructionDiffRow { ins_ref: Some(*i), ..Default::default() });
    }
    resolve_branches(&ops, &mut instruction_rows);
    Ok(SymbolDiff {
        target_symbol: None,
        match_percent: None,
        diff_score: None,
        instruction_rows,
        ..Default::default()
    })
}

const PENALTY_IMM_DIFF: u64 = 1;
const PENALTY_REG_DIFF: u64 = 5;
const PENALTY_REPLACE: u64 = 60;
const PENALTY_INSERT_DELETE: u64 = 100;

pub fn diff_code(
    left_obj: &Object,
    right_obj: &Object,
    left_symbol_idx: usize,
    right_symbol_idx: usize,
    diff_config: &DiffObjConfig,
) -> Result<(SymbolDiff, SymbolDiff)> {
    let left_symbol = &left_obj.symbols[left_symbol_idx];
    let right_symbol = &right_obj.symbols[right_symbol_idx];
    let left_section = left_symbol
        .section
        .and_then(|i| left_obj.sections.get(i))
        .ok_or_else(|| anyhow!("Missing section for symbol"))?;
    let right_section = right_symbol
        .section
        .and_then(|i| right_obj.sections.get(i))
        .ok_or_else(|| anyhow!("Missing section for symbol"))?;
    let left_data = left_section
        .data_range(left_symbol.address, left_symbol.size as usize)
        .ok_or_else(|| {
            anyhow!(
                "Symbol data out of bounds: {:#x}..{:#x}",
                left_symbol.address,
                left_symbol.address + left_symbol.size
            )
        })?;
    let right_data = right_section
        .data_range(right_symbol.address, right_symbol.size as usize)
        .ok_or_else(|| {
            anyhow!(
                "Symbol data out of bounds: {:#x}..{:#x}",
                right_symbol.address,
                right_symbol.address + right_symbol.size
            )
        })?;

    let left_section_idx = left_symbol.section.unwrap();
    let right_section_idx = right_symbol.section.unwrap();
    let left_ops = left_obj.arch.scan_instructions(
        ResolvedSymbol {
            obj: left_obj,
            symbol_index: left_symbol_idx,
            symbol: left_symbol,
            section_index: left_section_idx,
            section: left_section,
            data: left_data,
        },
        diff_config,
    )?;
    let right_ops = right_obj.arch.scan_instructions(
        ResolvedSymbol {
            obj: right_obj,
            symbol_index: right_symbol_idx,
            symbol: right_symbol,
            section_index: right_section_idx,
            section: right_section,
            data: right_data,
        },
        diff_config,
    )?;
    let (mut left_rows, mut right_rows) = diff_instructions(&left_ops, &right_ops)?;
    resolve_branches(&left_ops, &mut left_rows);
    resolve_branches(&right_ops, &mut right_rows);

    let mut diff_state = InstructionDiffState::default();
    for (left_row, right_row) in left_rows.iter_mut().zip(right_rows.iter_mut()) {
        let result = diff_instruction(
            left_obj,
            right_obj,
            left_symbol_idx,
            right_symbol_idx,
            left_row.ins_ref,
            right_row.ins_ref,
            left_row,
            right_row,
            diff_config,
            &mut diff_state,
        )?;
        left_row.kind = result.kind;
        right_row.kind = result.kind;
        left_row.arg_diff = result.left_args_diff;
        right_row.arg_diff = result.right_args_diff;
    }

    let max_score = left_ops.len() as u64 * PENALTY_INSERT_DELETE;
    let diff_score = diff_state.diff_score.min(max_score);
    let match_percent = if max_score == 0 {
        100.0
    } else {
        ((1.0 - (diff_score as f64 / max_score as f64)) * 100.0) as f32
    };

    Ok((
        SymbolDiff {
            target_symbol: Some(right_symbol_idx),
            match_percent: Some(match_percent),
            diff_score: Some((diff_score, max_score)),
            instruction_rows: left_rows,
            ..Default::default()
        },
        SymbolDiff {
            target_symbol: Some(left_symbol_idx),
            match_percent: Some(match_percent),
            diff_score: Some((diff_score, max_score)),
            instruction_rows: right_rows,
            ..Default::default()
        },
    ))
}

fn diff_instructions(
    left_insts: &[InstructionRef],
    right_insts: &[InstructionRef],
) -> Result<(Vec<InstructionDiffRow>, Vec<InstructionDiffRow>)> {
    let left_ops = left_insts.iter().map(|i| i.opcode).collect::<Vec<_>>();
    let right_ops = right_insts.iter().map(|i| i.opcode).collect::<Vec<_>>();
    let ops = similar::capture_diff_slices(similar::Algorithm::Patience, &left_ops, &right_ops);
    if ops.is_empty() {
        ensure!(left_insts.len() == right_insts.len());
        let left_diff = left_insts
            .iter()
            .map(|i| InstructionDiffRow { ins_ref: Some(*i), ..Default::default() })
            .collect();
        let right_diff = right_insts
            .iter()
            .map(|i| InstructionDiffRow { ins_ref: Some(*i), ..Default::default() })
            .collect();
        return Ok((left_diff, right_diff));
    }

    let row_count = ops
        .iter()
        .map(|op| match *op {
            similar::DiffOp::Equal { len, .. } => len,
            similar::DiffOp::Delete { old_len, .. } => old_len,
            similar::DiffOp::Insert { new_len, .. } => new_len,
            similar::DiffOp::Replace { old_len, new_len, .. } => old_len.max(new_len),
        })
        .sum();
    let mut left_diff = Vec::<InstructionDiffRow>::with_capacity(row_count);
    let mut right_diff = Vec::<InstructionDiffRow>::with_capacity(row_count);
    for op in ops {
        let (_tag, left_range, right_range) = op.as_tag_tuple();
        let len = left_range.len().max(right_range.len());
        left_diff.extend(
            left_range
                .clone()
                .map(|i| InstructionDiffRow { ins_ref: Some(left_insts[i]), ..Default::default() }),
        );
        right_diff.extend(
            right_range.clone().map(|i| InstructionDiffRow {
                ins_ref: Some(right_insts[i]),
                ..Default::default()
            }),
        );
        if left_range.len() < len {
            left_diff.extend((left_range.len()..len).map(|_| InstructionDiffRow::default()));
        }
        if right_range.len() < len {
            right_diff.extend((right_range.len()..len).map(|_| InstructionDiffRow::default()));
        }
    }
    Ok((left_diff, right_diff))
}

fn arg_to_string(arg: &InstructionArg, reloc: Option<ResolvedRelocation>) -> String {
    match arg {
        InstructionArg::Value(arg) => arg.to_string(),
        InstructionArg::Reloc => {
            reloc.as_ref().map_or_else(|| "<unknown>".to_string(), |r| r.symbol.name.clone())
        }
        InstructionArg::BranchDest(arg) => arg.to_string(),
    }
}

fn resolve_branches(ops: &[InstructionRef], rows: &mut [InstructionDiffRow]) {
    let mut branch_idx = 0u32;
    // Map addresses to indices
    let mut addr_map = BTreeMap::<u64, u32>::new();
    for (i, ins_diff) in rows.iter().enumerate() {
        if let Some(ins) = ins_diff.ins_ref {
            addr_map.insert(ins.address, i as u32);
        }
    }
    // Generate branches
    let mut branches = BTreeMap::<u32, InstructionBranchFrom>::new();
    for ((i, ins_diff), ins) in
        rows.iter_mut().enumerate().filter(|(_, row)| row.ins_ref.is_some()).zip(ops)
    {
        if let Some(ins_idx) = ins.branch_dest.and_then(|a| addr_map.get(&a).copied()) {
            match branches.entry(ins_idx) {
                btree_map::Entry::Vacant(e) => {
                    ins_diff.branch_to = Some(InstructionBranchTo { ins_idx, branch_idx });
                    e.insert(InstructionBranchFrom { ins_idx: vec![i as u32], branch_idx });
                    branch_idx += 1;
                }
                btree_map::Entry::Occupied(e) => {
                    let branch = e.into_mut();
                    ins_diff.branch_to =
                        Some(InstructionBranchTo { ins_idx, branch_idx: branch.branch_idx });
                    branch.ins_idx.push(i as u32);
                }
            }
        }
    }
    // Store branch from
    for (i, branch) in branches {
        rows[i as usize].branch_from = Some(branch);
    }
}

pub(crate) fn address_eq(left: ResolvedRelocation, right: ResolvedRelocation) -> bool {
    if right.symbol.size == 0 && left.symbol.size != 0 {
        // The base relocation is against a pool but the target relocation isn't.
        // This can happen in rare cases where the compiler will generate a pool+addend relocation
        // in the base's data, but the one detected in the target is direct with no addend.
        // Just check that the final address is the same so these count as a match.
        left.symbol.address as i64 + left.relocation.addend
            == right.symbol.address as i64 + right.relocation.addend
    } else {
        // But otherwise, if the compiler isn't using a pool, we're more strict and check that the
        // target symbol address and relocation addend both match exactly.
        left.symbol.address == right.symbol.address
            && left.relocation.addend == right.relocation.addend
    }
}

pub(crate) fn section_name_eq(
    left_obj: &Object,
    right_obj: &Object,
    left_section_index: usize,
    right_section_index: usize,
) -> bool {
    left_obj.sections.get(left_section_index).is_some_and(|left_section| {
        right_obj
            .sections
            .get(right_section_index)
            .is_some_and(|right_section| left_section.name == right_section.name)
    })
}

fn reloc_eq(
    left_obj: &Object,
    right_obj: &Object,
    left_ins: ResolvedInstructionRef,
    right_ins: ResolvedInstructionRef,
    diff_config: &DiffObjConfig,
) -> bool {
    let relax_reloc_diffs = diff_config.function_reloc_diffs == FunctionRelocDiffs::None;
    let (left_reloc, right_reloc) = match (left_ins.relocation, right_ins.relocation) {
        (Some(left_reloc), Some(right_reloc)) => (left_reloc, right_reloc),
        // If relocations are relaxed, match if left is missing a reloc
        (None, Some(_)) => return relax_reloc_diffs,
        (None, None) => return true,
        _ => return false,
    };
    if left_reloc.relocation.flags != right_reloc.relocation.flags {
        return false;
    }
    if relax_reloc_diffs {
        return true;
    }

    let symbol_name_addend_matches = left_reloc.symbol.name == right_reloc.symbol.name
        && left_reloc.relocation.addend == right_reloc.relocation.addend;
    match (&left_reloc.symbol.section, &right_reloc.symbol.section) {
        (Some(sl), Some(sr)) => {
            // Match if section and name or address match
            section_name_eq(left_obj, right_obj, *sl, *sr)
                && (diff_config.function_reloc_diffs == FunctionRelocDiffs::DataValue
                    || symbol_name_addend_matches
                    || address_eq(left_reloc, right_reloc))
                && (diff_config.function_reloc_diffs == FunctionRelocDiffs::NameAddress
                    || left_reloc.symbol.kind != SymbolKind::Object
                    || right_reloc.symbol.size == 0 // Likely a pool symbol like ...data, don't treat this as a diff
                    || display_ins_data_literals(left_obj, left_ins)
                        == display_ins_data_literals(right_obj, right_ins))
        }
        (None, Some(_)) => {
            // Match if possibly stripped weak symbol
            symbol_name_addend_matches && right_reloc.symbol.flags.contains(SymbolFlag::Weak)
        }
        (Some(_), None) | (None, None) => symbol_name_addend_matches,
    }
}

fn arg_eq(
    left_obj: &Object,
    right_obj: &Object,
    left_row: &InstructionDiffRow,
    right_row: &InstructionDiffRow,
    left_arg: &InstructionArg,
    right_arg: &InstructionArg,
    left_ins: ResolvedInstructionRef,
    right_ins: ResolvedInstructionRef,
    diff_config: &DiffObjConfig,
) -> bool {
    match left_arg {
        InstructionArg::Value(l) => match right_arg {
            InstructionArg::Value(r) => l.loose_eq(r),
            // If relocations are relaxed, match if left is a constant and right is a reloc
            // Useful for instances where the target object is created without relocations
            InstructionArg::Reloc => diff_config.function_reloc_diffs == FunctionRelocDiffs::None,
            _ => false,
        },
        InstructionArg::Reloc => {
            matches!(right_arg, InstructionArg::Reloc)
                && reloc_eq(left_obj, right_obj, left_ins, right_ins, diff_config)
        }
        InstructionArg::BranchDest(_) => match right_arg {
            // Compare dest instruction idx after diffing
            InstructionArg::BranchDest(_) => {
                left_row.branch_to.as_ref().map(|b| b.ins_idx)
                    == right_row.branch_to.as_ref().map(|b| b.ins_idx)
            }
            // If relocations are relaxed, match if left is a constant and right is a reloc
            // Useful for instances where the target object is created without relocations
            InstructionArg::Reloc => diff_config.function_reloc_diffs == FunctionRelocDiffs::None,
            _ => false,
        },
    }
}

#[derive(Default)]
struct InstructionDiffState {
    diff_score: u64,
    left_arg_idx: u32,
    right_arg_idx: u32,
    left_args_idx: BTreeMap<String, u32>,
    right_args_idx: BTreeMap<String, u32>,
}

#[derive(Default)]
struct InstructionDiffResult {
    kind: InstructionDiffKind,
    left_args_diff: Vec<InstructionArgDiffIndex>,
    right_args_diff: Vec<InstructionArgDiffIndex>,
}

impl InstructionDiffResult {
    #[inline]
    const fn new(kind: InstructionDiffKind) -> Self {
        Self { kind, left_args_diff: Vec::new(), right_args_diff: Vec::new() }
    }
}

fn diff_instruction(
    left_obj: &Object,
    right_obj: &Object,
    left_symbol_idx: usize,
    right_symbol_idx: usize,
    l: Option<InstructionRef>,
    r: Option<InstructionRef>,
    left_row: &InstructionDiffRow,
    right_row: &InstructionDiffRow,
    diff_config: &DiffObjConfig,
    state: &mut InstructionDiffState,
) -> Result<InstructionDiffResult> {
    let (l, r) = match (l, r) {
        (Some(l), Some(r)) => (l, r),
        (Some(_), None) => {
            state.diff_score += PENALTY_INSERT_DELETE;
            return Ok(InstructionDiffResult::new(InstructionDiffKind::Delete));
        }
        (None, Some(_)) => {
            state.diff_score += PENALTY_INSERT_DELETE;
            return Ok(InstructionDiffResult::new(InstructionDiffKind::Insert));
        }
        (None, None) => return Ok(InstructionDiffResult::new(InstructionDiffKind::None)),
    };

    // If opcodes don't match, replace
    if l.opcode != r.opcode {
        state.diff_score += PENALTY_REPLACE;
        return Ok(InstructionDiffResult::new(InstructionDiffKind::Replace));
    }

    let left_resolved = left_obj
        .resolve_instruction_ref(left_symbol_idx, l)
        .context("Failed to resolve left instruction")?;
    let right_resolved = right_obj
        .resolve_instruction_ref(right_symbol_idx, r)
        .context("Failed to resolve right instruction")?;

    if left_resolved.code != right_resolved.code
        || !reloc_eq(left_obj, right_obj, left_resolved, right_resolved, diff_config)
    {
        // If either the raw code bytes or relocations don't match, process instructions and compare args
        let left_ins = left_obj.arch.process_instruction(left_resolved, diff_config)?;
        let right_ins = right_obj.arch.process_instruction(right_resolved, diff_config)?;
        if left_ins.args.len() != right_ins.args.len() {
            state.diff_score += PENALTY_REPLACE;
            return Ok(InstructionDiffResult::new(InstructionDiffKind::Replace));
        }
        let mut result = InstructionDiffResult::new(InstructionDiffKind::None);
        if left_ins.mnemonic != right_ins.mnemonic {
            state.diff_score += PENALTY_REG_DIFF;
            result.kind = InstructionDiffKind::OpMismatch;
        }
        for (a, b) in left_ins.args.iter().zip(right_ins.args.iter()) {
            if arg_eq(
                left_obj,
                right_obj,
                left_row,
                right_row,
                a,
                b,
                left_resolved,
                right_resolved,
                diff_config,
            ) {
                result.left_args_diff.push(InstructionArgDiffIndex::NONE);
                result.right_args_diff.push(InstructionArgDiffIndex::NONE);
            } else {
                state.diff_score += if let InstructionArg::Value(
                    InstructionArgValue::Signed(_) | InstructionArgValue::Unsigned(_),
                ) = a
                {
                    PENALTY_IMM_DIFF
                } else {
                    PENALTY_REG_DIFF
                };
                if result.kind == InstructionDiffKind::None {
                    result.kind = InstructionDiffKind::ArgMismatch;
                }
                let a_str = arg_to_string(a, left_resolved.relocation);
                let a_diff = match state.left_args_idx.entry(a_str) {
                    btree_map::Entry::Vacant(e) => {
                        let idx = state.left_arg_idx;
                        state.left_arg_idx = idx + 1;
                        e.insert(idx);
                        idx
                    }
                    btree_map::Entry::Occupied(e) => *e.get(),
                };
                let b_str = arg_to_string(b, right_resolved.relocation);
                let b_diff = match state.right_args_idx.entry(b_str) {
                    btree_map::Entry::Vacant(e) => {
                        let idx = state.right_arg_idx;
                        state.right_arg_idx = idx + 1;
                        e.insert(idx);
                        idx
                    }
                    btree_map::Entry::Occupied(e) => *e.get(),
                };
                result.left_args_diff.push(InstructionArgDiffIndex::new(a_diff));
                result.right_args_diff.push(InstructionArgDiffIndex::new(b_diff));
            }
        }
        if result.kind == InstructionDiffKind::None
            && left_resolved.code.len() != right_resolved.code.len()
        {
            // If everything else matches but the raw code length differs (e.g. x86 instructions
            // with same disassembly but different encoding), mark as op mismatch
            result.kind = InstructionDiffKind::OpMismatch;
            state.diff_score += PENALTY_REG_DIFF;
        }
        return Ok(result);
    }

    Ok(InstructionDiffResult::new(InstructionDiffKind::None))
}

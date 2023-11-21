use std::{
    cmp::max,
    collections::BTreeMap,
    time::{Duration, Instant},
};

use anyhow::Result;
use similar::{capture_diff_slices_deadline, Algorithm};

use crate::{
    diff::{
        editops::{editops_find, LevEditType},
        DiffAlg, ProcessCodeResult,
    },
    obj::{
        mips, ppc, ObjArchitecture, ObjInfo, ObjInsArg, ObjInsArgDiff, ObjInsBranchFrom,
        ObjInsBranchTo, ObjInsDiff, ObjInsDiffKind, ObjReloc, ObjSymbol, ObjSymbolFlags,
    },
};

pub fn no_diff_code(
    arch: ObjArchitecture,
    data: &[u8],
    symbol: &mut ObjSymbol,
    relocs: &[ObjReloc],
    line_info: &Option<BTreeMap<u32, u32>>,
) -> Result<()> {
    let code =
        &data[symbol.section_address as usize..(symbol.section_address + symbol.size) as usize];
    let out = match arch {
        ObjArchitecture::PowerPc => ppc::process_code(code, symbol.address, relocs, line_info)?,
        ObjArchitecture::Mips => mips::process_code(
            code,
            symbol.address,
            symbol.address + symbol.size,
            relocs,
            line_info,
        )?,
    };

    let mut diff = Vec::<ObjInsDiff>::new();
    for i in out.insts {
        diff.push(ObjInsDiff { ins: Some(i), kind: ObjInsDiffKind::None, ..Default::default() });
    }
    resolve_branches(&mut diff);
    symbol.instructions = diff;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn diff_code(
    alg: DiffAlg,
    arch: ObjArchitecture,
    left_data: &[u8],
    right_data: &[u8],
    left_symbol: &mut ObjSymbol,
    right_symbol: &mut ObjSymbol,
    left_relocs: &[ObjReloc],
    right_relocs: &[ObjReloc],
    left_line_info: &Option<BTreeMap<u32, u32>>,
    right_line_info: &Option<BTreeMap<u32, u32>>,
) -> Result<()> {
    let left_code = &left_data[left_symbol.section_address as usize
        ..(left_symbol.section_address + left_symbol.size) as usize];
    let right_code = &right_data[right_symbol.section_address as usize
        ..(right_symbol.section_address + right_symbol.size) as usize];
    let (left_out, right_out) = match arch {
        ObjArchitecture::PowerPc => (
            ppc::process_code(left_code, left_symbol.address, left_relocs, left_line_info)?,
            ppc::process_code(right_code, right_symbol.address, right_relocs, right_line_info)?,
        ),
        ObjArchitecture::Mips => (
            mips::process_code(
                left_code,
                left_symbol.address,
                left_symbol.address + left_symbol.size,
                left_relocs,
                left_line_info,
            )?,
            mips::process_code(
                right_code,
                right_symbol.address,
                left_symbol.address + left_symbol.size,
                right_relocs,
                right_line_info,
            )?,
        ),
    };

    let mut left_diff = Vec::<ObjInsDiff>::new();
    let mut right_diff = Vec::<ObjInsDiff>::new();
    match alg {
        DiffAlg::Levenshtein => {
            diff_instructions_lev(
                &mut left_diff,
                &mut right_diff,
                left_symbol,
                right_symbol,
                &left_out,
                &right_out,
            )?;
        }
        DiffAlg::Lcs => {
            diff_instructions_similar(
                Algorithm::Lcs,
                &mut left_diff,
                &mut right_diff,
                &left_out,
                &right_out,
            )?;
        }
        DiffAlg::Myers => {
            diff_instructions_similar(
                Algorithm::Myers,
                &mut left_diff,
                &mut right_diff,
                &left_out,
                &right_out,
            )?;
        }
        DiffAlg::Patience => {
            diff_instructions_similar(
                Algorithm::Patience,
                &mut left_diff,
                &mut right_diff,
                &left_out,
                &right_out,
            )?;
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

    let total = left_out.insts.len();
    let percent = if diff_state.diff_count >= total {
        0.0
    } else {
        ((total - diff_state.diff_count) as f32 / total as f32) * 100.0
    };
    left_symbol.match_percent = Some(percent);
    right_symbol.match_percent = Some(percent);

    left_symbol.instructions = left_diff;
    right_symbol.instructions = right_diff;

    Ok(())
}

fn diff_instructions_similar(
    alg: Algorithm,
    left_diff: &mut Vec<ObjInsDiff>,
    right_diff: &mut Vec<ObjInsDiff>,
    left_code: &ProcessCodeResult,
    right_code: &ProcessCodeResult,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let ops = capture_diff_slices_deadline(alg, &left_code.ops, &right_code.ops, Some(deadline));
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

fn diff_instructions_lev(
    left_diff: &mut Vec<ObjInsDiff>,
    right_diff: &mut Vec<ObjInsDiff>,
    left_symbol: &ObjSymbol,
    right_symbol: &ObjSymbol,
    left_code: &ProcessCodeResult,
    right_code: &ProcessCodeResult,
) -> Result<()> {
    let edit_ops = editops_find(&left_code.ops, &right_code.ops);

    let mut op_iter = edit_ops.iter();
    let mut left_iter = left_code.insts.iter();
    let mut right_iter = right_code.insts.iter();
    let mut cur_op = op_iter.next();
    let mut cur_left = left_iter.next();
    let mut cur_right = right_iter.next();
    while let Some(op) = cur_op {
        let left_addr = op.first_start as u32 * 4;
        let right_addr = op.second_start as u32 * 4;
        while let (Some(left), Some(right)) = (cur_left, cur_right) {
            if (left.address - left_symbol.address as u32) < left_addr {
                left_diff.push(ObjInsDiff { ins: Some(left.clone()), ..ObjInsDiff::default() });
                right_diff.push(ObjInsDiff { ins: Some(right.clone()), ..ObjInsDiff::default() });
            } else {
                break;
            }
            cur_left = left_iter.next();
            cur_right = right_iter.next();
        }
        if let (Some(left), Some(right)) = (cur_left, cur_right) {
            if (left.address - left_symbol.address as u32) != left_addr {
                return Err(anyhow::Error::msg("Instruction address mismatch (left)"));
            }
            if (right.address - right_symbol.address as u32) != right_addr {
                return Err(anyhow::Error::msg("Instruction address mismatch (right)"));
            }
            match op.op_type {
                LevEditType::Replace => {
                    left_diff.push(ObjInsDiff { ins: Some(left.clone()), ..ObjInsDiff::default() });
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
                    left_diff.push(ObjInsDiff { ins: Some(left.clone()), ..ObjInsDiff::default() });
                    right_diff.push(ObjInsDiff::default());
                    cur_left = left_iter.next();
                }
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

    Ok(())
}

fn resolve_branches(vec: &mut [ObjInsDiff]) {
    let mut branch_idx = 0usize;
    // Map addresses to indices
    let mut addr_map = BTreeMap::<u32, usize>::new();
    for (i, ins_diff) in vec.iter().enumerate() {
        if let Some(ins) = &ins_diff.ins {
            addr_map.insert(ins.address, i);
        }
    }
    // Generate branches
    let mut branches = BTreeMap::<usize, ObjInsBranchFrom>::new();
    for (i, ins_diff) in vec.iter_mut().enumerate() {
        if let Some(ins) = &ins_diff.ins {
            // if ins.ins.is_blr() || ins.reloc.is_some() {
            //     continue;
            // }
            if let Some(ins_idx) = ins
                .args
                .iter()
                .find_map(|a| if let ObjInsArg::BranchOffset(offs) = a { Some(offs) } else { None })
                .and_then(|offs| addr_map.get(&((ins.address as i32 + offs) as u32)))
            {
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

fn address_eq(left: &ObjSymbol, right: &ObjSymbol) -> bool {
    left.address as i64 + left.addend == right.address as i64 + right.addend
}

fn reloc_eq(left_reloc: Option<&ObjReloc>, right_reloc: Option<&ObjReloc>) -> bool {
    let (Some(left), Some(right)) = (left_reloc, right_reloc) else {
        return false;
    };
    if left.kind != right.kind {
        return false;
    }

    let name_matches = left.target.name == right.target.name;
    match (&left.target_section, &right.target_section) {
        (Some(sl), Some(sr)) => {
            // Match if section and name or address match
            sl == sr && (name_matches || address_eq(&left.target, &right.target))
        }
        (Some(_), None) => false,
        (None, Some(_)) => {
            // Match if possibly stripped weak symbol
            name_matches && right.target.flags.0.contains(ObjSymbolFlags::Weak)
        }
        (None, None) => name_matches,
    }
}

fn arg_eq(
    left: &ObjInsArg,
    right: &ObjInsArg,
    left_diff: &ObjInsDiff,
    right_diff: &ObjInsDiff,
) -> bool {
    return match left {
        ObjInsArg::PpcArg(l) => match right {
            ObjInsArg::PpcArg(r) => format!("{l}") == format!("{r}"),
            _ => false,
        },
        ObjInsArg::Reloc => {
            matches!(right, ObjInsArg::Reloc)
                && reloc_eq(
                    left_diff.ins.as_ref().and_then(|i| i.reloc.as_ref()),
                    right_diff.ins.as_ref().and_then(|i| i.reloc.as_ref()),
                )
        }
        ObjInsArg::RelocWithBase => {
            matches!(right, ObjInsArg::RelocWithBase)
                && reloc_eq(
                    left_diff.ins.as_ref().and_then(|i| i.reloc.as_ref()),
                    right_diff.ins.as_ref().and_then(|i| i.reloc.as_ref()),
                )
        }
        ObjInsArg::MipsArg(ls) | ObjInsArg::MipsArgWithBase(ls) => {
            matches!(right, ObjInsArg::MipsArg(rs) | ObjInsArg::MipsArgWithBase(rs) if ls == rs)
        }
        ObjInsArg::BranchOffset(_) => {
            // Compare dest instruction idx after diffing
            left_diff.branch_to.as_ref().map(|b| b.ins_idx)
                == right_diff.branch_to.as_ref().map(|b| b.ins_idx)
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
        if left_ins.args.len() != right_ins.args.len() || left_ins.op != right_ins.op {
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
                    ObjInsArg::PpcArg(arg) => format!("{arg}"),
                    ObjInsArg::Reloc | ObjInsArg::RelocWithBase => String::new(),
                    ObjInsArg::MipsArg(str) | ObjInsArg::MipsArgWithBase(str) => str.clone(),
                    ObjInsArg::BranchOffset(arg) => format!("{arg}"),
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
                    ObjInsArg::PpcArg(arg) => format!("{arg}"),
                    ObjInsArg::Reloc | ObjInsArg::RelocWithBase => String::new(),
                    ObjInsArg::MipsArg(str) | ObjInsArg::MipsArgWithBase(str) => str.clone(),
                    ObjInsArg::BranchOffset(arg) => format!("{arg}"),
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

pub fn find_section_and_symbol(obj: &ObjInfo, name: &str) -> Option<(usize, usize)> {
    for (section_idx, section) in obj.sections.iter().enumerate() {
        let symbol_idx = match section.symbols.iter().position(|symbol| symbol.name == name) {
            Some(symbol_idx) => symbol_idx,
            None => continue,
        };
        return Some((section_idx, symbol_idx));
    }
    None
}

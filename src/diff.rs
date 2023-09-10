use std::{collections::BTreeMap, mem::take};

use anyhow::Result;

use crate::{
    editops::{editops_find, LevEditType},
    obj::{
        mips, ppc, ObjArchitecture, ObjDataDiff, ObjDataDiffKind, ObjInfo, ObjInsArg,
        ObjInsArgDiff, ObjInsBranchFrom, ObjInsBranchTo, ObjInsDiff, ObjInsDiffKind, ObjReloc,
        ObjSection, ObjSectionKind, ObjSymbol, ObjSymbolFlags,
    },
};

fn no_diff_code(
    arch: ObjArchitecture,
    data: &[u8],
    symbol: &mut ObjSymbol,
    relocs: &[ObjReloc],
    line_info: &Option<BTreeMap<u32, u32>>,
) -> Result<()> {
    let code =
        &data[symbol.section_address as usize..(symbol.section_address + symbol.size) as usize];
    let (_, ins) = match arch {
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
    for i in ins {
        diff.push(ObjInsDiff { ins: Some(i), kind: ObjInsDiffKind::None, ..Default::default() });
    }
    resolve_branches(&mut diff);
    symbol.instructions = diff;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn diff_code(
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
    let ((left_ops, left_insts), (right_ops, right_insts)) = match arch {
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
                if (left.address - left_symbol.address as u32) < left_addr {
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
                if (left.address - left_symbol.address as u32) != left_addr {
                    return Err(anyhow::Error::msg("Instruction address mismatch (left)"));
                }
                if (right.address - right_symbol.address as u32) != right_addr {
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

fn find_section_and_symbol(obj: &ObjInfo, name: &str) -> Option<(usize, usize)> {
    for (section_idx, section) in obj.sections.iter().enumerate() {
        let symbol_idx = match section.symbols.iter().position(|symbol| symbol.name == name) {
            Some(symbol_idx) => symbol_idx,
            None => continue,
        };
        return Some((section_idx, symbol_idx));
    }
    None
}

pub fn diff_objs(mut left: Option<&mut ObjInfo>, mut right: Option<&mut ObjInfo>) -> Result<()> {
    if let Some(left) = left.as_mut() {
        for left_section in &mut left.sections {
            if left_section.kind == ObjSectionKind::Code {
                for left_symbol in &mut left_section.symbols {
                    if let Some((right, (right_section_idx, right_symbol_idx))) =
                        right.as_mut().and_then(|obj| {
                            find_section_and_symbol(obj, &left_symbol.name).map(|s| (obj, s))
                        })
                    {
                        let right_section = &mut right.sections[right_section_idx];
                        let right_symbol = &mut right_section.symbols[right_symbol_idx];
                        left_symbol.diff_symbol = Some(right_symbol.name.clone());
                        right_symbol.diff_symbol = Some(left_symbol.name.clone());
                        diff_code(
                            left.architecture,
                            &left_section.data,
                            &right_section.data,
                            left_symbol,
                            right_symbol,
                            &left_section.relocations,
                            &right_section.relocations,
                            &left.line_info,
                            &right.line_info,
                        )?;
                    } else {
                        no_diff_code(
                            left.architecture,
                            &left_section.data,
                            left_symbol,
                            &left_section.relocations,
                            &left.line_info,
                        )?;
                    }
                }
            } else if let Some(right_section) = right
                .as_mut()
                .and_then(|obj| obj.sections.iter_mut().find(|s| s.name == left_section.name))
            {
                if left_section.kind == ObjSectionKind::Data {
                    diff_data(left_section, right_section);
                    // diff_data_symbols(left_section, right_section)?;
                } else if left_section.kind == ObjSectionKind::Bss {
                    diff_bss_symbols(&mut left_section.symbols, &mut right_section.symbols)?;
                }
            } else if left_section.kind == ObjSectionKind::Data {
                no_diff_data(left_section);
            }
        }
    }
    if let Some(right) = right.as_mut() {
        for right_section in right.sections.iter_mut() {
            if right_section.kind == ObjSectionKind::Code {
                for right_symbol in &mut right_section.symbols {
                    if right_symbol.instructions.is_empty() {
                        no_diff_code(
                            right.architecture,
                            &right_section.data,
                            right_symbol,
                            &right_section.relocations,
                            &right.line_info,
                        )?;
                    }
                }
            } else if right_section.kind == ObjSectionKind::Data
                && right_section.data_diff.is_empty()
            {
                no_diff_data(right_section);
            }
        }
    }
    if let (Some(left), Some(right)) = (left, right) {
        diff_bss_symbols(&mut left.common, &mut right.common)?;
    }
    Ok(())
}

fn diff_bss_symbols(left_symbols: &mut [ObjSymbol], right_symbols: &mut [ObjSymbol]) -> Result<()> {
    for left_symbol in left_symbols {
        if let Some(right_symbol) = right_symbols.iter_mut().find(|s| s.name == left_symbol.name) {
            left_symbol.diff_symbol = Some(right_symbol.name.clone());
            right_symbol.diff_symbol = Some(left_symbol.name.clone());
            let percent = if left_symbol.size == right_symbol.size { 100.0 } else { 50.0 };
            left_symbol.match_percent = Some(percent);
            right_symbol.match_percent = Some(percent);
        }
    }
    Ok(())
}

// WIP diff-by-symbol
#[allow(dead_code)]
fn diff_data_symbols(left: &mut ObjSection, right: &mut ObjSection) -> Result<()> {
    let mut left_ops = Vec::<u32>::with_capacity(left.symbols.len());
    let mut right_ops = Vec::<u32>::with_capacity(right.symbols.len());
    for left_symbol in &left.symbols {
        let data = &left.data
            [left_symbol.address as usize..(left_symbol.address + left_symbol.size) as usize];
        let hash = twox_hash::xxh3::hash64(data);
        left_ops.push(hash as u32);
    }
    for symbol in &right.symbols {
        let data = &right.data[symbol.address as usize..(symbol.address + symbol.size) as usize];
        let hash = twox_hash::xxh3::hash64(data);
        right_ops.push(hash as u32);
    }

    let edit_ops = editops_find(&left_ops, &right_ops);
    if edit_ops.is_empty() && !left.data.is_empty() {
        let mut left_iter = left.symbols.iter_mut();
        let mut right_iter = right.symbols.iter_mut();
        loop {
            let (left_symbol, right_symbol) = match (left_iter.next(), right_iter.next()) {
                (Some(l), Some(r)) => (l, r),
                (None, None) => break,
                _ => return Err(anyhow::Error::msg("L/R mismatch in diff_data_symbols")),
            };
            let left_data = &left.data
                [left_symbol.address as usize..(left_symbol.address + left_symbol.size) as usize];
            let right_data = &right.data[right_symbol.address as usize
                ..(right_symbol.address + right_symbol.size) as usize];

            left.data_diff.push(ObjDataDiff {
                data: left_data.to_vec(),
                kind: ObjDataDiffKind::None,
                len: left_symbol.size as usize,
                symbol: left_symbol.name.clone(),
            });
            right.data_diff.push(ObjDataDiff {
                data: right_data.to_vec(),
                kind: ObjDataDiffKind::None,
                len: right_symbol.size as usize,
                symbol: right_symbol.name.clone(),
            });
            left_symbol.diff_symbol = Some(right_symbol.name.clone());
            left_symbol.match_percent = Some(100.0);
            right_symbol.diff_symbol = Some(left_symbol.name.clone());
            right_symbol.match_percent = Some(100.0);
        }
        return Ok(());
    }
    Ok(())
}

fn diff_data(left: &mut ObjSection, right: &mut ObjSection) {
    let edit_ops = editops_find(&left.data, &right.data);
    if edit_ops.is_empty() && !left.data.is_empty() {
        left.data_diff = vec![ObjDataDiff {
            data: left.data.clone(),
            kind: ObjDataDiffKind::None,
            len: left.data.len(),
            symbol: String::new(),
        }];
        right.data_diff = vec![ObjDataDiff {
            data: right.data.clone(),
            kind: ObjDataDiffKind::None,
            len: right.data.len(),
            symbol: String::new(),
        }];
        return;
    }

    let mut left_diff = Vec::<ObjDataDiff>::new();
    let mut right_diff = Vec::<ObjDataDiff>::new();
    let mut left_cur = 0usize;
    let mut right_cur = 0usize;
    let mut cur_op = LevEditType::Replace;
    let mut cur_left_data = Vec::<u8>::new();
    let mut cur_right_data = Vec::<u8>::new();
    for op in edit_ops {
        if cur_op != op.op_type || left_cur < op.first_start || right_cur < op.second_start {
            match cur_op {
                LevEditType::Replace => {
                    let left_data = take(&mut cur_left_data);
                    let right_data = take(&mut cur_right_data);
                    let left_data_len = left_data.len();
                    let right_data_len = right_data.len();
                    left_diff.push(ObjDataDiff {
                        data: left_data,
                        kind: ObjDataDiffKind::Replace,
                        len: left_data_len,
                        symbol: String::new(),
                    });
                    right_diff.push(ObjDataDiff {
                        data: right_data,
                        kind: ObjDataDiffKind::Replace,
                        len: right_data_len,
                        symbol: String::new(),
                    });
                }
                LevEditType::Insert => {
                    let right_data = take(&mut cur_right_data);
                    let right_data_len = right_data.len();
                    left_diff.push(ObjDataDiff {
                        data: vec![],
                        kind: ObjDataDiffKind::Insert,
                        len: right_data_len,
                        symbol: String::new(),
                    });
                    right_diff.push(ObjDataDiff {
                        data: right_data,
                        kind: ObjDataDiffKind::Insert,
                        len: right_data_len,
                        symbol: String::new(),
                    });
                }
                LevEditType::Delete => {
                    let left_data = take(&mut cur_left_data);
                    let left_data_len = left_data.len();
                    left_diff.push(ObjDataDiff {
                        data: left_data,
                        kind: ObjDataDiffKind::Delete,
                        len: left_data_len,
                        symbol: String::new(),
                    });
                    right_diff.push(ObjDataDiff {
                        data: vec![],
                        kind: ObjDataDiffKind::Delete,
                        len: left_data_len,
                        symbol: String::new(),
                    });
                }
            }
        }
        if left_cur < op.first_start {
            left_diff.push(ObjDataDiff {
                data: left.data[left_cur..op.first_start].to_vec(),
                kind: ObjDataDiffKind::None,
                len: op.first_start - left_cur,
                symbol: String::new(),
            });
            left_cur = op.first_start;
        }
        if right_cur < op.second_start {
            right_diff.push(ObjDataDiff {
                data: right.data[right_cur..op.second_start].to_vec(),
                kind: ObjDataDiffKind::None,
                len: op.second_start - right_cur,
                symbol: String::new(),
            });
            right_cur = op.second_start;
        }
        match op.op_type {
            LevEditType::Replace => {
                cur_left_data.push(left.data[left_cur]);
                cur_right_data.push(right.data[right_cur]);
                left_cur += 1;
                right_cur += 1;
            }
            LevEditType::Insert => {
                cur_right_data.push(right.data[right_cur]);
                right_cur += 1;
            }
            LevEditType::Delete => {
                cur_left_data.push(left.data[left_cur]);
                left_cur += 1;
            }
        }
        cur_op = op.op_type;
    }
    // if left_cur < left.data.len() {
    //     let len = left.data.len() - left_cur;
    //     left_diff.push(ObjDataDiff {
    //         data: left.data[left_cur..].to_vec(),
    //         kind: ObjDataDiffKind::Delete,
    //         len,
    //     });
    //     right_diff.push(ObjDataDiff { data: vec![], kind: ObjDataDiffKind::Delete, len });
    // } else if right_cur < right.data.len() {
    //     let len = right.data.len() - right_cur;
    //     left_diff.push(ObjDataDiff { data: vec![], kind: ObjDataDiffKind::Insert, len });
    //     right_diff.push(ObjDataDiff {
    //         data: right.data[right_cur..].to_vec(),
    //         kind: ObjDataDiffKind::Insert,
    //         len,
    //     });
    // }

    // TODO: merge with above
    match cur_op {
        LevEditType::Replace => {
            let left_data = take(&mut cur_left_data);
            let right_data = take(&mut cur_right_data);
            let left_data_len = left_data.len();
            let right_data_len = right_data.len();
            left_diff.push(ObjDataDiff {
                data: left_data,
                kind: ObjDataDiffKind::Replace,
                len: left_data_len,
                symbol: String::new(),
            });
            right_diff.push(ObjDataDiff {
                data: right_data,
                kind: ObjDataDiffKind::Replace,
                len: right_data_len,
                symbol: String::new(),
            });
        }
        LevEditType::Insert => {
            let right_data = take(&mut cur_right_data);
            let right_data_len = right_data.len();
            left_diff.push(ObjDataDiff {
                data: vec![],
                kind: ObjDataDiffKind::Insert,
                len: right_data_len,
                symbol: String::new(),
            });
            right_diff.push(ObjDataDiff {
                data: right_data,
                kind: ObjDataDiffKind::Insert,
                len: right_data_len,
                symbol: String::new(),
            });
        }
        LevEditType::Delete => {
            let left_data = take(&mut cur_left_data);
            let left_data_len = left_data.len();
            left_diff.push(ObjDataDiff {
                data: left_data,
                kind: ObjDataDiffKind::Delete,
                len: left_data_len,
                symbol: String::new(),
            });
            right_diff.push(ObjDataDiff {
                data: vec![],
                kind: ObjDataDiffKind::Delete,
                len: left_data_len,
                symbol: String::new(),
            });
        }
    }

    if left_cur < left.data.len() {
        left_diff.push(ObjDataDiff {
            data: left.data[left_cur..].to_vec(),
            kind: ObjDataDiffKind::None,
            len: left.data.len() - left_cur,
            symbol: String::new(),
        });
    }
    if right_cur < right.data.len() {
        right_diff.push(ObjDataDiff {
            data: right.data[right_cur..].to_vec(),
            kind: ObjDataDiffKind::None,
            len: right.data.len() - right_cur,
            symbol: String::new(),
        });
    }

    left.data_diff = left_diff;
    right.data_diff = right_diff;
}

fn no_diff_data(section: &mut ObjSection) {
    section.data_diff = vec![ObjDataDiff {
        data: section.data.clone(),
        kind: ObjDataDiffKind::None,
        len: section.data.len(),
        symbol: String::new(),
    }];
}

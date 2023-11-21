use std::{
    cmp::{max, min, Ordering},
    mem::take,
    time::{Duration, Instant},
};

use anyhow::{bail, Result};
use similar::{capture_diff_slices_deadline, Algorithm};

use crate::{
    diff::{
        editops::{editops_find, LevEditType},
        DiffAlg,
    },
    obj::{ObjDataDiff, ObjDataDiffKind, ObjSection, ObjSymbol},
};

pub fn diff_data(alg: DiffAlg, left: &mut ObjSection, right: &mut ObjSection) -> Result<()> {
    match alg {
        DiffAlg::Levenshtein => diff_data_lev(left, right),
        DiffAlg::Lcs => diff_data_similar(Algorithm::Lcs, left, right),
        DiffAlg::Myers => diff_data_similar(Algorithm::Myers, left, right),
        DiffAlg::Patience => diff_data_similar(Algorithm::Patience, left, right),
    }
}

pub fn diff_bss_symbols(
    left_symbols: &mut [ObjSymbol],
    right_symbols: &mut [ObjSymbol],
) -> Result<()> {
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
pub fn diff_data_symbols(left: &mut ObjSection, right: &mut ObjSection) -> Result<()> {
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

pub fn diff_data_similar(
    alg: Algorithm,
    left: &mut ObjSection,
    right: &mut ObjSection,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let ops = capture_diff_slices_deadline(alg, &left.data, &right.data, Some(deadline));

    let mut left_diff = Vec::<ObjDataDiff>::new();
    let mut right_diff = Vec::<ObjDataDiff>::new();
    for op in ops {
        let (tag, left_range, right_range) = op.as_tag_tuple();
        let left_len = left_range.len();
        let right_len = right_range.len();
        let mut len = max(left_len, right_len);
        let kind = match tag {
            similar::DiffTag::Equal => ObjDataDiffKind::None,
            similar::DiffTag::Delete => ObjDataDiffKind::Delete,
            similar::DiffTag::Insert => ObjDataDiffKind::Insert,
            similar::DiffTag::Replace => {
                // Ensure replacements are equal length
                len = min(left_len, right_len);
                ObjDataDiffKind::Replace
            }
        };
        let left_data = &left.data[left_range];
        let right_data = &right.data[right_range];
        left_diff.push(ObjDataDiff {
            data: left_data[..min(len, left_data.len())].to_vec(),
            kind,
            len,
            ..Default::default()
        });
        right_diff.push(ObjDataDiff {
            data: right_data[..min(len, right_data.len())].to_vec(),
            kind,
            len,
            ..Default::default()
        });
        if kind == ObjDataDiffKind::Replace {
            match left_len.cmp(&right_len) {
                Ordering::Less => {
                    let len = right_len - left_len;
                    left_diff.push(ObjDataDiff {
                        data: vec![],
                        kind: ObjDataDiffKind::Insert,
                        len,
                        ..Default::default()
                    });
                    right_diff.push(ObjDataDiff {
                        data: right_data[left_len..right_len].to_vec(),
                        kind: ObjDataDiffKind::Insert,
                        len,
                        ..Default::default()
                    });
                }
                Ordering::Greater => {
                    let len = left_len - right_len;
                    left_diff.push(ObjDataDiff {
                        data: left_data[right_len..left_len].to_vec(),
                        kind: ObjDataDiffKind::Delete,
                        len,
                        ..Default::default()
                    });
                    right_diff.push(ObjDataDiff {
                        data: vec![],
                        kind: ObjDataDiffKind::Delete,
                        len,
                        ..Default::default()
                    });
                }
                Ordering::Equal => {}
            }
        }
    }

    left.data_diff = left_diff;
    right.data_diff = right_diff;
    Ok(())
}

pub fn diff_data_lev(left: &mut ObjSection, right: &mut ObjSection) -> Result<()> {
    let matrix_size = (left.data.len() as u64).saturating_mul(right.data.len() as u64);
    if matrix_size > 1_000_000_000 {
        bail!(
            "Data section {} too large for Levenshtein diff ({} * {} = {})",
            left.name,
            left.data.len(),
            right.data.len(),
            matrix_size
        );
    }

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
        return Ok(());
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
    return Ok(());
}

pub fn no_diff_data(section: &mut ObjSection) {
    section.data_diff = vec![ObjDataDiff {
        data: section.data.clone(),
        kind: ObjDataDiffKind::None,
        len: section.data.len(),
        symbol: String::new(),
    }];
}

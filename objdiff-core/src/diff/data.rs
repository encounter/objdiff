use std::{
    cmp::{max, min, Ordering},
    time::{Duration, Instant},
};

use anyhow::Result;
use similar::{capture_diff_slices_deadline, Algorithm};

use crate::{
    diff::{ ObjDataDiff, ObjDataDiffKind, ObjSectionDiff, ObjSymbolDiff},
    obj::{ObjInfo, ObjSection},
};
use crate::obj::SymbolRef;

pub fn diff_bss_symbol(
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    left_symbol_ref: SymbolRef,
    right_symbol_ref: SymbolRef,
) -> Result<(ObjSymbolDiff, ObjSymbolDiff)> {
    let (_, left_symbol) = left_obj.section_symbol(left_symbol_ref);
    let (_, right_symbol) = right_obj.section_symbol(right_symbol_ref);
    let percent = if left_symbol.size == right_symbol.size { 100.0 } else { 50.0 };
    Ok((
        ObjSymbolDiff {
            diff_symbol: Some(right_symbol_ref),
            instructions: vec![],
            match_percent: Some(percent),
        },
        ObjSymbolDiff {
            diff_symbol: Some(left_symbol_ref),
            instructions: vec![],
            match_percent: Some(percent),
        },
    ))
}

pub fn no_diff_bss_symbol(_obj: &ObjInfo, _symbol_ref: SymbolRef) -> ObjSymbolDiff {
    ObjSymbolDiff { diff_symbol: None, instructions: vec![], match_percent: Some(0.0) }
}

pub fn diff_data(
    left: &ObjSection,
    right: &ObjSection,
) -> Result<(ObjSectionDiff, ObjSectionDiff)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let ops =
        capture_diff_slices_deadline(Algorithm::Patience, &left.data, &right.data, Some(deadline));

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

    Ok((
        ObjSectionDiff {
            symbols: vec![],
            data_diff: left_diff,
            // TODO
            match_percent: None,
        },
        ObjSectionDiff {
            symbols: vec![],
            data_diff: right_diff,
            // TODO
            match_percent: None,
        },
    ))
}

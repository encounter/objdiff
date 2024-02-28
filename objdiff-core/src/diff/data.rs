use std::{
    cmp::{max, min, Ordering},
    time::{Duration, Instant},
};

use anyhow::Result;
use similar::{capture_diff_slices_deadline, Algorithm};

use crate::obj::{ObjDataDiff, ObjDataDiffKind, ObjSection, ObjSymbol};

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

pub fn diff_data(left: &mut ObjSection, right: &mut ObjSection) -> Result<()> {
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

    left.data_diff = left_diff;
    right.data_diff = right_diff;
    Ok(())
}

pub fn no_diff_data(section: &mut ObjSection) {
    section.data_diff = vec![ObjDataDiff {
        data: section.data.clone(),
        kind: ObjDataDiffKind::None,
        len: section.data.len(),
        symbol: String::new(),
    }];
}

use std::{
    cmp::{max, min, Ordering},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Result};
use similar::{capture_diff_slices_deadline, get_diff_ratio, Algorithm};

use crate::{
    diff::{ObjDataDiff, ObjDataDiffKind, ObjSectionDiff, ObjSymbolDiff},
    obj::{ObjInfo, ObjSection, SymbolRef},
};

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
            symbol_ref: left_symbol_ref,
            diff_symbol: Some(right_symbol_ref),
            instructions: vec![],
            match_percent: Some(percent),
        },
        ObjSymbolDiff {
            symbol_ref: right_symbol_ref,
            diff_symbol: Some(left_symbol_ref),
            instructions: vec![],
            match_percent: Some(percent),
        },
    ))
}

pub fn no_diff_symbol(_obj: &ObjInfo, symbol_ref: SymbolRef) -> ObjSymbolDiff {
    ObjSymbolDiff { symbol_ref, diff_symbol: None, instructions: vec![], match_percent: None }
}

/// Compare the data sections of two object files.
pub fn diff_data_section(
    left: &ObjSection,
    right: &ObjSection,
    left_section_diff: &ObjSectionDiff,
    right_section_diff: &ObjSectionDiff,
) -> Result<(ObjSectionDiff, ObjSectionDiff)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let left_max =
        left.symbols.iter().map(|s| s.section_address + s.size).max().unwrap_or(0).min(left.size);
    let right_max =
        right.symbols.iter().map(|s| s.section_address + s.size).max().unwrap_or(0).min(right.size);
    let left_data = &left.data[..left_max as usize];
    let right_data = &right.data[..right_max as usize];
    let ops =
        capture_diff_slices_deadline(Algorithm::Patience, left_data, right_data, Some(deadline));
    let match_percent = get_diff_ratio(&ops, left_data.len(), right_data.len()) * 100.0;

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

    let (mut left_section_diff, mut right_section_diff) =
        diff_generic_section(left, right, left_section_diff, right_section_diff)?;
    left_section_diff.data_diff = left_diff;
    right_section_diff.data_diff = right_diff;
    // Use the highest match percent between two options:
    // - Left symbols matching right symbols by name
    // - Diff of the data itself
    if left_section_diff.match_percent.unwrap_or(-1.0) < match_percent {
        left_section_diff.match_percent = Some(match_percent);
        right_section_diff.match_percent = Some(match_percent);
    }
    Ok((left_section_diff, right_section_diff))
}

pub fn diff_data_symbol(
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    left_symbol_ref: SymbolRef,
    right_symbol_ref: SymbolRef,
) -> Result<(ObjSymbolDiff, ObjSymbolDiff)> {
    let (left_section, left_symbol) = left_obj.section_symbol(left_symbol_ref);
    let (right_section, right_symbol) = right_obj.section_symbol(right_symbol_ref);

    let left_section = left_section.ok_or_else(|| anyhow!("Data symbol section not found"))?;
    let right_section = right_section.ok_or_else(|| anyhow!("Data symbol section not found"))?;

    let left_data = &left_section.data[left_symbol.section_address as usize
        ..(left_symbol.section_address + left_symbol.size) as usize];
    let right_data = &right_section.data[right_symbol.section_address as usize
        ..(right_symbol.section_address + right_symbol.size) as usize];

    let deadline = Instant::now() + Duration::from_secs(5);
    let ops =
        capture_diff_slices_deadline(Algorithm::Patience, left_data, right_data, Some(deadline));
    let match_percent = get_diff_ratio(&ops, left_data.len(), right_data.len()) * 100.0;

    Ok((
        ObjSymbolDiff {
            symbol_ref: left_symbol_ref,
            diff_symbol: Some(right_symbol_ref),
            instructions: vec![],
            match_percent: Some(match_percent),
        },
        ObjSymbolDiff {
            symbol_ref: right_symbol_ref,
            diff_symbol: Some(left_symbol_ref),
            instructions: vec![],
            match_percent: Some(match_percent),
        },
    ))
}

/// Compares a section of two object files.
/// This essentially adds up the match percentage of each symbol in the section.
pub fn diff_generic_section(
    left: &ObjSection,
    _right: &ObjSection,
    left_diff: &ObjSectionDiff,
    _right_diff: &ObjSectionDiff,
) -> Result<(ObjSectionDiff, ObjSectionDiff)> {
    let match_percent = if left_diff.symbols.iter().all(|d| d.match_percent == Some(100.0)) {
        100.0 // Avoid fp precision issues
    } else {
        left.symbols
            .iter()
            .zip(left_diff.symbols.iter())
            .map(|(s, d)| d.match_percent.unwrap_or(0.0) * s.size as f32)
            .sum::<f32>()
            / left.size as f32
    };
    Ok((
        ObjSectionDiff { symbols: vec![], data_diff: vec![], match_percent: Some(match_percent) },
        ObjSectionDiff { symbols: vec![], data_diff: vec![], match_percent: Some(match_percent) },
    ))
}

/// Compare the addresses and sizes of each symbol in the BSS sections.
pub fn diff_bss_section(
    left: &ObjSection,
    right: &ObjSection,
    left_diff: &ObjSectionDiff,
    right_diff: &ObjSectionDiff,
) -> Result<(ObjSectionDiff, ObjSectionDiff)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let left_sizes = left.symbols.iter().map(|s| (s.section_address, s.size)).collect::<Vec<_>>();
    let right_sizes = right.symbols.iter().map(|s| (s.section_address, s.size)).collect::<Vec<_>>();
    let ops = capture_diff_slices_deadline(
        Algorithm::Patience,
        &left_sizes,
        &right_sizes,
        Some(deadline),
    );
    let mut match_percent = get_diff_ratio(&ops, left_sizes.len(), right_sizes.len()) * 100.0;

    // Use the highest match percent between two options:
    // - Left symbols matching right symbols by name
    // - Diff of the addresses and sizes of each symbol
    let (generic_diff, _) = diff_generic_section(left, right, left_diff, right_diff)?;
    if generic_diff.match_percent.unwrap_or(-1.0) > match_percent {
        match_percent = generic_diff.match_percent.unwrap();
    }

    Ok((
        ObjSectionDiff { symbols: vec![], data_diff: vec![], match_percent: Some(match_percent) },
        ObjSectionDiff { symbols: vec![], data_diff: vec![], match_percent: Some(match_percent) },
    ))
}

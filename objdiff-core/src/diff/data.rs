use std::{
    cmp::{max, min, Ordering},
    ops::Range,
};

use anyhow::{anyhow, Result};
use similar::{capture_diff_slices_deadline, get_diff_ratio, Algorithm};

use super::code::{address_eq, section_name_eq};
use crate::{
    diff::{ObjDataDiff, ObjDataDiffKind, ObjDataRelocDiff, ObjSectionDiff, ObjSymbolDiff},
    obj::{ObjInfo, ObjReloc, ObjSection, ObjSymbolFlags, SymbolRef},
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
            target_symbol: Some(right_symbol_ref),
            instructions: vec![],
            match_percent: Some(percent),
        },
        ObjSymbolDiff {
            symbol_ref: right_symbol_ref,
            target_symbol: Some(left_symbol_ref),
            instructions: vec![],
            match_percent: Some(percent),
        },
    ))
}

pub fn no_diff_symbol(_obj: &ObjInfo, symbol_ref: SymbolRef) -> ObjSymbolDiff {
    ObjSymbolDiff { symbol_ref, target_symbol: None, instructions: vec![], match_percent: None }
}

fn reloc_eq(left_obj: &ObjInfo, right_obj: &ObjInfo, left: &ObjReloc, right: &ObjReloc) -> bool {
    if left.flags != right.flags {
        return false;
    }

    let symbol_name_addend_matches =
        left.target.name == right.target.name && left.addend == right.addend;
    match (&left.target.orig_section_index, &right.target.orig_section_index) {
        (Some(sl), Some(sr)) => {
            // Match if section and name+addend or address match
            section_name_eq(left_obj, right_obj, *sl, *sr)
                && (symbol_name_addend_matches || address_eq(left, right))
        }
        (Some(_), None) => false,
        (None, Some(_)) => {
            // Match if possibly stripped weak symbol
            symbol_name_addend_matches && right.target.flags.0.contains(ObjSymbolFlags::Weak)
        }
        (None, None) => symbol_name_addend_matches,
    }
}

/// Compares relocations contained with a certain data range.
/// The ObjDataDiffKind for each diff will either be `None`` (if the relocation matches),
/// or `Replace` (if a relocation was changed, added, or removed).
/// `Insert` and `Delete` are not used when a relocation is added or removed to avoid confusing diffs
/// where it looks like the bytes themselves were changed but actually only the relocations changed.
fn diff_data_relocs_for_range(
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    left: &ObjSection,
    right: &ObjSection,
    left_range: Range<usize>,
    right_range: Range<usize>,
) -> Vec<(ObjDataDiffKind, Option<ObjReloc>, Option<ObjReloc>)> {
    let mut diffs = Vec::new();
    for left_reloc in left.relocations.iter() {
        if !left_range.contains(&(left_reloc.address as usize)) {
            continue;
        }
        let left_offset = left_reloc.address as usize - left_range.start;
        let Some(right_reloc) = right.relocations.iter().find(|r| {
            if !right_range.contains(&(r.address as usize)) {
                return false;
            }
            let right_offset = r.address as usize - right_range.start;
            right_offset == left_offset
        }) else {
            diffs.push((ObjDataDiffKind::Delete, Some(left_reloc.clone()), None));
            continue;
        };
        if reloc_eq(left_obj, right_obj, left_reloc, right_reloc) {
            diffs.push((
                ObjDataDiffKind::None,
                Some(left_reloc.clone()),
                Some(right_reloc.clone()),
            ));
        } else {
            diffs.push((
                ObjDataDiffKind::Replace,
                Some(left_reloc.clone()),
                Some(right_reloc.clone()),
            ));
        }
    }
    for right_reloc in right.relocations.iter() {
        if !right_range.contains(&(right_reloc.address as usize)) {
            continue;
        }
        let right_offset = right_reloc.address as usize - right_range.start;
        let Some(_) = left.relocations.iter().find(|r| {
            if !left_range.contains(&(r.address as usize)) {
                return false;
            }
            let left_offset = r.address as usize - left_range.start;
            left_offset == right_offset
        }) else {
            diffs.push((ObjDataDiffKind::Insert, None, Some(right_reloc.clone())));
            continue;
        };
        // No need to check the cases for relocations being deleted or matching again.
        // They were already handled in the loop over the left relocs.
    }
    diffs
}

/// Compare the data sections of two object files.
pub fn diff_data_section(
    left_obj: &ObjInfo,
    right_obj: &ObjInfo,
    left: &ObjSection,
    right: &ObjSection,
    left_section_diff: &ObjSectionDiff,
    right_section_diff: &ObjSectionDiff,
) -> Result<(ObjSectionDiff, ObjSectionDiff)> {
    let left_max =
        left.symbols.iter().map(|s| s.section_address + s.size).max().unwrap_or(0).min(left.size);
    let right_max =
        right.symbols.iter().map(|s| s.section_address + s.size).max().unwrap_or(0).min(right.size);
    let left_data = &left.data[..left_max as usize];
    let right_data = &right.data[..right_max as usize];
    let ops = capture_diff_slices_deadline(Algorithm::Patience, left_data, right_data, None);
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

    let mut left_reloc_diffs = Vec::new();
    let mut right_reloc_diffs = Vec::new();
    for (diff_kind, left_reloc, right_reloc) in diff_data_relocs_for_range(
        left_obj,
        right_obj,
        left,
        right,
        0..left_max as usize,
        0..right_max as usize,
    ) {
        if let Some(left_reloc) = left_reloc {
            let len = left_obj.arch.get_reloc_byte_size(left_reloc.flags);
            let range = left_reloc.address as usize..left_reloc.address as usize + len;
            left_reloc_diffs.push(ObjDataRelocDiff { reloc: left_reloc, kind: diff_kind, range });
        }
        if let Some(right_reloc) = right_reloc {
            let len = right_obj.arch.get_reloc_byte_size(right_reloc.flags);
            let range = right_reloc.address as usize..right_reloc.address as usize + len;
            right_reloc_diffs.push(ObjDataRelocDiff { reloc: right_reloc, kind: diff_kind, range });
        }
    }

    let (mut left_section_diff, mut right_section_diff) =
        diff_generic_section(left, right, left_section_diff, right_section_diff)?;
    let all_left_relocs_match = left_reloc_diffs.iter().all(|d| d.kind == ObjDataDiffKind::None);
    left_section_diff.data_diff = left_diff;
    right_section_diff.data_diff = right_diff;
    left_section_diff.reloc_diff = left_reloc_diffs;
    right_section_diff.reloc_diff = right_reloc_diffs;
    if all_left_relocs_match {
        // Use the highest match percent between two options:
        // - Left symbols matching right symbols by name
        // - Diff of the data itself
        // We only do this when all relocations on the left side match.
        if left_section_diff.match_percent.unwrap_or(-1.0) < match_percent {
            left_section_diff.match_percent = Some(match_percent);
            right_section_diff.match_percent = Some(match_percent);
        }
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

    let left_range = left_symbol.section_address as usize
        ..(left_symbol.section_address + left_symbol.size) as usize;
    let right_range = right_symbol.section_address as usize
        ..(right_symbol.section_address + right_symbol.size) as usize;
    let left_data = &left_section.data[left_range.clone()];
    let right_data = &right_section.data[right_range.clone()];

    let reloc_diffs = diff_data_relocs_for_range(
        left_obj,
        right_obj,
        left_section,
        right_section,
        left_range,
        right_range,
    );

    let ops = capture_diff_slices_deadline(Algorithm::Patience, left_data, right_data, None);
    let bytes_match_ratio = get_diff_ratio(&ops, left_data.len(), right_data.len());

    let mut match_ratio = bytes_match_ratio;
    if !reloc_diffs.is_empty() {
        let mut total_reloc_bytes = 0;
        let mut matching_reloc_bytes = 0;
        for (diff_kind, left_reloc, right_reloc) in reloc_diffs {
            let reloc_diff_len = match (left_reloc, right_reloc) {
                (None, None) => unreachable!(),
                (None, Some(right_reloc)) => right_obj.arch.get_reloc_byte_size(right_reloc.flags),
                (Some(left_reloc), _) => left_obj.arch.get_reloc_byte_size(left_reloc.flags),
            };
            total_reloc_bytes += reloc_diff_len;
            if diff_kind == ObjDataDiffKind::None {
                matching_reloc_bytes += reloc_diff_len;
            }
        }
        if total_reloc_bytes > 0 {
            let relocs_match_ratio = matching_reloc_bytes as f32 / total_reloc_bytes as f32;
            // Adjust the overall match ratio to include relocation differences.
            // We calculate it so that bytes that contain a relocation are counted twice: once for the
            // byte's raw value, and once for its relocation.
            // e.g. An 8 byte symbol that has 8 matching raw bytes and a single 4 byte relocation that
            // doesn't match would show as 66% (weighted average of 100% and 0%).
            match_ratio = ((bytes_match_ratio * (left_data.len() as f32))
                + (relocs_match_ratio * total_reloc_bytes as f32))
                / (left_data.len() + total_reloc_bytes) as f32;
        }
    }

    let match_percent = match_ratio * 100.0;

    Ok((
        ObjSymbolDiff {
            symbol_ref: left_symbol_ref,
            target_symbol: Some(right_symbol_ref),
            instructions: vec![],
            match_percent: Some(match_percent),
        },
        ObjSymbolDiff {
            symbol_ref: right_symbol_ref,
            target_symbol: Some(left_symbol_ref),
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
        ObjSectionDiff {
            symbols: vec![],
            data_diff: vec![],
            reloc_diff: vec![],
            match_percent: Some(match_percent),
        },
        ObjSectionDiff {
            symbols: vec![],
            data_diff: vec![],
            reloc_diff: vec![],
            match_percent: Some(match_percent),
        },
    ))
}

/// Compare the addresses and sizes of each symbol in the BSS sections.
pub fn diff_bss_section(
    left: &ObjSection,
    right: &ObjSection,
    left_diff: &ObjSectionDiff,
    right_diff: &ObjSectionDiff,
) -> Result<(ObjSectionDiff, ObjSectionDiff)> {
    let left_sizes = left.symbols.iter().map(|s| (s.section_address, s.size)).collect::<Vec<_>>();
    let right_sizes = right.symbols.iter().map(|s| (s.section_address, s.size)).collect::<Vec<_>>();
    let ops = capture_diff_slices_deadline(Algorithm::Patience, &left_sizes, &right_sizes, None);
    let mut match_percent = get_diff_ratio(&ops, left_sizes.len(), right_sizes.len()) * 100.0;

    // Use the highest match percent between two options:
    // - Left symbols matching right symbols by name
    // - Diff of the addresses and sizes of each symbol
    let (generic_diff, _) = diff_generic_section(left, right, left_diff, right_diff)?;
    if generic_diff.match_percent.unwrap_or(-1.0) > match_percent {
        match_percent = generic_diff.match_percent.unwrap();
    }

    Ok((
        ObjSectionDiff {
            symbols: vec![],
            data_diff: vec![],
            reloc_diff: vec![],
            match_percent: Some(match_percent),
        },
        ObjSectionDiff {
            symbols: vec![],
            data_diff: vec![],
            reloc_diff: vec![],
            match_percent: Some(match_percent),
        },
    ))
}

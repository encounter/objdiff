use alloc::{vec, vec::Vec};
use core::{cmp::Ordering, ops::Range};

use anyhow::{anyhow, Result};
use similar::{capture_diff_slices, get_diff_ratio, Algorithm};

use super::{
    code::{address_eq, section_name_eq},
    DataDiff, DataDiffKind, DataRelocationDiff, ObjectDiff, SectionDiff, SymbolDiff,
};
use crate::obj::{Object, Relocation, ResolvedRelocation, SymbolFlag, SymbolKind};

pub fn diff_bss_symbol(
    left_obj: &Object,
    right_obj: &Object,
    left_symbol_ref: usize,
    right_symbol_ref: usize,
) -> Result<(SymbolDiff, SymbolDiff)> {
    let left_symbol = &left_obj.symbols[left_symbol_ref];
    let right_symbol = &right_obj.symbols[right_symbol_ref];
    let percent = if left_symbol.size == right_symbol.size { 100.0 } else { 50.0 };
    Ok((
        SymbolDiff {
            target_symbol: Some(right_symbol_ref),
            match_percent: Some(percent),
            diff_score: None,
            instruction_rows: vec![],
        },
        SymbolDiff {
            target_symbol: Some(left_symbol_ref),
            match_percent: Some(percent),
            diff_score: None,
            instruction_rows: vec![],
        },
    ))
}

fn reloc_eq(
    left_obj: &Object,
    right_obj: &Object,
    left: &ResolvedRelocation,
    right: &ResolvedRelocation,
) -> bool {
    if left.relocation.flags != right.relocation.flags {
        return false;
    }

    let symbol_name_addend_matches =
        left.symbol.name == right.symbol.name && left.relocation.addend == right.relocation.addend;
    match (left.symbol.section, right.symbol.section) {
        (Some(sl), Some(sr)) => {
            // Match if section and name+addend or address match
            section_name_eq(left_obj, right_obj, sl, sr)
                && (symbol_name_addend_matches || address_eq(left, right))
        }
        (Some(_), None) => false,
        (None, Some(_)) => {
            // Match if possibly stripped weak symbol
            symbol_name_addend_matches && right.symbol.flags.contains(SymbolFlag::Weak)
        }
        (None, None) => symbol_name_addend_matches,
    }
}

#[inline]
fn resolve_relocation<'obj>(
    obj: &'obj Object,
    reloc: &'obj Relocation,
) -> ResolvedRelocation<'obj> {
    let symbol = &obj.symbols[reloc.target_symbol];
    ResolvedRelocation { relocation: reloc, symbol }
}

/// Compares relocations contained with a certain data range.
/// The DataDiffKind for each diff will either be `None`` (if the relocation matches),
/// or `Replace` (if a relocation was changed, added, or removed).
/// `Insert` and `Delete` are not used when a relocation is added or removed to avoid confusing diffs
/// where it looks like the bytes themselves were changed but actually only the relocations changed.
fn diff_data_relocs_for_range<'left, 'right>(
    left_obj: &'left Object,
    right_obj: &'right Object,
    left_section_idx: usize,
    right_section_idx: usize,
    left_range: Range<usize>,
    right_range: Range<usize>,
) -> Vec<(DataDiffKind, Option<ResolvedRelocation<'left>>, Option<ResolvedRelocation<'right>>)> {
    let left_section = &left_obj.sections[left_section_idx];
    let right_section = &right_obj.sections[right_section_idx];
    let mut diffs = Vec::new();
    for left_reloc in left_section.relocations.iter() {
        if !left_range.contains(&(left_reloc.address as usize)) {
            continue;
        }
        let left_offset = left_reloc.address as usize - left_range.start;
        let left_reloc = resolve_relocation(left_obj, left_reloc);
        let Some(right_reloc) = right_section.relocations.iter().find(|r| {
            if !right_range.contains(&(r.address as usize)) {
                return false;
            }
            let right_offset = r.address as usize - right_range.start;
            right_offset == left_offset
        }) else {
            diffs.push((DataDiffKind::Delete, Some(left_reloc), None));
            continue;
        };
        let right_reloc = resolve_relocation(right_obj, right_reloc);
        if reloc_eq(left_obj, right_obj, &left_reloc, &right_reloc) {
            diffs.push((DataDiffKind::None, Some(left_reloc), Some(right_reloc)));
        } else {
            diffs.push((
                DataDiffKind::Replace,
                Some(left_reloc),
                Some(right_reloc),
            ));
        }
    }
    for right_reloc in right_section.relocations.iter() {
        if !right_range.contains(&(right_reloc.address as usize)) {
            continue;
        }
        let right_offset = right_reloc.address as usize - right_range.start;
        let right_reloc = resolve_relocation(right_obj, right_reloc);
        let Some(_) = left_section.relocations.iter().find(|r| {
            if !left_range.contains(&(r.address as usize)) {
                return false;
            }
            let left_offset = r.address as usize - left_range.start;
            left_offset == right_offset
        }) else {
            diffs.push((DataDiffKind::Insert, None, Some(right_reloc)));
            continue;
        };
        // No need to check the cases for relocations being deleted or matching again.
        // They were already handled in the loop over the left relocs.
    }
    diffs
}

/// Compare the data sections of two object files.
pub fn diff_data_section(
    left_obj: &Object,
    right_obj: &Object,
    left_diff: &ObjectDiff,
    right_diff: &ObjectDiff,
    left_section_idx: usize,
    right_section_idx: usize,
) -> Result<(SectionDiff, SectionDiff)> {
    let left_section = &left_obj.sections[left_section_idx];
    let right_section = &right_obj.sections[right_section_idx];
    let left_max = left_obj
        .symbols
        .iter()
        .filter_map(|s| {
            if s.section != Some(left_section_idx) || s.kind == SymbolKind::Section {
                return None;
            }
            s.address.checked_sub(left_section.address).map(|a| a + s.size)
        })
        .max()
        .unwrap_or(0)
        .min(left_section.size);
    let right_max = right_obj
        .symbols
        .iter()
        .filter_map(|s| {
            if s.section != Some(right_section_idx) || s.kind == SymbolKind::Section {
                return None;
            }
            s.address.checked_sub(right_section.address).map(|a| a + s.size)
        })
        .max()
        .unwrap_or(0)
        .min(right_section.size);
    let left_data = &left_section.data[..left_max as usize];
    let right_data = &right_section.data[..right_max as usize];
    let ops = capture_diff_slices(Algorithm::Patience, left_data, right_data);
    let match_percent = get_diff_ratio(&ops, left_data.len(), right_data.len()) * 100.0;

    let mut left_data_diff = Vec::<DataDiff>::new();
    let mut right_data_diff = Vec::<DataDiff>::new();
    for op in ops {
        let (tag, left_range, right_range) = op.as_tag_tuple();
        let left_len = left_range.len();
        let right_len = right_range.len();
        let mut len = left_len.max(right_len);
        let kind = match tag {
            similar::DiffTag::Equal => DataDiffKind::None,
            similar::DiffTag::Delete => DataDiffKind::Delete,
            similar::DiffTag::Insert => DataDiffKind::Insert,
            similar::DiffTag::Replace => {
                // Ensure replacements are equal length
                len = left_len.min(right_len);
                DataDiffKind::Replace
            }
        };
        let left_data = &left_section.data[left_range];
        let right_data = &right_section.data[right_range];
        left_data_diff.push(DataDiff {
            data: left_data[..len.min(left_data.len())].to_vec(),
            kind,
            len,
            ..Default::default()
        });
        right_data_diff.push(DataDiff {
            data: right_data[..len.min(right_data.len())].to_vec(),
            kind,
            len,
            ..Default::default()
        });
        if kind == DataDiffKind::Replace {
            match left_len.cmp(&right_len) {
                Ordering::Less => {
                    let len = right_len - left_len;
                    left_data_diff.push(DataDiff {
                        data: vec![],
                        kind: DataDiffKind::Insert,
                        len,
                        ..Default::default()
                    });
                    right_data_diff.push(DataDiff {
                        data: right_data[left_len..right_len].to_vec(),
                        kind: DataDiffKind::Insert,
                        len,
                        ..Default::default()
                    });
                }
                Ordering::Greater => {
                    let len = left_len - right_len;
                    left_data_diff.push(DataDiff {
                        data: left_data[right_len..left_len].to_vec(),
                        kind: DataDiffKind::Delete,
                        len,
                        ..Default::default()
                    });
                    right_data_diff.push(DataDiff {
                        data: vec![],
                        kind: DataDiffKind::Delete,
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
        left_section_idx,
        right_section_idx,
        0..left_max as usize,
        0..right_max as usize,
    ) {
        if let Some(left_reloc) = left_reloc {
            let len = left_obj.arch.get_reloc_byte_size(left_reloc.relocation.flags);
            let range = left_reloc.relocation.address as usize
                ..left_reloc.relocation.address as usize + len;
            left_reloc_diffs.push(DataRelocationDiff { kind: diff_kind, range });
        }
        if let Some(right_reloc) = right_reloc {
            let len = right_obj.arch.get_reloc_byte_size(right_reloc.relocation.flags);
            let range = right_reloc.relocation.address as usize
                ..right_reloc.relocation.address as usize + len;
            right_reloc_diffs.push(DataRelocationDiff { kind: diff_kind, range });
        }
    }

    let (mut left_section_diff, mut right_section_diff) = diff_generic_section(
        left_obj,
        right_obj,
        left_diff,
        right_diff,
        left_section_idx,
        right_section_idx,
    )?;
    let all_left_relocs_match = left_reloc_diffs.iter().all(|d| d.kind == DataDiffKind::None);
    left_section_diff.data_diff = left_data_diff;
    right_section_diff.data_diff = right_data_diff;
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
    left_obj: &Object,
    right_obj: &Object,
    left_symbol_idx: usize,
    right_symbol_idx: usize,
) -> Result<(SymbolDiff, SymbolDiff)> {
    let left_symbol = &left_obj.symbols[left_symbol_idx];
    let right_symbol = &right_obj.symbols[right_symbol_idx];

    let left_section_idx =
        left_symbol.section.ok_or_else(|| anyhow!("Data symbol section not found"))?;
    let right_section_idx =
        right_symbol.section.ok_or_else(|| anyhow!("Data symbol section not found"))?;

    let left_section = &left_obj.sections[left_section_idx];
    let right_section = &right_obj.sections[right_section_idx];

    let left_start = left_symbol
        .address
        .checked_sub(left_section.address)
        .ok_or_else(|| anyhow!("Symbol address out of section bounds"))?;
    let right_start = right_symbol
        .address
        .checked_sub(right_section.address)
        .ok_or_else(|| anyhow!("Symbol address out of section bounds"))?;
    let left_end = left_start + left_symbol.size;
    if left_end > left_section.size {
        return Err(anyhow!(
            "Symbol {} size out of section bounds ({} > {})",
            left_symbol.name,
            left_end,
            left_section.size
        ));
    }
    let right_end = right_start + right_symbol.size;
    if right_end > right_section.size {
        return Err(anyhow!(
            "Symbol {} size out of section bounds ({} > {})",
            right_symbol.name,
            right_end,
            right_section.size
        ));
    }
    let left_range = left_start as usize..left_end as usize;
    let right_range = right_start as usize..right_end as usize;
    let left_data = &left_section.data[left_range.clone()];
    let right_data = &right_section.data[right_range.clone()];

    let reloc_diffs = diff_data_relocs_for_range(
        left_obj,
        right_obj,
        left_section_idx,
        right_section_idx,
        left_range,
        right_range,
    );

    let ops = capture_diff_slices(Algorithm::Patience, left_data, right_data);
    let bytes_match_ratio = get_diff_ratio(&ops, left_data.len(), right_data.len());

    let mut match_ratio = bytes_match_ratio;
    if !reloc_diffs.is_empty() {
        let mut total_reloc_bytes = 0;
        let mut matching_reloc_bytes = 0;
        for (diff_kind, left_reloc, right_reloc) in reloc_diffs {
            let reloc_diff_len = match (left_reloc, right_reloc) {
                (None, None) => unreachable!(),
                (None, Some(right_reloc)) => {
                    right_obj.arch.get_reloc_byte_size(right_reloc.relocation.flags)
                }
                (Some(left_reloc), _) => {
                    left_obj.arch.get_reloc_byte_size(left_reloc.relocation.flags)
                }
            };
            total_reloc_bytes += reloc_diff_len;
            if diff_kind == DataDiffKind::None {
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
        SymbolDiff {
            target_symbol: Some(right_symbol_idx),
            match_percent: Some(match_percent),
            diff_score: None,
            instruction_rows: vec![],
        },
        SymbolDiff {
            target_symbol: Some(left_symbol_idx),
            match_percent: Some(match_percent),
            diff_score: None,
            instruction_rows: vec![],
        },
    ))
}

/// Compares a section of two object files.
/// This essentially adds up the match percentage of each symbol in the section.
pub fn diff_generic_section(
    left_obj: &Object,
    _right_obj: &Object,
    left_diff: &ObjectDiff,
    _right_diff: &ObjectDiff,
    left_section_idx: usize,
    _right_section_idx: usize,
) -> Result<(SectionDiff, SectionDiff)> {
    let match_percent = if left_obj
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.section == Some(left_section_idx) && s.kind != SymbolKind::Section)
        .map(|(i, _)| &left_diff.symbols[i])
        .all(|d| d.match_percent == Some(100.0))
    {
        100.0 // Avoid fp precision issues
    } else {
        let (matched, total) = left_obj
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| s.section == Some(left_section_idx) && s.kind != SymbolKind::Section)
            .map(|(i, s)| (s, &left_diff.symbols[i]))
            .fold((0.0, 0.0), |(matched, total), (s, d)| {
                (matched + d.match_percent.unwrap_or(0.0) * s.size as f32, total + s.size as f32)
            });
        if total == 0.0 {
            100.0
        } else {
            matched / total
        }
    };
    Ok((
        SectionDiff { match_percent: Some(match_percent), data_diff: vec![], reloc_diff: vec![] },
        SectionDiff { match_percent: Some(match_percent), data_diff: vec![], reloc_diff: vec![] },
    ))
}

/// Compare the addresses and sizes of each symbol in the BSS sections.
pub fn diff_bss_section(
    left_obj: &Object,
    right_obj: &Object,
    left_diff: &ObjectDiff,
    right_diff: &ObjectDiff,
    left_section_idx: usize,
    right_section_idx: usize,
) -> Result<(SectionDiff, SectionDiff)> {
    let left_section = &left_obj.sections[left_section_idx];
    let left_sizes = left_obj
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.section == Some(left_section_idx) && s.kind != SymbolKind::Section)
        .filter_map(|(_, s)| s.address.checked_sub(left_section.address).map(|a| (a, s.size)))
        .collect::<Vec<_>>();
    let right_section = &right_obj.sections[right_section_idx];
    let right_sizes = right_obj
        .symbols
        .iter()
        .enumerate()
        .filter(|(_, s)| s.section == Some(right_section_idx) && s.kind != SymbolKind::Section)
        .filter_map(|(_, s)| s.address.checked_sub(right_section.address).map(|a| (a, s.size)))
        .collect::<Vec<_>>();
    let ops = capture_diff_slices(Algorithm::Patience, &left_sizes, &right_sizes);
    let mut match_percent = get_diff_ratio(&ops, left_sizes.len(), right_sizes.len()) * 100.0;

    // Use the highest match percent between two options:
    // - Left symbols matching right symbols by name
    // - Diff of the addresses and sizes of each symbol
    let (generic_diff, _) = diff_generic_section(
        left_obj,
        right_obj,
        left_diff,
        right_diff,
        left_section_idx,
        right_section_idx,
    )?;
    if generic_diff.match_percent.unwrap_or(-1.0) > match_percent {
        match_percent = generic_diff.match_percent.unwrap();
    }

    Ok((
        SectionDiff { match_percent: Some(match_percent), data_diff: vec![], reloc_diff: vec![] },
        SectionDiff { match_percent: Some(match_percent), data_diff: vec![], reloc_diff: vec![] },
    ))
}

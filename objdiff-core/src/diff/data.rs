use alloc::{vec, vec::Vec};
use core::{cmp::Ordering, ops::Range};

use anyhow::{Result, anyhow};
use similar::{Algorithm, capture_diff_slices, get_diff_ratio};

use super::{
    DataDiff, DataDiffKind, DataDiffRow, DataRelocationDiff, ObjectDiff, SectionDiff, SymbolDiff,
    code::{address_eq, section_name_eq},
};
use crate::obj::{Object, Relocation, ResolvedRelocation, Symbol, SymbolFlag, SymbolKind};

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
            ..Default::default()
        },
        SymbolDiff {
            target_symbol: Some(left_symbol_ref),
            match_percent: Some(percent),
            diff_score: None,
            ..Default::default()
        },
    ))
}

pub fn symbol_name_matches(left_name: &str, right_name: &str) -> bool {
    // Match Metrowerks symbol$1234 against symbol$2345
    if let Some((prefix, suffix)) = left_name.split_once('$') {
        if !suffix.chars().all(char::is_numeric) {
            return false;
        }
        right_name
            .split_once('$')
            .is_some_and(|(p, s)| p == prefix && s.chars().all(char::is_numeric))
    } else {
        left_name == right_name
    }
}

fn reloc_eq(
    left_obj: &Object,
    right_obj: &Object,
    left: ResolvedRelocation,
    right: ResolvedRelocation,
) -> bool {
    if left.relocation.flags != right.relocation.flags {
        return false;
    }

    let symbol_name_addend_matches = symbol_name_matches(&left.symbol.name, &right.symbol.name)
        && left.relocation.addend == right.relocation.addend;
    match (left.symbol.section, right.symbol.section) {
        (Some(sl), Some(sr)) => {
            // Match if section and name+addend or address match
            section_name_eq(left_obj, right_obj, sl, sr)
                && (symbol_name_addend_matches || address_eq(left, right))
        }
        (Some(_), None) | (None, Some(_)) | (None, None) => symbol_name_addend_matches,
    }
}

#[inline]
pub fn resolve_relocation<'obj>(
    symbols: &'obj [Symbol],
    reloc: &'obj Relocation,
) -> ResolvedRelocation<'obj> {
    let symbol = &symbols[reloc.target_symbol];
    ResolvedRelocation { relocation: reloc, symbol }
}

/// Compares the bytes within a certain data range.
fn diff_data_range(left_data: &[u8], right_data: &[u8]) -> (f32, Vec<DataDiff>, Vec<DataDiff>) {
    let ops = capture_diff_slices(Algorithm::Patience, left_data, right_data);
    let bytes_match_ratio = get_diff_ratio(&ops, left_data.len(), right_data.len());

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
        let left_data = &left_data[left_range];
        let right_data = &right_data[right_range];
        left_data_diff.push(DataDiff {
            data: left_data[..len.min(left_data.len())].to_vec(),
            kind,
            size: len,
        });
        right_data_diff.push(DataDiff {
            data: right_data[..len.min(right_data.len())].to_vec(),
            kind,
            size: len,
        });
        if kind == DataDiffKind::Replace {
            match left_len.cmp(&right_len) {
                Ordering::Less => {
                    let len = right_len - left_len;
                    left_data_diff.push(DataDiff {
                        data: vec![],
                        kind: DataDiffKind::Insert,
                        size: len,
                    });
                    right_data_diff.push(DataDiff {
                        data: right_data[left_len..right_len].to_vec(),
                        kind: DataDiffKind::Insert,
                        size: len,
                    });
                }
                Ordering::Greater => {
                    let len = left_len - right_len;
                    left_data_diff.push(DataDiff {
                        data: left_data[right_len..left_len].to_vec(),
                        kind: DataDiffKind::Delete,
                        size: len,
                    });
                    right_data_diff.push(DataDiff {
                        data: vec![],
                        kind: DataDiffKind::Delete,
                        size: len,
                    });
                }
                Ordering::Equal => {}
            }
        }
    }

    (bytes_match_ratio, left_data_diff, right_data_diff)
}

/// Compares relocations contained within a certain data range.
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
        let left_reloc = resolve_relocation(&left_obj.symbols, left_reloc);
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
        let right_reloc = resolve_relocation(&right_obj.symbols, right_reloc);
        if reloc_eq(left_obj, right_obj, left_reloc, right_reloc) {
            diffs.push((DataDiffKind::None, Some(left_reloc), Some(right_reloc)));
        } else {
            diffs.push((DataDiffKind::Replace, Some(left_reloc), Some(right_reloc)));
        }
    }
    for right_reloc in right_section.relocations.iter() {
        if !right_range.contains(&(right_reloc.address as usize)) {
            continue;
        }
        let right_offset = right_reloc.address as usize - right_range.start;
        let right_reloc = resolve_relocation(&right_obj.symbols, right_reloc);
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

pub fn no_diff_data_section(obj: &Object, section_idx: usize) -> Result<SectionDiff> {
    let section = &obj.sections[section_idx];

    let data_diff = vec![DataDiff {
        data: section.data.0.clone(),
        kind: DataDiffKind::None,
        size: section.data.len(),
    }];

    let mut reloc_diffs = Vec::new();
    for reloc in section.relocations.iter() {
        let reloc_len = obj.arch.data_reloc_size(reloc.flags);
        let range = reloc.address..reloc.address + reloc_len as u64;
        reloc_diffs.push(DataRelocationDiff {
            reloc: reloc.clone(),
            kind: DataDiffKind::None,
            range,
        });
    }

    Ok(SectionDiff { match_percent: Some(0.0), data_diff, reloc_diff: reloc_diffs })
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
    let left_max = symbols_matching_section(&left_obj.symbols, left_section_idx)
        .filter_map(|(_, s)| s.address.checked_sub(left_section.address).map(|a| a + s.size))
        .max()
        .unwrap_or(0)
        .min(left_section.size);
    let right_max = symbols_matching_section(&right_obj.symbols, right_section_idx)
        .filter_map(|(_, s)| s.address.checked_sub(right_section.address).map(|a| a + s.size))
        .max()
        .unwrap_or(0)
        .min(right_section.size);
    let left_data = &left_section.data[..left_max as usize];
    let right_data = &right_section.data[..right_max as usize];

    let (bytes_match_ratio, left_data_diff, right_data_diff) =
        diff_data_range(left_data, right_data);
    let match_percent = bytes_match_ratio * 100.0;

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
            let len = left_obj.arch.data_reloc_size(left_reloc.relocation.flags);
            let range = left_reloc.relocation.address..left_reloc.relocation.address + len as u64;
            left_reloc_diffs.push(DataRelocationDiff {
                reloc: left_reloc.relocation.clone(),
                kind: diff_kind,
                range,
            });
        }
        if let Some(right_reloc) = right_reloc {
            let len = right_obj.arch.data_reloc_size(right_reloc.relocation.flags);
            let range = right_reloc.relocation.address..right_reloc.relocation.address + len as u64;
            right_reloc_diffs.push(DataRelocationDiff {
                reloc: right_reloc.relocation.clone(),
                kind: diff_kind,
                range,
            });
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
        }
    }
    Ok((left_section_diff, right_section_diff))
}

pub fn no_diff_data_symbol(obj: &Object, symbol_index: usize) -> Result<SymbolDiff> {
    let symbol = &obj.symbols[symbol_index];
    let section_idx = symbol.section.ok_or_else(|| anyhow!("Data symbol section not found"))?;
    let section = &obj.sections[section_idx];

    let start = symbol
        .address
        .checked_sub(section.address)
        .ok_or_else(|| anyhow!("Symbol address out of section bounds"))?;
    let end = start + symbol.size;
    if end > section.size {
        return Err(anyhow!(
            "Symbol {} size out of section bounds ({} > {})",
            symbol.name,
            end,
            section.size
        ));
    }
    let range = start as usize..end as usize;
    let data = &section.data[range.clone()];

    let data_diff = vec![DataDiff {
        data: data.to_vec(),
        kind: DataDiffKind::None,
        size: symbol.size as usize,
    }];

    let mut reloc_diffs = Vec::new();
    for reloc in section.relocations.iter() {
        if !range.contains(&(reloc.address as usize)) {
            continue;
        }
        let reloc_len = obj.arch.data_reloc_size(reloc.flags);
        let range = reloc.address..reloc.address + reloc_len as u64;
        reloc_diffs.push(DataRelocationDiff {
            reloc: reloc.clone(),
            kind: DataDiffKind::None,
            range,
        });
    }

    let data_rows = build_data_diff_rows(&data_diff, &reloc_diffs, symbol.address);
    Ok(SymbolDiff {
        target_symbol: None,
        match_percent: None,
        diff_score: None,
        data_rows,
        ..Default::default()
    })
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

    let (bytes_match_ratio, left_data_diff, right_data_diff) =
        diff_data_range(left_data, right_data);

    let reloc_diffs = diff_data_relocs_for_range(
        left_obj,
        right_obj,
        left_section_idx,
        right_section_idx,
        left_range,
        right_range,
    );

    let mut match_ratio = bytes_match_ratio;
    let mut left_reloc_diffs = Vec::new();
    let mut right_reloc_diffs = Vec::new();
    if !reloc_diffs.is_empty() {
        let mut total_reloc_bytes = 0;
        let mut matching_reloc_bytes = 0;
        for (diff_kind, left_reloc, right_reloc) in reloc_diffs {
            let reloc_diff_len = match (left_reloc, right_reloc) {
                (None, None) => unreachable!(),
                (None, Some(right_reloc)) => {
                    right_obj.arch.data_reloc_size(right_reloc.relocation.flags)
                }
                (Some(left_reloc), _) => left_obj.arch.data_reloc_size(left_reloc.relocation.flags),
            };
            total_reloc_bytes += reloc_diff_len;
            if diff_kind == DataDiffKind::None {
                matching_reloc_bytes += reloc_diff_len;
            }

            if let Some(left_reloc) = left_reloc {
                let len = left_obj.arch.data_reloc_size(left_reloc.relocation.flags);
                let range =
                    left_reloc.relocation.address..left_reloc.relocation.address + len as u64;
                left_reloc_diffs.push(DataRelocationDiff {
                    reloc: left_reloc.relocation.clone(),
                    kind: diff_kind,
                    range,
                });
            }
            if let Some(right_reloc) = right_reloc {
                let len = right_obj.arch.data_reloc_size(right_reloc.relocation.flags);
                let range =
                    right_reloc.relocation.address..right_reloc.relocation.address + len as u64;
                right_reloc_diffs.push(DataRelocationDiff {
                    reloc: right_reloc.relocation.clone(),
                    kind: diff_kind,
                    range,
                });
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

    left_reloc_diffs
        .sort_by(|a, b| a.range.start.cmp(&b.range.start).then(a.range.end.cmp(&b.range.end)));
    right_reloc_diffs
        .sort_by(|a, b| a.range.start.cmp(&b.range.start).then(a.range.end.cmp(&b.range.end)));

    let match_percent = match_ratio * 100.0;
    let left_rows = build_data_diff_rows(&left_data_diff, &left_reloc_diffs, left_symbol.address);
    let right_rows =
        build_data_diff_rows(&right_data_diff, &right_reloc_diffs, right_symbol.address);

    Ok((
        SymbolDiff {
            target_symbol: Some(right_symbol_idx),
            match_percent: Some(match_percent),
            diff_score: None,
            data_rows: left_rows,
            ..Default::default()
        },
        SymbolDiff {
            target_symbol: Some(left_symbol_idx),
            match_percent: Some(match_percent),
            diff_score: None,
            data_rows: right_rows,
            ..Default::default()
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
    let match_percent = if symbols_matching_section(&left_obj.symbols, left_section_idx)
        .map(|(i, _)| &left_diff.symbols[i])
        .all(|d| d.match_percent == Some(100.0))
    {
        100.0 // Avoid fp precision issues
    } else {
        let (matched, total) = symbols_matching_section(&left_obj.symbols, left_section_idx)
            .map(|(i, s)| (s, &left_diff.symbols[i]))
            .fold((0.0, 0.0), |(matched, total), (s, d)| {
                (matched + d.match_percent.unwrap_or(0.0) * s.size as f32, total + s.size as f32)
            });
        if total == 0.0 { 100.0 } else { matched / total }
    };
    Ok((
        SectionDiff { match_percent: Some(match_percent), data_diff: vec![], reloc_diff: vec![] },
        SectionDiff { match_percent: None, data_diff: vec![], reloc_diff: vec![] },
    ))
}

pub fn no_diff_bss_section() -> Result<SectionDiff> {
    Ok(SectionDiff { match_percent: Some(0.0), data_diff: vec![], reloc_diff: vec![] })
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
    let left_sizes = symbols_matching_section(&left_obj.symbols, left_section_idx)
        .filter_map(|(_, s)| s.address.checked_sub(left_section.address).map(|a| (a, s.size)))
        .collect::<Vec<_>>();
    let right_section = &right_obj.sections[right_section_idx];
    let right_sizes = symbols_matching_section(&right_obj.symbols, right_section_idx)
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
        SectionDiff { match_percent: None, data_diff: vec![], reloc_diff: vec![] },
    ))
}

fn symbols_matching_section(
    symbols: &[Symbol],
    section_idx: usize,
) -> impl Iterator<Item = (usize, &Symbol)> + '_ {
    symbols.iter().enumerate().filter(move |(_, s)| {
        s.section == Some(section_idx)
            && s.kind != SymbolKind::Section
            && s.size > 0
            && !s.flags.contains(SymbolFlag::Ignored)
    })
}

pub const BYTES_PER_ROW: usize = 16;

fn build_data_diff_row(
    data_diffs: &[DataDiff],
    reloc_diffs: &[DataRelocationDiff],
    symbol_address: u64,
    row_index: usize,
) -> DataDiffRow {
    let row_start = row_index * BYTES_PER_ROW;
    let row_end = row_start + BYTES_PER_ROW;
    let mut row_diff = DataDiffRow {
        address: symbol_address + row_start as u64,
        segments: Vec::new(),
        relocations: Vec::new(),
    };

    // Collect all segments that overlap with this row
    let mut current_offset = 0;
    for diff in data_diffs {
        let diff_end = current_offset + diff.size;
        if current_offset < row_end && diff_end > row_start {
            let start_in_diff = row_start.saturating_sub(current_offset);
            let end_in_diff = row_end.min(diff_end) - current_offset;
            if start_in_diff < end_in_diff {
                let data_slice = if diff.data.is_empty() {
                    Vec::new()
                } else {
                    diff.data[start_in_diff..end_in_diff.min(diff.data.len())].to_vec()
                };
                row_diff.segments.push(DataDiff {
                    data: data_slice,
                    kind: diff.kind,
                    size: end_in_diff - start_in_diff,
                });
            }
        }
        current_offset = diff_end;
        if current_offset >= row_start + BYTES_PER_ROW {
            break;
        }
    }

    // Collect all relocations that overlap with this row
    let row_end_absolute = row_diff.address + BYTES_PER_ROW as u64;
    row_diff.relocations = reloc_diffs
        .iter()
        .filter(|rd| rd.range.start < row_end_absolute && rd.range.end > row_diff.address)
        .cloned()
        .collect();

    row_diff
}

fn build_data_diff_rows(
    segments: &[DataDiff],
    relocations: &[DataRelocationDiff],
    symbol_address: u64,
) -> Vec<DataDiffRow> {
    let total_len = segments.iter().map(|s| s.size as u64).sum::<u64>();
    let num_rows = total_len.div_ceil(BYTES_PER_ROW as u64) as usize;
    (0..num_rows)
        .map(|row_index| build_data_diff_row(segments, relocations, symbol_address, row_index))
        .collect()
}

use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::String,
    vec,
    vec::Vec,
};
use core::{num::NonZeroU32, ops::Range};

use anyhow::Result;

use crate::{
    diff::{
        code::{diff_code, no_diff_code},
        data::{
            diff_bss_section, diff_bss_symbol, diff_data_section, diff_data_symbol,
            diff_generic_section,
        },
    },
    obj::{InstructionRef, Object, Relocation, SectionKind, Symbol, SymbolFlag},
};

pub mod code;
pub mod data;
pub mod display;

include!(concat!(env!("OUT_DIR"), "/config.gen.rs"));

impl DiffObjConfig {
    pub fn separator(&self) -> &'static str { if self.space_between_args { ", " } else { "," } }
}

#[derive(Debug, Clone)]
pub struct SectionDiff {
    // pub target_section: Option<usize>,
    pub match_percent: Option<f32>,
    pub data_diff: Vec<DataDiff>,
    pub reloc_diff: Vec<DataRelocationDiff>,
}

#[derive(Debug, Clone, Default)]
pub struct SymbolDiff {
    /// The symbol index in the _other_ object that this symbol was diffed against
    pub target_symbol: Option<usize>,
    pub match_percent: Option<f32>,
    pub diff_score: Option<(u64, u64)>,
    pub instruction_rows: Vec<InstructionDiffRow>,
}

#[derive(Debug, Clone, Default)]
pub struct MappingSymbolDiff {
    pub symbol_index: usize,
    pub symbol_diff: SymbolDiff,
}

#[derive(Debug, Clone, Default)]
pub struct InstructionDiffRow {
    /// Instruction reference
    pub ins_ref: Option<InstructionRef>,
    /// Diff kind
    pub kind: InstructionDiffKind,
    /// Branches from instruction(s)
    pub branch_from: Option<InstructionBranchFrom>,
    /// Branches to instruction
    pub branch_to: Option<InstructionBranchTo>,
    /// Arg diffs
    pub arg_diff: Vec<InstructionArgDiffIndex>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum InstructionDiffKind {
    #[default]
    None,
    OpMismatch,
    ArgMismatch,
    Replace,
    Delete,
    Insert,
}

#[derive(Debug, Clone, Default)]
pub struct DataDiff {
    pub data: Vec<u8>,
    pub kind: DataDiffKind,
    pub len: usize,
    pub symbol: String,
}

#[derive(Debug, Clone)]
pub struct DataRelocationDiff {
    pub reloc: Relocation,
    pub kind: DataDiffKind,
    pub range: Range<usize>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum DataDiffKind {
    #[default]
    None,
    Replace,
    Delete,
    Insert,
}

/// Index of the argument diff for coloring.
#[repr(transparent)]
#[derive(Debug, Copy, Clone, Default)]
pub struct InstructionArgDiffIndex(pub Option<NonZeroU32>);

impl InstructionArgDiffIndex {
    pub const NONE: Self = Self(None);

    #[inline(always)]
    pub fn new(idx: u32) -> Self {
        Self(Some(unsafe { NonZeroU32::new_unchecked(idx.saturating_add(1)) }))
    }

    #[inline(always)]
    pub fn get(&self) -> Option<u32> { self.0.map(|idx| idx.get() - 1) }

    #[inline(always)]
    pub fn is_some(&self) -> bool { self.0.is_some() }

    #[inline(always)]
    pub fn is_none(&self) -> bool { self.0.is_none() }
}

#[derive(Debug, Clone)]
pub struct InstructionBranchFrom {
    /// Source instruction indices
    pub ins_idx: Vec<u32>,
    /// Incrementing index for coloring
    pub branch_idx: u32,
}

#[derive(Debug, Clone)]
pub struct InstructionBranchTo {
    /// Target instruction index
    pub ins_idx: u32,
    /// Incrementing index for coloring
    pub branch_idx: u32,
}

#[derive(Debug, Default)]
pub struct ObjectDiff {
    /// A list of all symbol diffs in the object.
    pub symbols: Vec<SymbolDiff>,
    /// A list of all section diffs in the object.
    pub sections: Vec<SectionDiff>,
    /// If `selecting_left` or `selecting_right` is set, this is the list of symbols
    /// that are being mapped to the other object.
    pub mapping_symbols: Vec<MappingSymbolDiff>,
}

impl ObjectDiff {
    pub fn new_from_obj(obj: &Object) -> Self {
        let mut result = Self {
            symbols: Vec::with_capacity(obj.symbols.len()),
            sections: Vec::with_capacity(obj.sections.len()),
            mapping_symbols: vec![],
        };
        for _ in obj.symbols.iter() {
            result.symbols.push(SymbolDiff {
                target_symbol: None,
                match_percent: None,
                diff_score: None,
                instruction_rows: vec![],
            });
        }
        for _ in obj.sections.iter() {
            result.sections.push(SectionDiff {
                // target_section: None,
                match_percent: None,
                data_diff: vec![],
                reloc_diff: vec![],
            });
        }
        result
    }
}

#[derive(Debug, Default)]
pub struct DiffObjsResult {
    pub left: Option<ObjectDiff>,
    pub right: Option<ObjectDiff>,
    pub prev: Option<ObjectDiff>,
}

pub fn diff_objs(
    left: Option<&Object>,
    right: Option<&Object>,
    prev: Option<&Object>,
    diff_config: &DiffObjConfig,
    mapping_config: &MappingConfig,
) -> Result<DiffObjsResult> {
    let symbol_matches = matching_symbols(left, right, prev, mapping_config)?;
    let section_matches = matching_sections(left, right)?;
    let mut left = left.map(|p| (p, ObjectDiff::new_from_obj(p)));
    let mut right = right.map(|p| (p, ObjectDiff::new_from_obj(p)));
    let mut prev = prev.map(|p| (p, ObjectDiff::new_from_obj(p)));

    for symbol_match in symbol_matches {
        match symbol_match {
            SymbolMatch {
                left: Some(left_symbol_ref),
                right: Some(right_symbol_ref),
                prev: prev_symbol_ref,
                section_kind,
            } => {
                let (left_obj, left_out) = left.as_mut().unwrap();
                let (right_obj, right_out) = right.as_mut().unwrap();
                match section_kind {
                    SectionKind::Code => {
                        let (left_diff, right_diff) = diff_code(
                            left_obj,
                            right_obj,
                            left_symbol_ref,
                            right_symbol_ref,
                            diff_config,
                        )?;
                        left_out.symbols[left_symbol_ref] = left_diff;
                        right_out.symbols[right_symbol_ref] = right_diff;

                        if let Some(prev_symbol_ref) = prev_symbol_ref {
                            let (_prev_obj, prev_out) = prev.as_mut().unwrap();
                            let (_, prev_diff) = diff_code(
                                left_obj,
                                right_obj,
                                right_symbol_ref,
                                prev_symbol_ref,
                                diff_config,
                            )?;
                            prev_out.symbols[prev_symbol_ref] = prev_diff;
                        }
                    }
                    SectionKind::Data => {
                        let (left_diff, right_diff) = diff_data_symbol(
                            left_obj,
                            right_obj,
                            left_symbol_ref,
                            right_symbol_ref,
                        )?;
                        left_out.symbols[left_symbol_ref] = left_diff;
                        right_out.symbols[right_symbol_ref] = right_diff;
                    }
                    SectionKind::Bss | SectionKind::Common => {
                        let (left_diff, right_diff) = diff_bss_symbol(
                            left_obj,
                            right_obj,
                            left_symbol_ref,
                            right_symbol_ref,
                        )?;
                        left_out.symbols[left_symbol_ref] = left_diff;
                        right_out.symbols[right_symbol_ref] = right_diff;
                    }
                    SectionKind::Unknown => unreachable!(),
                }
            }
            SymbolMatch { left: Some(left_symbol_ref), right: None, prev: _, section_kind } => {
                let (left_obj, left_out) = left.as_mut().unwrap();
                match section_kind {
                    SectionKind::Code => {
                        left_out.symbols[left_symbol_ref] =
                            no_diff_code(left_obj, left_symbol_ref, diff_config)?;
                    }
                    SectionKind::Data | SectionKind::Bss | SectionKind::Common => {
                        // Nothing needs to be done
                    }
                    SectionKind::Unknown => unreachable!(),
                }
            }
            SymbolMatch { left: None, right: Some(right_symbol_ref), prev: _, section_kind } => {
                let (right_obj, right_out) = right.as_mut().unwrap();
                match section_kind {
                    SectionKind::Code => {
                        right_out.symbols[right_symbol_ref] =
                            no_diff_code(right_obj, right_symbol_ref, diff_config)?;
                    }
                    SectionKind::Data | SectionKind::Bss | SectionKind::Common => {
                        // Nothing needs to be done
                    }
                    SectionKind::Unknown => unreachable!(),
                }
            }
            SymbolMatch { left: None, right: None, .. } => {
                // Should not happen
            }
        }
    }

    for section_match in section_matches {
        if let SectionMatch {
            left: Some(left_section_idx),
            right: Some(right_section_idx),
            section_kind,
        } = section_match
        {
            let (left_obj, left_out) = left.as_mut().unwrap();
            let (right_obj, right_out) = right.as_mut().unwrap();
            match section_kind {
                SectionKind::Code => {
                    let (left_diff, right_diff) = diff_generic_section(
                        left_obj,
                        right_obj,
                        left_out,
                        right_out,
                        left_section_idx,
                        right_section_idx,
                    )?;
                    left_out.sections[left_section_idx] = left_diff;
                    right_out.sections[right_section_idx] = right_diff;
                }
                SectionKind::Data => {
                    let (left_diff, right_diff) = diff_data_section(
                        left_obj,
                        right_obj,
                        left_out,
                        right_out,
                        left_section_idx,
                        right_section_idx,
                    )?;
                    left_out.sections[left_section_idx] = left_diff;
                    right_out.sections[right_section_idx] = right_diff;
                }
                SectionKind::Bss | SectionKind::Common => {
                    let (left_diff, right_diff) = diff_bss_section(
                        left_obj,
                        right_obj,
                        left_out,
                        right_out,
                        left_section_idx,
                        right_section_idx,
                    )?;
                    left_out.sections[left_section_idx] = left_diff;
                    right_out.sections[right_section_idx] = right_diff;
                }
                SectionKind::Unknown => unreachable!(),
            }
        }
    }

    if let (Some((right_obj, right_out)), Some((left_obj, left_out))) =
        (right.as_mut(), left.as_mut())
    {
        if let Some(right_name) = &mapping_config.selecting_left {
            generate_mapping_symbols(right_obj, right_name, left_obj, left_out, diff_config)?;
        }
        if let Some(left_name) = &mapping_config.selecting_right {
            generate_mapping_symbols(left_obj, left_name, right_obj, right_out, diff_config)?;
        }
    }

    Ok(DiffObjsResult {
        left: left.map(|(_, o)| o),
        right: right.map(|(_, o)| o),
        prev: prev.map(|(_, o)| o),
    })
}

/// When we're selecting a symbol to use as a comparison, we'll create comparisons for all
/// symbols in the other object that match the selected symbol's section and kind. This allows
/// us to display match percentages for all symbols in the other object that could be selected.
fn generate_mapping_symbols(
    base_obj: &Object,
    base_name: &str,
    target_obj: &Object,
    target_out: &mut ObjectDiff,
    config: &DiffObjConfig,
) -> Result<()> {
    let Some(base_symbol_ref) = symbol_ref_by_name(base_obj, base_name) else {
        return Ok(());
    };
    let base_section_kind = symbol_section_kind(base_obj, &base_obj.symbols[base_symbol_ref]);
    for (target_symbol_index, target_symbol) in target_obj.symbols.iter().enumerate() {
        if symbol_section_kind(target_obj, target_symbol) != base_section_kind {
            continue;
        }
        match base_section_kind {
            SectionKind::Code => {
                let (left_diff, _right_diff) =
                    diff_code(target_obj, base_obj, target_symbol_index, base_symbol_ref, config)?;
                target_out.mapping_symbols.push(MappingSymbolDiff {
                    symbol_index: target_symbol_index,
                    symbol_diff: left_diff,
                });
            }
            SectionKind::Data => {
                let (left_diff, _right_diff) =
                    diff_data_symbol(target_obj, base_obj, target_symbol_index, base_symbol_ref)?;
                target_out.mapping_symbols.push(MappingSymbolDiff {
                    symbol_index: target_symbol_index,
                    symbol_diff: left_diff,
                });
            }
            SectionKind::Bss | SectionKind::Common => {
                let (left_diff, _right_diff) =
                    diff_bss_symbol(target_obj, base_obj, target_symbol_index, base_symbol_ref)?;
                target_out.mapping_symbols.push(MappingSymbolDiff {
                    symbol_index: target_symbol_index,
                    symbol_diff: left_diff,
                });
            }
            SectionKind::Unknown => {}
        }
    }
    Ok(())
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct SymbolMatch {
    left: Option<usize>,
    right: Option<usize>,
    prev: Option<usize>,
    section_kind: SectionKind,
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct SectionMatch {
    left: Option<usize>,
    right: Option<usize>,
    section_kind: SectionKind,
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct MappingConfig {
    /// Manual symbol mappings
    pub mappings: BTreeMap<String, String>,
    /// The right object symbol name that we're selecting a left symbol for
    pub selecting_left: Option<String>,
    /// The left object symbol name that we're selecting a right symbol for
    pub selecting_right: Option<String>,
}

fn symbol_ref_by_name(obj: &Object, name: &str) -> Option<usize> {
    obj.symbols.iter().position(|s| s.name == name)
}

fn apply_symbol_mappings(
    left: &Object,
    right: &Object,
    mapping_config: &MappingConfig,
    left_used: &mut BTreeSet<usize>,
    right_used: &mut BTreeSet<usize>,
    matches: &mut Vec<SymbolMatch>,
) -> Result<()> {
    // If we're selecting a symbol to use as a comparison, mark it as used
    // This ensures that we don't match it to another symbol at any point
    if let Some(left_name) = &mapping_config.selecting_left {
        if let Some(left_symbol) = symbol_ref_by_name(left, left_name) {
            left_used.insert(left_symbol);
        }
    }
    if let Some(right_name) = &mapping_config.selecting_right {
        if let Some(right_symbol) = symbol_ref_by_name(right, right_name) {
            right_used.insert(right_symbol);
        }
    }

    // Apply manual symbol mappings
    for (left_name, right_name) in &mapping_config.mappings {
        let Some(left_symbol_index) = symbol_ref_by_name(left, left_name) else {
            continue;
        };
        if left_used.contains(&left_symbol_index) {
            continue;
        }
        let Some(right_symbol_index) = symbol_ref_by_name(right, right_name) else {
            continue;
        };
        if right_used.contains(&right_symbol_index) {
            continue;
        }
        let left_section_kind = left
            .symbols
            .get(left_symbol_index)
            .and_then(|s| s.section)
            .and_then(|section_index| left.sections.get(section_index))
            .map_or(SectionKind::Unknown, |s| s.kind);
        let right_section_kind = right
            .symbols
            .get(right_symbol_index)
            .and_then(|s| s.section)
            .and_then(|section_index| right.sections.get(section_index))
            .map_or(SectionKind::Unknown, |s| s.kind);
        if left_section_kind != right_section_kind {
            log::warn!(
                "Symbol section kind mismatch: {} ({:?}) vs {} ({:?})",
                left_name,
                left_section_kind,
                right_name,
                right_section_kind
            );
            continue;
        }
        matches.push(SymbolMatch {
            left: Some(left_symbol_index),
            right: Some(right_symbol_index),
            prev: None, // TODO
            section_kind: left_section_kind,
        });
        left_used.insert(left_symbol_index);
        right_used.insert(right_symbol_index);
    }
    Ok(())
}

/// Find matching symbols between each object.
fn matching_symbols(
    left: Option<&Object>,
    right: Option<&Object>,
    prev: Option<&Object>,
    mappings: &MappingConfig,
) -> Result<Vec<SymbolMatch>> {
    let mut matches = Vec::new();
    let mut left_used = BTreeSet::new();
    let mut right_used = BTreeSet::new();
    if let Some(left) = left {
        if let Some(right) = right {
            apply_symbol_mappings(
                left,
                right,
                mappings,
                &mut left_used,
                &mut right_used,
                &mut matches,
            )?;
        }
        for (symbol_idx, symbol) in left.symbols.iter().enumerate() {
            let section_kind = symbol_section_kind(left, symbol);
            if section_kind == SectionKind::Unknown {
                continue;
            }
            if left_used.contains(&symbol_idx) {
                continue;
            }
            let symbol_match = SymbolMatch {
                left: Some(symbol_idx),
                right: find_symbol(right, left, symbol, Some(&right_used)),
                prev: find_symbol(prev, left, symbol, None),
                section_kind,
            };
            matches.push(symbol_match);
            if let Some(right) = symbol_match.right {
                right_used.insert(right);
            }
        }
    }
    if let Some(right) = right {
        for (symbol_idx, symbol) in right.symbols.iter().enumerate() {
            let section_kind = symbol_section_kind(right, symbol);
            if section_kind == SectionKind::Unknown {
                continue;
            }
            if right_used.contains(&symbol_idx) {
                continue;
            }
            matches.push(SymbolMatch {
                left: None,
                right: Some(symbol_idx),
                prev: find_symbol(prev, right, symbol, None),
                section_kind,
            });
        }
    }
    Ok(matches)
}

fn unmatched_symbols<'obj, 'used>(
    obj: &'obj Object,
    used: Option<&'used BTreeSet<usize>>,
) -> impl Iterator<Item = (usize, &'obj Symbol)> + 'used
where
    'obj: 'used,
{
    obj.symbols.iter().enumerate().filter(move |&(symbol_idx, _)| {
        // Skip symbols that have already been matched
        !used.is_some_and(|u| u.contains(&symbol_idx))
    })
}

fn symbol_section<'obj>(obj: &'obj Object, symbol: &Symbol) -> Option<(&'obj str, SectionKind)> {
    if let Some(section) = symbol.section.and_then(|section_idx| obj.sections.get(section_idx)) {
        Some((section.name.as_str(), section.kind))
    } else if symbol.flags.contains(SymbolFlag::Common) {
        Some((".comm", SectionKind::Common))
    } else {
        None
    }
}

fn symbol_section_kind(obj: &Object, symbol: &Symbol) -> SectionKind {
    match symbol.section {
        Some(section_index) => obj.sections[section_index].kind,
        None if symbol.flags.contains(SymbolFlag::Common) => SectionKind::Common,
        None => SectionKind::Unknown,
    }
}

fn find_symbol(
    obj: Option<&Object>,
    in_obj: &Object,
    in_symbol: &Symbol,
    used: Option<&BTreeSet<usize>>,
) -> Option<usize> {
    let obj = obj?;
    let (section_name, section_kind) = symbol_section(in_obj, in_symbol)?;
    // Try to find an exact name match
    if let Some((symbol_idx, _)) = unmatched_symbols(obj, used).find(|(_, symbol)| {
        symbol.name == in_symbol.name && symbol_section_kind(obj, symbol) == section_kind
    }) {
        return Some(symbol_idx);
    }
    // Match compiler-generated symbols against each other (e.g. @251 -> @60)
    // If they are at the same address in the same section
    if in_symbol.name.starts_with('@')
        && matches!(section_kind, SectionKind::Data | SectionKind::Bss)
    {
        if let Some((symbol_idx, _)) = unmatched_symbols(obj, used).find(|(_, symbol)| {
            let Some(section_index) = symbol.section else {
                return false;
            };
            symbol.name.starts_with('@')
                && symbol.address == in_symbol.address
                && obj.sections[section_index].name == section_name
        }) {
            return Some(symbol_idx);
        }
    }
    // Match Metrowerks symbol$1234 against symbol$2345
    if let Some((prefix, suffix)) = in_symbol.name.split_once('$') {
        if !suffix.chars().all(char::is_numeric) {
            return None;
        }
        if let Some((symbol_idx, _)) = unmatched_symbols(obj, used).find(|&(_, symbol)| {
            if let Some((p, s)) = symbol.name.split_once('$') {
                prefix == p
                    && s.chars().all(char::is_numeric)
                    && symbol_section_kind(obj, symbol) == section_kind
            } else {
                false
            }
        }) {
            return Some(symbol_idx);
        }
    }
    None
}

/// Find matching sections between each object.
fn matching_sections(left: Option<&Object>, right: Option<&Object>) -> Result<Vec<SectionMatch>> {
    let mut matches = Vec::new();
    if let Some(left) = left {
        for (section_idx, section) in left.sections.iter().enumerate() {
            if section.kind == SectionKind::Unknown {
                continue;
            }
            matches.push(SectionMatch {
                left: Some(section_idx),
                right: find_section(right, &section.name, section.kind),
                section_kind: section.kind,
            });
        }
    }
    if let Some(right) = right {
        for (section_idx, section) in right.sections.iter().enumerate() {
            if section.kind == SectionKind::Unknown {
                continue;
            }
            if matches.iter().any(|m| m.right == Some(section_idx)) {
                continue;
            }
            matches.push(SectionMatch {
                left: None,
                right: Some(section_idx),
                section_kind: section.kind,
            });
        }
    }
    Ok(matches)
}

fn find_section(obj: Option<&Object>, name: &str, section_kind: SectionKind) -> Option<usize> {
    obj?.sections.iter().position(|s| s.kind == section_kind && s.name == name)
}

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
            diff_generic_section, no_diff_bss_section, no_diff_data_section, no_diff_data_symbol,
            symbol_name_matches,
        },
    },
    obj::{InstructionRef, Object, Relocation, SectionKind, Symbol, SymbolFlag},
};

pub mod code;
pub mod data;
pub mod demangler;
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
    pub data_rows: Vec<DataDiffRow>,
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
    pub size: usize,
    pub kind: DataDiffKind,
}

#[derive(Debug, Clone)]
pub struct DataRelocationDiff {
    pub reloc: Relocation,
    pub range: Range<u64>,
    pub kind: DataDiffKind,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum DataDiffKind {
    #[default]
    None,
    Replace,
    Delete,
    Insert,
}

#[derive(Debug, Clone, Default)]
pub struct DataDiffRow {
    pub address: u64,
    pub segments: Vec<DataDiff>,
    pub relocations: Vec<DataRelocationDiff>,
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
                ..Default::default()
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
                    SectionKind::Data => {
                        left_out.symbols[left_symbol_ref] =
                            no_diff_data_symbol(left_obj, left_symbol_ref)?;
                    }
                    SectionKind::Bss | SectionKind::Common => {
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
                    SectionKind::Data => {
                        right_out.symbols[right_symbol_ref] =
                            no_diff_data_symbol(right_obj, right_symbol_ref)?;
                    }
                    SectionKind::Bss | SectionKind::Common => {
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
        match section_match {
            SectionMatch {
                left: Some(left_section_idx),
                right: Some(right_section_idx),
                section_kind,
            } => {
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
            SectionMatch { left: Some(left_section_idx), right: None, section_kind } => {
                let (left_obj, left_out) = left.as_mut().unwrap();
                match section_kind {
                    SectionKind::Code => {}
                    SectionKind::Data => {
                        left_out.sections[left_section_idx] =
                            no_diff_data_section(left_obj, left_section_idx)?;
                    }
                    SectionKind::Bss | SectionKind::Common => {
                        left_out.sections[left_section_idx] = no_diff_bss_section()?;
                    }
                    SectionKind::Unknown => unreachable!(),
                }
            }
            SectionMatch { left: None, right: Some(right_section_idx), section_kind } => {
                let (right_obj, right_out) = right.as_mut().unwrap();
                match section_kind {
                    SectionKind::Code => {}
                    SectionKind::Data => {
                        right_out.sections[right_section_idx] =
                            no_diff_data_section(right_obj, right_section_idx)?;
                    }
                    SectionKind::Bss | SectionKind::Common => {
                        right_out.sections[right_section_idx] = no_diff_bss_section()?;
                    }
                    SectionKind::Unknown => unreachable!(),
                }
            }
            SectionMatch { left: None, right: None, .. } => {
                // Should not happen
            }
        }
    }

    if let (Some((right_obj, right_out)), Some((left_obj, left_out))) =
        (right.as_mut(), left.as_mut())
    {
        if let Some(right_name) = mapping_config.selecting_left.as_deref() {
            generate_mapping_symbols(
                left_obj,
                left_out,
                right_obj,
                right_out,
                MappingSymbol::Right(right_name),
                diff_config,
            )?;
        }
        if let Some(left_name) = mapping_config.selecting_right.as_deref() {
            generate_mapping_symbols(
                left_obj,
                left_out,
                right_obj,
                right_out,
                MappingSymbol::Left(left_name),
                diff_config,
            )?;
        }
    }

    Ok(DiffObjsResult {
        left: left.map(|(_, o)| o),
        right: right.map(|(_, o)| o),
        prev: prev.map(|(_, o)| o),
    })
}

#[derive(Clone, Copy)]
enum MappingSymbol<'a> {
    Left(&'a str),
    Right(&'a str),
}

/// When we're selecting a symbol to use as a comparison, we'll create comparisons for all
/// symbols in the other object that match the selected symbol's section and kind. This allows
/// us to display match percentages for all symbols in the other object that could be selected.
fn generate_mapping_symbols(
    left_obj: &Object,
    left_out: &mut ObjectDiff,
    right_obj: &Object,
    right_out: &mut ObjectDiff,
    mapping_symbol: MappingSymbol,
    config: &DiffObjConfig,
) -> Result<()> {
    let (base_obj, base_name, target_obj) = match mapping_symbol {
        MappingSymbol::Left(name) => (left_obj, name, right_obj),
        MappingSymbol::Right(name) => (right_obj, name, left_obj),
    };
    let Some(base_symbol_ref) = base_obj.symbol_by_name(base_name) else {
        return Ok(());
    };
    let base_section_kind = symbol_section_kind(base_obj, &base_obj.symbols[base_symbol_ref]);
    for (target_symbol_index, target_symbol) in target_obj.symbols.iter().enumerate() {
        if target_symbol.size == 0
            || target_symbol.flags.contains(SymbolFlag::Ignored)
            || symbol_section_kind(target_obj, target_symbol) != base_section_kind
        {
            continue;
        }
        let (left_symbol_idx, right_symbol_idx) = match mapping_symbol {
            MappingSymbol::Left(_) => (base_symbol_ref, target_symbol_index),
            MappingSymbol::Right(_) => (target_symbol_index, base_symbol_ref),
        };
        let (left_diff, right_diff) = match base_section_kind {
            SectionKind::Code => {
                diff_code(left_obj, right_obj, left_symbol_idx, right_symbol_idx, config)
            }
            SectionKind::Data => {
                diff_data_symbol(left_obj, right_obj, left_symbol_idx, right_symbol_idx)
            }
            SectionKind::Bss | SectionKind::Common => {
                diff_bss_symbol(left_obj, right_obj, left_symbol_idx, right_symbol_idx)
            }
            SectionKind::Unknown => continue,
        }?;
        match mapping_symbol {
            MappingSymbol::Left(_) => right_out.mapping_symbols.push(MappingSymbolDiff {
                symbol_index: right_symbol_idx,
                symbol_diff: right_diff,
            }),
            MappingSymbol::Right(_) => left_out
                .mapping_symbols
                .push(MappingSymbolDiff { symbol_index: left_symbol_idx, symbol_diff: left_diff }),
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
    if let Some(left_name) = &mapping_config.selecting_left
        && let Some(left_symbol) = left.symbol_by_name(left_name)
    {
        left_used.insert(left_symbol);
    }
    if let Some(right_name) = &mapping_config.selecting_right
        && let Some(right_symbol) = right.symbol_by_name(right_name)
    {
        right_used.insert(right_symbol);
    }

    // Apply manual symbol mappings
    for (left_name, right_name) in &mapping_config.mappings {
        let Some(left_symbol_index) = left.symbol_by_name(left_name) else {
            continue;
        };
        if left_used.contains(&left_symbol_index) {
            continue;
        }
        let Some(right_symbol_index) = right.symbol_by_name(right_name) else {
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
                "Symbol section kind mismatch: {left_name} ({left_section_kind:?}) vs {right_name} ({right_section_kind:?})"
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
        // Do two passes for nameless literals. The first only pairs up perfect matches to ensure
        // those are correct first, while the second pass catches near matches.
        for fuzzy_literals in [false, true] {
            for (symbol_idx, symbol) in left.symbols.iter().enumerate() {
                if symbol.size == 0 || symbol.flags.contains(SymbolFlag::Ignored) {
                    continue;
                }
                let section_kind = symbol_section_kind(left, symbol);
                if section_kind == SectionKind::Unknown {
                    continue;
                }
                if left_used.contains(&symbol_idx) {
                    continue;
                }
                let symbol_match = SymbolMatch {
                    left: Some(symbol_idx),
                    right: find_symbol(right, left, symbol_idx, Some(&right_used), fuzzy_literals),
                    prev: find_symbol(prev, left, symbol_idx, None, fuzzy_literals),
                    section_kind,
                };
                matches.push(symbol_match);
                if let Some(right) = symbol_match.right {
                    left_used.insert(symbol_idx);
                    right_used.insert(right);
                }
            }
        }
    }
    if let Some(right) = right {
        // Do two passes for nameless literals. The first only pairs up perfect matches to ensure
        // those are correct first, while the second pass catches near matches.
        for fuzzy_literals in [false, true] {
            for (symbol_idx, symbol) in right.symbols.iter().enumerate() {
                if symbol.size == 0 || symbol.flags.contains(SymbolFlag::Ignored) {
                    continue;
                }
                let section_kind = symbol_section_kind(right, symbol);
                if section_kind == SectionKind::Unknown {
                    continue;
                }
                if right_used.contains(&symbol_idx) {
                    continue;
                }
                let symbol_match = SymbolMatch {
                    left: None,
                    right: Some(symbol_idx),
                    prev: find_symbol(prev, right, symbol_idx, None, fuzzy_literals),
                    section_kind,
                };
                matches.push(symbol_match);
                if symbol_match.prev.is_some() {
                    right_used.insert(symbol_idx);
                }
            }
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
    obj.symbols.iter().enumerate().filter(move |&(symbol_idx, symbol)| {
        !symbol.flags.contains(SymbolFlag::Ignored)
            // Skip symbols that have already been matched
            && !used.is_some_and(|u| u.contains(&symbol_idx))
    })
}

fn symbol_section<'obj>(obj: &'obj Object, symbol: &Symbol) -> Option<(&'obj str, SectionKind)> {
    if let Some(section) = symbol.section.and_then(|section_idx| obj.sections.get(section_idx)) {
        // Match x86 .rdata$r against .rdata$rs
        let section_name =
            section.name.split_once('$').map_or(section.name.as_str(), |(prefix, _)| prefix);
        Some((section_name, section.kind))
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
    in_symbol_idx: usize,
    used: Option<&BTreeSet<usize>>,
    fuzzy_literals: bool,
) -> Option<usize> {
    let in_symbol = &in_obj.symbols[in_symbol_idx];
    let obj = obj?;
    let (section_name, section_kind) = symbol_section(in_obj, in_symbol)?;

    // Match compiler-generated symbols against each other (e.g. @251 -> @60)
    // If they are in the same section and have the same value
    if in_symbol.is_name_compiler_generated
        && matches!(section_kind, SectionKind::Code | SectionKind::Data | SectionKind::Bss)
    {
        let mut closest_match_symbol_idx = None;
        let mut closest_match_percent = 0.0;
        for (symbol_idx, symbol) in unmatched_symbols(obj, used) {
            let Some(section_index) = symbol.section else {
                continue;
            };
            if obj.sections[section_index].name != section_name {
                continue;
            }
            if !symbol.is_name_compiler_generated {
                continue;
            }
            match section_kind {
                SectionKind::Data | SectionKind::Code => {
                    // For code or data, pick the first symbol with exactly matching bytes and relocations.
                    // If no symbols match exactly, and `fuzzy_literals` is true, pick the closest
                    // plausible match instead.
                    if let Ok((left_diff, _right_diff)) =
                        diff_data_symbol(in_obj, obj, in_symbol_idx, symbol_idx)
                        && let Some(match_percent) = left_diff.match_percent
                        && (match_percent == 100.0
                            || (fuzzy_literals
                                && match_percent >= 50.0
                                && match_percent > closest_match_percent))
                    {
                        closest_match_symbol_idx = Some(symbol_idx);
                        closest_match_percent = match_percent;
                        if match_percent == 100.0 {
                            break;
                        }
                    }
                }
                SectionKind::Bss => {
                    // For BSS, pick the first symbol that has the exact matching size.
                    if in_symbol.size == symbol.size {
                        closest_match_symbol_idx = Some(symbol_idx);
                        break;
                    }
                }
                _ => unreachable!(),
            }
        }
        return closest_match_symbol_idx;
    }

    // Try to find a symbol with a matching name
    if let Some((symbol_idx, _)) = unmatched_symbols(obj, used)
        .filter(|&(_, symbol)| {
            symbol_name_matches(in_symbol, symbol)
                && symbol_section_kind(obj, symbol) == section_kind
                && symbol_section(obj, symbol).is_some_and(|(name, _)| name == section_name)
        })
        .min_by_key(|&(_, symbol)| (symbol.section, symbol.address))
    {
        return Some(symbol_idx);
    }

    None
}

/// Find matching sections between each object.
fn matching_sections(left: Option<&Object>, right: Option<&Object>) -> Result<Vec<SectionMatch>> {
    let mut matches = Vec::with_capacity(
        left.as_ref()
            .map_or(0, |o| o.sections.len())
            .max(right.as_ref().map_or(0, |o| o.sections.len())),
    );
    if let Some(left) = left {
        for (section_idx, section) in left.sections.iter().enumerate() {
            if section.kind == SectionKind::Unknown {
                continue;
            }
            matches.push(SectionMatch {
                left: Some(section_idx),
                right: find_section(right, &section.name, section.kind, &matches),
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

fn find_section(
    obj: Option<&Object>,
    name: &str,
    section_kind: SectionKind,
    matches: &[SectionMatch],
) -> Option<usize> {
    obj?.sections.iter().enumerate().position(|(i, s)| {
        s.kind == section_kind && s.name == name && !matches.iter().any(|m| m.right == Some(i))
    })
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DiffSide {
    /// The target/expected side of the diff.
    Target,
    /// The base side of the diff.
    Base,
}

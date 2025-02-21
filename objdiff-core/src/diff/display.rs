use alloc::{
    borrow::Cow,
    collections::BTreeSet,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::cmp::Ordering;

use anyhow::Result;
use itertools::Itertools;
use regex::Regex;

use crate::{
    diff::{DiffObjConfig, InstructionArgDiffIndex, InstructionDiffRow, ObjectDiff, SymbolDiff},
    obj::{
        InstructionArg, InstructionArgValue, Object, SectionFlag, SectionKind, Symbol, SymbolFlag,
        SymbolKind,
    },
};

#[derive(Debug, Copy, Clone)]
pub enum DiffText<'a> {
    /// Basic text
    Basic(&'a str),
    /// Line number
    Line(u32),
    /// Instruction address
    Address(u64),
    /// Instruction mnemonic
    Opcode(&'a str, u16),
    /// Instruction argument
    Argument(&'a InstructionArgValue),
    /// Branch destination
    BranchDest(u64),
    /// Symbol name
    Symbol(&'a Symbol),
    /// Relocation addend
    Addend(i64),
    /// Number of spaces
    Spacing(usize),
    /// End of line
    Eol,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum HighlightKind {
    #[default]
    None,
    Opcode(u16),
    Argument(InstructionArgValue),
    Symbol(String),
    Address(u64),
}

pub enum InstructionPart {
    Basic(&'static str),
    Opcode(Cow<'static, str>, u16),
    Arg(InstructionArg),
    Separator,
}

pub fn display_row(
    obj: &Object,
    symbol_index: usize,
    ins_row: &InstructionDiffRow,
    diff_config: &DiffObjConfig,
    mut cb: impl FnMut(DiffText, InstructionArgDiffIndex) -> Result<()>,
) -> Result<()> {
    let Some(ins_ref) = ins_row.ins_ref else {
        cb(DiffText::Eol, InstructionArgDiffIndex::NONE)?;
        return Ok(());
    };
    let symbol = &obj.symbols[symbol_index];
    let Some(section_index) = symbol.section else {
        cb(DiffText::Eol, InstructionArgDiffIndex::NONE)?;
        return Ok(());
    };
    let section = &obj.sections[section_index];
    let Some(data) = section.data_range(ins_ref.address, ins_ref.size as usize) else {
        cb(DiffText::Eol, InstructionArgDiffIndex::NONE)?;
        return Ok(());
    };
    if let Some(line) = section.line_info.range(..=ins_ref.address).last().map(|(_, &b)| b) {
        cb(DiffText::Line(line), InstructionArgDiffIndex::NONE)?;
    }
    cb(
        DiffText::Address(ins_ref.address.saturating_sub(symbol.address)),
        InstructionArgDiffIndex::NONE,
    )?;
    if let Some(branch) = &ins_row.branch_from {
        cb(DiffText::Basic(" ~> "), InstructionArgDiffIndex::new(branch.branch_idx))?;
    } else {
        cb(DiffText::Spacing(4), InstructionArgDiffIndex::NONE)?;
    }
    let mut arg_idx = 0;
    let relocation = section.relocation_at(ins_ref.address, obj);
    obj.arch.display_instruction(
        ins_ref,
        data,
        relocation,
        symbol.address..symbol.address + symbol.size,
        section_index,
        diff_config,
        &mut |part| match part {
            InstructionPart::Basic(text) => {
                cb(DiffText::Basic(text), InstructionArgDiffIndex::NONE)
            }
            InstructionPart::Opcode(mnemonic, opcode) => {
                cb(DiffText::Opcode(mnemonic.as_ref(), opcode), InstructionArgDiffIndex::NONE)
            }
            InstructionPart::Arg(arg) => {
                let diff_index = ins_row.arg_diff.get(arg_idx).copied().unwrap_or_default();
                arg_idx += 1;
                match arg {
                    InstructionArg::Value(ref value) => cb(DiffText::Argument(value), diff_index),
                    InstructionArg::Reloc => {
                        let resolved = relocation.unwrap();
                        cb(DiffText::Symbol(resolved.symbol), diff_index)?;
                        if resolved.relocation.addend != 0 {
                            cb(DiffText::Addend(resolved.relocation.addend), diff_index)?;
                        }
                        Ok(())
                    }
                    InstructionArg::BranchDest(dest) => {
                        if let Some(addr) = dest.checked_sub(symbol.address) {
                            cb(DiffText::BranchDest(addr), diff_index)
                        } else {
                            cb(
                                DiffText::Argument(&InstructionArgValue::Opaque(Cow::Borrowed(
                                    "<invalid>",
                                ))),
                                diff_index,
                            )
                        }
                    }
                }
            }
            InstructionPart::Separator => {
                cb(DiffText::Basic(diff_config.separator()), InstructionArgDiffIndex::NONE)
            }
        },
    )?;
    if let Some(branch) = &ins_row.branch_to {
        cb(DiffText::Basic(" ~>"), InstructionArgDiffIndex::new(branch.branch_idx))?;
    }
    cb(DiffText::Eol, InstructionArgDiffIndex::NONE)?;
    Ok(())
}

impl PartialEq<DiffText<'_>> for HighlightKind {
    fn eq(&self, other: &DiffText) -> bool {
        match (self, other) {
            (HighlightKind::Opcode(a), DiffText::Opcode(_, b)) => a == b,
            (HighlightKind::Argument(a), DiffText::Argument(b)) => a.loose_eq(b),
            (HighlightKind::Symbol(a), DiffText::Symbol(b)) => a == &b.name,
            (HighlightKind::Address(a), DiffText::Address(b) | DiffText::BranchDest(b)) => a == b,
            _ => false,
        }
    }
}

impl PartialEq<HighlightKind> for DiffText<'_> {
    fn eq(&self, other: &HighlightKind) -> bool { other.eq(self) }
}

impl From<DiffText<'_>> for HighlightKind {
    fn from(value: DiffText<'_>) -> Self {
        match value {
            DiffText::Opcode(_, op) => HighlightKind::Opcode(op),
            DiffText::Argument(arg) => HighlightKind::Argument(arg.clone()),
            DiffText::Symbol(sym) => HighlightKind::Symbol(sym.name.to_string()),
            DiffText::Address(addr) | DiffText::BranchDest(addr) => HighlightKind::Address(addr),
            _ => HighlightKind::None,
        }
    }
}

pub enum ContextMenuItem {
    Copy { value: String, label: Option<String> },
    Navigate { label: String },
}

pub enum HoverItemColor {
    Normal,     // Gray
    Emphasized, // White
    Special,    // Blue
}

pub struct HoverItem {
    pub text: String,
    pub color: HoverItemColor,
}

pub fn symbol_context(_obj: &Object, symbol: &Symbol) -> Vec<ContextMenuItem> {
    let mut out = Vec::new();
    if let Some(name) = &symbol.demangled_name {
        out.push(ContextMenuItem::Copy { value: name.clone(), label: None });
    }
    out.push(ContextMenuItem::Copy { value: symbol.name.clone(), label: None });
    if let Some(address) = symbol.virtual_address {
        out.push(ContextMenuItem::Copy {
            value: format!("{:#x}", address),
            label: Some("virtual address".to_string()),
        });
    }
    // if let Some(_extab) = obj.arch.ppc().and_then(|ppc| ppc.extab_for_symbol(symbol)) {
    //     out.push(ContextMenuItem::Navigate { label: "Decode exception table".to_string() });
    // }
    out
}

pub fn symbol_hover(_obj: &Object, symbol: &Symbol) -> Vec<HoverItem> {
    let mut out = Vec::new();
    out.push(HoverItem {
        text: format!("Name: {}", symbol.name),
        color: HoverItemColor::Emphasized,
    });
    out.push(HoverItem {
        text: format!("Address: {:x}", symbol.address),
        color: HoverItemColor::Emphasized,
    });
    if symbol.flags.contains(SymbolFlag::SizeInferred) {
        out.push(HoverItem {
            text: format!("Size: {:x} (inferred)", symbol.size),
            color: HoverItemColor::Emphasized,
        });
    } else {
        out.push(HoverItem {
            text: format!("Size: {:x}", symbol.size),
            color: HoverItemColor::Emphasized,
        });
    }
    if let Some(address) = symbol.virtual_address {
        out.push(HoverItem {
            text: format!("Virtual address: {:#x}", address),
            color: HoverItemColor::Special,
        });
    }
    // if let Some(extab) = obj.arch.ppc().and_then(|ppc| ppc.extab_for_symbol(symbol)) {
    //     out.push(HoverItem {
    //         text: format!("extab symbol: {}", extab.etb_symbol.name),
    //         color: HoverItemColor::Special,
    //     });
    //     out.push(HoverItem {
    //         text: format!("extabindex symbol: {}", extab.eti_symbol.name),
    //         color: HoverItemColor::Special,
    //     });
    // }
    out
}

#[derive(Copy, Clone)]
pub enum SymbolFilter<'a> {
    None,
    Search(&'a Regex),
    Mapping(usize, Option<&'a Regex>),
}

fn symbol_matches_filter(
    symbol: &Symbol,
    diff: &SymbolDiff,
    filter: SymbolFilter<'_>,
    show_hidden_symbols: bool,
) -> bool {
    // Ignore absolute symbols
    if symbol.section.is_none() && !symbol.flags.contains(SymbolFlag::Common) {
        return false;
    }
    if !show_hidden_symbols && symbol.flags.contains(SymbolFlag::Hidden) {
        return false;
    }
    match filter {
        SymbolFilter::None => true,
        SymbolFilter::Search(regex) => {
            regex.is_match(&symbol.name)
                || symbol.demangled_name.as_deref().is_some_and(|s| regex.is_match(s))
        }
        SymbolFilter::Mapping(symbol_ref, regex) => {
            diff.target_symbol == Some(symbol_ref)
                && regex.is_none_or(|r| {
                    r.is_match(&symbol.name)
                        || symbol.demangled_name.as_deref().is_some_and(|s| r.is_match(s))
                })
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct SectionDisplaySymbol {
    pub symbol: usize,
    pub is_mapping_symbol: bool,
}

#[derive(Debug, Clone)]
pub struct SectionDisplay {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub match_percent: Option<f32>,
    pub symbols: Vec<SectionDisplaySymbol>,
}

pub fn display_sections(
    obj: &Object,
    diff: &ObjectDiff,
    filter: SymbolFilter<'_>,
    show_hidden_symbols: bool,
    show_mapped_symbols: bool,
    reverse_fn_order: bool,
) -> Vec<SectionDisplay> {
    let mut mapping = BTreeSet::new();
    let is_mapping_symbol = if let SymbolFilter::Mapping(_, _) = filter {
        for mapping_diff in &diff.mapping_symbols {
            let symbol = &obj.symbols[mapping_diff.symbol_index];
            if !symbol_matches_filter(
                symbol,
                &mapping_diff.symbol_diff,
                filter,
                show_hidden_symbols,
            ) {
                continue;
            }
            if !show_mapped_symbols {
                let symbol_diff = &diff.symbols[mapping_diff.symbol_index];
                if symbol_diff.target_symbol.is_some() {
                    continue;
                }
            }
            mapping.insert((symbol.section, mapping_diff.symbol_index));
        }
        true
    } else {
        for (symbol_idx, (symbol, symbol_diff)) in obj.symbols.iter().zip(&diff.symbols).enumerate()
        {
            if !symbol_matches_filter(symbol, symbol_diff, filter, show_hidden_symbols) {
                continue;
            }
            mapping.insert((symbol.section, symbol_idx));
        }
        false
    };
    let num_sections = mapping.iter().map(|(section_idx, _)| *section_idx).dedup().count();
    let mut sections = Vec::with_capacity(num_sections);
    for (section_idx, group) in &mapping.iter().chunk_by(|(section_idx, _)| *section_idx) {
        let mut symbols = group
            .map(|&(_, symbol)| SectionDisplaySymbol { symbol, is_mapping_symbol })
            .collect::<Vec<_>>();
        if let Some(section_idx) = section_idx {
            let section = &obj.sections[section_idx];
            if section.kind == SectionKind::Unknown || section.flags.contains(SectionFlag::Hidden) {
                // Skip unknown and hidden sections
                continue;
            }
            let section_diff = &diff.sections[section_idx];
            if section.kind == SectionKind::Code && reverse_fn_order {
                symbols.sort_by(|a, b| {
                    let a_symbol = &obj.symbols[a.symbol];
                    let b_symbol = &obj.symbols[b.symbol];
                    symbol_sort_reverse(a_symbol, b_symbol)
                });
            } else {
                symbols.sort_by(|a, b| {
                    let a_symbol = &obj.symbols[a.symbol];
                    let b_symbol = &obj.symbols[b.symbol];
                    symbol_sort(a_symbol, b_symbol)
                });
            }
            sections.push(SectionDisplay {
                id: section.id.clone(),
                name: if section.flags.contains(SectionFlag::Combined) {
                    format!("{} [combined]", section.name)
                } else {
                    section.name.clone()
                },
                size: section.size,
                match_percent: section_diff.match_percent,
                symbols,
            });
        } else {
            // Don't sort, preserve order of absolute symbols
            sections.push(SectionDisplay {
                id: ".comm".to_string(),
                name: ".comm".to_string(),
                size: 0,
                match_percent: None,
                symbols,
            });
        }
    }
    sections.sort_by(|a, b| a.id.cmp(&b.id));
    sections
}

fn section_symbol_sort(a: &Symbol, b: &Symbol) -> Ordering {
    if a.kind == SymbolKind::Section {
        if b.kind != SymbolKind::Section {
            return Ordering::Less;
        }
    } else if b.kind == SymbolKind::Section {
        return Ordering::Greater;
    }
    Ordering::Equal
}

fn symbol_sort(a: &Symbol, b: &Symbol) -> Ordering {
    section_symbol_sort(a, b).then(a.address.cmp(&b.address)).then(a.size.cmp(&b.size))
}

fn symbol_sort_reverse(a: &Symbol, b: &Symbol) -> Ordering {
    section_symbol_sort(a, b).then(b.address.cmp(&a.address)).then(b.size.cmp(&a.size))
}

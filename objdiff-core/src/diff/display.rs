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
    diff::{DiffObjConfig, InstructionDiffKind, InstructionDiffRow, ObjectDiff, SymbolDiff},
    obj::{
        InstructionArg, InstructionArgValue, Object, SectionFlag, SectionKind, Symbol, SymbolFlag,
        SymbolKind,
    },
};

#[derive(Debug, Clone)]
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
    Argument(InstructionArgValue<'a>),
    /// Branch destination
    BranchDest(u64),
    /// Symbol name
    Symbol(&'a Symbol),
    /// Relocation addend
    Addend(i64),
    /// Number of spaces
    Spacing(u8),
    /// End of line
    Eol,
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, Hash)]
pub enum DiffTextColor {
    #[default]
    Normal, // Grey
    Dim,     // Dark grey
    Bright,  // White
    Replace, // Blue
    Delete,  // Red
    Insert,  // Green
    Rotating(u8),
}

#[derive(Debug, Clone)]
pub struct DiffTextSegment<'a> {
    pub text: DiffText<'a>,
    pub color: DiffTextColor,
    pub pad_to: u8,
}

impl<'a> DiffTextSegment<'a> {
    #[inline(always)]
    pub fn basic(text: &'a str, color: DiffTextColor) -> Self {
        Self { text: DiffText::Basic(text), color, pad_to: 0 }
    }

    #[inline(always)]
    pub fn spacing(spaces: u8) -> Self {
        Self { text: DiffText::Spacing(spaces), color: DiffTextColor::Normal, pad_to: 0 }
    }
}

const EOL_SEGMENT: DiffTextSegment<'static> =
    DiffTextSegment { text: DiffText::Eol, color: DiffTextColor::Normal, pad_to: 0 };

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum HighlightKind {
    #[default]
    None,
    Opcode(u16),
    Argument(InstructionArgValue<'static>),
    Symbol(String),
    Address(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstructionPart<'a> {
    Basic(Cow<'a, str>),
    Opcode(Cow<'a, str>, u16),
    Arg(InstructionArg<'a>),
    Separator,
}

impl<'a> InstructionPart<'a> {
    #[inline(always)]
    pub fn basic<T>(s: T) -> Self
    where T: Into<Cow<'a, str>> {
        InstructionPart::Basic(s.into())
    }

    #[inline(always)]
    pub fn opcode<T>(s: T, o: u16) -> Self
    where T: Into<Cow<'a, str>> {
        InstructionPart::Opcode(s.into(), o)
    }

    #[inline(always)]
    pub fn opaque<T>(s: T) -> Self
    where T: Into<Cow<'a, str>> {
        InstructionPart::Arg(InstructionArg::Value(InstructionArgValue::Opaque(s.into())))
    }

    #[inline(always)]
    pub fn signed<T>(v: T) -> InstructionPart<'static>
    where T: Into<i64> {
        InstructionPart::Arg(InstructionArg::Value(InstructionArgValue::Signed(v.into())))
    }

    #[inline(always)]
    pub fn unsigned<T>(v: T) -> InstructionPart<'static>
    where T: Into<u64> {
        InstructionPart::Arg(InstructionArg::Value(InstructionArgValue::Unsigned(v.into())))
    }

    #[inline(always)]
    pub fn branch_dest<T>(v: T) -> InstructionPart<'static>
    where T: Into<u64> {
        InstructionPart::Arg(InstructionArg::BranchDest(v.into()))
    }

    #[inline(always)]
    pub fn reloc() -> InstructionPart<'static> { InstructionPart::Arg(InstructionArg::Reloc) }

    #[inline(always)]
    pub fn separator() -> InstructionPart<'static> { InstructionPart::Separator }

    pub fn into_static(self) -> InstructionPart<'static> {
        match self {
            InstructionPart::Basic(s) => InstructionPart::Basic(Cow::Owned(s.into_owned())),
            InstructionPart::Opcode(s, o) => InstructionPart::Opcode(Cow::Owned(s.into_owned()), o),
            InstructionPart::Arg(a) => InstructionPart::Arg(a.into_static()),
            InstructionPart::Separator => InstructionPart::Separator,
        }
    }
}

pub fn display_row(
    obj: &Object,
    symbol_index: usize,
    ins_row: &InstructionDiffRow,
    diff_config: &DiffObjConfig,
    mut cb: impl FnMut(DiffTextSegment) -> Result<()>,
) -> Result<()> {
    let Some(ins_ref) = ins_row.ins_ref else {
        cb(EOL_SEGMENT)?;
        return Ok(());
    };
    let symbol = &obj.symbols[symbol_index];
    let Some(section_index) = symbol.section else {
        cb(DiffTextSegment::basic("<invalid>", DiffTextColor::Delete))?;
        cb(EOL_SEGMENT)?;
        return Ok(());
    };
    let section = &obj.sections[section_index];
    let Some(data) = section.data_range(ins_ref.address, ins_ref.size as usize) else {
        cb(DiffTextSegment::basic("<invalid>", DiffTextColor::Delete))?;
        cb(EOL_SEGMENT)?;
        return Ok(());
    };
    if let Some(line) = section.line_info.range(..=ins_ref.address).last().map(|(_, &b)| b) {
        cb(DiffTextSegment { text: DiffText::Line(line), color: DiffTextColor::Dim, pad_to: 5 })?;
    }
    cb(DiffTextSegment {
        text: DiffText::Address(ins_ref.address.saturating_sub(symbol.address)),
        color: DiffTextColor::Normal,
        pad_to: 5,
    })?;
    if let Some(branch) = &ins_row.branch_from {
        cb(DiffTextSegment::basic(" ~> ", DiffTextColor::Rotating(branch.branch_idx as u8)))?;
    } else {
        cb(DiffTextSegment::spacing(4))?;
    }
    let mut arg_idx = 0;
    let relocation = section.relocation_at(ins_ref, obj);
    let mut displayed_relocation = false;
    obj.arch.display_instruction(
        ins_ref,
        data,
        relocation,
        symbol.address..symbol.address + symbol.size,
        section_index,
        diff_config,
        &mut |part| match part {
            InstructionPart::Basic(text) => {
                if text.chars().all(|c| c == ' ') {
                    cb(DiffTextSegment::spacing(text.len() as u8))
                } else {
                    cb(DiffTextSegment::basic(&text, DiffTextColor::Normal))
                }
            }
            InstructionPart::Opcode(mnemonic, opcode) => cb(DiffTextSegment {
                text: DiffText::Opcode(mnemonic.as_ref(), opcode),
                color: if ins_row.kind == InstructionDiffKind::OpMismatch {
                    DiffTextColor::Replace
                } else {
                    DiffTextColor::Normal
                },
                pad_to: 10,
            }),
            InstructionPart::Arg(arg) => {
                let diff_index = ins_row.arg_diff.get(arg_idx).copied().unwrap_or_default();
                arg_idx += 1;
                match arg {
                    InstructionArg::Value(value) => cb(DiffTextSegment {
                        text: DiffText::Argument(value),
                        color: diff_index
                            .get()
                            .map_or(DiffTextColor::Normal, |i| DiffTextColor::Rotating(i as u8)),
                        pad_to: 0,
                    }),
                    InstructionArg::Reloc => {
                        displayed_relocation = true;
                        let resolved = relocation.unwrap();
                        let color = diff_index
                            .get()
                            .map_or(DiffTextColor::Bright, |i| DiffTextColor::Rotating(i as u8));
                        cb(DiffTextSegment {
                            text: DiffText::Symbol(resolved.symbol),
                            color,
                            pad_to: 0,
                        })?;
                        if resolved.relocation.addend != 0 {
                            cb(DiffTextSegment {
                                text: DiffText::Addend(resolved.relocation.addend),
                                color,
                                pad_to: 0,
                            })?;
                        }
                        Ok(())
                    }
                    InstructionArg::BranchDest(dest) => {
                        if let Some(addr) = dest.checked_sub(symbol.address) {
                            cb(DiffTextSegment {
                                text: DiffText::BranchDest(addr),
                                color: diff_index.get().map_or(DiffTextColor::Normal, |i| {
                                    DiffTextColor::Rotating(i as u8)
                                }),
                                pad_to: 0,
                            })
                        } else {
                            cb(DiffTextSegment {
                                text: DiffText::Argument(InstructionArgValue::Opaque(
                                    Cow::Borrowed("<invalid>"),
                                )),
                                color: diff_index.get().map_or(DiffTextColor::Normal, |i| {
                                    DiffTextColor::Rotating(i as u8)
                                }),
                                pad_to: 0,
                            })
                        }
                    }
                }
            }
            InstructionPart::Separator => {
                cb(DiffTextSegment::basic(diff_config.separator(), DiffTextColor::Normal))
            }
        },
    )?;
    // Fallback for relocation that wasn't displayed
    if relocation.is_some() && !displayed_relocation {
        cb(DiffTextSegment::basic(" <", DiffTextColor::Normal))?;
        let resolved = relocation.unwrap();
        let diff_index = ins_row.arg_diff.get(arg_idx).copied().unwrap_or_default();
        let color =
            diff_index.get().map_or(DiffTextColor::Bright, |i| DiffTextColor::Rotating(i as u8));
        cb(DiffTextSegment { text: DiffText::Symbol(resolved.symbol), color, pad_to: 0 })?;
        if resolved.relocation.addend != 0 {
            cb(DiffTextSegment {
                text: DiffText::Addend(resolved.relocation.addend),
                color,
                pad_to: 0,
            })?;
        }
        cb(DiffTextSegment::basic(">", DiffTextColor::Normal))?;
    }
    if let Some(branch) = &ins_row.branch_to {
        cb(DiffTextSegment::basic(" ~>", DiffTextColor::Rotating(branch.branch_idx as u8)))?;
    }
    cb(EOL_SEGMENT)?;
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

impl From<&DiffText<'_>> for HighlightKind {
    fn from(value: &DiffText<'_>) -> Self {
        match value {
            DiffText::Opcode(_, op) => HighlightKind::Opcode(*op),
            DiffText::Argument(arg) => HighlightKind::Argument(arg.to_static()),
            DiffText::Symbol(sym) => HighlightKind::Symbol(sym.name.to_string()),
            DiffText::Address(addr) | DiffText::BranchDest(addr) => HighlightKind::Address(*addr),
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
    if !show_hidden_symbols && (symbol.size == 0 || symbol.flags.contains(SymbolFlag::Hidden)) {
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

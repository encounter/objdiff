use std::collections::HashSet;

use anyhow::Result;

use crate::{
    config::SymbolMappings,
    diff::{
        code::{diff_code, no_diff_code, process_code_symbol},
        data::{
            diff_bss_section, diff_bss_symbol, diff_data_section, diff_data_symbol,
            diff_generic_section, no_diff_symbol,
        },
    },
    obj::{ObjInfo, ObjIns, ObjSection, ObjSectionKind, ObjSymbol, SymbolRef},
};

pub mod code;
pub mod data;
pub mod display;

#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    serde::Deserialize,
    serde::Serialize,
    strum::VariantArray,
    strum::EnumMessage,
    tsify_next::Tsify,
)]
pub enum X86Formatter {
    #[default]
    #[strum(message = "Intel (default)")]
    Intel,
    #[strum(message = "AT&T")]
    Gas,
    #[strum(message = "NASM")]
    Nasm,
    #[strum(message = "MASM")]
    Masm,
}

#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    serde::Deserialize,
    serde::Serialize,
    strum::VariantArray,
    strum::EnumMessage,
    tsify_next::Tsify,
)]
pub enum MipsAbi {
    #[default]
    #[strum(message = "Auto (default)")]
    Auto,
    #[strum(message = "O32")]
    O32,
    #[strum(message = "N32")]
    N32,
    #[strum(message = "N64")]
    N64,
}

#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    serde::Deserialize,
    serde::Serialize,
    strum::VariantArray,
    strum::EnumMessage,
    tsify_next::Tsify,
)]
pub enum MipsInstrCategory {
    #[default]
    #[strum(message = "Auto (default)")]
    Auto,
    #[strum(message = "CPU")]
    Cpu,
    #[strum(message = "RSP (N64)")]
    Rsp,
    #[strum(message = "R3000 GTE (PS1)")]
    R3000Gte,
    #[strum(message = "R4000 ALLEGREX (PSP)")]
    R4000Allegrex,
    #[strum(message = "R5900 EE (PS2)")]
    R5900,
}

#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    serde::Deserialize,
    serde::Serialize,
    strum::VariantArray,
    strum::EnumMessage,
    tsify_next::Tsify,
)]
pub enum ArmArchVersion {
    #[default]
    #[strum(message = "Auto (default)")]
    Auto,
    #[strum(message = "ARMv4T (GBA)")]
    V4T,
    #[strum(message = "ARMv5TE (DS)")]
    V5TE,
    #[strum(message = "ARMv6K (3DS)")]
    V6K,
}

#[derive(
    Debug,
    Copy,
    Clone,
    Default,
    Eq,
    PartialEq,
    serde::Deserialize,
    serde::Serialize,
    strum::VariantArray,
    strum::EnumMessage,
    tsify_next::Tsify,
)]
pub enum ArmR9Usage {
    #[default]
    #[strum(
        message = "R9 or V6 (default)",
        detailed_message = "Use R9 as a general-purpose register."
    )]
    GeneralPurpose,
    #[strum(
        message = "SB (static base)",
        detailed_message = "Used for position-independent data (PID)."
    )]
    Sb,
    #[strum(message = "TR (TLS register)", detailed_message = "Used for thread-local storage.")]
    Tr,
}

#[inline]
const fn default_true() -> bool { true }

#[derive(Debug, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize, tsify_next::Tsify)]
#[tsify(from_wasm_abi)]
#[serde(default)]
pub struct DiffObjConfig {
    pub relax_reloc_diffs: bool,
    #[serde(default = "default_true")]
    pub space_between_args: bool,
    pub combine_data_sections: bool,
    #[serde(default)]
    pub symbol_mappings: MappingConfig,
    // x86
    pub x86_formatter: X86Formatter,
    // MIPS
    pub mips_abi: MipsAbi,
    pub mips_instr_category: MipsInstrCategory,
    // ARM
    pub arm_arch_version: ArmArchVersion,
    pub arm_unified_syntax: bool,
    pub arm_av_registers: bool,
    pub arm_r9_usage: ArmR9Usage,
    pub arm_sl_usage: bool,
    pub arm_fp_usage: bool,
    pub arm_ip_usage: bool,
}

impl Default for DiffObjConfig {
    fn default() -> Self {
        Self {
            relax_reloc_diffs: false,
            space_between_args: true,
            combine_data_sections: false,
            symbol_mappings: Default::default(),
            x86_formatter: Default::default(),
            mips_abi: Default::default(),
            mips_instr_category: Default::default(),
            arm_arch_version: Default::default(),
            arm_unified_syntax: true,
            arm_av_registers: false,
            arm_r9_usage: Default::default(),
            arm_sl_usage: false,
            arm_fp_usage: false,
            arm_ip_usage: false,
        }
    }
}

impl DiffObjConfig {
    pub fn separator(&self) -> &'static str {
        if self.space_between_args {
            ", "
        } else {
            ","
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObjSectionDiff {
    pub symbols: Vec<ObjSymbolDiff>,
    pub data_diff: Vec<ObjDataDiff>,
    pub match_percent: Option<f32>,
}

impl ObjSectionDiff {
    fn merge(&mut self, other: ObjSectionDiff) {
        // symbols ignored
        self.data_diff = other.data_diff;
        self.match_percent = other.match_percent;
    }
}

#[derive(Debug, Clone, Default)]
pub struct ObjSymbolDiff {
    /// The symbol ref this object
    pub symbol_ref: SymbolRef,
    /// The symbol ref in the _other_ object that this symbol was diffed against
    pub target_symbol: Option<SymbolRef>,
    pub instructions: Vec<ObjInsDiff>,
    pub match_percent: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct ObjInsDiff {
    pub ins: Option<ObjIns>,
    /// Diff kind
    pub kind: ObjInsDiffKind,
    /// Branches from instruction
    pub branch_from: Option<ObjInsBranchFrom>,
    /// Branches to instruction
    pub branch_to: Option<ObjInsBranchTo>,
    /// Arg diffs
    pub arg_diff: Vec<Option<ObjInsArgDiff>>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum ObjInsDiffKind {
    #[default]
    None,
    OpMismatch,
    ArgMismatch,
    Replace,
    Delete,
    Insert,
}

#[derive(Debug, Clone, Default)]
pub struct ObjDataDiff {
    pub data: Vec<u8>,
    pub kind: ObjDataDiffKind,
    pub len: usize,
    pub symbol: String,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum ObjDataDiffKind {
    #[default]
    None,
    Replace,
    Delete,
    Insert,
}

#[derive(Debug, Copy, Clone)]
pub struct ObjInsArgDiff {
    /// Incrementing index for coloring
    pub idx: usize,
}

#[derive(Debug, Clone)]
pub struct ObjInsBranchFrom {
    /// Source instruction indices
    pub ins_idx: Vec<usize>,
    /// Incrementing index for coloring
    pub branch_idx: usize,
}

#[derive(Debug, Clone)]
pub struct ObjInsBranchTo {
    /// Target instruction index
    pub ins_idx: usize,
    /// Incrementing index for coloring
    pub branch_idx: usize,
}

#[derive(Default)]
pub struct ObjDiff {
    /// A list of all section diffs in the object.
    pub sections: Vec<ObjSectionDiff>,
    /// Common BSS symbols don't live in a section, so they're stored separately.
    pub common: Vec<ObjSymbolDiff>,
    /// If `selecting_left` or `selecting_right` is set, this is the list of symbols
    /// that are being mapped to the other object.
    pub mapping_symbols: Vec<ObjSymbolDiff>,
}

impl ObjDiff {
    pub fn new_from_obj(obj: &ObjInfo) -> Self {
        let mut result = Self {
            sections: Vec::with_capacity(obj.sections.len()),
            common: Vec::with_capacity(obj.common.len()),
            mapping_symbols: vec![],
        };
        for (section_idx, section) in obj.sections.iter().enumerate() {
            let mut symbols = Vec::with_capacity(section.symbols.len());
            for (symbol_idx, _) in section.symbols.iter().enumerate() {
                symbols.push(ObjSymbolDiff {
                    symbol_ref: SymbolRef { section_idx, symbol_idx },
                    target_symbol: None,
                    instructions: vec![],
                    match_percent: None,
                });
            }
            result.sections.push(ObjSectionDiff {
                symbols,
                data_diff: vec![ObjDataDiff {
                    data: section.data.clone(),
                    kind: ObjDataDiffKind::None,
                    len: section.data.len(),
                    symbol: section.name.clone(),
                }],
                match_percent: None,
            });
        }
        for (symbol_idx, _) in obj.common.iter().enumerate() {
            result.common.push(ObjSymbolDiff {
                symbol_ref: SymbolRef { section_idx: obj.sections.len(), symbol_idx },
                target_symbol: None,
                instructions: vec![],
                match_percent: None,
            });
        }
        result
    }

    #[inline]
    pub fn section_diff(&self, section_idx: usize) -> &ObjSectionDiff {
        &self.sections[section_idx]
    }

    #[inline]
    pub fn section_diff_mut(&mut self, section_idx: usize) -> &mut ObjSectionDiff {
        &mut self.sections[section_idx]
    }

    #[inline]
    pub fn symbol_diff(&self, symbol_ref: SymbolRef) -> &ObjSymbolDiff {
        if symbol_ref.section_idx == self.sections.len() {
            &self.common[symbol_ref.symbol_idx]
        } else {
            &self.section_diff(symbol_ref.section_idx).symbols[symbol_ref.symbol_idx]
        }
    }

    #[inline]
    pub fn symbol_diff_mut(&mut self, symbol_ref: SymbolRef) -> &mut ObjSymbolDiff {
        if symbol_ref.section_idx == self.sections.len() {
            &mut self.common[symbol_ref.symbol_idx]
        } else {
            &mut self.section_diff_mut(symbol_ref.section_idx).symbols[symbol_ref.symbol_idx]
        }
    }
}

#[derive(Default)]
pub struct DiffObjsResult {
    pub left: Option<ObjDiff>,
    pub right: Option<ObjDiff>,
    pub prev: Option<ObjDiff>,
}

pub fn diff_objs(
    config: &DiffObjConfig,
    left: Option<&ObjInfo>,
    right: Option<&ObjInfo>,
    prev: Option<&ObjInfo>,
) -> Result<DiffObjsResult> {
    let symbol_matches = matching_symbols(left, right, prev, &config.symbol_mappings)?;
    let section_matches = matching_sections(left, right)?;
    let mut left = left.map(|p| (p, ObjDiff::new_from_obj(p)));
    let mut right = right.map(|p| (p, ObjDiff::new_from_obj(p)));
    let mut prev = prev.map(|p| (p, ObjDiff::new_from_obj(p)));

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
                    ObjSectionKind::Code => {
                        let left_code = process_code_symbol(left_obj, left_symbol_ref, config)?;
                        let right_code = process_code_symbol(right_obj, right_symbol_ref, config)?;
                        let (left_diff, right_diff) = diff_code(
                            left_obj,
                            right_obj,
                            &left_code,
                            &right_code,
                            left_symbol_ref,
                            right_symbol_ref,
                            config,
                        )?;
                        *left_out.symbol_diff_mut(left_symbol_ref) = left_diff;
                        *right_out.symbol_diff_mut(right_symbol_ref) = right_diff;

                        if let Some(prev_symbol_ref) = prev_symbol_ref {
                            let (prev_obj, prev_out) = prev.as_mut().unwrap();
                            let prev_code = process_code_symbol(prev_obj, prev_symbol_ref, config)?;
                            let (_, prev_diff) = diff_code(
                                left_obj,
                                right_obj,
                                &right_code,
                                &prev_code,
                                right_symbol_ref,
                                prev_symbol_ref,
                                config,
                            )?;
                            *prev_out.symbol_diff_mut(prev_symbol_ref) = prev_diff;
                        }
                    }
                    ObjSectionKind::Data => {
                        let (left_diff, right_diff) = diff_data_symbol(
                            left_obj,
                            right_obj,
                            left_symbol_ref,
                            right_symbol_ref,
                        )?;
                        *left_out.symbol_diff_mut(left_symbol_ref) = left_diff;
                        *right_out.symbol_diff_mut(right_symbol_ref) = right_diff;
                    }
                    ObjSectionKind::Bss => {
                        let (left_diff, right_diff) = diff_bss_symbol(
                            left_obj,
                            right_obj,
                            left_symbol_ref,
                            right_symbol_ref,
                        )?;
                        *left_out.symbol_diff_mut(left_symbol_ref) = left_diff;
                        *right_out.symbol_diff_mut(right_symbol_ref) = right_diff;
                    }
                }
            }
            SymbolMatch { left: Some(left_symbol_ref), right: None, prev: _, section_kind } => {
                let (left_obj, left_out) = left.as_mut().unwrap();
                match section_kind {
                    ObjSectionKind::Code => {
                        let code = process_code_symbol(left_obj, left_symbol_ref, config)?;
                        *left_out.symbol_diff_mut(left_symbol_ref) =
                            no_diff_code(&code, left_symbol_ref)?;
                    }
                    ObjSectionKind::Data | ObjSectionKind::Bss => {
                        *left_out.symbol_diff_mut(left_symbol_ref) =
                            no_diff_symbol(left_obj, left_symbol_ref);
                    }
                }
            }
            SymbolMatch { left: None, right: Some(right_symbol_ref), prev: _, section_kind } => {
                let (right_obj, right_out) = right.as_mut().unwrap();
                match section_kind {
                    ObjSectionKind::Code => {
                        let code = process_code_symbol(right_obj, right_symbol_ref, config)?;
                        *right_out.symbol_diff_mut(right_symbol_ref) =
                            no_diff_code(&code, right_symbol_ref)?;
                    }
                    ObjSectionKind::Data | ObjSectionKind::Bss => {
                        *right_out.symbol_diff_mut(right_symbol_ref) =
                            no_diff_symbol(right_obj, right_symbol_ref);
                    }
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
            let left_section = &left_obj.sections[left_section_idx];
            let right_section = &right_obj.sections[right_section_idx];
            match section_kind {
                ObjSectionKind::Code => {
                    let left_section_diff = left_out.section_diff(left_section_idx);
                    let right_section_diff = right_out.section_diff(right_section_idx);
                    let (left_diff, right_diff) = diff_generic_section(
                        left_section,
                        right_section,
                        left_section_diff,
                        right_section_diff,
                    )?;
                    left_out.section_diff_mut(left_section_idx).merge(left_diff);
                    right_out.section_diff_mut(right_section_idx).merge(right_diff);
                }
                ObjSectionKind::Data => {
                    let left_section_diff = left_out.section_diff(left_section_idx);
                    let right_section_diff = right_out.section_diff(right_section_idx);
                    let (left_diff, right_diff) = diff_data_section(
                        left_section,
                        right_section,
                        left_section_diff,
                        right_section_diff,
                    )?;
                    left_out.section_diff_mut(left_section_idx).merge(left_diff);
                    right_out.section_diff_mut(right_section_idx).merge(right_diff);
                }
                ObjSectionKind::Bss => {
                    let left_section_diff = left_out.section_diff(left_section_idx);
                    let right_section_diff = right_out.section_diff(right_section_idx);
                    let (left_diff, right_diff) = diff_bss_section(
                        left_section,
                        right_section,
                        left_section_diff,
                        right_section_diff,
                    )?;
                    left_out.section_diff_mut(left_section_idx).merge(left_diff);
                    right_out.section_diff_mut(right_section_idx).merge(right_diff);
                }
            }
        }
    }

    if let (Some((right_obj, right_out)), Some((left_obj, left_out))) =
        (right.as_mut(), left.as_mut())
    {
        if let Some(right_name) = &config.symbol_mappings.selecting_left {
            generate_mapping_symbols(right_obj, right_name, left_obj, left_out, config)?;
        }
        if let Some(left_name) = &config.symbol_mappings.selecting_right {
            generate_mapping_symbols(left_obj, left_name, right_obj, right_out, config)?;
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
    base_obj: &ObjInfo,
    base_name: &str,
    target_obj: &ObjInfo,
    target_out: &mut ObjDiff,
    config: &DiffObjConfig,
) -> Result<()> {
    let Some(base_symbol_ref) = symbol_ref_by_name(base_obj, base_name) else {
        return Ok(());
    };
    let (base_section, base_symbol) = base_obj.section_symbol(base_symbol_ref);
    let Some(base_section) = base_section else {
        return Ok(());
    };
    let base_code = match base_section.kind {
        ObjSectionKind::Code => Some(process_code_symbol(base_obj, base_symbol_ref, config)?),
        _ => None,
    };
    for (target_section_index, target_section) in
        target_obj.sections.iter().enumerate().filter(|(_, s)| s.kind == base_section.kind)
    {
        for (target_symbol_index, _target_symbol) in
            target_section.symbols.iter().enumerate().filter(|(_, s)| s.kind == base_symbol.kind)
        {
            let target_symbol_ref =
                SymbolRef { section_idx: target_section_index, symbol_idx: target_symbol_index };
            match base_section.kind {
                ObjSectionKind::Code => {
                    let target_code = process_code_symbol(target_obj, target_symbol_ref, config)?;
                    let (left_diff, _right_diff) = diff_code(
                        target_obj,
                        base_obj,
                        &target_code,
                        base_code.as_ref().unwrap(),
                        target_symbol_ref,
                        base_symbol_ref,
                        config,
                    )?;
                    target_out.mapping_symbols.push(left_diff);
                }
                ObjSectionKind::Data => {
                    let (left_diff, _right_diff) =
                        diff_data_symbol(target_obj, base_obj, target_symbol_ref, base_symbol_ref)?;
                    target_out.mapping_symbols.push(left_diff);
                }
                ObjSectionKind::Bss => {
                    let (left_diff, _right_diff) =
                        diff_bss_symbol(target_obj, base_obj, target_symbol_ref, base_symbol_ref)?;
                    target_out.mapping_symbols.push(left_diff);
                }
            }
        }
    }
    Ok(())
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct SymbolMatch {
    left: Option<SymbolRef>,
    right: Option<SymbolRef>,
    prev: Option<SymbolRef>,
    section_kind: ObjSectionKind,
}

#[derive(Copy, Clone, Eq, PartialEq)]
struct SectionMatch {
    left: Option<usize>,
    right: Option<usize>,
    section_kind: ObjSectionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, serde::Deserialize, serde::Serialize)]
pub struct MappingConfig {
    /// Manual symbol mappings
    pub mappings: SymbolMappings,
    /// The right object symbol name that we're selecting a left symbol for
    pub selecting_left: Option<String>,
    /// The left object symbol name that we're selecting a right symbol for
    pub selecting_right: Option<String>,
}

fn symbol_ref_by_name(obj: &ObjInfo, name: &str) -> Option<SymbolRef> {
    for (section_idx, section) in obj.sections.iter().enumerate() {
        for (symbol_idx, symbol) in section.symbols.iter().enumerate() {
            if symbol.name == name {
                return Some(SymbolRef { section_idx, symbol_idx });
            }
        }
    }
    None
}

fn apply_symbol_mappings(
    left: &ObjInfo,
    right: &ObjInfo,
    mapping_config: &MappingConfig,
    left_used: &mut HashSet<SymbolRef>,
    right_used: &mut HashSet<SymbolRef>,
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
        let Some(left_symbol) = symbol_ref_by_name(left, left_name) else {
            continue;
        };
        if left_used.contains(&left_symbol) {
            continue;
        }
        let Some(right_symbol) = symbol_ref_by_name(right, right_name) else {
            continue;
        };
        if right_used.contains(&right_symbol) {
            continue;
        }
        let left_section = &left.sections[left_symbol.section_idx];
        let right_section = &right.sections[right_symbol.section_idx];
        if left_section.kind != right_section.kind {
            log::warn!(
                "Symbol section kind mismatch: {} ({:?}) vs {} ({:?})",
                left_name,
                left_section.kind,
                right_name,
                right_section.kind
            );
            continue;
        }
        matches.push(SymbolMatch {
            left: Some(left_symbol),
            right: Some(right_symbol),
            prev: None, // TODO
            section_kind: left_section.kind,
        });
        left_used.insert(left_symbol);
        right_used.insert(right_symbol);
    }
    Ok(())
}

/// Find matching symbols between each object.
fn matching_symbols(
    left: Option<&ObjInfo>,
    right: Option<&ObjInfo>,
    prev: Option<&ObjInfo>,
    mappings: &MappingConfig,
) -> Result<Vec<SymbolMatch>> {
    let mut matches = Vec::new();
    let mut left_used = HashSet::new();
    let mut right_used = HashSet::new();
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
        for (section_idx, section) in left.sections.iter().enumerate() {
            for (symbol_idx, symbol) in section.symbols.iter().enumerate() {
                let symbol_ref = SymbolRef { section_idx, symbol_idx };
                if left_used.contains(&symbol_ref) {
                    continue;
                }
                let symbol_match = SymbolMatch {
                    left: Some(symbol_ref),
                    right: find_symbol(right, symbol, section, Some(&right_used)),
                    prev: find_symbol(prev, symbol, section, None),
                    section_kind: section.kind,
                };
                matches.push(symbol_match);
                if let Some(right) = symbol_match.right {
                    right_used.insert(right);
                }
            }
        }
        for (symbol_idx, symbol) in left.common.iter().enumerate() {
            let symbol_ref = SymbolRef { section_idx: left.sections.len(), symbol_idx };
            if left_used.contains(&symbol_ref) {
                continue;
            }
            let symbol_match = SymbolMatch {
                left: Some(symbol_ref),
                right: find_common_symbol(right, symbol),
                prev: find_common_symbol(prev, symbol),
                section_kind: ObjSectionKind::Bss,
            };
            matches.push(symbol_match);
            if let Some(right) = symbol_match.right {
                right_used.insert(right);
            }
        }
    }
    if let Some(right) = right {
        for (section_idx, section) in right.sections.iter().enumerate() {
            for (symbol_idx, symbol) in section.symbols.iter().enumerate() {
                let symbol_ref = SymbolRef { section_idx, symbol_idx };
                if right_used.contains(&symbol_ref) {
                    continue;
                }
                matches.push(SymbolMatch {
                    left: None,
                    right: Some(symbol_ref),
                    prev: find_symbol(prev, symbol, section, None),
                    section_kind: section.kind,
                });
            }
        }
        for (symbol_idx, symbol) in right.common.iter().enumerate() {
            let symbol_ref = SymbolRef { section_idx: right.sections.len(), symbol_idx };
            if right_used.contains(&symbol_ref) {
                continue;
            }
            matches.push(SymbolMatch {
                left: None,
                right: Some(symbol_ref),
                prev: find_common_symbol(prev, symbol),
                section_kind: ObjSectionKind::Bss,
            });
        }
    }
    Ok(matches)
}

fn unmatched_symbols<'section, 'used>(
    section: &'section ObjSection,
    section_idx: usize,
    used: Option<&'used HashSet<SymbolRef>>,
) -> impl Iterator<Item = (usize, &'section ObjSymbol)> + 'used
where
    'section: 'used,
{
    section.symbols.iter().enumerate().filter(move |&(symbol_idx, _)| {
        // Skip symbols that have already been matched
        !used.map(|u| u.contains(&SymbolRef { section_idx, symbol_idx })).unwrap_or(false)
    })
}

fn find_symbol(
    obj: Option<&ObjInfo>,
    in_symbol: &ObjSymbol,
    in_section: &ObjSection,
    used: Option<&HashSet<SymbolRef>>,
) -> Option<SymbolRef> {
    let obj = obj?;
    // Try to find an exact name match
    for (section_idx, section) in obj.sections.iter().enumerate() {
        if section.kind != in_section.kind {
            continue;
        }
        if let Some((symbol_idx, _)) = unmatched_symbols(section, section_idx, used)
            .find(|(_, symbol)| symbol.name == in_symbol.name)
        {
            return Some(SymbolRef { section_idx, symbol_idx });
        }
    }
    // Match compiler-generated symbols against each other (e.g. @251 -> @60)
    // If they are at the same address in the same section
    if in_symbol.name.starts_with('@')
        && matches!(in_section.kind, ObjSectionKind::Data | ObjSectionKind::Bss)
    {
        if let Some((section_idx, section)) =
            obj.sections.iter().enumerate().find(|(_, s)| s.name == in_section.name)
        {
            if let Some((symbol_idx, _)) =
                unmatched_symbols(section, section_idx, used).find(|(_, symbol)| {
                    symbol.address == in_symbol.address && symbol.name.starts_with('@')
                })
            {
                return Some(SymbolRef { section_idx, symbol_idx });
            }
        }
    }
    // Match Metrowerks symbol$1234 against symbol$2345
    if let Some((prefix, suffix)) = in_symbol.name.split_once('$') {
        if !suffix.chars().all(char::is_numeric) {
            return None;
        }
        for (section_idx, section) in obj.sections.iter().enumerate() {
            if section.kind != in_section.kind {
                continue;
            }
            if let Some((symbol_idx, _)) =
                unmatched_symbols(section, section_idx, used).find(|&(_, symbol)| {
                    if let Some((p, s)) = symbol.name.split_once('$') {
                        prefix == p && s.chars().all(char::is_numeric)
                    } else {
                        false
                    }
                })
            {
                return Some(SymbolRef { section_idx, symbol_idx });
            }
        }
    }
    None
}

fn find_common_symbol(obj: Option<&ObjInfo>, in_symbol: &ObjSymbol) -> Option<SymbolRef> {
    let obj = obj?;
    for (symbol_idx, symbol) in obj.common.iter().enumerate() {
        if symbol.name == in_symbol.name {
            return Some(SymbolRef { section_idx: obj.sections.len(), symbol_idx });
        }
    }
    None
}

/// Find matching sections between each object.
fn matching_sections(left: Option<&ObjInfo>, right: Option<&ObjInfo>) -> Result<Vec<SectionMatch>> {
    let mut matches = Vec::new();
    if let Some(left) = left {
        for (section_idx, section) in left.sections.iter().enumerate() {
            matches.push(SectionMatch {
                left: Some(section_idx),
                right: find_section(right, &section.name, section.kind),
                section_kind: section.kind,
            });
        }
    }
    if let Some(right) = right {
        for (section_idx, section) in right.sections.iter().enumerate() {
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

fn find_section(obj: Option<&ObjInfo>, name: &str, section_kind: ObjSectionKind) -> Option<usize> {
    for (section_idx, section) in obj?.sections.iter().enumerate() {
        if section.kind != section_kind {
            continue;
        }
        if section.name == name {
            return Some(section_idx);
        }
    }
    None
}

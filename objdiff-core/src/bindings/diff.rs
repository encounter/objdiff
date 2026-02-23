#![allow(clippy::needless_lifetimes)] // Generated serde code

use alloc::{
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::fmt::Write;

use anyhow::Result;

use crate::{
    diff::{self, DiffObjConfig, display::InstructionPart},
    obj::{self, Object, SymbolFlag},
};

// Protobuf diff types
include!(concat!(env!("OUT_DIR"), "/objdiff.diff.rs"));
#[cfg(feature = "serde")]
include!(concat!(env!("OUT_DIR"), "/objdiff.diff.serde.rs"));

impl DiffResult {
    pub fn new(
        left: Option<(&Object, &diff::ObjectDiff)>,
        right: Option<(&Object, &diff::ObjectDiff)>,
        diff_config: &DiffObjConfig,
    ) -> Result<Self> {
        Ok(Self {
            left: left.map(|(obj, diff)| DiffObject::new(obj, diff, diff_config)).transpose()?,
            right: right.map(|(obj, diff)| DiffObject::new(obj, diff, diff_config)).transpose()?,
        })
    }
}

impl DiffObject {
    pub fn new(obj: &Object, diff: &diff::ObjectDiff, diff_config: &DiffObjConfig) -> Result<Self> {
        let mut sections = Vec::with_capacity(obj.sections.len());
        for (section_idx, section) in obj.sections.iter().enumerate() {
            let section_diff = &diff.sections[section_idx];
            sections.push(DiffSection::new(obj, section, section_diff));
        }

        let mut symbols = Vec::with_capacity(obj.symbols.len());
        for (symbol_idx, symbol) in obj.symbols.iter().enumerate() {
            let symbol_diff = &diff.symbols[symbol_idx];
            if symbol.size == 0 || symbol.flags.contains(SymbolFlag::Ignored) {
                continue;
            }
            symbols.push(DiffSymbol::new(obj, symbol_idx, symbol, symbol_diff, diff_config)?);
        }

        Ok(Self { sections, symbols })
    }
}

impl DiffSection {
    pub fn new(obj: &Object, section: &obj::Section, section_diff: &diff::SectionDiff) -> Self {
        Self {
            name: section.name.clone(),
            kind: DiffSectionKind::from(section.kind) as i32,
            size: section.size,
            address: section.address,
            match_percent: section_diff.match_percent,
            data_diff: section_diff.data_diff.iter().map(DiffDataSegment::from).collect(),
            reloc_diff: section_diff
                .reloc_diff
                .iter()
                .map(|r| DiffDataRelocation::new(obj, r))
                .collect(),
        }
    }
}

impl From<obj::SectionKind> for DiffSectionKind {
    fn from(value: obj::SectionKind) -> Self {
        match value {
            obj::SectionKind::Unknown => DiffSectionKind::SectionUnknown,
            obj::SectionKind::Code => DiffSectionKind::SectionCode,
            obj::SectionKind::Data => DiffSectionKind::SectionData,
            obj::SectionKind::Bss => DiffSectionKind::SectionBss,
            obj::SectionKind::Common => DiffSectionKind::SectionCommon,
        }
    }
}

impl DiffSymbol {
    pub fn new(
        obj: &Object,
        symbol_idx: usize,
        symbol: &obj::Symbol,
        symbol_diff: &diff::SymbolDiff,
        diff_config: &DiffObjConfig,
    ) -> Result<Self> {
        // Convert instruction rows
        let instructions = symbol_diff
            .instruction_rows
            .iter()
            .map(|row| DiffInstructionRow::new(obj, symbol_idx, row, diff_config))
            .collect::<Result<Vec<_>>>()?;

        // Convert data diff - flatten DataDiffRow segments into a single list
        let data_diff: Vec<DiffDataSegment> = symbol_diff
            .data_rows
            .iter()
            .flat_map(|row| row.segments.iter().map(DiffDataSegment::from))
            .collect();

        Ok(Self {
            // Symbol metadata
            name: symbol.name.clone(),
            demangled_name: symbol.demangled_name.clone(),
            address: symbol.address,
            size: symbol.size,
            flags: symbol_flags(&symbol.flags),
            kind: DiffSymbolKind::from(symbol.kind) as i32,
            // Diff information
            target_symbol: symbol_diff.target_symbol.map(|i| i as u32),
            match_percent: symbol_diff.match_percent,
            instructions,
            data_diff,
        })
    }
}

impl From<obj::SymbolKind> for DiffSymbolKind {
    fn from(value: obj::SymbolKind) -> Self {
        match value {
            obj::SymbolKind::Unknown => DiffSymbolKind::SymbolUnknown,
            obj::SymbolKind::Function => DiffSymbolKind::SymbolFunction,
            obj::SymbolKind::Object => DiffSymbolKind::SymbolObject,
            obj::SymbolKind::Section => DiffSymbolKind::SymbolSection,
        }
    }
}

fn symbol_flags(flags: &obj::SymbolFlagSet) -> u32 {
    let mut result = 0u32;
    if flags.contains(SymbolFlag::Global) {
        result |= DiffSymbolFlag::SymbolGlobal as u32;
    }
    if flags.contains(SymbolFlag::Local) {
        result |= DiffSymbolFlag::SymbolLocal as u32;
    }
    if flags.contains(SymbolFlag::Weak) {
        result |= DiffSymbolFlag::SymbolWeak as u32;
    }
    if flags.contains(SymbolFlag::Common) {
        result |= DiffSymbolFlag::SymbolCommon as u32;
    }
    if flags.contains(SymbolFlag::Hidden) {
        result |= DiffSymbolFlag::SymbolHidden as u32;
    }
    result
}

impl DiffInstructionRow {
    pub fn new(
        obj: &Object,
        symbol_idx: usize,
        row: &diff::InstructionDiffRow,
        diff_config: &DiffObjConfig,
    ) -> Result<Self> {
        let instruction = if let Some(ins_ref) = row.ins_ref {
            let resolved = obj.resolve_instruction_ref(symbol_idx, ins_ref);
            resolved.map(|r| DiffInstruction::new(obj, r, diff_config)).transpose()?
        } else {
            None
        };

        let arg_diff =
            row.arg_diff.iter().map(|d| DiffInstructionArgDiff { diff_index: d.get() }).collect();

        Ok(Self { diff_kind: DiffKind::from(row.kind) as i32, instruction, arg_diff })
    }
}

impl DiffInstruction {
    pub fn new(
        obj: &Object,
        resolved: obj::ResolvedInstructionRef,
        diff_config: &DiffObjConfig,
    ) -> Result<Self> {
        let mut formatted = String::new();
        let mut parts = vec![];
        let separator = diff_config.separator();

        // Use the arch's display_instruction to get formatted parts
        obj.arch.display_instruction(resolved, diff_config, &mut |part| {
            write_instruction_part(&mut formatted, &part, separator, resolved.relocation);
            parts
                .push(DiffInstructionPart { part: Some(diff_instruction_part::Part::from(&part)) });
            Ok(())
        })?;

        let relocation = resolved.relocation.map(|r| DiffRelocation::new(obj, r));

        let line_number = resolved
            .section
            .line_info
            .range(..=resolved.ins_ref.address)
            .last()
            .map(|(_, &line)| line);

        Ok(Self {
            address: resolved.ins_ref.address,
            size: resolved.ins_ref.size as u32,
            formatted,
            parts,
            relocation,
            branch_dest: resolved.ins_ref.branch_dest,
            line_number,
        })
    }
}

fn write_instruction_part(
    out: &mut String,
    part: &InstructionPart,
    separator: &str,
    reloc: Option<obj::ResolvedRelocation>,
) {
    match part {
        InstructionPart::Basic(s) => out.push_str(s),
        InstructionPart::Opcode(s, _) => {
            out.push_str(s);
            out.push(' ');
        }
        InstructionPart::Arg(arg) => match arg {
            obj::InstructionArg::Value(v) => {
                let _ = write!(out, "{}", v);
            }
            obj::InstructionArg::Reloc => {
                if let Some(resolved) = reloc {
                    out.push_str(&resolved.symbol.name);
                    if resolved.relocation.addend != 0 {
                        if resolved.relocation.addend < 0 {
                            let _ = write!(out, "-{:#x}", -resolved.relocation.addend);
                        } else {
                            let _ = write!(out, "+{:#x}", resolved.relocation.addend);
                        }
                    }
                }
            }
            obj::InstructionArg::BranchDest(dest) => {
                let _ = write!(out, "{:#x}", dest);
            }
        },
        InstructionPart::Separator => out.push_str(separator),
    }
}

impl diff_instruction_part::Part {
    fn from(part: &InstructionPart) -> Self {
        match part {
            InstructionPart::Basic(s) => diff_instruction_part::Part::Basic(s.to_string()),
            InstructionPart::Opcode(mnemonic, opcode) => {
                diff_instruction_part::Part::Opcode(DiffOpcode {
                    mnemonic: mnemonic.to_string(),
                    opcode: *opcode as u32,
                })
            }
            InstructionPart::Arg(arg) => {
                diff_instruction_part::Part::Arg(DiffInstructionArg::from(arg))
            }
            InstructionPart::Separator => diff_instruction_part::Part::Separator(true),
        }
    }
}

impl From<&obj::InstructionArg<'_>> for DiffInstructionArg {
    fn from(arg: &obj::InstructionArg) -> Self {
        let arg = match arg {
            obj::InstructionArg::Value(v) => match v {
                obj::InstructionArgValue::Signed(v) => diff_instruction_arg::Arg::Signed(*v),
                obj::InstructionArgValue::Unsigned(v) => diff_instruction_arg::Arg::Unsigned(*v),
                obj::InstructionArgValue::Opaque(v) => {
                    diff_instruction_arg::Arg::Opaque(v.to_string())
                }
            },
            obj::InstructionArg::Reloc => diff_instruction_arg::Arg::Reloc(true),
            obj::InstructionArg::BranchDest(dest) => diff_instruction_arg::Arg::BranchDest(*dest),
        };
        DiffInstructionArg { arg: Some(arg) }
    }
}

impl DiffRelocation {
    pub fn new(obj: &Object, resolved: obj::ResolvedRelocation) -> Self {
        let type_val = relocation_type(resolved.relocation.flags);
        let type_name = obj
            .arch
            .reloc_name(resolved.relocation.flags)
            .map(|s| s.to_string())
            .unwrap_or_default();
        Self {
            r#type: type_val,
            type_name,
            target_symbol: resolved.relocation.target_symbol as u32,
            addend: resolved.relocation.addend,
        }
    }
}

fn relocation_type(flags: obj::RelocationFlags) -> u32 {
    match flags {
        obj::RelocationFlags::Elf(r_type) => r_type,
        obj::RelocationFlags::Coff(typ) => typ as u32,
    }
}

impl From<diff::InstructionDiffKind> for DiffKind {
    fn from(value: diff::InstructionDiffKind) -> Self {
        match value {
            diff::InstructionDiffKind::None => DiffKind::DiffNone,
            diff::InstructionDiffKind::OpMismatch => DiffKind::DiffOpMismatch,
            diff::InstructionDiffKind::ArgMismatch => DiffKind::DiffArgMismatch,
            diff::InstructionDiffKind::Replace => DiffKind::DiffReplace,
            diff::InstructionDiffKind::Delete => DiffKind::DiffDelete,
            diff::InstructionDiffKind::Insert => DiffKind::DiffInsert,
        }
    }
}

impl From<diff::DataDiffKind> for DiffKind {
    fn from(value: diff::DataDiffKind) -> Self {
        match value {
            diff::DataDiffKind::None => DiffKind::DiffNone,
            diff::DataDiffKind::Replace => DiffKind::DiffReplace,
            diff::DataDiffKind::Delete => DiffKind::DiffDelete,
            diff::DataDiffKind::Insert => DiffKind::DiffInsert,
        }
    }
}

impl From<&diff::DataDiff> for DiffDataSegment {
    fn from(value: &diff::DataDiff) -> Self {
        Self {
            kind: DiffKind::from(value.kind) as i32,
            data: value.data.clone(),
            size: value.size as u64,
        }
    }
}

impl DiffDataRelocation {
    pub fn new(obj: &Object, value: &diff::DataRelocationDiff) -> Self {
        let type_val = relocation_type(value.reloc.flags);
        let type_name =
            obj.arch.reloc_name(value.reloc.flags).map(|s| s.to_string()).unwrap_or_default();
        Self {
            relocation: Some(DiffRelocation {
                r#type: type_val,
                type_name,
                target_symbol: value.reloc.target_symbol as u32,
                addend: value.reloc.addend,
            }),
            kind: DiffKind::from(value.kind) as i32,
            start: value.range.start,
            end: value.range.end,
        }
    }
}

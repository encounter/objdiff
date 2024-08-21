use crate::{
    diff::{
        ObjDataDiff, ObjDataDiffKind, ObjDiff, ObjInsArgDiff, ObjInsBranchFrom, ObjInsBranchTo,
        ObjInsDiff, ObjInsDiffKind, ObjSectionDiff, ObjSymbolDiff,
    },
    obj::{
        ObjInfo, ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjSectionKind, ObjSymbol,
        ObjSymbolFlagSet, ObjSymbolFlags,
    },
};

// Protobuf diff types
include!(concat!(env!("OUT_DIR"), "/objdiff.diff.rs"));
include!(concat!(env!("OUT_DIR"), "/objdiff.diff.serde.rs"));

impl DiffResult {
    pub fn new(left: Option<(&ObjInfo, &ObjDiff)>, right: Option<(&ObjInfo, &ObjDiff)>) -> Self {
        Self {
            left: left.map(|(obj, diff)| ObjectDiff::new(obj, diff)),
            right: right.map(|(obj, diff)| ObjectDiff::new(obj, diff)),
        }
    }
}

impl ObjectDiff {
    pub fn new(obj: &ObjInfo, diff: &ObjDiff) -> Self {
        Self {
            sections: diff
                .sections
                .iter()
                .enumerate()
                .map(|(i, d)| SectionDiff::new(obj, i, d))
                .collect(),
        }
    }
}

impl SectionDiff {
    pub fn new(obj: &ObjInfo, section_index: usize, section_diff: &ObjSectionDiff) -> Self {
        let section = &obj.sections[section_index];
        let functions = section_diff.symbols.iter().map(|d| FunctionDiff::new(obj, d)).collect();
        let data = section_diff.data_diff.iter().map(|d| DataDiff::new(obj, d)).collect();
        Self {
            name: section.name.to_string(),
            kind: SectionKind::from(section.kind) as i32,
            size: section.size,
            address: section.address,
            functions,
            data,
            match_percent: section_diff.match_percent,
        }
    }
}

impl From<ObjSectionKind> for SectionKind {
    fn from(value: ObjSectionKind) -> Self {
        match value {
            ObjSectionKind::Code => SectionKind::SectionText,
            ObjSectionKind::Data => SectionKind::SectionData,
            ObjSectionKind::Bss => SectionKind::SectionBss,
            // TODO common
        }
    }
}

impl FunctionDiff {
    pub fn new(object: &ObjInfo, symbol_diff: &ObjSymbolDiff) -> Self {
        let (_section, symbol) = object.section_symbol(symbol_diff.symbol_ref);
        // let diff_symbol = symbol_diff.diff_symbol.map(|symbol_ref| {
        //     let (_section, symbol) = object.section_symbol(symbol_ref);
        //     Symbol::from(symbol)
        // });
        let instructions = symbol_diff.instructions.iter().map(InstructionDiff::from).collect();
        Self {
            symbol: Some(Symbol::from(symbol)),
            // diff_symbol,
            instructions,
            match_percent: symbol_diff.match_percent,
        }
    }
}

impl DataDiff {
    pub fn new(_object: &ObjInfo, data_diff: &ObjDataDiff) -> Self {
        Self {
            kind: DiffKind::from(data_diff.kind) as i32,
            data: data_diff.data.clone(),
            size: data_diff.len as u64,
        }
    }
}

impl<'a> From<&'a ObjSymbol> for Symbol {
    fn from(value: &'a ObjSymbol) -> Self {
        Self {
            name: value.name.to_string(),
            demangled_name: value.demangled_name.clone(),
            address: value.address,
            size: value.size,
            flags: symbol_flags(value.flags),
        }
    }
}

fn symbol_flags(value: ObjSymbolFlagSet) -> u32 {
    let mut flags = 0u32;
    if value.0.contains(ObjSymbolFlags::Global) {
        flags |= SymbolFlag::SymbolNone as u32;
    }
    if value.0.contains(ObjSymbolFlags::Local) {
        flags |= SymbolFlag::SymbolLocal as u32;
    }
    if value.0.contains(ObjSymbolFlags::Weak) {
        flags |= SymbolFlag::SymbolWeak as u32;
    }
    if value.0.contains(ObjSymbolFlags::Common) {
        flags |= SymbolFlag::SymbolCommon as u32;
    }
    if value.0.contains(ObjSymbolFlags::Hidden) {
        flags |= SymbolFlag::SymbolHidden as u32;
    }
    flags
}

impl<'a> From<&'a ObjIns> for Instruction {
    fn from(value: &'a ObjIns) -> Self {
        Self {
            address: value.address,
            size: value.size as u32,
            opcode: value.op as u32,
            mnemonic: value.mnemonic.clone(),
            formatted: value.formatted.clone(),
            arguments: value.args.iter().map(Argument::from).collect(),
            relocation: value.reloc.as_ref().map(Relocation::from),
            branch_dest: value.branch_dest,
            line_number: value.line,
            original: value.orig.clone(),
        }
    }
}

impl<'a> From<&'a ObjInsArg> for Argument {
    fn from(value: &'a ObjInsArg) -> Self {
        Self {
            value: Some(match value {
                ObjInsArg::PlainText(s) => argument::Value::PlainText(s.to_string()),
                ObjInsArg::Arg(v) => argument::Value::Argument(ArgumentValue::from(v)),
                ObjInsArg::Reloc => argument::Value::Relocation(ArgumentRelocation {}),
                ObjInsArg::BranchDest(dest) => argument::Value::BranchDest(*dest),
            }),
        }
    }
}

impl From<&ObjInsArgValue> for ArgumentValue {
    fn from(value: &ObjInsArgValue) -> Self {
        Self {
            value: Some(match value {
                ObjInsArgValue::Signed(v) => argument_value::Value::Signed(*v),
                ObjInsArgValue::Unsigned(v) => argument_value::Value::Unsigned(*v),
                ObjInsArgValue::Opaque(v) => argument_value::Value::Opaque(v.to_string()),
            }),
        }
    }
}

impl<'a> From<&'a ObjReloc> for Relocation {
    fn from(value: &ObjReloc) -> Self {
        Self {
            r#type: match value.flags {
                object::RelocationFlags::Elf { r_type } => r_type,
                object::RelocationFlags::MachO { r_type, .. } => r_type as u32,
                object::RelocationFlags::Coff { typ } => typ as u32,
                object::RelocationFlags::Xcoff { r_rtype, .. } => r_rtype as u32,
                _ => unreachable!(),
            },
            type_name: String::new(), // TODO
            target: Some(RelocationTarget::from(&value.target)),
        }
    }
}

impl<'a> From<&'a ObjSymbol> for RelocationTarget {
    fn from(value: &'a ObjSymbol) -> Self {
        Self { symbol: Some(Symbol::from(value)), addend: value.addend }
    }
}

impl<'a> From<&'a ObjInsDiff> for InstructionDiff {
    fn from(value: &'a ObjInsDiff) -> Self {
        Self {
            instruction: value.ins.as_ref().map(Instruction::from),
            diff_kind: DiffKind::from(value.kind) as i32,
            branch_from: value.branch_from.as_ref().map(InstructionBranchFrom::from),
            branch_to: value.branch_to.as_ref().map(InstructionBranchTo::from),
            arg_diff: value.arg_diff.iter().map(ArgumentDiff::from).collect(),
        }
    }
}

impl From<&Option<ObjInsArgDiff>> for ArgumentDiff {
    fn from(value: &Option<ObjInsArgDiff>) -> Self {
        Self { diff_index: value.as_ref().map(|v| v.idx as u32) }
    }
}

impl From<ObjInsDiffKind> for DiffKind {
    fn from(value: ObjInsDiffKind) -> Self {
        match value {
            ObjInsDiffKind::None => DiffKind::DiffNone,
            ObjInsDiffKind::OpMismatch => DiffKind::DiffOpMismatch,
            ObjInsDiffKind::ArgMismatch => DiffKind::DiffArgMismatch,
            ObjInsDiffKind::Replace => DiffKind::DiffReplace,
            ObjInsDiffKind::Delete => DiffKind::DiffDelete,
            ObjInsDiffKind::Insert => DiffKind::DiffInsert,
        }
    }
}

impl From<ObjDataDiffKind> for DiffKind {
    fn from(value: ObjDataDiffKind) -> Self {
        match value {
            ObjDataDiffKind::None => DiffKind::DiffNone,
            ObjDataDiffKind::Replace => DiffKind::DiffReplace,
            ObjDataDiffKind::Delete => DiffKind::DiffDelete,
            ObjDataDiffKind::Insert => DiffKind::DiffInsert,
        }
    }
}

impl<'a> From<&'a ObjInsBranchFrom> for InstructionBranchFrom {
    fn from(value: &'a ObjInsBranchFrom) -> Self {
        Self {
            instruction_index: value.ins_idx.iter().map(|&x| x as u32).collect(),
            branch_index: value.branch_idx as u32,
        }
    }
}

impl<'a> From<&'a ObjInsBranchTo> for InstructionBranchTo {
    fn from(value: &'a ObjInsBranchTo) -> Self {
        Self { instruction_index: value.ins_idx as u32, branch_index: value.branch_idx as u32 }
    }
}

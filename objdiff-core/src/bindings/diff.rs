#![allow(clippy::needless_lifetimes)] // Generated serde code

use crate::{diff, obj};

// Protobuf diff types
include!(concat!(env!("OUT_DIR"), "/objdiff.diff.rs"));
#[cfg(feature = "serde")]
include!(concat!(env!("OUT_DIR"), "/objdiff.diff.serde.rs"));

impl DiffResult {
    pub fn new(
        _left: Option<(&obj::Object, &diff::ObjectDiff)>,
        _right: Option<(&obj::Object, &diff::ObjectDiff)>,
    ) -> Self {
        Self {
            // TODO
            // left: left.map(|(obj, diff)| ObjectDiff::new(obj, diff)),
            // right: right.map(|(obj, diff)| ObjectDiff::new(obj, diff)),
            left: None,
            right: None,
        }
    }
}

// impl ObjectDiff {
//     pub fn new(obj: &obj::Object, diff: &diff::ObjectDiff) -> Self {
//         Self {
//             sections: diff
//                 .sections
//                 .iter()
//                 .enumerate()
//                 .map(|(i, d)| SectionDiff::new(obj, i, d))
//                 .collect(),
//         }
//     }
// }
//
// impl SectionDiff {
//     pub fn new(obj: &obj::Object, section_index: usize, section_diff: &diff::SectionDiff) -> Self {
//         let section = &obj.sections[section_index];
//         let symbols = section_diff.symbols.iter().map(|d| SymbolDiff::new(obj, d)).collect();
//         let data = section_diff.data_diff.iter().map(|d| DataDiff::new(obj, d)).collect();
//         // TODO: section_diff.reloc_diff
//         Self {
//             name: section.name.to_string(),
//             kind: SectionKind::from(section.kind) as i32,
//             size: section.size,
//             address: section.address,
//             symbols,
//             data,
//             match_percent: section_diff.match_percent,
//         }
//     }
// }
//
// impl From<obj::SectionKind> for SectionKind {
//     fn from(value: obj::SectionKind) -> Self {
//         match value {
//             obj::SectionKind::Code => SectionKind::SectionText,
//             obj::SectionKind::Data => SectionKind::SectionData,
//             obj::SectionKind::Bss => SectionKind::SectionBss,
//             // TODO common
//         }
//     }
// }
//
// impl SymbolDiff {
//     pub fn new(object: &obj::Object, symbol_diff: &diff::SymbolDiff) -> Self {
//         let symbol = object.symbols[symbol_diff.symbol_index];
//         let instructions = symbol_diff
//             .instruction_rows
//             .iter()
//             .map(|ins_diff| InstructionDiff::new(object, ins_diff))
//             .collect();
//         Self {
//             symbol: Some(Symbol::new(symbol)),
//             instructions,
//             match_percent: symbol_diff.match_percent,
//             target: symbol_diff.target_symbol.map(SymbolRef::from),
//         }
//     }
// }
//
// impl DataDiff {
//     pub fn new(_object: &obj::Object, data_diff: &diff::DataDiff) -> Self {
//         Self {
//             kind: DiffKind::from(data_diff.kind) as i32,
//             data: data_diff.data.clone(),
//             size: data_diff.len as u64,
//         }
//     }
// }
//
// impl Symbol {
//     pub fn new(value: &ObjSymbol) -> Self {
//         Self {
//             name: value.name.to_string(),
//             demangled_name: value.demangled_name.clone(),
//             address: value.address,
//             size: value.size,
//             flags: symbol_flags(value.flags),
//         }
//     }
// }
//
// fn symbol_flags(value: ObjSymbolFlagSet) -> u32 {
//     let mut flags = 0u32;
//     if value.0.contains(ObjSymbolFlags::Global) {
//         flags |= SymbolFlag::SymbolGlobal as u32;
//     }
//     if value.0.contains(ObjSymbolFlags::Local) {
//         flags |= SymbolFlag::SymbolLocal as u32;
//     }
//     if value.0.contains(ObjSymbolFlags::Weak) {
//         flags |= SymbolFlag::SymbolWeak as u32;
//     }
//     if value.0.contains(ObjSymbolFlags::Common) {
//         flags |= SymbolFlag::SymbolCommon as u32;
//     }
//     if value.0.contains(ObjSymbolFlags::Hidden) {
//         flags |= SymbolFlag::SymbolHidden as u32;
//     }
//     flags
// }
//
// impl Instruction {
//     pub fn new(object: &obj::Object, instruction: &ObjIns) -> Self {
//         Self {
//             address: instruction.address,
//             size: instruction.size as u32,
//             opcode: instruction.op as u32,
//             mnemonic: instruction.mnemonic.to_string(),
//             formatted: instruction.formatted.clone(),
//             arguments: instruction.args.iter().map(Argument::new).collect(),
//             relocation: instruction.reloc.as_ref().map(|reloc| Relocation::new(object, reloc)),
//             branch_dest: instruction.branch_dest,
//             line_number: instruction.line,
//             original: instruction.orig.clone(),
//         }
//     }
// }
//
// impl Argument {
//     pub fn new(value: &ObjInsArg) -> Self {
//         Self {
//             value: Some(match value {
//                 ObjInsArg::PlainText(s) => argument::Value::PlainText(s.to_string()),
//                 ObjInsArg::Arg(v) => argument::Value::Argument(ArgumentValue::new(v)),
//                 ObjInsArg::Reloc => argument::Value::Relocation(ArgumentRelocation {}),
//                 ObjInsArg::BranchDest(dest) => argument::Value::BranchDest(*dest),
//             }),
//         }
//     }
// }
//
// impl ArgumentValue {
//     pub fn new(value: &ObjInsArgValue) -> Self {
//         Self {
//             value: Some(match value {
//                 ObjInsArgValue::Signed(v) => argument_value::Value::Signed(*v),
//                 ObjInsArgValue::Unsigned(v) => argument_value::Value::Unsigned(*v),
//                 ObjInsArgValue::Opaque(v) => argument_value::Value::Opaque(v.to_string()),
//             }),
//         }
//     }
// }
//
// impl Relocation {
//     pub fn new(object: &obj::Object, reloc: &ObjReloc) -> Self {
//         Self {
//             r#type: match reloc.flags {
//                 object::RelocationFlags::Elf { r_type } => r_type,
//                 object::RelocationFlags::MachO { r_type, .. } => r_type as u32,
//                 object::RelocationFlags::Coff { typ } => typ as u32,
//                 object::RelocationFlags::Xcoff { r_rtype, .. } => r_rtype as u32,
//                 _ => unreachable!(),
//             },
//             type_name: object.arch.display_reloc(reloc.flags).into_owned(),
//             target: Some(RelocationTarget {
//                 symbol: Some(Symbol::new(&reloc.target)),
//                 addend: reloc.addend,
//             }),
//         }
//     }
// }
//
// impl InstructionDiff {
//     pub fn new(object: &obj::Object, instruction_diff: &ObjInsDiff) -> Self {
//         Self {
//             instruction: instruction_diff.ins.as_ref().map(|ins| Instruction::new(object, ins)),
//             diff_kind: DiffKind::from(instruction_diff.kind) as i32,
//             branch_from: instruction_diff.branch_from.as_ref().map(InstructionBranchFrom::new),
//             branch_to: instruction_diff.branch_to.as_ref().map(InstructionBranchTo::new),
//             arg_diff: instruction_diff.arg_diff.iter().map(ArgumentDiff::new).collect(),
//         }
//     }
// }
//
// impl ArgumentDiff {
//     pub fn new(value: &Option<ObjInsArgDiff>) -> Self {
//         Self { diff_index: value.as_ref().map(|v| v.idx as u32) }
//     }
// }
//
// impl From<ObjInsDiffKind> for DiffKind {
//     fn from(value: ObjInsDiffKind) -> Self {
//         match value {
//             ObjInsDiffKind::None => DiffKind::DiffNone,
//             ObjInsDiffKind::OpMismatch => DiffKind::DiffOpMismatch,
//             ObjInsDiffKind::ArgMismatch => DiffKind::DiffArgMismatch,
//             ObjInsDiffKind::Replace => DiffKind::DiffReplace,
//             ObjInsDiffKind::Delete => DiffKind::DiffDelete,
//             ObjInsDiffKind::Insert => DiffKind::DiffInsert,
//         }
//     }
// }
//
// impl From<ObjDataDiffKind> for DiffKind {
//     fn from(value: ObjDataDiffKind) -> Self {
//         match value {
//             ObjDataDiffKind::None => DiffKind::DiffNone,
//             ObjDataDiffKind::Replace => DiffKind::DiffReplace,
//             ObjDataDiffKind::Delete => DiffKind::DiffDelete,
//             ObjDataDiffKind::Insert => DiffKind::DiffInsert,
//         }
//     }
// }
//
// impl InstructionBranchFrom {
//     pub fn new(value: &ObjInsBranchFrom) -> Self {
//         Self {
//             instruction_index: value.ins_idx.iter().map(|&x| x as u32).collect(),
//             branch_index: value.branch_idx as u32,
//         }
//     }
// }
//
// impl InstructionBranchTo {
//     pub fn new(value: &ObjInsBranchTo) -> Self {
//         Self { instruction_index: value.ins_idx as u32, branch_index: value.branch_idx as u32 }
//     }
// }

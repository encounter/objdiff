pub mod read;
pub mod split_meta;

use std::{borrow::Cow, collections::BTreeMap, fmt, path::PathBuf};

use filetime::FileTime;
use flagset::{flags, FlagSet};
use object::RelocationFlags;
use split_meta::SplitMeta;

use crate::{arch::ObjArch, util::ReallySigned};

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ObjSectionKind {
    Code,
    Data,
    Bss,
}
flags! {
    pub enum ObjSymbolFlags: u8 {
        Global,
        Local,
        Weak,
        Common,
        Hidden,
    }
}
#[derive(Debug, Copy, Clone, Default)]
pub struct ObjSymbolFlagSet(pub FlagSet<ObjSymbolFlags>);

#[derive(Debug, Clone)]
pub struct ObjSection {
    pub name: String,
    pub kind: ObjSectionKind,
    pub address: u64,
    pub size: u64,
    pub data: Vec<u8>,
    pub orig_index: usize,
    pub symbols: Vec<ObjSymbol>,
    pub relocations: Vec<ObjReloc>,
    pub virtual_address: Option<u64>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ObjInsArgValue {
    Signed(i64),
    Unsigned(u64),
    Opaque(Cow<'static, str>),
}

impl ObjInsArgValue {
    pub fn loose_eq(&self, other: &ObjInsArgValue) -> bool {
        match (self, other) {
            (ObjInsArgValue::Signed(a), ObjInsArgValue::Signed(b)) => a == b,
            (ObjInsArgValue::Unsigned(a), ObjInsArgValue::Unsigned(b)) => a == b,
            (ObjInsArgValue::Signed(a), ObjInsArgValue::Unsigned(b))
            | (ObjInsArgValue::Unsigned(b), ObjInsArgValue::Signed(a)) => *a as u64 == *b,
            (ObjInsArgValue::Opaque(a), ObjInsArgValue::Opaque(b)) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for ObjInsArgValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ObjInsArgValue::Signed(v) => write!(f, "{:#x}", ReallySigned(*v)),
            ObjInsArgValue::Unsigned(v) => write!(f, "{:#x}", v),
            ObjInsArgValue::Opaque(v) => write!(f, "{}", v),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ObjInsArg {
    PlainText(Cow<'static, str>),
    Arg(ObjInsArgValue),
    Reloc,
    BranchDest(u64),
}

impl ObjInsArg {
    pub fn loose_eq(&self, other: &ObjInsArg) -> bool {
        match (self, other) {
            (ObjInsArg::Arg(a), ObjInsArg::Arg(b)) => a.loose_eq(b),
            (ObjInsArg::Reloc, ObjInsArg::Reloc) => true,
            (ObjInsArg::BranchDest(a), ObjInsArg::BranchDest(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ObjIns {
    pub address: u64,
    pub size: u8,
    pub op: u16,
    pub mnemonic: String,
    pub args: Vec<ObjInsArg>,
    pub reloc: Option<ObjReloc>,
    pub branch_dest: Option<u64>,
    /// Line number
    pub line: Option<u64>,
    /// Formatted instruction
    pub formatted: String,
    /// Original (unsimplified) instruction
    pub orig: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ObjSymbol {
    pub name: String,
    pub demangled_name: Option<String>,
    pub address: u64,
    pub section_address: u64,
    pub size: u64,
    pub size_known: bool,
    pub flags: ObjSymbolFlagSet,
    pub addend: i64,
    /// Original virtual address (from .note.split section)
    pub virtual_address: Option<u64>,
}

pub struct ObjInfo {
    pub arch: Box<dyn ObjArch>,
    pub path: PathBuf,
    pub timestamp: FileTime,
    pub sections: Vec<ObjSection>,
    /// Common BSS symbols
    pub common: Vec<ObjSymbol>,
    /// Line number info (.line or .debug_line section)
    pub line_info: Option<BTreeMap<u64, u64>>,
    /// Split object metadata (.note.split section)
    pub split_meta: Option<SplitMeta>,
}

#[derive(Debug, Clone)]
pub struct ObjReloc {
    pub flags: RelocationFlags,
    pub address: u64,
    pub target: ObjSymbol,
    pub target_section: Option<String>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolRef {
    pub section_idx: usize,
    pub symbol_idx: usize,
}

impl ObjInfo {
    pub fn section_symbol(&self, symbol_ref: SymbolRef) -> (&ObjSection, &ObjSymbol) {
        let section = &self.sections[symbol_ref.section_idx];
        let symbol = &section.symbols[symbol_ref.symbol_idx];
        (section, symbol)
    }
}

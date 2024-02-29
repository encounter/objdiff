pub mod elf;
#[cfg(feature = "mips")]
pub mod mips;
#[cfg(feature = "ppc")]
pub mod ppc;
pub mod split_meta;

use std::{collections::BTreeMap, fmt, path::PathBuf};

use filetime::FileTime;
use flagset::{flags, FlagSet};
use split_meta::SplitMeta;

use crate::util::ReallySigned;

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
    pub index: usize,
    pub symbols: Vec<ObjSymbol>,
    pub relocations: Vec<ObjReloc>,
    pub virtual_address: Option<u64>,

    // Diff
    pub data_diff: Vec<ObjDataDiff>,
    pub match_percent: f32,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ObjInsArgValue {
    Signed(i16),
    Unsigned(u16),
    Opaque(String),
}

impl ObjInsArgValue {
    pub fn loose_eq(&self, other: &ObjInsArgValue) -> bool {
        match (self, other) {
            (ObjInsArgValue::Signed(a), ObjInsArgValue::Signed(b)) => a == b,
            (ObjInsArgValue::Unsigned(a), ObjInsArgValue::Unsigned(b)) => a == b,
            (ObjInsArgValue::Signed(a), ObjInsArgValue::Unsigned(b))
            | (ObjInsArgValue::Unsigned(b), ObjInsArgValue::Signed(a)) => *a as u16 == *b,
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
    Arg(ObjInsArgValue),
    ArgWithBase(ObjInsArgValue),
    Reloc,
    RelocWithBase,
    BranchOffset(i32),
}

impl ObjInsArg {
    pub fn loose_eq(&self, other: &ObjInsArg) -> bool {
        match (self, other) {
            (ObjInsArg::Arg(a), ObjInsArg::Arg(b)) => a.loose_eq(b),
            (ObjInsArg::ArgWithBase(a), ObjInsArg::ArgWithBase(b)) => a.loose_eq(b),
            (ObjInsArg::Reloc, ObjInsArg::Reloc) => true,
            (ObjInsArg::RelocWithBase, ObjInsArg::RelocWithBase) => true,
            (ObjInsArg::BranchOffset(a), ObjInsArg::BranchOffset(b)) => a == b,
            _ => false,
        }
    }
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

#[derive(Debug, Clone)]
pub struct ObjIns {
    pub address: u32,
    pub code: u32,
    pub op: u8,
    pub mnemonic: String,
    pub args: Vec<ObjInsArg>,
    pub reloc: Option<ObjReloc>,
    pub branch_dest: Option<u32>,
    /// Line number
    pub line: Option<u64>,
    /// Original (unsimplified) instruction
    pub orig: Option<String>,
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
pub enum ObjDataDiffKind {
    #[default]
    None,
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
    /// Original virtual address (from .splitmeta section)
    pub virtual_address: Option<u64>,

    // Diff
    pub diff_symbol: Option<String>,
    pub instructions: Vec<ObjInsDiff>,
    pub match_percent: Option<f32>,
}

#[derive(Debug, Copy, Clone)]
pub enum ObjArchitecture {
    #[cfg(feature = "ppc")]
    PowerPc,
    #[cfg(feature = "mips")]
    Mips,
}

#[derive(Debug, Clone)]
pub struct ObjInfo {
    pub architecture: ObjArchitecture,
    pub path: PathBuf,
    pub timestamp: FileTime,
    pub sections: Vec<ObjSection>,
    /// Common BSS symbols
    pub common: Vec<ObjSymbol>,
    /// Line number info (.line or .debug_line section)
    pub line_info: Option<BTreeMap<u64, u64>>,
    /// Split object metadata (.splitmeta section)
    pub split_meta: Option<SplitMeta>,
}

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ObjRelocKind {
    Absolute,
    #[cfg(feature = "ppc")]
    PpcAddr16Hi,
    #[cfg(feature = "ppc")]
    PpcAddr16Ha,
    #[cfg(feature = "ppc")]
    PpcAddr16Lo,
    // #[cfg(feature = "ppc")]
    // PpcAddr32,
    // #[cfg(feature = "ppc")]
    // PpcRel32,
    // #[cfg(feature = "ppc")]
    // PpcAddr24,
    #[cfg(feature = "ppc")]
    PpcRel24,
    // #[cfg(feature = "ppc")]
    // PpcAddr14,
    #[cfg(feature = "ppc")]
    PpcRel14,
    #[cfg(feature = "ppc")]
    PpcEmbSda21,
    #[cfg(feature = "mips")]
    Mips26,
    #[cfg(feature = "mips")]
    MipsHi16,
    #[cfg(feature = "mips")]
    MipsLo16,
    #[cfg(feature = "mips")]
    MipsGot16,
    #[cfg(feature = "mips")]
    MipsCall16,
    #[cfg(feature = "mips")]
    MipsGpRel16,
    #[cfg(feature = "mips")]
    MipsGpRel32,
}

#[derive(Debug, Clone)]
pub struct ObjReloc {
    pub kind: ObjRelocKind,
    pub address: u64,
    pub target: ObjSymbol,
    pub target_section: Option<String>,
}

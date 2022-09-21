pub mod elf;
pub mod mips;
pub mod ppc;

use std::path::PathBuf;

use flagset::{flags, FlagSet};

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
    }
}
#[derive(Debug, Copy, Clone, Default)]
pub struct ObjSymbolFlagSet(pub(crate) FlagSet<ObjSymbolFlags>);
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
}
#[derive(Debug, Clone)]
pub enum ObjInsArg {
    PpcArg(ppc750cl::Argument),
    MipsArg(String),
    Reloc,
    RelocWithBase,
    BranchOffset(i32),
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

    // Diff
    pub diff_symbol: Option<String>,
    pub instructions: Vec<ObjInsDiff>,
    pub match_percent: f32,
}
#[derive(Debug, Copy, Clone)]
pub enum ObjArchitecture {
    PowerPc,
    Mips,
}
#[derive(Debug, Clone)]
pub struct ObjInfo {
    pub architecture: ObjArchitecture,
    pub path: PathBuf,
    pub sections: Vec<ObjSection>,
    pub common: Vec<ObjSymbol>,
}
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum ObjRelocKind {
    Absolute,
    PpcAddr16Hi,
    PpcAddr16Ha,
    PpcAddr16Lo,
    // PpcAddr32,
    // PpcRel32,
    // PpcAddr24,
    PpcRel24,
    // PpcAddr14,
    PpcRel14,
    PpcEmbSda21,
    Mips32,
    Mips26,
    MipsHi16,
    MipsLo16,
}
#[derive(Debug, Clone)]
pub struct ObjReloc {
    pub kind: ObjRelocKind,
    pub address: u64,
    pub target: ObjSymbol,
    pub target_section: Option<String>,
}

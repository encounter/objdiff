pub mod read;
pub mod split_meta;

use alloc::{
    borrow::Cow,
    boxed::Box,
    collections::BTreeMap,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::{
    fmt,
    num::{NonZeroU32, NonZeroU64},
};

use flagset::{FlagSet, flags};

use crate::{
    arch::{Arch, ArchDummy},
    obj::split_meta::SplitMeta,
    util::ReallySigned,
};

#[derive(Debug, Eq, PartialEq, Copy, Clone, Default)]
pub enum SectionKind {
    #[default]
    Unknown = -1,
    Code,
    Data,
    Bss,
    Common,
}

flags! {
    #[derive(Hash)]
    pub enum SymbolFlag: u8 {
        Global,
        Local,
        Weak,
        Common,
        Hidden,
        /// Has extra data associated with the symbol
        /// (e.g. exception table entry)
        HasExtra,
        /// Symbol size was missing and was inferred
        SizeInferred,
        /// Symbol should be ignored by any diffing
        Ignored,
    }
}

pub type SymbolFlagSet = FlagSet<SymbolFlag>;

flags! {
    #[derive(Hash)]
    pub enum SectionFlag: u8 {
        /// Section combined from multiple input sections
        Combined,
    }
}

pub type SectionFlagSet = FlagSet<SectionFlag>;

#[derive(Debug, Clone, Default)]
pub struct Section {
    /// Unique section ID
    pub id: String,
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub kind: SectionKind,
    pub data: SectionData,
    pub flags: SectionFlagSet,
    pub align: Option<NonZeroU64>,
    pub relocations: Vec<Relocation>,
    /// Line number info (.line or .debug_line section)
    pub line_info: BTreeMap<u64, u32>,
    /// Original virtual address (from .note.split section)
    pub virtual_address: Option<u64>,
}

#[derive(Clone, Default)]
#[repr(transparent)]
pub struct SectionData(pub Vec<u8>);

impl core::ops::Deref for SectionData {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target { &self.0 }
}

impl fmt::Debug for SectionData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SectionData").field(&self.0.len()).finish()
    }
}

impl Section {
    pub fn data_range(&self, address: u64, size: usize) -> Option<&[u8]> {
        let offset = address.checked_sub(self.address)?;
        self.data.get(offset as usize..offset as usize + size)
    }

    // The alignment to use when "Combine data/text sections" is enabled.
    pub fn combined_alignment(&self) -> u64 {
        const MIN_ALIGNMENT: u64 = 4;
        self.align.map(|align| align.get().max(MIN_ALIGNMENT)).unwrap_or(MIN_ALIGNMENT)
    }

    pub fn relocation_at<'obj>(
        &'obj self,
        obj: &'obj Object,
        ins_ref: InstructionRef,
    ) -> Option<ResolvedRelocation<'obj>> {
        match self.relocations.binary_search_by_key(&ins_ref.address, |r| r.address) {
            Ok(i) => self.relocations.get(i),
            Err(i) => self
                .relocations
                .get(i)
                .filter(|r| r.address < ins_ref.address + ins_ref.size as u64),
        }
        .and_then(|relocation| {
            let symbol = obj.symbols.get(relocation.target_symbol)?;
            Some(ResolvedRelocation { relocation, symbol })
        })
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum InstructionArgValue<'a> {
    Signed(i64),
    Unsigned(u64),
    Opaque(Cow<'a, str>),
}

impl InstructionArgValue<'_> {
    pub fn loose_eq(&self, other: &InstructionArgValue) -> bool {
        match (self, other) {
            (InstructionArgValue::Signed(a), InstructionArgValue::Signed(b)) => a == b,
            (InstructionArgValue::Unsigned(a), InstructionArgValue::Unsigned(b)) => a == b,
            (InstructionArgValue::Signed(a), InstructionArgValue::Unsigned(b))
            | (InstructionArgValue::Unsigned(b), InstructionArgValue::Signed(a)) => *a as u64 == *b,
            (InstructionArgValue::Opaque(a), InstructionArgValue::Opaque(b)) => a == b,
            _ => false,
        }
    }

    pub fn to_static(&self) -> InstructionArgValue<'static> {
        match self {
            InstructionArgValue::Signed(v) => InstructionArgValue::Signed(*v),
            InstructionArgValue::Unsigned(v) => InstructionArgValue::Unsigned(*v),
            InstructionArgValue::Opaque(v) => InstructionArgValue::Opaque(v.to_string().into()),
        }
    }

    pub fn into_static(self) -> InstructionArgValue<'static> {
        match self {
            InstructionArgValue::Signed(v) => InstructionArgValue::Signed(v),
            InstructionArgValue::Unsigned(v) => InstructionArgValue::Unsigned(v),
            InstructionArgValue::Opaque(v) => {
                InstructionArgValue::Opaque(Cow::Owned(v.into_owned()))
            }
        }
    }
}

impl fmt::Display for InstructionArgValue<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstructionArgValue::Signed(v) => write!(f, "{:#x}", ReallySigned(*v)),
            InstructionArgValue::Unsigned(v) => write!(f, "{v:#x}"),
            InstructionArgValue::Opaque(v) => write!(f, "{v}"),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum InstructionArg<'a> {
    Value(InstructionArgValue<'a>),
    Reloc,
    BranchDest(u64),
}

impl InstructionArg<'_> {
    pub fn loose_eq(&self, other: &InstructionArg) -> bool {
        match (self, other) {
            (InstructionArg::Value(a), InstructionArg::Value(b)) => a.loose_eq(b),
            (InstructionArg::Reloc, InstructionArg::Reloc) => true,
            (InstructionArg::BranchDest(a), InstructionArg::BranchDest(b)) => a == b,
            _ => false,
        }
    }

    pub fn to_static(&self) -> InstructionArg<'static> {
        match self {
            InstructionArg::Value(v) => InstructionArg::Value(v.to_static()),
            InstructionArg::Reloc => InstructionArg::Reloc,
            InstructionArg::BranchDest(v) => InstructionArg::BranchDest(*v),
        }
    }

    pub fn into_static(self) -> InstructionArg<'static> {
        match self {
            InstructionArg::Value(v) => InstructionArg::Value(v.into_static()),
            InstructionArg::Reloc => InstructionArg::Reloc,
            InstructionArg::BranchDest(v) => InstructionArg::BranchDest(v),
        }
    }
}

#[derive(Copy, Clone, Debug, Default)]
pub struct InstructionRef {
    pub address: u64,
    pub size: u8,
    pub opcode: u16,
    pub branch_dest: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ParsedInstruction {
    pub ins_ref: InstructionRef,
    pub mnemonic: Cow<'static, str>,
    pub args: Vec<InstructionArg<'static>>,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, Default)]
pub enum SymbolKind {
    #[default]
    Unknown,
    Function,
    Object,
    Section,
}

#[derive(Debug)]
pub enum FlowAnalysisValue {
    Text(String),
}

pub trait FlowAnalysisResult: core::fmt::Debug + Send {
    fn get_argument_value_at_address(
        &self,
        address: u64,
        argument: u8,
    ) -> Option<&FlowAnalysisValue>;
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Default)]
pub struct Symbol {
    pub name: String,
    pub demangled_name: Option<String>,
    pub address: u64,
    pub size: u64,
    pub kind: SymbolKind,
    pub section: Option<usize>,
    pub flags: SymbolFlagSet,
    /// Alignment (from Metrowerks .comment section)
    pub align: Option<NonZeroU32>,
    /// Original virtual address (from .note.split section)
    pub virtual_address: Option<u64>,
}

#[derive(Debug)]
pub struct Object {
    pub arch: Box<dyn Arch>,
    pub endianness: object::Endianness,
    pub symbols: Vec<Symbol>,
    pub sections: Vec<Section>,
    /// Split object metadata (.note.split section)
    pub split_meta: Option<SplitMeta>,
    #[cfg(feature = "std")]
    pub path: Option<std::path::PathBuf>,
    #[cfg(feature = "std")]
    pub timestamp: Option<filetime::FileTime>,
    pub flow_analysis_results: BTreeMap<u64, Box<dyn FlowAnalysisResult>>,
}

impl Default for Object {
    fn default() -> Self {
        Self {
            arch: ArchDummy::new(),
            endianness: object::Endianness::Little,
            symbols: vec![],
            sections: vec![],
            split_meta: None,
            #[cfg(feature = "std")]
            path: None,
            #[cfg(feature = "std")]
            timestamp: None,
            flow_analysis_results: BTreeMap::<u64, Box<dyn FlowAnalysisResult>>::new(),
        }
    }
}

impl Object {
    pub fn resolve_instruction_ref(
        &self,
        symbol_index: usize,
        ins_ref: InstructionRef,
    ) -> Option<ResolvedInstructionRef<'_>> {
        let symbol = self.symbols.get(symbol_index)?;
        let section_index = symbol.section?;
        let section = self.sections.get(section_index)?;
        let offset = ins_ref.address.checked_sub(section.address)?;
        let code = section.data.get(offset as usize..offset as usize + ins_ref.size as usize)?;
        let relocation = section.relocation_at(self, ins_ref);
        Some(ResolvedInstructionRef {
            ins_ref,
            symbol_index,
            symbol,
            section,
            section_index,
            code,
            relocation,
        })
    }

    pub fn symbol_data(&self, symbol_index: usize) -> Option<&[u8]> {
        let symbol = self.symbols.get(symbol_index)?;
        let section_index = symbol.section?;
        let section = self.sections.get(section_index)?;
        let offset = symbol.address.checked_sub(section.address)?;
        section.data.get(offset as usize..offset as usize + symbol.size as usize)
    }

    pub fn symbol_by_name(&self, name: &str) -> Option<usize> {
        self.symbols.iter().position(|symbol| symbol.section.is_some() && symbol.name == name)
    }

    pub fn has_flow_analysis_result(&self) -> bool { !self.flow_analysis_results.is_empty() }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Relocation {
    pub flags: RelocationFlags,
    pub address: u64,
    pub target_symbol: usize,
    pub addend: i64,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum RelocationFlags {
    Elf(u32),
    Coff(u16),
}

#[derive(Debug, Copy, Clone)]
pub struct ResolvedRelocation<'a> {
    pub relocation: &'a Relocation,
    pub symbol: &'a Symbol,
}

#[derive(Debug, Copy, Clone)]
pub struct ResolvedSymbol<'obj> {
    pub obj: &'obj Object,
    pub symbol_index: usize,
    pub symbol: &'obj Symbol,
    pub section_index: usize,
    pub section: &'obj Section,
    pub data: &'obj [u8],
}

#[derive(Debug, Copy, Clone)]
pub struct ResolvedInstructionRef<'obj> {
    pub ins_ref: InstructionRef,
    pub symbol_index: usize,
    pub symbol: &'obj Symbol,
    pub section_index: usize,
    pub section: &'obj Section,
    pub code: &'obj [u8],
    pub relocation: Option<ResolvedRelocation<'obj>>,
}

static DUMMY_SYMBOL: Symbol = Symbol {
    name: String::new(),
    demangled_name: None,
    address: 0,
    size: 0,
    kind: SymbolKind::Unknown,
    section: None,
    flags: SymbolFlagSet::empty(),
    align: None,
    virtual_address: None,
};

static DUMMY_SECTION: Section = Section {
    id: String::new(),
    name: String::new(),
    address: 0,
    size: 0,
    kind: SectionKind::Unknown,
    data: SectionData(Vec::new()),
    flags: SectionFlagSet::empty(),
    align: None,
    relocations: Vec::new(),
    line_info: BTreeMap::new(),
    virtual_address: None,
};

impl Default for ResolvedInstructionRef<'_> {
    fn default() -> Self {
        Self {
            ins_ref: InstructionRef::default(),
            symbol_index: 0,
            symbol: &DUMMY_SYMBOL,
            section_index: 0,
            section: &DUMMY_SECTION,
            code: &[],
            relocation: None,
        }
    }
}

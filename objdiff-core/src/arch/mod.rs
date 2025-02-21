use alloc::{borrow::Cow, boxed::Box, format, string::String, vec, vec::Vec};
use core::{ffi::CStr, fmt, fmt::Debug, ops::Range};

use anyhow::{bail, Result};
use byteorder::ByteOrder;
use object::{File, Relocation, Section};

use crate::{
    diff::{display::InstructionPart, DiffObjConfig},
    obj::{
        InstructionRef, ParsedInstruction, RelocationFlags, ResolvedRelocation, ScannedInstruction,
        SymbolFlagSet, SymbolKind,
    },
    util::ReallySigned,
};

#[cfg(feature = "arm")]
mod arm;
#[cfg(feature = "arm64")]
mod arm64;
#[cfg(feature = "mips")]
pub mod mips;
#[cfg(feature = "ppc")]
pub mod ppc;
#[cfg(feature = "x86")]
pub mod x86;

/// Represents the type of data associated with an instruction
pub enum DataType {
    Int8,
    Int16,
    Int32,
    Int64,
    Int128,
    Float,
    Double,
    Bytes,
    String,
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataType::Int8 => write!(f, "Int8"),
            DataType::Int16 => write!(f, "Int16"),
            DataType::Int32 => write!(f, "Int32"),
            DataType::Int64 => write!(f, "Int64"),
            DataType::Int128 => write!(f, "Int128"),
            DataType::Float => write!(f, "Float"),
            DataType::Double => write!(f, "Double"),
            DataType::Bytes => write!(f, "Bytes"),
            DataType::String => write!(f, "String"),
        }
    }
}

impl DataType {
    pub fn display_labels<Endian: ByteOrder>(&self, bytes: &[u8]) -> Vec<String> {
        let mut strs = Vec::new();
        for literal in self.display_literals::<Endian>(bytes) {
            strs.push(format!("{}: {}", self, literal))
        }
        strs
    }

    pub fn display_literals<Endian: ByteOrder>(&self, bytes: &[u8]) -> Vec<String> {
        let mut strs = Vec::new();
        if self.required_len().is_some_and(|l| bytes.len() < l) {
            log::warn!("Failed to display a symbol value for a symbol whose size is too small for instruction referencing it.");
            return strs;
        }
        let mut bytes = bytes;
        if self.required_len().is_some_and(|l| bytes.len() > l) {
            // If the symbol's size is larger a single instance of this data type, we take just the
            // bytes necessary for one of them in order to display the first element of the array.
            bytes = &bytes[0..self.required_len().unwrap()];
            // TODO: Attempt to interpret large symbols as arrays of a smaller type and show all
            // elements of the array instead. https://github.com/encounter/objdiff/issues/124
            // However, note that the stride of an array can not always be determined just by the
            // data type guessed by the single instruction accessing it. There can also be arrays of
            // structs that contain multiple elements of different types, so if other elements after
            // the first one were to be displayed in this manner, they may be inaccurate.
        }

        match self {
            DataType::Int8 => {
                let i = i8::from_ne_bytes(bytes.try_into().unwrap());
                strs.push(format!("{:#x}", i));

                if i < 0 {
                    strs.push(format!("{:#x}", ReallySigned(i)));
                }
            }
            DataType::Int16 => {
                let i = Endian::read_i16(bytes);
                strs.push(format!("{:#x}", i));

                if i < 0 {
                    strs.push(format!("{:#x}", ReallySigned(i)));
                }
            }
            DataType::Int32 => {
                let i = Endian::read_i32(bytes);
                strs.push(format!("{:#x}", i));

                if i < 0 {
                    strs.push(format!("{:#x}", ReallySigned(i)));
                }
            }
            DataType::Int64 => {
                let i = Endian::read_i64(bytes);
                strs.push(format!("{:#x}", i));

                if i < 0 {
                    strs.push(format!("{:#x}", ReallySigned(i)));
                }
            }
            DataType::Int128 => {
                let i = Endian::read_i128(bytes);
                strs.push(format!("{:#x}", i));

                if i < 0 {
                    strs.push(format!("{:#x}", ReallySigned(i)));
                }
            }
            DataType::Float => {
                strs.push(format!("{:?}f", Endian::read_f32(bytes)));
            }
            DataType::Double => {
                strs.push(format!("{:?}", Endian::read_f64(bytes)));
            }
            DataType::Bytes => {
                strs.push(format!("{:#?}", bytes));
            }
            DataType::String => {
                if let Ok(cstr) = CStr::from_bytes_until_nul(bytes) {
                    strs.push(format!("{:?}", cstr));
                }
            }
        }

        strs
    }

    fn required_len(&self) -> Option<usize> {
        match self {
            DataType::Int8 => Some(1),
            DataType::Int16 => Some(2),
            DataType::Int32 => Some(4),
            DataType::Int64 => Some(8),
            DataType::Int128 => Some(16),
            DataType::Float => Some(4),
            DataType::Double => Some(8),
            DataType::Bytes => None,
            DataType::String => None,
        }
    }
}

pub trait Arch: Send + Sync + Debug {
    /// Generate a list of instructions references (offset, size, opcode) from the given code.
    ///
    /// The opcode IDs are used to generate the initial diff. Implementations should do as little
    /// parsing as possible here: just enough to identify the base instruction opcode, size, and
    /// possible branch destination (for visual representation). As needed, instructions are parsed
    /// via `process_instruction` to compare their arguments.
    fn scan_instructions(
        &self,
        address: u64,
        code: &[u8],
        section_index: usize,
        diff_config: &DiffObjConfig,
    ) -> Result<Vec<ScannedInstruction>>;

    /// Parse an instruction to gather its mnemonic and arguments for more detailed comparison.
    ///
    /// This is called only when we need to compare the arguments of an instruction.
    fn process_instruction(
        &self,
        ins_ref: InstructionRef,
        code: &[u8],
        relocation: Option<ResolvedRelocation>,
        function_range: Range<u64>,
        section_index: usize,
        diff_config: &DiffObjConfig,
    ) -> Result<ParsedInstruction> {
        let mut mnemonic = None;
        let mut args = Vec::with_capacity(8);
        self.display_instruction(
            ins_ref,
            code,
            relocation,
            function_range,
            section_index,
            diff_config,
            &mut |part| {
                match part {
                    InstructionPart::Opcode(m, _) => mnemonic = Some(m),
                    InstructionPart::Arg(arg) => args.push(arg),
                    _ => {}
                }
                Ok(())
            },
        )?;
        Ok(ParsedInstruction { ins_ref, mnemonic: mnemonic.unwrap_or_default(), args })
    }

    /// Format an instruction for display.
    ///
    /// Implementations should call the callback for each part of the instruction: usually the
    /// mnemonic and arguments, plus any separators and visual formatting.
    fn display_instruction(
        &self,
        ins_ref: InstructionRef,
        code: &[u8],
        relocation: Option<ResolvedRelocation>,
        function_range: Range<u64>,
        section_index: usize,
        diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()>;

    fn implcit_addend(
        &self,
        file: &object::File<'_>,
        section: &object::Section,
        address: u64,
        relocation: &object::Relocation,
        flags: RelocationFlags,
    ) -> Result<i64>;

    fn demangle(&self, _name: &str) -> Option<String> { None }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str>;

    fn get_reloc_byte_size(&self, flags: RelocationFlags) -> usize;

    fn symbol_address(&self, address: u64, _kind: SymbolKind) -> u64 { address }

    fn extra_symbol_flags(&self, _symbol: &object::Symbol) -> SymbolFlagSet {
        SymbolFlagSet::default()
    }

    fn guess_data_type(
        &self,
        _ins_ref: InstructionRef,
        _code: &[u8],
        _relocation: Option<ResolvedRelocation>,
    ) -> Option<DataType> {
        None
    }

    fn display_data_labels(&self, _ty: DataType, bytes: &[u8]) -> Vec<String> {
        vec![format!("Bytes: {:#x?}", bytes)]
    }

    fn display_data_literals(&self, _ty: DataType, bytes: &[u8]) -> Vec<String> {
        vec![format!("{:#?}", bytes)]
    }

    fn display_ins_data_labels(
        &self,
        _ins_ref: InstructionRef,
        _code: &[u8],
        _relocation: Option<ResolvedRelocation>,
    ) -> Vec<String> {
        // TODO
        // let Some(reloc) = relocation else {
        //     return Vec::new();
        // };
        // if reloc.relocation.addend >= 0 && reloc.symbol.bytes.len() > reloc.relocation.addend as usize {
        //     return self
        //         .guess_data_type(ins)
        //         .map(|ty| {
        //             self.display_data_labels(ty, &reloc.target.bytes[reloc.addend as usize..])
        //         })
        //         .unwrap_or_default();
        // }
        Vec::new()
    }

    fn display_ins_data_literals(
        &self,
        _ins_ref: InstructionRef,
        _code: &[u8],
        _relocation: Option<ResolvedRelocation>,
    ) -> Vec<String> {
        // TODO
        // let Some(reloc) = ins.reloc.as_ref() else {
        //     return Vec::new();
        // };
        // if reloc.addend >= 0 && reloc.target.bytes.len() > reloc.addend as usize {
        //     return self
        //         .guess_data_type(ins)
        //         .map(|ty| {
        //             self.display_data_literals(ty, &reloc.target.bytes[reloc.addend as usize..])
        //         })
        //         .unwrap_or_default();
        // }
        Vec::new()
    }
}

pub fn new_arch(object: &object::File) -> Result<Box<dyn Arch>> {
    use object::Object as _;
    Ok(match object.architecture() {
        #[cfg(feature = "ppc")]
        object::Architecture::PowerPc => Box::new(ppc::ArchPpc::new(object)?),
        #[cfg(feature = "mips")]
        object::Architecture::Mips => Box::new(mips::ArchMips::new(object)?),
        #[cfg(feature = "x86")]
        object::Architecture::I386 | object::Architecture::X86_64 => {
            Box::new(x86::ArchX86::new(object)?)
        }
        #[cfg(feature = "arm")]
        object::Architecture::Arm => Box::new(arm::ArchArm::new(object)?),
        #[cfg(feature = "arm64")]
        object::Architecture::Aarch64 => Box::new(arm64::ArchArm64::new(object)?),
        arch => bail!("Unsupported architecture: {arch:?}"),
    })
}

#[derive(Debug, Default)]
pub struct ArchDummy {}

impl ArchDummy {
    pub fn new() -> Box<Self> { Box::new(Self {}) }
}

impl Arch for ArchDummy {
    fn scan_instructions(
        &self,
        _address: u64,
        _code: &[u8],
        _section_index: usize,
        _diff_config: &DiffObjConfig,
    ) -> Result<Vec<ScannedInstruction>> {
        Ok(Vec::new())
    }

    fn display_instruction(
        &self,
        _ins_ref: InstructionRef,
        _code: &[u8],
        _relocation: Option<ResolvedRelocation>,
        _function_range: Range<u64>,
        _section_index: usize,
        _diff_config: &DiffObjConfig,
        _cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        Ok(())
    }

    fn implcit_addend(
        &self,
        _file: &File<'_>,
        _section: &Section,
        _address: u64,
        _relocation: &Relocation,
        _flags: RelocationFlags,
    ) -> Result<i64> {
        Ok(0)
    }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        format!("{flags:?}").into()
    }

    fn get_reloc_byte_size(&self, _flags: RelocationFlags) -> usize { 0 }
}

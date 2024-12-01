use std::{borrow::Cow, collections::BTreeMap, ffi::CStr};

use anyhow::{bail, Result};
use byteorder::ByteOrder;
use object::{Architecture, File, Object, ObjectSymbol, Relocation, RelocationFlags, Symbol};

use crate::{
    diff::DiffObjConfig,
    obj::{ObjIns, ObjReloc, ObjSection},
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

impl DataType {
    pub fn display_bytes<Endian: ByteOrder>(&self, bytes: &[u8]) -> Option<String> {
        if self.required_len().is_some_and(|l| bytes.len() < l) {
            log::warn!("Failed to display a symbol value for a symbol whose size is too small for instruction referencing it.");
            return None;
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
                if i < 0 {
                    format!("Int8: {:#x} ({:#x})", i, ReallySigned(i))
                } else {
                    format!("Int8: {:#x}", i)
                }
            }
            DataType::Int16 => {
                let i = Endian::read_i16(bytes);
                if i < 0 {
                    format!("Int16: {:#x} ({:#x})", i, ReallySigned(i))
                } else {
                    format!("Int16: {:#x}", i)
                }
            }
            DataType::Int32 => {
                let i = Endian::read_i32(bytes);
                if i < 0 {
                    format!("Int32: {:#x} ({:#x})", i, ReallySigned(i))
                } else {
                    format!("Int32: {:#x}", i)
                }
            }
            DataType::Int64 => {
                let i = Endian::read_i64(bytes);
                if i < 0 {
                    format!("Int64: {:#x} ({:#x})", i, ReallySigned(i))
                } else {
                    format!("Int64: {:#x}", i)
                }
            }
            DataType::Int128 => {
                let i = Endian::read_i128(bytes);
                if i < 0 {
                    format!("Int128: {:#x} ({:#x})", i, ReallySigned(i))
                } else {
                    format!("Int128: {:#x}", i)
                }
            }
            DataType::Float => {
                format!("Float: {}", Endian::read_f32(bytes))
            }
            DataType::Double => {
                format!("Double: {}", Endian::read_f64(bytes))
            }
            DataType::Bytes => {
                format!("Bytes: {:#?}", bytes)
            }
            DataType::String => {
                format!("String: {:?}", CStr::from_bytes_until_nul(bytes).ok()?)
            }
        }
        .into()
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

pub trait ObjArch: Send + Sync {
    #[expect(clippy::too_many_arguments)]
    fn process_code(
        &self,
        address: u64,
        code: &[u8],
        section_index: usize,
        relocations: &[ObjReloc],
        line_info: &BTreeMap<u64, u32>,
        config: &DiffObjConfig,
        sections: &[ObjSection],
    ) -> Result<ProcessCodeResult>;

    fn implcit_addend(
        &self,
        file: &File<'_>,
        section: &ObjSection,
        address: u64,
        reloc: &Relocation,
    ) -> Result<i64>;

    fn demangle(&self, _name: &str) -> Option<String> { None }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str>;

    fn symbol_address(&self, symbol: &Symbol) -> u64 { symbol.address() }

    fn guess_data_type(&self, _instruction: &ObjIns) -> Option<DataType> { None }

    fn display_data_type(&self, _ty: DataType, bytes: &[u8]) -> Option<String> {
        Some(format!("Bytes: {:#x?}", bytes))
    }

    // Downcast methods
    #[cfg(feature = "ppc")]
    fn ppc(&self) -> Option<&ppc::ObjArchPpc> { None }
}

pub struct ProcessCodeResult {
    pub ops: Vec<u16>,
    pub insts: Vec<ObjIns>,
}

pub fn new_arch(object: &File) -> Result<Box<dyn ObjArch>> {
    Ok(match object.architecture() {
        #[cfg(feature = "ppc")]
        Architecture::PowerPc => Box::new(ppc::ObjArchPpc::new(object)?),
        #[cfg(feature = "mips")]
        Architecture::Mips => Box::new(mips::ObjArchMips::new(object)?),
        #[cfg(feature = "x86")]
        Architecture::I386 | Architecture::X86_64 => Box::new(x86::ObjArchX86::new(object)?),
        #[cfg(feature = "arm")]
        Architecture::Arm => Box::new(arm::ObjArchArm::new(object)?),
        #[cfg(feature = "arm64")]
        Architecture::Aarch64 => Box::new(arm64::ObjArchArm64::new(object)?),
        arch => bail!("Unsupported architecture: {arch:?}"),
    })
}

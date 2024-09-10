use std::{borrow::Cow, collections::BTreeMap};

use anyhow::{bail, Result};
use object::{Architecture, File, Object, ObjectSymbol, Relocation, RelocationFlags, Symbol};

use crate::{
    diff::DiffObjConfig,
    obj::{ObjIns, ObjReloc, ObjSection},
};

#[cfg(feature = "arm")]
mod arm;
#[cfg(feature = "mips")]
pub mod mips;
#[cfg(feature = "ppc")]
pub mod ppc;
#[cfg(feature = "x86")]
pub mod x86;

pub trait ObjArch: Send + Sync {
    fn process_code(
        &self,
        address: u64,
        code: &[u8],
        section_index: usize,
        relocations: &[ObjReloc],
        line_info: &BTreeMap<u64, u32>,
        config: &DiffObjConfig,
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
        arch => bail!("Unsupported architecture: {arch:?}"),
    })
}

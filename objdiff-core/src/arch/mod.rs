use std::borrow::Cow;

use anyhow::{bail, Result};
use object::{Architecture, Object, Relocation, RelocationFlags};

use crate::{
    diff::DiffObjConfig,
    obj::{ObjInfo, ObjIns, ObjSection, SymbolRef},
};

#[cfg(feature = "mips")]
mod mips;
#[cfg(feature = "ppc")]
mod ppc;
#[cfg(feature = "x86")]
mod x86;
#[cfg(feature = "arm")]
mod arm;

pub trait ObjArch: Send + Sync {
    fn process_code(
        &self,
        obj: &ObjInfo,
        symbol_ref: SymbolRef,
        config: &DiffObjConfig,
    ) -> Result<ProcessCodeResult>;

    fn implcit_addend(&self, section: &ObjSection, address: u64, reloc: &Relocation)
        -> Result<i64>;

    fn demangle(&self, _name: &str) -> Option<String> { None }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str>;
}

pub struct ProcessCodeResult {
    pub ops: Vec<u16>,
    pub insts: Vec<ObjIns>,
}

pub fn new_arch(object: &object::File) -> Result<Box<dyn ObjArch>> {
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

use std::borrow::Cow;
use std::collections::BTreeMap;

use anyhow::{bail, Result};
use object::{Architecture, Object, Relocation, RelocationFlags};

use crate::{
    diff::{DiffObjConfig, ProcessCodeResult},
    obj::{ObjReloc, ObjSection},
};

#[cfg(feature = "mips")]
mod mips;
#[cfg(feature = "ppc")]
mod ppc;
#[cfg(feature = "x86")]
mod x86;

pub trait ObjArch: Send + Sync {
    fn process_code(
        &self,
        config: &DiffObjConfig,
        data: &[u8],
        address: u64,
        relocs: &[ObjReloc],
        line_info: &Option<BTreeMap<u64, u64>>,
    ) -> Result<ProcessCodeResult>;

    fn implcit_addend(&self, section: &ObjSection, address: u64, reloc: &Relocation)
        -> Result<i64>;

    fn demangle(&self, _name: &str) -> Option<String> { None }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str>;
}

pub fn new_arch(object: &object::File) -> Result<Box<dyn ObjArch>> {
    Ok(match object.architecture() {
        #[cfg(feature = "ppc")]
        Architecture::PowerPc => Box::new(ppc::ObjArchPpc::new(object)?),
        #[cfg(feature = "mips")]
        Architecture::Mips => Box::new(mips::ObjArchMips::new(object)?),
        #[cfg(feature = "x86")]
        Architecture::I386 | Architecture::X86_64 => Box::new(x86::ObjArchX86::new(object)?),
        arch => bail!("Unsupported architecture: {arch:?}"),
    })
}

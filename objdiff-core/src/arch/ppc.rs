use std::{borrow::Cow, collections::BTreeMap};

use anyhow::{bail, ensure, Result};
use cwextab::{decode_extab, ExceptionTableData};
use object::{
    elf, File, Object, ObjectSection, ObjectSymbol, Relocation, RelocationFlags, RelocationTarget,
    Symbol, SymbolKind,
};
use ppc750cl::{Argument, InsIter, GPR};

use crate::{
    arch::{ObjArch, ProcessCodeResult},
    diff::DiffObjConfig,
    obj::{ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjSection, ObjSymbol},
};

// Relative relocation, can be Simm, Offset or BranchDest
fn is_relative_arg(arg: &Argument) -> bool {
    matches!(arg, Argument::Simm(_) | Argument::Offset(_) | Argument::BranchDest(_))
}

// Relative or absolute relocation, can be Uimm, Simm or Offset
fn is_rel_abs_arg(arg: &Argument) -> bool {
    matches!(arg, Argument::Uimm(_) | Argument::Simm(_) | Argument::Offset(_))
}

fn is_offset_arg(arg: &Argument) -> bool { matches!(arg, Argument::Offset(_)) }

pub struct ObjArchPpc {
    /// Exception info
    pub extab: Option<BTreeMap<usize, ExceptionInfo>>,
}

impl ObjArchPpc {
    pub fn new(file: &File) -> Result<Self> { Ok(Self { extab: decode_exception_info(file)? }) }
}

impl ObjArch for ObjArchPpc {
    fn process_code(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        relocations: &[ObjReloc],
        line_info: &BTreeMap<u64, u32>,
        config: &DiffObjConfig,
    ) -> Result<ProcessCodeResult> {
        let ins_count = code.len() / 4;
        let mut ops = Vec::<u16>::with_capacity(ins_count);
        let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
        for (cur_addr, mut ins) in InsIter::new(code, address as u32) {
            let reloc = relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);
            if let Some(reloc) = reloc {
                // Zero out relocations
                ins.code = match reloc.flags {
                    RelocationFlags::Elf { r_type: elf::R_PPC_EMB_SDA21 } => ins.code & !0x1FFFFF,
                    RelocationFlags::Elf { r_type: elf::R_PPC_REL24 } => ins.code & !0x3FFFFFC,
                    RelocationFlags::Elf { r_type: elf::R_PPC_REL14 } => ins.code & !0xFFFC,
                    RelocationFlags::Elf {
                        r_type: elf::R_PPC_ADDR16_HI | elf::R_PPC_ADDR16_HA | elf::R_PPC_ADDR16_LO,
                    } => ins.code & !0xFFFF,
                    _ => ins.code,
                };
            }

            let orig = ins.basic().to_string();
            let simplified = ins.simplified();
            let formatted = simplified.to_string();

            let mut reloc_arg = None;
            if let Some(reloc) = reloc {
                match reloc.flags {
                    RelocationFlags::Elf { r_type: elf::R_PPC_EMB_SDA21 } => {
                        reloc_arg = Some(1);
                    }
                    RelocationFlags::Elf { r_type: elf::R_PPC_REL24 | elf::R_PPC_REL14 } => {
                        reloc_arg = simplified.args.iter().rposition(is_relative_arg);
                    }
                    RelocationFlags::Elf {
                        r_type: elf::R_PPC_ADDR16_HI | elf::R_PPC_ADDR16_HA | elf::R_PPC_ADDR16_LO,
                    } => {
                        reloc_arg = simplified.args.iter().rposition(is_rel_abs_arg);
                    }
                    _ => {}
                }
            }

            let mut args = vec![];
            let mut branch_dest = None;
            let mut writing_offset = false;
            for (idx, arg) in simplified.args_iter().enumerate() {
                if idx > 0 && !writing_offset {
                    args.push(ObjInsArg::PlainText(config.separator().into()));
                }

                if reloc_arg == Some(idx) {
                    let reloc = reloc.unwrap();
                    push_reloc(&mut args, reloc)?;
                    // For @sda21, we can omit the register argument
                    if matches!(reloc.flags, RelocationFlags::Elf { r_type: elf::R_PPC_EMB_SDA21 })
                        // Sanity check: the next argument should be r0
                        && matches!(simplified.args.get(idx + 1), Some(Argument::GPR(GPR(0))))
                    {
                        break;
                    }
                } else {
                    match arg {
                        Argument::Simm(simm) => {
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(simm.0 as i64)));
                        }
                        Argument::Uimm(uimm) => {
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(uimm.0 as u64)));
                        }
                        Argument::Offset(offset) => {
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(offset.0 as i64)));
                        }
                        Argument::BranchDest(dest) => {
                            let dest = cur_addr.wrapping_add_signed(dest.0) as u64;
                            args.push(ObjInsArg::BranchDest(dest));
                            branch_dest = Some(dest);
                        }
                        _ => {
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                                arg.to_string().into(),
                            )));
                        }
                    };
                }

                if writing_offset {
                    args.push(ObjInsArg::PlainText(")".into()));
                    writing_offset = false;
                }
                if is_offset_arg(arg) {
                    args.push(ObjInsArg::PlainText("(".into()));
                    writing_offset = true;
                }
            }

            ops.push(ins.op as u16);
            let line = line_info.range(..=cur_addr as u64).last().map(|(_, &b)| b);
            insts.push(ObjIns {
                address: cur_addr as u64,
                size: 4,
                mnemonic: simplified.mnemonic.to_string(),
                args,
                reloc: reloc.cloned(),
                op: ins.op as u16,
                branch_dest,
                line,
                formatted,
                orig: Some(orig),
            });
        }
        Ok(ProcessCodeResult { ops, insts })
    }

    fn implcit_addend(
        &self,
        _file: &File<'_>,
        _section: &ObjSection,
        address: u64,
        reloc: &Relocation,
    ) -> Result<i64> {
        bail!("Unsupported PPC implicit relocation {:#x}:{:?}", address, reloc.flags())
    }

    fn demangle(&self, name: &str) -> Option<String> {
        cwdemangle::demangle(name, &cwdemangle::DemangleOptions::default())
    }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        match flags {
            RelocationFlags::Elf { r_type } => match r_type {
                elf::R_PPC_ADDR16_LO => Cow::Borrowed("R_PPC_ADDR16_LO"),
                elf::R_PPC_ADDR16_HI => Cow::Borrowed("R_PPC_ADDR16_HI"),
                elf::R_PPC_ADDR16_HA => Cow::Borrowed("R_PPC_ADDR16_HA"),
                elf::R_PPC_EMB_SDA21 => Cow::Borrowed("R_PPC_EMB_SDA21"),
                elf::R_PPC_ADDR32 => Cow::Borrowed("R_PPC_ADDR32"),
                elf::R_PPC_UADDR32 => Cow::Borrowed("R_PPC_UADDR32"),
                elf::R_PPC_REL24 => Cow::Borrowed("R_PPC_REL24"),
                elf::R_PPC_REL14 => Cow::Borrowed("R_PPC_REL14"),
                _ => Cow::Owned(format!("<{flags:?}>")),
            },
            _ => Cow::Owned(format!("<{flags:?}>")),
        }
    }

    fn ppc(&self) -> Option<&ObjArchPpc> { Some(self) }
}

impl ObjArchPpc {
    pub fn extab_for_symbol(&self, symbol: &ObjSymbol) -> Option<&ExceptionInfo> {
        symbol.original_index.and_then(|i| self.extab.as_ref()?.get(&i))
    }
}

fn push_reloc(args: &mut Vec<ObjInsArg>, reloc: &ObjReloc) -> Result<()> {
    match reloc.flags {
        RelocationFlags::Elf { r_type } => match r_type {
            elf::R_PPC_ADDR16_LO => {
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText("@l".into()));
            }
            elf::R_PPC_ADDR16_HI => {
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText("@h".into()));
            }
            elf::R_PPC_ADDR16_HA => {
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText("@ha".into()));
            }
            elf::R_PPC_EMB_SDA21 => {
                args.push(ObjInsArg::Reloc);
                args.push(ObjInsArg::PlainText("@sda21".into()));
            }
            elf::R_PPC_ADDR32 | elf::R_PPC_UADDR32 | elf::R_PPC_REL24 | elf::R_PPC_REL14 => {
                args.push(ObjInsArg::Reloc);
            }
            _ => bail!("Unsupported ELF PPC relocation type {r_type}"),
        },
        flags => bail!("Unsupported PPC relocation kind: {flags:?}"),
    };
    Ok(())
}

#[derive(Debug, Clone)]
pub struct ExtabSymbolRef {
    pub original_index: usize,
    pub name: String,
    pub demangled_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExceptionInfo {
    pub eti_symbol: ExtabSymbolRef,
    pub etb_symbol: ExtabSymbolRef,
    pub data: ExceptionTableData,
    pub dtors: Vec<ExtabSymbolRef>,
}

fn decode_exception_info(file: &File<'_>) -> Result<Option<BTreeMap<usize, ExceptionInfo>>> {
    let Some(extab_section) = file.section_by_name("extab") else {
        return Ok(None);
    };
    let Some(extabindex_section) = file.section_by_name("extabindex") else {
        return Ok(None);
    };

    let mut result = BTreeMap::new();
    let extab_relocations = extab_section.relocations().collect::<BTreeMap<u64, Relocation>>();
    let extabindex_relocations =
        extabindex_section.relocations().collect::<BTreeMap<u64, Relocation>>();

    for extabindex in file.symbols().filter(|symbol| {
        symbol.section_index() == Some(extabindex_section.index())
            && symbol.kind() == SymbolKind::Data
    }) {
        if extabindex.size() != 12 {
            log::warn!("Invalid extabindex entry size {}", extabindex.size());
            continue;
        }

        // Each extabindex entry has two relocations:
        // - 0x0: The function that the exception table is for
        // - 0x8: The relevant entry in extab section
        let Some(extab_func_reloc) = extabindex_relocations.get(&extabindex.address()) else {
            log::warn!("Failed to find function relocation for extabindex entry");
            continue;
        };
        let Some(extab_reloc) = extabindex_relocations.get(&(extabindex.address() + 8)) else {
            log::warn!("Failed to find extab relocation for extabindex entry");
            continue;
        };

        // Resolve the function and extab symbols
        let Some(extab_func) = relocation_symbol(file, extab_func_reloc)? else {
            log::warn!("Failed to find function symbol for extabindex entry");
            continue;
        };
        let extab_func_name = extab_func.name()?;
        let Some(extab) = relocation_symbol(file, extab_reloc)? else {
            log::warn!("Failed to find extab symbol for extabindex entry");
            continue;
        };

        let extab_start_addr = extab.address() - extab_section.address();
        let extab_end_addr = extab_start_addr + extab.size();

        // All relocations in the extab section are dtors
        let mut dtors: Vec<ExtabSymbolRef> = vec![];
        for (_, reloc) in extab_relocations.range(extab_start_addr..extab_end_addr) {
            let Some(symbol) = relocation_symbol(file, reloc)? else {
                log::warn!("Failed to find symbol for extab relocation");
                continue;
            };
            dtors.push(make_symbol_ref(&symbol)?);
        }

        // Decode the extab data
        let Some(extab_data) = extab_section.data_range(extab_start_addr, extab.size())? else {
            log::warn!("Failed to get extab data for function {}", extab_func_name);
            continue;
        };
        let data = match decode_extab(extab_data) {
            Ok(decoded_data) => decoded_data,
            Err(e) => {
                log::warn!("Exception table decoding failed for function {}, reason: {}",
                extab_func_name, e.to_string());
                return Ok(None);
            }
        };

        //Add the new entry to the list
        result.insert(extab_func.index().0, ExceptionInfo {
            eti_symbol: make_symbol_ref(&extabindex)?,
            etb_symbol: make_symbol_ref(&extab)?,
            data,
            dtors,
        });
    }

    Ok(Some(result))
}

fn relocation_symbol<'data, 'file>(
    file: &'file File<'data>,
    relocation: &Relocation,
) -> Result<Option<Symbol<'data, 'file>>> {
    let addend = relocation.addend();
    match relocation.target() {
        RelocationTarget::Symbol(idx) => {
            ensure!(addend == 0, "Symbol relocations must have zero addend");
            Ok(Some(file.symbol_by_index(idx)?))
        }
        RelocationTarget::Section(idx) => {
            ensure!(addend >= 0, "Section relocations must have non-negative addend");
            let addend = addend as u64;
            Ok(file
                .symbols()
                .find(|symbol| symbol.section_index() == Some(idx) && symbol.address() == addend))
        }
        target => bail!("Unsupported relocation target: {target:?}"),
    }
}

fn make_symbol_ref(symbol: &Symbol) -> Result<ExtabSymbolRef> {
    let name = symbol.name()?.to_string();
    let demangled_name = cwdemangle::demangle(&name, &cwdemangle::DemangleOptions::default());
    Ok(ExtabSymbolRef { original_index: symbol.index().0, name, demangled_name })
}

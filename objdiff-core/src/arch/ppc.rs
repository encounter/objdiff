use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
};

use anyhow::{bail, ensure, Result};
use byteorder::BigEndian;
use cwextab::{decode_extab, ExceptionTableData};
use object::{
    elf, File, Object, ObjectSection, ObjectSymbol, Relocation, RelocationFlags, RelocationTarget,
    Symbol, SymbolKind,
};
use ppc750cl::{Argument, InsIter, Opcode, ParsedIns, GPR};

use crate::{
    arch::{DataType, ObjArch, ProcessCodeResult},
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
        let fake_pool_reloc_for_addr =
            generate_fake_pool_reloc_for_addr_mapping(address, code, relocations);
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
                mnemonic: Cow::Borrowed(simplified.mnemonic),
                args,
                reloc: reloc.cloned(),
                fake_pool_reloc: fake_pool_reloc_for_addr.get(&cur_addr).cloned(),
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
                elf::R_PPC_NONE => Cow::Borrowed("R_PPC_NONE"), // We use this for fake pool relocs
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

    fn guess_data_type(&self, instruction: &ObjIns) -> Option<super::DataType> {
        if instruction
            .reloc
            .as_ref()
            .or(instruction.fake_pool_reloc.as_ref())
            .is_some_and(|r| r.target.name.starts_with("@stringBase"))
        {
            return Some(DataType::String);
        }

        guess_data_type_from_load_store_inst_op(Opcode::from(instruction.op as u8))
    }

    fn display_data_type(&self, ty: DataType, bytes: &[u8]) -> Option<String> {
        ty.display_bytes::<BigEndian>(bytes)
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
                log::warn!(
                    "Exception table decoding failed for function {}, reason: {}",
                    extab_func_name,
                    e.to_string()
                );
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

fn guess_data_type_from_load_store_inst_op(inst_op: Opcode) -> Option<DataType> {
    match inst_op {
        Opcode::Lbz | Opcode::Lbzu | Opcode::Lbzux | Opcode::Lbzx => Some(DataType::Int8),
        Opcode::Lhz | Opcode::Lhzu | Opcode::Lhzux | Opcode::Lhzx => Some(DataType::Int16),
        Opcode::Lha | Opcode::Lhau | Opcode::Lhaux | Opcode::Lhax => Some(DataType::Int16),
        Opcode::Lwz | Opcode::Lwzu | Opcode::Lwzux | Opcode::Lwzx => Some(DataType::Int32),
        Opcode::Lfs | Opcode::Lfsu | Opcode::Lfsux | Opcode::Lfsx => Some(DataType::Float),
        Opcode::Lfd | Opcode::Lfdu | Opcode::Lfdux | Opcode::Lfdx => Some(DataType::Double),

        Opcode::Stb | Opcode::Stbu | Opcode::Stbux | Opcode::Stbx => Some(DataType::Int8),
        Opcode::Sth | Opcode::Sthu | Opcode::Sthux | Opcode::Sthx => Some(DataType::Int16),
        Opcode::Stw | Opcode::Stwu | Opcode::Stwux | Opcode::Stwx => Some(DataType::Int32),
        Opcode::Stfs | Opcode::Stfsu | Opcode::Stfsux | Opcode::Stfsx => Some(DataType::Float),
        Opcode::Stfd | Opcode::Stfdu | Opcode::Stfdux | Opcode::Stfdx => Some(DataType::Double),
        _ => None,
    }
}

// Given an instruction, determine if it could accessing data at the address in a register.
// If so, return the offset added to the register's address, the register containing that address,
// and (optionally) which destination register the address is being copied into.
fn get_offset_and_addr_gpr_for_possible_pool_reference(
    opcode: Opcode,
    simplified: &ParsedIns,
) -> Option<(i16, GPR, Option<GPR>)> {
    let args = &simplified.args;
    if guess_data_type_from_load_store_inst_op(opcode).is_some() {
        match (args[1], args[2]) {
            (Argument::Offset(offset), Argument::GPR(addr_src_gpr)) => {
                // e.g. lwz. Immediate offset.
                Some((offset.0, addr_src_gpr, None))
            }
            (Argument::GPR(addr_src_gpr), Argument::GPR(_offset_gpr)) => {
                // e.g. lwzx. The offset is in a register and was likely calculated from an index.
                // Treat the offset as being 0 in this case to show the first element of the array.
                // It may be possible to show all elements by figuring out the stride of the array
                // from the calculations performed on the index before it's put into offset_gpr, but
                // this would be much more complicated, so it's not currently done.
                Some((0, addr_src_gpr, None))
            }
            _ => None,
        }
    } else {
        // If it's not a load/store instruction, there's two more possibilities we need to handle.
        // 1. It could be a reference to @stringBase.
        // 2. It could be moving the relocation address plus an offset into a different register to
        //    load from later.
        // If either of these match, we also want to return the destination register that the
        // address is being copied into so that we can detect any future references to that new
        // register as well.
        match (opcode, args[0], args[1], args[2]) {
            (
                Opcode::Addi,
                Argument::GPR(addr_dst_gpr),
                Argument::GPR(addr_src_gpr),
                Argument::Simm(simm),
            ) => Some((simm.0, addr_src_gpr, Some(addr_dst_gpr))),
            (
                Opcode::Or,
                Argument::GPR(addr_dst_gpr),
                Argument::GPR(addr_src_gpr),
                Argument::None,
            ) => Some((0, addr_src_gpr, Some(addr_dst_gpr))), // `mr` or `mr.`
            _ => None,
        }
    }
}

// We create a fake relocation for an instruction, vaguely simulating what the actual relocation
// might have looked like if it wasn't pooled. This is so minimal changes are needed to display
// pooled accesses vs non-pooled accesses. We set the relocation type to R_PPC_NONE to indicate that
// there isn't really a relocation here, as copying the pool relocation's type wouldn't make sense.
// Also, if this instruction is accessing the middle of a symbol instead of the start, we add an
// addend to indicate that.
fn make_fake_pool_reloc(offset: i16, cur_addr: u32, pool_reloc: &ObjReloc) -> Option<ObjReloc> {
    let offset_from_pool = pool_reloc.addend + offset as i64;
    let target_address = pool_reloc.target.address.checked_add_signed(offset_from_pool)?;
    let orig_section_index = pool_reloc.target.orig_section_index?;
    // We also need to create a fake target symbol to go inside our fake relocation.
    // This is because we don't have access to list of all symbols in this section, so we can't find
    // the real symbol yet. Instead we make a placeholder that has the correct `orig_section_index`
    // and `address` fields, and then later on when this information is displayed to the user, we
    // can find the real symbol by searching through the object's section's symbols for one that
    // contains this address.
    let fake_target_symbol = ObjSymbol {
        name: "".to_string(),
        demangled_name: None,
        address: target_address,
        section_address: 0,
        size: 0,
        size_known: false,
        kind: Default::default(),
        flags: Default::default(),
        orig_section_index: Some(orig_section_index),
        virtual_address: None,
        original_index: None,
        bytes: vec![],
    };
    // The addend is also fake because we don't know yet if the `target_address` here is the exact
    // start of the symbol or if it's in the middle of it.
    let fake_addend = 0;
    Some(ObjReloc {
        flags: RelocationFlags::Elf { r_type: elf::R_PPC_NONE },
        address: cur_addr as u64,
        target: fake_target_symbol,
        addend: fake_addend,
    })
}

// Searches through all instructions in a function, determining which registers have the addresses
// of pooled data relocations in them, finding which instructions load data from those addresses,
// and constructing a mapping of the address of that instruction to a "fake pool relocation" that
// simulates what that instruction's relocation would look like if data hadn't been pooled.
// Limitations: This method currently only goes through the instructions in a function in linear
// order, from start to finish. It does *not* follow any branches. This means that it could have
// false positives or false negatives in determining which relocation is currently loaded in which
// register at any given point in the function, as control flow is not respected.
// There are currently no known examples of this method producing inaccurate results in reality, but
// if examples are found, it may be possible to update this method to also follow all branches so
// that it produces more accurate results.
fn generate_fake_pool_reloc_for_addr_mapping(
    address: u64,
    code: &[u8],
    relocations: &[ObjReloc],
) -> HashMap<u32, ObjReloc> {
    let mut active_pool_relocs = HashMap::new();
    let mut pool_reloc_for_addr = HashMap::new();
    for (cur_addr, ins) in InsIter::new(code, address as u32) {
        let simplified = ins.simplified();
        let reloc = relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);

        if let Some(reloc) = reloc {
            // This instruction has a real relocation, so it may be a pool load we want to keep
            // track of.
            let args = &simplified.args;
            match (ins.op, args[0], args[1], args[2]) {
                (
                    Opcode::Addi,
                    Argument::GPR(addr_dst_gpr),
                    Argument::GPR(_addr_src_gpr),
                    Argument::Simm(_simm),
                ) => {
                    active_pool_relocs.insert(addr_dst_gpr.0, reloc.clone()); // `lis` + `addi`
                }
                (
                    Opcode::Ori,
                    Argument::GPR(addr_dst_gpr),
                    Argument::GPR(_addr_src_gpr),
                    Argument::Uimm(_uimm),
                ) => {
                    active_pool_relocs.insert(addr_dst_gpr.0, reloc.clone()); // `lis` + `ori`
                }
                (Opcode::B, _, _, _) => {
                    if simplified.mnemonic == "bl" {
                        // When encountering a function call, clear any active pool relocations from
                        // the volatile registers (r0, r3-r12), but not the nonvolatile registers.
                        active_pool_relocs.remove(&0);
                        for gpr in 3..12 {
                            active_pool_relocs.remove(&gpr);
                        }
                    }
                }
                _ => {}
            }
        } else if let Some((offset, addr_src_gpr, addr_dst_gpr)) =
            get_offset_and_addr_gpr_for_possible_pool_reference(ins.op, &simplified)
        {
            // This instruction doesn't have a real relocation, so it may be a reference to one of
            // the already-loaded pools.
            if let Some(pool_reloc) = active_pool_relocs.get(&addr_src_gpr.0) {
                if let Some(fake_pool_reloc) = make_fake_pool_reloc(offset, cur_addr, pool_reloc) {
                    pool_reloc_for_addr.insert(cur_addr, fake_pool_reloc);
                }
                if let Some(addr_dst_gpr) = addr_dst_gpr {
                    // If the address of the pool relocation got copied into another register, we
                    // need to keep track of it in that register too as future instructions may
                    // reference the symbol indirectly via this new register, instead of the
                    // register the symbol's address was originally loaded into.
                    // For example, the start of the function might `lis` + `addi` the start of the
                    // ...data pool into r25, and then later the start of a loop will `addi` r25
                    // with the offset within the .data section of an array variable into r21.
                    // Then the body of the loop will `lwzx` one of the array elements from r21.
                    let mut new_reloc = pool_reloc.clone();
                    new_reloc.addend += offset as i64;
                    active_pool_relocs.insert(addr_dst_gpr.0, new_reloc);
                }
            }
        }
    }

    pool_reloc_for_addr
}

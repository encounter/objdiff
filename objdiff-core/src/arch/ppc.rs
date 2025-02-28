use alloc::{
    borrow::Cow,
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::ops::Range;

use anyhow::{bail, ensure, Result};
use byteorder::BigEndian;
use cwextab::{decode_extab, ExceptionTableData};
use flagset::Flags;
use object::{elf, Object as _, ObjectSection as _, ObjectSymbol as _};

use crate::{
    arch::{Arch, DataType},
    diff::{display::InstructionPart, DiffObjConfig},
    obj::{
        InstructionRef, Relocation, RelocationFlags, ResolvedRelocation, ScannedInstruction,
        Symbol, SymbolFlag, SymbolFlagSet,
    },
};

// Relative relocation, can be Simm, Offset or BranchDest
fn is_relative_arg(arg: &ppc750cl::Argument) -> bool {
    matches!(
        arg,
        ppc750cl::Argument::Simm(_)
            | ppc750cl::Argument::Offset(_)
            | ppc750cl::Argument::BranchDest(_)
    )
}

// Relative or absolute relocation, can be Uimm, Simm or Offset
fn is_rel_abs_arg(arg: &ppc750cl::Argument) -> bool {
    matches!(
        arg,
        ppc750cl::Argument::Uimm(_) | ppc750cl::Argument::Simm(_) | ppc750cl::Argument::Offset(_)
    )
}

fn is_offset_arg(arg: &ppc750cl::Argument) -> bool { matches!(arg, ppc750cl::Argument::Offset(_)) }

#[derive(Debug)]
pub struct ArchPpc {
    /// Exception info
    pub extab: Option<BTreeMap<usize, ExceptionInfo>>,
}

impl ArchPpc {
    pub fn new(file: &object::File) -> Result<Self> {
        Ok(Self { extab: decode_exception_info(file)? })
    }

    fn find_reloc_arg(
        &self,
        ins: &ppc750cl::ParsedIns,
        resolved: Option<ResolvedRelocation>,
    ) -> Option<usize> {
        match resolved?.relocation.flags {
            RelocationFlags::Elf(elf::R_PPC_EMB_SDA21) => Some(1),
            RelocationFlags::Elf(elf::R_PPC_REL24 | elf::R_PPC_REL14) => {
                ins.args.iter().rposition(is_relative_arg)
            }
            RelocationFlags::Elf(
                elf::R_PPC_ADDR16_HI | elf::R_PPC_ADDR16_HA | elf::R_PPC_ADDR16_LO,
            ) => ins.args.iter().rposition(is_rel_abs_arg),
            _ => None,
        }
    }
}

impl Arch for ArchPpc {
    fn scan_instructions(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        _diff_config: &DiffObjConfig,
    ) -> Result<Vec<ScannedInstruction>> {
        ensure!(code.len() & 3 == 0, "Code length must be a multiple of 4");
        let ins_count = code.len() / 4;
        let mut insts = Vec::<ScannedInstruction>::with_capacity(ins_count);
        for (cur_addr, ins) in ppc750cl::InsIter::new(code, address as u32) {
            insts.push(ScannedInstruction {
                ins_ref: InstructionRef {
                    address: cur_addr as u64,
                    size: 4,
                    opcode: u8::from(ins.op) as u16,
                },
                branch_dest: ins.branch_dest(cur_addr).map(u64::from),
            });
        }
        Ok(insts)
    }

    fn display_instruction(
        &self,
        ins_ref: InstructionRef,
        code: &[u8],
        relocation: Option<ResolvedRelocation>,
        _function_range: Range<u64>,
        _section_index: usize,
        _diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        let mut code = u32::from_be_bytes(code.try_into()?);
        if let Some(resolved) = relocation {
            code = zero_reloc(code, resolved.relocation);
        }
        let op = ppc750cl::Opcode::from(ins_ref.opcode as u8);
        let ins = ppc750cl::Ins { code, op }.simplified();

        cb(InstructionPart::opcode(ins.mnemonic, ins_ref.opcode))?;

        let reloc_arg = self.find_reloc_arg(&ins, relocation);

        let mut writing_offset = false;
        for (idx, arg) in ins.args_iter().enumerate() {
            if idx > 0 && !writing_offset {
                cb(InstructionPart::separator())?;
            }

            if reloc_arg == Some(idx) {
                let resolved = relocation.unwrap();
                display_reloc(resolved, cb)?;
                // For @sda21, we can omit the register argument
                if matches!(resolved.relocation.flags, RelocationFlags::Elf(elf::R_PPC_EMB_SDA21))
                    // Sanity check: the next argument should be r0
                    && matches!(ins.args.get(idx + 1), Some(ppc750cl::Argument::GPR(ppc750cl::GPR(0))))
                {
                    break;
                }
            } else {
                match arg {
                    ppc750cl::Argument::Simm(simm) => cb(InstructionPart::signed(simm.0)),
                    ppc750cl::Argument::Uimm(uimm) => cb(InstructionPart::unsigned(uimm.0)),
                    ppc750cl::Argument::Offset(offset) => cb(InstructionPart::signed(offset.0)),
                    ppc750cl::Argument::BranchDest(dest) => cb(InstructionPart::branch_dest(
                        (ins_ref.address as u32).wrapping_add_signed(dest.0),
                    )),
                    _ => cb(InstructionPart::opaque(arg.to_string())),
                }?;
            }

            if writing_offset {
                cb(InstructionPart::basic(")"))?;
                writing_offset = false;
            }
            if is_offset_arg(arg) {
                cb(InstructionPart::basic("("))?;
                writing_offset = true;
            }
        }

        Ok(())
    }

    fn implcit_addend(
        &self,
        _file: &object::File<'_>,
        _section: &object::Section,
        address: u64,
        _relocation: &object::Relocation,
        flags: RelocationFlags,
    ) -> Result<i64> {
        bail!("Unsupported PPC implicit relocation {:#x}:{:?}", address, flags)
    }

    fn demangle(&self, name: &str) -> Option<String> {
        cwdemangle::demangle(name, &cwdemangle::DemangleOptions::default())
    }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
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

    fn extra_symbol_flags(&self, symbol: &object::Symbol) -> SymbolFlagSet {
        if self.extab.as_ref().is_some_and(|extab| extab.contains_key(&symbol.index().0)) {
            SymbolFlag::HasExtra.into()
        } else {
            SymbolFlag::none()
        }
    }

    fn get_reloc_byte_size(&self, flags: RelocationFlags) -> usize {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                elf::R_PPC_ADDR32 => 4,
                elf::R_PPC_UADDR32 => 4,
                _ => 1,
            },
            _ => 1,
        }
    }

    fn guess_data_type(
        &self,
        ins_ref: InstructionRef,
        _code: &[u8],
        relocation: Option<ResolvedRelocation>,
    ) -> Option<DataType> {
        if relocation.is_some_and(|r| r.symbol.name.starts_with("@stringBase")) {
            return Some(DataType::String);
        }

        guess_data_type_from_load_store_inst_op(ppc750cl::Opcode::from(ins_ref.opcode as u8))
    }

    fn display_data_labels(&self, ty: DataType, bytes: &[u8]) -> Vec<String> {
        ty.display_labels::<BigEndian>(bytes)
    }

    fn display_data_literals(&self, ty: DataType, bytes: &[u8]) -> Vec<String> {
        ty.display_literals::<BigEndian>(bytes)
    }
}

impl ArchPpc {
    pub fn extab_for_symbol(&self, _symbol: &Symbol) -> Option<&ExceptionInfo> {
        // TODO
        // symbol.original_index.and_then(|i| self.extab.as_ref()?.get(&i))
        None
    }
}

fn zero_reloc(code: u32, reloc: &Relocation) -> u32 {
    match reloc.flags {
        RelocationFlags::Elf(elf::R_PPC_EMB_SDA21) => code & !0x1FFFFF,
        RelocationFlags::Elf(elf::R_PPC_REL24) => code & !0x3FFFFFC,
        RelocationFlags::Elf(elf::R_PPC_REL14) => code & !0xFFFC,
        RelocationFlags::Elf(
            elf::R_PPC_ADDR16_HI | elf::R_PPC_ADDR16_HA | elf::R_PPC_ADDR16_LO,
        ) => code & !0xFFFF,
        _ => code,
    }
}

fn display_reloc(
    resolved: ResolvedRelocation,
    cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
) -> Result<()> {
    match resolved.relocation.flags {
        RelocationFlags::Elf(r_type) => match r_type {
            elf::R_PPC_ADDR16_LO => {
                cb(InstructionPart::reloc())?;
                cb(InstructionPart::basic("@l"))?;
            }
            elf::R_PPC_ADDR16_HI => {
                cb(InstructionPart::reloc())?;
                cb(InstructionPart::basic("@h"))?;
            }
            elf::R_PPC_ADDR16_HA => {
                cb(InstructionPart::reloc())?;
                cb(InstructionPart::basic("@ha"))?;
            }
            elf::R_PPC_EMB_SDA21 => {
                cb(InstructionPart::reloc())?;
                cb(InstructionPart::basic("@sda21"))?;
            }
            elf::R_PPC_ADDR32 | elf::R_PPC_UADDR32 | elf::R_PPC_REL24 | elf::R_PPC_REL14 => {
                cb(InstructionPart::reloc())?;
            }
            elf::R_PPC_NONE => {
                // Fake pool relocation.
                cb(InstructionPart::basic("<"))?;
                cb(InstructionPart::reloc())?;
                cb(InstructionPart::basic(">"))?;
            }
            _ => cb(InstructionPart::reloc())?,
        },
        _ => cb(InstructionPart::reloc())?,
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

fn decode_exception_info(
    file: &object::File<'_>,
) -> Result<Option<BTreeMap<usize, ExceptionInfo>>> {
    let Some(extab_section) = file.section_by_name("extab") else {
        return Ok(None);
    };
    let Some(extabindex_section) = file.section_by_name("extabindex") else {
        return Ok(None);
    };

    let mut result = BTreeMap::new();
    let extab_relocations =
        extab_section.relocations().collect::<BTreeMap<u64, object::Relocation>>();
    let extabindex_relocations =
        extabindex_section.relocations().collect::<BTreeMap<u64, object::Relocation>>();

    for extabindex in file.symbols().filter(|symbol| {
        symbol.section_index() == Some(extabindex_section.index())
            && symbol.kind() == object::SymbolKind::Data
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
    file: &'file object::File<'data>,
    relocation: &object::Relocation,
) -> Result<Option<object::Symbol<'data, 'file>>> {
    let addend = relocation.addend();
    match relocation.target() {
        object::RelocationTarget::Symbol(idx) => {
            ensure!(addend == 0, "Symbol relocations must have zero addend");
            Ok(Some(file.symbol_by_index(idx)?))
        }
        object::RelocationTarget::Section(idx) => {
            ensure!(addend >= 0, "Section relocations must have non-negative addend");
            let addend = addend as u64;
            Ok(file
                .symbols()
                .find(|symbol| symbol.section_index() == Some(idx) && symbol.address() == addend))
        }
        target => bail!("Unsupported relocation target: {target:?}"),
    }
}

fn make_symbol_ref(symbol: &object::Symbol) -> Result<ExtabSymbolRef> {
    let name = symbol.name()?.to_string();
    let demangled_name = cwdemangle::demangle(&name, &cwdemangle::DemangleOptions::default());
    Ok(ExtabSymbolRef { original_index: symbol.index().0, name, demangled_name })
}

fn guess_data_type_from_load_store_inst_op(inst_op: ppc750cl::Opcode) -> Option<DataType> {
    use ppc750cl::Opcode;
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
#[expect(unused)]
fn get_offset_and_addr_gpr_for_possible_pool_reference(
    opcode: ppc750cl::Opcode,
    simplified: &ppc750cl::ParsedIns,
) -> Option<(i16, ppc750cl::GPR, Option<ppc750cl::GPR>)> {
    use ppc750cl::{Argument, Opcode};
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
        // 1. It could be loading a pointer to a string.
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
                // `mr` or `mr.`
                Opcode::Or,
                Argument::GPR(addr_dst_gpr),
                Argument::GPR(addr_src_gpr),
                Argument::None,
            ) => Some((0, addr_src_gpr, Some(addr_dst_gpr))),
            (
                Opcode::Add,
                Argument::GPR(addr_dst_gpr),
                Argument::GPR(addr_src_gpr),
                Argument::GPR(_offset_gpr),
            ) => Some((0, addr_src_gpr, Some(addr_dst_gpr))),
            _ => None,
        }
    }
}

// Remove the relocation we're keeping track of in a particular register when an instruction reuses
// that register to hold some other value, unrelated to pool relocation addresses.
#[expect(unused)]
fn clear_overwritten_gprs(ins: ppc750cl::Ins, gpr_pool_relocs: &mut BTreeMap<u8, Relocation>) {
    use ppc750cl::{Argument, Arguments, Opcode};
    let mut def_args = Arguments::default();
    ins.parse_defs(&mut def_args);
    for arg in def_args {
        if let Argument::GPR(gpr) = arg {
            if ins.op == Opcode::Lmw {
                // `lmw` overwrites all registers from rd to r31.
                // ppc750cl only returns rd itself, so we manually clear the rest of them.
                for reg in gpr.0..31 {
                    gpr_pool_relocs.remove(&reg);
                }
                break;
            }
            gpr_pool_relocs.remove(&gpr.0);
        }
    }
}

// TODO
// // We create a fake relocation for an instruction, vaguely simulating what the actual relocation
// // might have looked like if it wasn't pooled. This is so minimal changes are needed to display
// // pooled accesses vs non-pooled accesses. We set the relocation type to R_PPC_NONE to indicate that
// // there isn't really a relocation here, as copying the pool relocation's type wouldn't make sense.
// // Also, if this instruction is accessing the middle of a symbol instead of the start, we add an
// // addend to indicate that.
// fn make_fake_pool_reloc(offset: i16, cur_addr: u32, pool_reloc: &Relocation) -> Option<Relocation> {
//     let offset_from_pool = pool_reloc.addend + offset as i64;
//     let target_address = pool_reloc.sy.address.checked_add_signed(offset_from_pool)?;
//     let target;
//     let addend;
//     if pool_reloc.target.orig_section_index.is_some() {
//         // If the target symbol is within this current object, then we also need to create a fake
//         // target symbol to go inside our fake relocation. This is because we don't have access to
//         // list of all symbols in this section, so we can't find the real symbol within the pool
//         // based on its address yet. Instead we make a placeholder that has the correct
//         // `orig_section_index` and `address` fields, and then later on when this information is
//         // displayed to the user, we can find the real symbol by searching through the object's
//         // section's symbols for one that contains this address.
//         target = ObjSymbol {
//             name: "".to_string(),
//             demangled_name: None,
//             address: target_address,
//             section_address: 0,
//             size: 0,
//             size_known: false,
//             kind: Default::default(),
//             flags: Default::default(),
//             orig_section_index: pool_reloc.target.orig_section_index,
//             virtual_address: None,
//             original_index: None,
//             bytes: vec![],
//         };
//         // The addend is also fake because we don't know yet if the `target_address` here is the exact
//         // start of the symbol or if it's in the middle of it.
//         addend = 0;
//     } else {
//         // But if the target symbol is in a different object (extern), then we simply copy the pool
//         // relocation's target. This is because it won't be possible to locate the actual symbol
//         // later on based only off of an offset without knowing the object or section it's in. And
//         // doing that for external symbols would also be unnecessary, because when the compiler
//         // generates an instruction that accesses an external "pool" plus some offset, that won't be
//         // a normal pool that contains other symbols within it that we want to display. It will be
//         // something like a vtable for a class with multiple inheritance (for example, dCcD_Cyl in
//         // The Wind Waker). So just showing that vtable symbol plus an addend to represent the
//         // offset into it works fine in this case, no fake symbol to hold an address is necessary.
//         target = pool_reloc.target.clone();
//         addend = pool_reloc.addend;
//     };
//     Some(ObjReloc {
//         flags: RelocationFlags::Elf { r_type: elf::R_PPC_NONE },
//         address: cur_addr as u64,
//         target,
//         addend,
//     })
// }
//
// // Searches through all instructions in a function, determining which registers have the addresses
// // of pooled data relocations in them, finding which instructions load data from those addresses,
// // and constructing a mapping of the address of that instruction to a "fake pool relocation" that
// // simulates what that instruction's relocation would look like if data hadn't been pooled.
// // This method tries to follow the function's proper control flow. It keeps track of a queue of
// // states it hasn't traversed yet, where each state holds an instruction address and a HashMap of
// // which registers hold which pool relocations at that point.
// // When a conditional or unconditional branch is encountered, the destination of the branch is added
// // to the queue. Conditional branches will traverse both the path where the branch is taken and the
// // one where it's not. Unconditional branches only follow the branch, ignoring any code immediately
// // after the branch instruction.
// // Limitations: This method cannot read jump tables. This is because the jump tables are located in
// // the .data section, but ObjArch.process_code only has access to the .text section. In order to
// // work around this limitation and avoid completely missing most code inside switch statements that
// // use jump tables, we instead guess that any parts of a function we missed were switch cases, and
// // traverse them as if the last `bctr` before that address had branched there. This should be fairly
// // accurate in practice - in testing the only instructions it seems to miss are double branches that
// // the compiler generates in error which can never be reached during normal execution anyway.
// fn generate_fake_pool_reloc_for_addr_mapping(
//     func_address: u64,
//     code: &[u8],
//     relocations: &[ObjReloc],
// ) -> BTreeMap<u32, ObjReloc> {
//     let mut visited_ins_addrs = BTreeSet::new();
//     let mut pool_reloc_for_addr = BTreeMap::new();
//     let mut ins_iters_with_gpr_state =
//         vec![(InsIter::new(code, func_address as u32), BTreeMap::new())];
//     let mut gpr_state_at_bctr = BTreeMap::new();
//     while let Some((ins_iter, mut gpr_pool_relocs)) = ins_iters_with_gpr_state.pop() {
//         for (cur_addr, ins) in ins_iter {
//             if visited_ins_addrs.contains(&cur_addr) {
//                 // Avoid getting stuck in an infinite loop when following looping branches.
//                 break;
//             }
//             visited_ins_addrs.insert(cur_addr);
//
//             let simplified = ins.simplified();
//
//             // First handle traversing the function's control flow.
//             let mut branch_dest = None;
//             for arg in simplified.args_iter() {
//                 if let Argument::BranchDest(dest) = arg {
//                     let dest = cur_addr.wrapping_add_signed(dest.0);
//                     branch_dest = Some(dest);
//                     break;
//                 }
//             }
//             if let Some(branch_dest) = branch_dest {
//                 if branch_dest >= func_address as u32
//                     && (branch_dest - func_address as u32) < code.len() as u32
//                 {
//                     let dest_offset_into_func = branch_dest - func_address as u32;
//                     let dest_code_slice = &code[dest_offset_into_func as usize..];
//                     match ins.op {
//                         Opcode::Bc => {
//                             // Conditional branch.
//                             // Add the branch destination to the queue to do later.
//                             ins_iters_with_gpr_state.push((
//                                 InsIter::new(dest_code_slice, branch_dest),
//                                 gpr_pool_relocs.clone(),
//                             ));
//                             // Then continue on with the current iterator.
//                         }
//                         Opcode::B => {
//                             if simplified.mnemonic != "bl" {
//                                 // Unconditional branch.
//                                 // Add the branch destination to the queue.
//                                 ins_iters_with_gpr_state.push((
//                                     InsIter::new(dest_code_slice, branch_dest),
//                                     gpr_pool_relocs.clone(),
//                                 ));
//                                 // Break out of the current iterator so we can do the newly added one.
//                                 break;
//                             }
//                         }
//                         _ => unreachable!(),
//                     }
//                 }
//             }
//             if let Opcode::Bcctr = ins.op {
//                 if simplified.mnemonic == "bctr" {
//                     // Unconditional branch to count register.
//                     // Likely a jump table.
//                     gpr_state_at_bctr.insert(cur_addr, gpr_pool_relocs.clone());
//                 }
//             }
//
//             // Then handle keeping track of which GPR contains which pool relocation.
//             let reloc = relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);
//             if let Some(reloc) = reloc {
//                 // This instruction has a real relocation, so it may be a pool load we want to keep
//                 // track of.
//                 let args = &simplified.args;
//                 match (ins.op, args[0], args[1], args[2]) {
//                     (
//                         // `lis` + `addi`
//                         Opcode::Addi,
//                         Argument::GPR(addr_dst_gpr),
//                         Argument::GPR(_addr_src_gpr),
//                         Argument::Simm(_simm),
//                     ) => {
//                         gpr_pool_relocs.insert(addr_dst_gpr.0, reloc.clone());
//                     }
//                     (
//                         // `lis` + `ori`
//                         Opcode::Ori,
//                         Argument::GPR(addr_dst_gpr),
//                         Argument::GPR(_addr_src_gpr),
//                         Argument::Uimm(_uimm),
//                     ) => {
//                         gpr_pool_relocs.insert(addr_dst_gpr.0, reloc.clone());
//                     }
//                     (Opcode::B, _, _, _) => {
//                         if simplified.mnemonic == "bl" {
//                             // When encountering a function call, clear any active pool relocations from
//                             // the volatile registers (r0, r3-r12), but not the nonvolatile registers.
//                             gpr_pool_relocs.remove(&0);
//                             for gpr in 3..12 {
//                                 gpr_pool_relocs.remove(&gpr);
//                             }
//                         }
//                     }
//                     _ => {
//                         clear_overwritten_gprs(ins, &mut gpr_pool_relocs);
//                     }
//                 }
//             } else if let Some((offset, addr_src_gpr, addr_dst_gpr)) =
//                 get_offset_and_addr_gpr_for_possible_pool_reference(ins.op, &simplified)
//             {
//                 // This instruction doesn't have a real relocation, so it may be a reference to one of
//                 // the already-loaded pools.
//                 if let Some(pool_reloc) = gpr_pool_relocs.get(&addr_src_gpr.0) {
//                     if let Some(fake_pool_reloc) =
//                         make_fake_pool_reloc(offset, cur_addr, pool_reloc)
//                     {
//                         pool_reloc_for_addr.insert(cur_addr, fake_pool_reloc);
//                     }
//                     if let Some(addr_dst_gpr) = addr_dst_gpr {
//                         // If the address of the pool relocation got copied into another register, we
//                         // need to keep track of it in that register too as future instructions may
//                         // reference the symbol indirectly via this new register, instead of the
//                         // register the symbol's address was originally loaded into.
//                         // For example, the start of the function might `lis` + `addi` the start of the
//                         // ...data pool into r25, and then later the start of a loop will `addi` r25
//                         // with the offset within the .data section of an array variable into r21.
//                         // Then the body of the loop will `lwzx` one of the array elements from r21.
//                         let mut new_reloc = pool_reloc.clone();
//                         new_reloc.addend += offset as i64;
//                         gpr_pool_relocs.insert(addr_dst_gpr.0, new_reloc);
//                     } else {
//                         clear_overwritten_gprs(ins, &mut gpr_pool_relocs);
//                     }
//                 } else {
//                     clear_overwritten_gprs(ins, &mut gpr_pool_relocs);
//                 }
//             } else {
//                 clear_overwritten_gprs(ins, &mut gpr_pool_relocs);
//             }
//         }
//
//         // Finally, if we're about to finish the outer loop and don't have any more control flow to
//         // follow, we check if there are any instruction addresses in this function that we missed.
//         // If so, and if there were any `bctr` instructions before those points in this function,
//         // then we try to traverse those missing spots as switch cases.
//         if ins_iters_with_gpr_state.is_empty() {
//             let unseen_addrs = (func_address as u32..func_address as u32 + code.len() as u32)
//                 .step_by(4)
//                 .filter(|addr| !visited_ins_addrs.contains(addr));
//             for unseen_addr in unseen_addrs {
//                 let prev_bctr_gpr_state = gpr_state_at_bctr
//                     .iter()
//                     .filter(|(&addr, _)| addr < unseen_addr)
//                     .min_by_key(|(&addr, _)| addr)
//                     .map(|(_, gpr_state)| gpr_state);
//                 if let Some(gpr_pool_relocs) = prev_bctr_gpr_state {
//                     let dest_offset_into_func = unseen_addr - func_address as u32;
//                     let dest_code_slice = &code[dest_offset_into_func as usize..];
//                     ins_iters_with_gpr_state.push((
//                         InsIter::new(dest_code_slice, unseen_addr),
//                         gpr_pool_relocs.clone(),
//                     ));
//                     break;
//                 }
//             }
//         }
//     }
//
//     pool_reloc_for_addr
// }

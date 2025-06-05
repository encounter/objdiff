use alloc::{
    collections::{BTreeMap, BTreeSet},
    string::{String, ToString},
    vec,
    vec::Vec,
};

use itertools::Itertools;
use std::ops::{Index, IndexMut};
use anyhow::{Result, bail, ensure};
use cwextab::{ExceptionTableData, decode_extab};
use flagset::Flags;
use object::{Object as _, ObjectSection as _, ObjectSymbol as _, elf};

use crate::{
    arch::{Arch, DataType},
    diff::{
        DiffObjConfig,
        data::resolve_relocation,
        display::{ContextItem, HoverItem, HoverItemColor, InstructionPart, SymbolNavigationKind},
    },
    obj::{
        FlowAnalysisResult, FlowAnalysisValue, InstructionRef, Object, Relocation, RelocationFlags,
        ResolvedInstructionRef, ResolvedRelocation, Symbol, SymbolFlag, SymbolFlagSet
    },
    util::{RawFloat, RawDouble},
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

fn is_store_instruction(op: ppc750cl::Opcode) -> bool {
    use ppc750cl::Opcode;
    match op {
        Opcode::Stbux | Opcode::Stbx | Opcode::Stfdux | Opcode::Stfdx | Opcode::Stfiwx |
        Opcode::Stfsux | Opcode::Stfsx | Opcode::Sthbrx | Opcode::Sthux | Opcode::Sthx |
        Opcode::Stswi | Opcode::Stswx | Opcode::Stwbrx | Opcode::Stwcx_ | Opcode::Stwux |
        Opcode::Stwx | Opcode::Stwu | Opcode::Stb | Opcode::Stbu | Opcode::Sth | Opcode::Sthu |
        Opcode::Stmw | Opcode::Stfs | Opcode::Stfsu | Opcode::Stfd | Opcode::Stfdu => true,
        _ => false,
    }
}

#[derive(Debug)]
pub struct ArchPpc {
    /// Exception info
    pub extab: Option<BTreeMap<usize, ExceptionInfo>>,
}

impl ArchPpc {
    pub fn new(file: &object::File) -> Result<Self> {
        Ok(Self { extab: decode_exception_info(file)? })
    }

    fn parse_ins_ref(&self, resolved: ResolvedInstructionRef) -> Result<ppc750cl::Ins> {
        let mut code = u32::from_be_bytes(resolved.code.try_into()?);
        if let Some(reloc) = resolved.relocation {
            code = zero_reloc(code, reloc.relocation);
        }
        let op = ppc750cl::Opcode::from(resolved.ins_ref.opcode as u8);
        Ok(ppc750cl::Ins { code, op })
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
    fn scan_instructions_internal(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        _relocations: &[Relocation],
        _diff_config: &DiffObjConfig,
    ) -> Result<Vec<InstructionRef>> {
        ensure!(code.len() & 3 == 0, "Code length must be a multiple of 4");
        let ins_count = code.len() / 4;
        let mut insts = Vec::<InstructionRef>::with_capacity(ins_count);
        for (cur_addr, ins) in ppc750cl::InsIter::new(code, address as u32) {
            insts.push(InstructionRef {
                address: cur_addr as u64,
                size: 4,
                opcode: u8::from(ins.op) as u16,
                branch_dest: ins.branch_dest(cur_addr).map(u64::from),
            });
        }
        Ok(insts)
    }

    fn display_instruction(
        &self,
        resolved: ResolvedInstructionRef,
        _diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        let ins = self.parse_ins_ref(resolved)?.simplified();

        cb(InstructionPart::opcode(ins.mnemonic, resolved.ins_ref.opcode))?;

        let reloc_arg = self.find_reloc_arg(&ins, resolved.relocation);

        let mut writing_offset = false;
        for (idx, arg) in ins.args_iter().enumerate() {
            if idx > 0 && !writing_offset {
                cb(InstructionPart::separator())?;
            }

            if reloc_arg == Some(idx) {
                let reloc = resolved.relocation.unwrap();
                display_reloc(reloc, cb)?;
                // For @sda21, we can omit the register argument
                if matches!(reloc.relocation.flags, RelocationFlags::Elf(elf::R_PPC_EMB_SDA21))
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
                        (resolved.ins_ref.address as u32).wrapping_add_signed(dest.0),
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

    // Could be replaced by data_flow_analysis once that feature stabilizes
    fn generate_pooled_relocations(
        &self,
        address: u64,
        code: &[u8],
        relocations: &[Relocation],
        symbols: &[Symbol],
    ) -> Vec<Relocation> {
        generate_fake_pool_relocations_for_function(address, code, relocations, symbols)
    }
    
    fn data_flow_analysis(
        &self,
        obj: &Object,
        symbol: &Symbol,
        code: &[u8],
        relocations: &[Relocation],
    ) -> Option<Box<dyn FlowAnalysisResult>> {
        Some(ppc_data_flow_analysis(obj, symbol, code, relocations))
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

    fn reloc_name(&self, flags: RelocationFlags) -> Option<&'static str> {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                elf::R_PPC_NONE => Some("R_PPC_NONE"), // We use this for fake pool relocs
                elf::R_PPC_ADDR16_LO => Some("R_PPC_ADDR16_LO"),
                elf::R_PPC_ADDR16_HI => Some("R_PPC_ADDR16_HI"),
                elf::R_PPC_ADDR16_HA => Some("R_PPC_ADDR16_HA"),
                elf::R_PPC_EMB_SDA21 => Some("R_PPC_EMB_SDA21"),
                elf::R_PPC_ADDR32 => Some("R_PPC_ADDR32"),
                elf::R_PPC_UADDR32 => Some("R_PPC_UADDR32"),
                elf::R_PPC_REL24 => Some("R_PPC_REL24"),
                elf::R_PPC_REL14 => Some("R_PPC_REL14"),
                _ => None,
            },
            _ => None,
        }
    }

    fn data_reloc_size(&self, flags: RelocationFlags) -> usize {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                elf::R_PPC_ADDR32 => 4,
                elf::R_PPC_UADDR32 => 4,
                _ => 1,
            },
            _ => 1,
        }
    }

    fn extra_symbol_flags(&self, symbol: &object::Symbol) -> SymbolFlagSet {
        if self.extab.as_ref().is_some_and(|extab| extab.contains_key(&(symbol.index().0 - 1))) {
            SymbolFlag::HasExtra.into()
        } else {
            SymbolFlag::none()
        }
    }

    fn guess_data_type(&self, resolved: ResolvedInstructionRef, bytes: &[u8]) -> Option<DataType> {
        if resolved.relocation.is_some_and(|r| r.symbol.name.starts_with("@stringBase")) {
            // Pooled string.
            return Some(DataType::String);
        }
        let opcode = ppc750cl::Opcode::from(resolved.ins_ref.opcode as u8);
        if let Some(ty) = guess_data_type_from_load_store_inst_op(opcode) {
            // Numeric type.
            return Some(ty);
        }
        if bytes.len() >= 2 && bytes.iter().position(|&c| c == b'\0') == Some(bytes.len() - 1) {
            // It may be an unpooled string if the symbol contains exactly one null byte at the end of the symbol.
            return Some(DataType::String);
        }
        None
    }

    fn symbol_hover(&self, _obj: &Object, symbol_index: usize) -> Vec<HoverItem> {
        let mut out = Vec::new();
        if let Some(extab) = self.extab_for_symbol(symbol_index) {
            out.push(HoverItem::Text {
                label: "extab symbol".into(),
                value: extab.etb_symbol.name.clone(),
                color: HoverItemColor::Special,
            });
            out.push(HoverItem::Text {
                label: "extabindex symbol".into(),
                value: extab.eti_symbol.name.clone(),
                color: HoverItemColor::Special,
            });
        }
        out
    }

    fn symbol_context(&self, _obj: &Object, symbol_index: usize) -> Vec<ContextItem> {
        let mut out = Vec::new();
        if let Some(_extab) = self.extab_for_symbol(symbol_index) {
            out.push(ContextItem::Navigate {
                label: "Decode exception table".to_string(),
                symbol_index,
                kind: SymbolNavigationKind::Extab,
            });
        }
        out
    }

    fn instruction_hover(&self, _obj: &Object, resolved: ResolvedInstructionRef) -> Vec<HoverItem> {
        let Ok(ins) = self.parse_ins_ref(resolved) else {
            return Vec::new();
        };
        let orig = ins.basic().to_string();
        let simplified = ins.simplified().to_string();
        let show_orig = orig != simplified;
        let rlwinm_decoded = rlwinmdec::decode(&orig);
        let mut out = Vec::with_capacity(2);
        if show_orig {
            out.push(HoverItem::Text {
                label: "Original".into(),
                value: orig,
                color: HoverItemColor::Normal,
            });
        }
        if let Some(decoded) = rlwinm_decoded {
            for line in decoded.lines() {
                out.push(HoverItem::Text {
                    label: Default::default(),
                    value: line.to_string(),
                    color: HoverItemColor::Special,
                });
            }
        }
        out
    }

    fn instruction_context(
        &self,
        _obj: &Object,
        resolved: ResolvedInstructionRef,
    ) -> Vec<ContextItem> {
        let Ok(ins) = self.parse_ins_ref(resolved) else {
            return Vec::new();
        };
        let orig = ins.basic().to_string();
        let simplified = ins.simplified().to_string();
        let show_orig = orig != simplified;
        let mut out = Vec::with_capacity(2);
        out.push(ContextItem::Copy { value: simplified, label: None });
        if show_orig {
            out.push(ContextItem::Copy { value: orig, label: Some("original".to_string()) });
        }
        out
    }
}

impl ArchPpc {
    pub fn extab_for_symbol(&self, symbol_index: usize) -> Option<&ExceptionInfo> {
        self.extab.as_ref()?.get(&symbol_index)
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
                    e
                );
                return Ok(None);
            }
        };

        //Add the new entry to the list
        result.insert(extab_func.index().0 - 1, ExceptionInfo {
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
    Ok(ExtabSymbolRef { original_index: symbol.index().0 - 1, name, demangled_name })
}

#[derive(Default, PartialEq, Eq, Copy, Hash, Clone, Debug)]
enum RegisterContent {
    #[default]
    Unknown,
    Variable, // Multiple potential values
    FloatConstant(RawFloat),
    DoubleConstant(RawDouble),
    IntConstant(i32),
    InputRegister(u8),
    Symbol(usize),
}

impl std::fmt::Display for RegisterContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegisterContent::Unknown => write!(f, "unknown"),
            RegisterContent::Variable => write!(f, "variable"),
            RegisterContent::IntConstant(i) =>
                // -i is safe because it's at most a 16 bit constant in the i32
                if *i >= 0 { write!(f, "0x{:x}", i) } else { write!(f, "-0x{:x}", -i) },
            RegisterContent::FloatConstant(RawFloat(fp)) => write!(f, "{fp:?}f"),
            RegisterContent::DoubleConstant(RawDouble(fp)) => write!(f, "{fp:?}d"),
            RegisterContent::InputRegister(p) => write!(f, "input{p}"),
            RegisterContent::Symbol(_u) => write!(f, "relocation"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct RegisterState {
    gpr: [RegisterContent; 32],
    fpr: [RegisterContent; 32],
}

impl RegisterState {
    fn new() -> Self {
        RegisterState {
            gpr: [RegisterContent::Unknown; 32],
            fpr: [RegisterContent::Unknown; 32],
        }
    }

    // During a function call, these registers must be assumed trashed.
    fn clear_volatile(&mut self) {
        self[ppc750cl::GPR(0)] = RegisterContent::Unknown;
        for i in 0..=13 {
            self[ppc750cl::GPR(i)] = RegisterContent::Unknown;
        }
        for i in 0..=13 {
            self[ppc750cl::FPR(i)] = RegisterContent::Unknown;
        }
    }

    // Mark potential input values.
    // Subsequent flow analysis will "realize" that they are not actually inputs if
    // they get overwritten with another value before getting read.
    fn set_potential_inputs(&mut self) {
        for g_reg in 3..=13 {
            self[ppc750cl::GPR(g_reg)] = RegisterContent::InputRegister(g_reg);
        }
        for f_reg in 1..=13 {
            self[ppc750cl::FPR(f_reg)] = RegisterContent::InputRegister(f_reg);
        }
    }

    // If the there is no value, we can take the new known value.
    // If there's a known value different than the new value, the content
    // must is variable.
    // Returns whether the current value was updated.
    fn unify_values(current: &mut RegisterContent, new: &RegisterContent) -> bool {
        if *current == *new {
            false
        } else {
            if *current == RegisterContent::Unknown {
                *current = *new;
                true
            } else if *current == RegisterContent::Variable {
                // Already variable
                false
            } else {
                *current = RegisterContent::Variable;
                true
            }
        }
    }

    // Unify currently known register contents in a give situation with new
    // information about the register contents in that situation.
    // Currently unknown register contents can be filled, but if there are
    // conflicting contents, we go back to unknown.
    fn unify(&mut self, other: &RegisterState) -> bool {
        let mut updated = false;
        for i in 0..32 {
            updated |= Self::unify_values(&mut self.gpr[i], &other.gpr[i]);
            updated |= Self::unify_values(&mut self.fpr[i], &other.fpr[i]);
        }
        updated
    }
}

impl Index<ppc750cl::GPR> for RegisterState {
    type Output = RegisterContent;
    fn index(&self, gpr: ppc750cl::GPR) -> &Self::Output {
        &self.gpr[gpr.0 as usize]
    }
}
impl IndexMut<ppc750cl::GPR> for RegisterState {
    fn index_mut(&mut self, gpr: ppc750cl::GPR) -> &mut Self::Output {
        &mut self.gpr[gpr.0 as usize]
    }
}

impl Index<ppc750cl::FPR> for RegisterState {
    type Output = RegisterContent;
    fn index(&self, fpr: ppc750cl::FPR) -> &Self::Output {
        &self.fpr[fpr.0 as usize]
    }
}
impl IndexMut<ppc750cl::FPR> for RegisterState {
    fn index_mut(&mut self, fpr: ppc750cl::FPR) -> &mut Self::Output {
        &mut self.fpr[fpr.0 as usize]
    }
}

fn execute_instruction(registers: &mut RegisterState, op: &ppc750cl::Opcode, args: &[ppc750cl::Argument; 5]) {
    use ppc750cl::{Opcode, Argument, GPR};
    match (op, args[0], args[1], args[2]) {
        (Opcode::Or, Argument::GPR(a), Argument::GPR(b), Argument::GPR(c)) => {
            // Move is implemented as or with self for ints
            if b == c {
                registers[a] = registers[b];
            } else {
                registers[a] = RegisterContent::Unknown;
            }
        }
        (Opcode::Fmr, Argument::FPR(a), Argument::FPR(b), _) => {
            registers[a] = registers[b];
        }
        (Opcode::Addi, Argument::GPR(a), Argument::GPR(GPR(0)), Argument::Simm(c)) => {
            // Load immidiate implemented as addi with addend = r0
            // Let Addi with other addends fall through to the case which
            // overwrites the destination
            registers[a] = RegisterContent::IntConstant(c.0 as i32);
        }
        (Opcode::Bcctr, _, _, _) => {
            // Called a function pointer, may have erased volatile registers
            registers.clear_volatile();
        }
        (Opcode::B, _, _, _) => {
            if get_branch_offset(args) == 0 {
                // Call to another function
                registers.clear_volatile();
            }
        }
        (Opcode::Stbu | Opcode::Sthu | Opcode::Stwu |
            Opcode::Stfsu | Opcode::Stfdu, _, _, Argument::GPR(rel)) => {
            // Storing with update, clear updated register (third arg)
            registers[rel] = RegisterContent::Unknown;
        }
        (Opcode::Stbux | Opcode::Sthux | Opcode::Stwux |
            Opcode::Stfsux | Opcode::Stfdux, _, Argument::GPR(rel), _) => {
            // Storing indexed with update, clear updated register (second arg)
            registers[rel] = RegisterContent::Unknown;
        }
        (Opcode::Stb | Opcode::Sth | Opcode::Stw |
            Opcode::Stbx | Opcode::Sthx | Opcode::Stwx |
            Opcode::Stfs | Opcode::Stfd, _, _, _) => {
            // Storing, does not change registers
        }
        (Opcode::Lmw, Argument::GPR(target), _, _) => {
            // `lmw` overwrites all registers from rd to r31.
            for reg in target.0..31 {
                registers[GPR(reg)] = RegisterContent::Unknown;
            }
        }
        (_, Argument::GPR(a), _, _) => {
            // Other operations which write to GPR a
            registers[a] = RegisterContent::Unknown;
        }
        (_, Argument::FPR(a), _, _) => {
            // Other operations which write to FPR a
            registers[a] = RegisterContent::Unknown;
        }
        (_, _, _, _) => {}
    }
    
}

fn get_branch_offset(args: &[ppc750cl::Argument; 5]) -> i32 {
    for arg in args.iter() {
        if let ppc750cl::Argument::BranchDest(dest) = arg {
            return dest.0 / 4;
        }
    }
    return 0;
}

#[derive(Debug, Default)]
struct PPCFlowAnalysisResult {
    argument_contents: BTreeMap<(u64, u8), FlowAnalysisValue>,
}

impl PPCFlowAnalysisResult {
    fn set_argument_value_at_address(&mut self, address: u64, argument: u8, value: FlowAnalysisValue) {
        self.argument_contents.insert((address, argument), value);
    }

    fn new() -> Self {
        PPCFlowAnalysisResult { argument_contents: Default::default() }
    }
}

impl FlowAnalysisResult for PPCFlowAnalysisResult {
    fn get_argument_value_at_address(&self, address: u64, argument: u8) -> Option<&FlowAnalysisValue> {
        self.argument_contents.get(&(address, argument))
    }
}

fn clamp_text_length(s: String, max: usize) -> String {
    if s.len() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max - 3).collect::<String>())
    }
}

// Executing op with args at cur_address, update current_state with symbols that
// come from relocations. That is, references to globals, floating point
// constants, string constants, etc.
fn fill_registers_from_relocations(
    current_state: &mut RegisterState,
    obj: &Object,
    cur_addr: u32,
    op: ppc750cl::Opcode,
    args: &[ppc750cl::Argument; 5],
    relocations: &[Relocation],
) {
    let reloc = relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);
    if let Some(reloc) = reloc {
        let bytes = obj.symbol_data(reloc.target_symbol).unwrap_or(&[]);
        let content = match guess_data_type_from_load_store_inst_op(op) {
            Some(DataType::Float) => RegisterContent::FloatConstant(RawFloat(match obj.endianness {
                object::Endianness::Little => f32::from_le_bytes(bytes.try_into().unwrap_or([0; 4])),
                object::Endianness::Big => f32::from_be_bytes(bytes.try_into().unwrap_or([0; 4])),
            })),
            Some(DataType::Double) => RegisterContent::DoubleConstant(RawDouble(match obj.endianness {
                object::Endianness::Little => f64::from_le_bytes(bytes.try_into().unwrap_or([0; 8])),
                object::Endianness::Big => f64::from_be_bytes(bytes.try_into().unwrap_or([0; 8])),
            })),
            _ => RegisterContent::Symbol(reloc.target_symbol),
        };
        // Only update the register state for loads. We may store to a reloc
        // address but that doesn't update register contents.
        if !is_store_instruction(op) {
            match (op, args[0]) {
                // Everything else is a load of some sort
                (_, ppc750cl::Argument::GPR(gpr)) => {
                    current_state[gpr] = content;
                }
                (_, ppc750cl::Argument::FPR(fpr)) => {
                    current_state[fpr] = content;
                }
                _ => {}
            }
        }
    }
}

fn ppc_data_flow_analysis(
    obj: &Object,
    func_symbol: &Symbol,
    code: &[u8],
    relocations: &[Relocation],
) -> Box<PPCFlowAnalysisResult> {
    use std::collections::HashSet;
    use ppc750cl::InsIter;
    use std::collections::VecDeque;
    let instructions = InsIter::new(code, func_symbol.address as u32).map(|(_addr, ins)| {
        (ins.op, ins.basic().args)
    }).collect_vec();

    let func_address = func_symbol.address;

    // Get initial register values from function parameters
    let mut initial_register_state = RegisterState::new();
    initial_register_state.set_potential_inputs();

    let mut execution_queue = VecDeque::<(usize, RegisterState)>::new();
    execution_queue.push_back((0, initial_register_state));

    // Execute the instructions against abstract data
    let mut failsafe_counter = 0;
    let mut taken_branches = HashSet::<(usize, RegisterState)>::new();
    let mut register_state_at = Vec::<RegisterState>::new();
    register_state_at.resize_with(instructions.len(), RegisterState::new);
    while let Some((mut index, mut current_state)) = execution_queue.pop_front() {
        while let Some((op, args)) = instructions.get(index) {
            // Record the state at this index
            // If recording does not result in any changes to the known values
            // we're done, because the subsequent values are a function of the
            // current values so we'll get the same result as the last time
            // we went down this path.
            if !register_state_at[index].unify(&current_state) {
                break;
            }

            // Execute the instruction to update the state
            execute_instruction(&mut current_state, op, args);

            // Fill in register state coming from relocations at this line. This
            // handles references to global variables, floating point constants,
            // etc.
            let cur_addr = (func_address as u32) + ((index * 4) as u32);
            fill_registers_from_relocations(&mut current_state, obj, cur_addr, *op, args, relocations);
            
            // Add conditional branches to execution queue
            // Only take a given (address, register state) combination once. If
            // the known register state is different we have to take the branch
            // again to stabilize the known values for backwards branches.
            if op == &ppc750cl::Opcode::Bc {
                let branch_state = (index, current_state.clone());
                if !taken_branches.contains(&branch_state) {
                    let offset = get_branch_offset(args);
                    let target_index = ((index as i32) + offset) as usize;
                    execution_queue.push_back((target_index, current_state.clone()));
                    taken_branches.insert(branch_state);

                    // We should never hit this case, but avoid getting stuck in
                    // an infinite loop if we hit some kind of bad behavior.
                    failsafe_counter += 1;
                    if failsafe_counter > 256 {
                        println!("Analysis of {} failed to stabilize", func_symbol.name);
                        return Default::default();
                    }
                }
            }

            // Update index
            if op == &ppc750cl::Opcode::B {
                // Unconditional branch
                let offset = get_branch_offset(args);
                if offset > 0 {
                    // Jump table or branch to over else clause.
                    index += offset as usize;
                } else if offset == 0 {
                    // Function call with relocation. We'll return to
                    // the next instruction.
                    index += 1;
                } else {
                    // Unconditional branch (E.g.: loop { ... })
                    // Also some compilations of loops put the conditional at
                    // the end and B to it for the check of the first iteration.
                    let branch_state = (index, current_state.clone());
                    if taken_branches.contains(&branch_state) {
                        break;
                    }
                    taken_branches.insert(branch_state);
                    index = ((index as i32) + offset) as usize;
                }
            } else {
                // Normal execution of next instruction
                index += 1;
            }
        }
    }

    // Store the relevant data flow values for simplified instructions
    generate_flow_analysis_result(&obj, func_address, code, register_state_at, relocations)
}

// Write the relevant part of the flow analysis out into the FlowAnalysisResult
// the rest of the application will use to query results of the flow analysis.
// Flow analysis will compute the known contents of every register at every
// line, but we only need to record the values of registers that are actually
// referenced at each line.
fn generate_flow_analysis_result(
    obj: &Object,
    base_address: u64,
    code: &[u8],
    register_state_at: Vec::<RegisterState>,
    relocations: &[Relocation]
) -> Box<PPCFlowAnalysisResult> {
    use ppc750cl::{InsIter, Argument, Offset, GPR};
    let mut analysis_result = PPCFlowAnalysisResult::new();
    for (addr, ins) in InsIter::new(code, 0) {
        let ins_address = base_address + (addr as u64);
        let index = addr / 4;
        let ppc750cl::ParsedIns {mnemonic, args} = ins.simplified();

        // Special case to show float and double constants on the line where
        // they are being loaded.
        // We need to do this before we break out on showing relocations in the
        // subsequent if statement.
        if ins.op == ppc750cl::Opcode::Lfs || ins.op == ppc750cl::Opcode::Lfd {
            // The value is set on the line AFTER the load, get it from there
            if let Some(next_state) = register_state_at.get(index as usize + 1) {
                // When loading from SDA it will be a relocation so Reg+Offset will both be zero
                match (args[0], args[1], args[2]) {
                    (Argument::FPR(fpr), Argument::Offset(Offset(0)), Argument::GPR(GPR(0))) => {
                        analysis_result.set_argument_value_at_address(ins_address, 1,
                            FlowAnalysisValue::Text(format!("{}", next_state[fpr])));
                        continue;
                    }
                    _ => {}
                }
            }
        }

        // If we're already showing relocations on a line don't also show data flow
        if relocations.iter().any(|r| (r.address & !3) == ins_address) {
            continue;
        }

        let is_store = mnemonic.starts_with("st");
        let default_register_state = RegisterState::new();
        let registers = register_state_at.get(index as usize).unwrap_or(&default_register_state);
        for (arg_index, arg) in args.into_iter().enumerate() {
            // Hacky shorthand for determining which arguments are sources,
            // We only want to show data flow for source registers, not target
            // registers. Technically there are some non-"st_" operations which
            // read from their first argument but they're rare.
            if (arg_index == 0) && !is_store {
                continue;
            }
            
            let content = match arg {
                Argument::GPR(gpr) => Some(registers[gpr]),
                Argument::FPR(fpr) => Some(registers[fpr]),
                _ => None,
            };
            let analysis_value = match content {
                Some(RegisterContent::Symbol(s)) => {
                    obj.symbols.get(s).map(|sym|
                        FlowAnalysisValue::Text(
                            clamp_text_length(sym.demangled_name.as_ref().unwrap_or(&sym.name).clone(), 20)))
                }
                Some(RegisterContent::InputRegister(reg)) => {
                    let reg_name = match arg {
                        Argument::GPR(_) => format!("input_r{reg}"),
                        Argument::FPR(_) => format!("input_f{reg}"),
                        _ => panic!("Register content should only be in a register"),
                    };
                    Some(FlowAnalysisValue::Text(reg_name))
                }
                Some(RegisterContent::Unknown) | Some(RegisterContent::Variable) => None,
                Some(value) => Some(FlowAnalysisValue::Text(format!("{value}"))),
                None => None,
            };
            if let Some(analysis_value) = analysis_value {
                analysis_result.set_argument_value_at_address(ins_address, arg_index as u8, analysis_value);
            }
        }
    }

    Box::new(analysis_result)
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

#[derive(Debug)]
struct PoolReference {
    addr_src_gpr: ppc750cl::GPR,
    addr_offset: i16,
    addr_dst_gpr: Option<ppc750cl::GPR>,
}

// Given an instruction, check if it could be accessing pooled data at the address in a register.
// If so, return information pertaining to where the instruction is getting that address from and
// what it's doing with the address (e.g. copying it into another register, adding an offset, etc).
fn get_pool_reference_for_inst(
    opcode: ppc750cl::Opcode,
    simplified: &ppc750cl::ParsedIns,
) -> Option<PoolReference> {
    use ppc750cl::{Argument, Opcode};
    let args = &simplified.args;
    if guess_data_type_from_load_store_inst_op(opcode).is_some() {
        match (args[1], args[2]) {
            (Argument::Offset(offset), Argument::GPR(addr_src_gpr)) => {
                // e.g. lwz. Immediate offset.
                Some(PoolReference { addr_src_gpr, addr_offset: offset.0, addr_dst_gpr: None })
            }
            (Argument::GPR(addr_src_gpr), Argument::GPR(_offset_gpr)) => {
                // e.g. lwzx. The offset is in a register and was likely calculated from an index.
                // Treat the offset as being 0 in this case to show the first element of the array.
                // It may be possible to show all elements by figuring out the stride of the array
                // from the calculations performed on the index before it's put into offset_gpr, but
                // this would be much more complicated, so it's not currently done.
                Some(PoolReference { addr_src_gpr, addr_offset: 0, addr_dst_gpr: None })
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
            ) => Some(PoolReference {
                addr_src_gpr,
                addr_offset: simm.0,
                addr_dst_gpr: Some(addr_dst_gpr),
            }),
            (
                // `mr` or `mr.`
                Opcode::Or,
                Argument::GPR(addr_dst_gpr),
                Argument::GPR(addr_src_gpr),
                Argument::None,
            ) => Some(PoolReference {
                addr_src_gpr,
                addr_offset: 0,
                addr_dst_gpr: Some(addr_dst_gpr),
            }),
            (
                Opcode::Add,
                Argument::GPR(addr_dst_gpr),
                Argument::GPR(addr_src_gpr),
                Argument::GPR(_offset_gpr),
            ) => Some(PoolReference {
                addr_src_gpr,
                addr_offset: 0,
                addr_dst_gpr: Some(addr_dst_gpr),
            }),
            _ => None,
        }
    }
}

// Remove the relocation we're keeping track of in a particular register when an instruction reuses
// that register to hold some other value, unrelated to pool relocation addresses.
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

// We create a fake relocation for an instruction, vaguely simulating what the actual relocation
// might have looked like if it wasn't pooled. This is so minimal changes are needed to display
// pooled accesses vs non-pooled accesses. We set the relocation type to R_PPC_NONE to indicate that
// there isn't really a relocation here, as copying the pool relocation's type wouldn't make sense.
// Also, if this instruction is accessing the middle of a symbol instead of the start, we add an
// addend to indicate that.
fn make_fake_pool_reloc(
    offset: i16,
    cur_addr: u32,
    pool_reloc: &Relocation,
    symbols: &[Symbol],
) -> Option<Relocation> {
    let pool_reloc = resolve_relocation(symbols, pool_reloc);
    let offset_from_pool = pool_reloc.relocation.addend + offset as i64;
    let target_address = pool_reloc.symbol.address.checked_add_signed(offset_from_pool)?;
    let target_symbol;
    let addend;
    if let Some(section_index) = pool_reloc.symbol.section {
        // Find the exact data symbol within the pool being accessed here based on the address.
        target_symbol = symbols.iter().position(|s| {
            s.section == Some(section_index)
                && s.size > 0
                && (s.address..s.address + s.size).contains(&target_address)
        })?;
        addend = target_address.checked_sub(symbols[target_symbol].address)? as i64;
    } else {
        // If the target symbol is in a different object (extern), we simply copy the pool
        // relocation's target. This is because it's not possible to locate the actual symbol if
        // it's extern. And doing that for external symbols would also be unnecessary, because when
        // the compiler generates an instruction that accesses an external "pool" plus some offset,
        // that won't be a normal pool that contains other symbols within it that we want to
        // display. It will be something like a vtable for a class with multiple inheritance (for
        // example, dCcD_Cyl in The Wind Waker). So just showing that vtable symbol plus an addend
        // to represent the offset into it works fine in this case.
        target_symbol = pool_reloc.relocation.target_symbol;
        addend = offset_from_pool;
    }
    Some(Relocation {
        flags: RelocationFlags::Elf(elf::R_PPC_NONE),
        address: cur_addr as u64,
        target_symbol,
        addend,
    })
}

// Searches through all instructions in a function, determining which registers have the addresses
// of pooled data relocations in them, finding which instructions load data from those addresses,
// and returns a Vec of "fake pool relocations" that simulate what a relocation for that instruction
// would look like if data hadn't been pooled.
// This method tries to follow the function's proper control flow. It keeps track of a queue of
// states it hasn't traversed yet, where each state holds an instruction address and a HashMap of
// which registers hold which pool relocations at that point.
// When a conditional or unconditional branch is encountered, the destination of the branch is added
// to the queue. Conditional branches will traverse both the path where the branch is taken and the
// one where it's not. Unconditional branches only follow the branch, ignoring any code immediately
// after the branch instruction.
// Limitations: This method does not currently read switch statement jump tables.
// Instead, we guess that any parts of a function we missed were switch cases, and traverse them as
// if the last `bctr` before that address had branched there. This should be fairly accurate in
// practice - in testing the only instructions it seems to miss are double branches that the
// compiler generates in error which can never be reached during normal execution anyway.
// It should be possible to implement jump tables properly by reading them out of .data. But this
// will require keeping track of what value is loaded into each register so we can retrieve the jump
// table symbol when we encounter a `bctr`.
fn generate_fake_pool_relocations_for_function(
    func_address: u64,
    code: &[u8],
    relocations: &[Relocation],
    symbols: &[Symbol],
) -> Vec<Relocation> {
    use ppc750cl::{Argument, InsIter, Opcode};
    let mut visited_ins_addrs = BTreeSet::new();
    let mut pool_reloc_for_addr = BTreeMap::new();
    let mut ins_iters_with_gpr_state =
        vec![(InsIter::new(code, func_address as u32), BTreeMap::new())];
    let mut gpr_state_at_bctr = BTreeMap::new();
    while let Some((ins_iter, mut gpr_pool_relocs)) = ins_iters_with_gpr_state.pop() {
        for (cur_addr, ins) in ins_iter {
            if visited_ins_addrs.contains(&cur_addr) {
                // Avoid getting stuck in an infinite loop when following looping branches.
                break;
            }
            visited_ins_addrs.insert(cur_addr);

            let simplified = ins.simplified();

            // First handle traversing the function's control flow.
            let mut branch_dest = None;
            for arg in simplified.args_iter() {
                if let Argument::BranchDest(dest) = arg {
                    let dest = cur_addr.wrapping_add_signed(dest.0);
                    branch_dest = Some(dest);
                    break;
                }
            }
            if let Some(branch_dest) = branch_dest {
                if branch_dest >= func_address as u32
                    && (branch_dest - func_address as u32) < code.len() as u32
                {
                    let dest_offset_into_func = branch_dest - func_address as u32;
                    let dest_code_slice = &code[dest_offset_into_func as usize..];
                    match ins.op {
                        Opcode::Bc => {
                            // Conditional branch.
                            // Add the branch destination to the queue to do later.
                            ins_iters_with_gpr_state.push((
                                InsIter::new(dest_code_slice, branch_dest),
                                gpr_pool_relocs.clone(),
                            ));
                            // Then continue on with the current iterator.
                        }
                        Opcode::B => {
                            if simplified.mnemonic != "bl" {
                                // Unconditional branch.
                                // Add the branch destination to the queue.
                                ins_iters_with_gpr_state.push((
                                    InsIter::new(dest_code_slice, branch_dest),
                                    gpr_pool_relocs.clone(),
                                ));
                                // Break out of the current iterator so we can do the newly added one.
                                break;
                            }
                        }
                        _ => unreachable!(),
                    }
                }
            }
            if let Opcode::Bcctr = ins.op {
                if simplified.mnemonic == "bctr" {
                    // Unconditional branch to count register.
                    // Likely a jump table.
                    gpr_state_at_bctr.insert(cur_addr, gpr_pool_relocs.clone());
                }
            }

            // Then handle keeping track of which GPR contains which pool relocation.
            let reloc = relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);
            if let Some(reloc) = reloc {
                // This instruction has a real relocation, so it may be a pool load we want to keep
                // track of.
                let args = &simplified.args;
                match (ins.op, args[0], args[1], args[2]) {
                    (
                        // `lis` + `addi`
                        Opcode::Addi,
                        Argument::GPR(addr_dst_gpr),
                        Argument::GPR(_addr_src_gpr),
                        Argument::Simm(_simm),
                    ) => {
                        gpr_pool_relocs.insert(addr_dst_gpr.0, reloc.clone());
                    }
                    (
                        // `lis` + `ori`
                        Opcode::Ori,
                        Argument::GPR(addr_dst_gpr),
                        Argument::GPR(_addr_src_gpr),
                        Argument::Uimm(_uimm),
                    ) => {
                        gpr_pool_relocs.insert(addr_dst_gpr.0, reloc.clone());
                    }
                    (Opcode::B, _, _, _) => {
                        if simplified.mnemonic == "bl" {
                            // When encountering a function call, clear any active pool relocations from
                            // the volatile registers (r0, r3-r12), but not the nonvolatile registers.
                            gpr_pool_relocs.remove(&0);
                            for gpr in 3..12 {
                                gpr_pool_relocs.remove(&gpr);
                            }
                        }
                    }
                    _ => {
                        clear_overwritten_gprs(ins, &mut gpr_pool_relocs);
                    }
                }
            } else if let Some(pool_ref) = get_pool_reference_for_inst(ins.op, &simplified) {
                // This instruction doesn't have a real relocation, so it may be a reference to one of
                // the already-loaded pools.
                if let Some(pool_reloc) = gpr_pool_relocs.get(&pool_ref.addr_src_gpr.0) {
                    if let Some(fake_pool_reloc) =
                        make_fake_pool_reloc(pool_ref.addr_offset, cur_addr, pool_reloc, symbols)
                    {
                        pool_reloc_for_addr.insert(cur_addr, fake_pool_reloc);
                    }
                    if let Some(addr_dst_gpr) = pool_ref.addr_dst_gpr {
                        // If the address of the pool relocation got copied into another register, we
                        // need to keep track of it in that register too as future instructions may
                        // reference the symbol indirectly via this new register, instead of the
                        // register the symbol's address was originally loaded into.
                        // For example, the start of the function might `lis` + `addi` the start of the
                        // ...data pool into r25, and then later the start of a loop will `addi` r25
                        // with the offset within the .data section of an array variable into r21.
                        // Then the body of the loop will `lwzx` one of the array elements from r21.
                        let mut new_reloc = pool_reloc.clone();
                        new_reloc.addend += pool_ref.addr_offset as i64;
                        gpr_pool_relocs.insert(addr_dst_gpr.0, new_reloc);
                    } else {
                        clear_overwritten_gprs(ins, &mut gpr_pool_relocs);
                    }
                } else {
                    clear_overwritten_gprs(ins, &mut gpr_pool_relocs);
                }
            } else {
                clear_overwritten_gprs(ins, &mut gpr_pool_relocs);
            }
        }

        // Finally, if we're about to finish the outer loop and don't have any more control flow to
        // follow, we check if there are any instruction addresses in this function that we missed.
        // If so, and if there were any `bctr` instructions before those points in this function,
        // then we try to traverse those missing spots as switch cases.
        if ins_iters_with_gpr_state.is_empty() {
            let unseen_addrs = (func_address as u32..func_address as u32 + code.len() as u32)
                .step_by(4)
                .filter(|addr| !visited_ins_addrs.contains(addr));
            for unseen_addr in unseen_addrs {
                let prev_bctr_gpr_state = gpr_state_at_bctr
                    .iter()
                    .filter(|&(&addr, _)| addr < unseen_addr)
                    .min_by_key(|&(&addr, _)| addr)
                    .map(|(_, gpr_state)| gpr_state);
                if let Some(gpr_pool_relocs) = prev_bctr_gpr_state {
                    let dest_offset_into_func = unseen_addr - func_address as u32;
                    let dest_code_slice = &code[dest_offset_into_func as usize..];
                    ins_iters_with_gpr_state.push((
                        InsIter::new(dest_code_slice, unseen_addr),
                        gpr_pool_relocs.clone(),
                    ));
                    break;
                }
            }
        }
    }

    pool_reloc_for_addr.values().cloned().collect()
}

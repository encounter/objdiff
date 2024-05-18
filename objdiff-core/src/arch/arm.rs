use std::{borrow::Cow, collections::HashMap};

use anyhow::{bail, Context, Result};
use armv5te::{arm, thumb};
use object::{
    elf, File, Object, ObjectSection, ObjectSymbol, Relocation, RelocationFlags, SectionIndex,
    SectionKind, Symbol,
};

use crate::{
    arch::{ObjArch, ProcessCodeResult},
    diff::DiffObjConfig,
    obj::{ObjInfo, ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjSection, SymbolRef},
};

pub struct ObjArchArm {
    /// Maps section index, to list of disasm modes (arm, thumb or data) sorted by address
    disasm_modes: HashMap<SectionIndex, Vec<DisasmMode>>,
}

impl ObjArchArm {
    pub fn new(file: &File) -> Result<Self> {
        match file {
            File::Elf32(_) => {
                let disasm_modes: HashMap<_, _> = file
                    .sections()
                    .filter(|s| s.kind() == SectionKind::Text)
                    .map(|s| {
                        let index = s.index();
                        let mut mapping_symbols: Vec<_> = file
                            .symbols()
                            .filter(|s| s.section_index().map(|i| i == index).unwrap_or(false))
                            .filter_map(|s| DisasmMode::from_symbol(&s))
                            .collect();
                        mapping_symbols.sort_unstable_by_key(|x| x.address);
                        (s.index(), mapping_symbols)
                    })
                    .collect();
                Ok(Self { disasm_modes })
            }
            _ => bail!("Unsupported file format {:?}", file.format()),
        }
    }
}

impl ObjArch for ObjArchArm {
    fn process_code(
        &self,
        obj: &ObjInfo,
        symbol_ref: SymbolRef,
        config: &DiffObjConfig,
    ) -> Result<ProcessCodeResult> {
        let (section, symbol) = obj.section_symbol(symbol_ref);
        let mut code = &section.data
            [symbol.section_address as usize..(symbol.section_address + symbol.size) as usize];

        let start_addr = symbol.address as u32;
        let end_addr = start_addr + symbol.size as u32;

        // Mapping symbols decide what kind of data comes after it. $a for ARM code, $t for Thumb code and $d for data.
        let mapping_symbols = self
            .disasm_modes
            .get(&SectionIndex(section.orig_index))
            .with_context(|| format!("No mappings symbols in the section of '{}'", symbol.name))?;
        let first_mapping = self
            .disasm_modes
            .get(&SectionIndex(section.orig_index))
            .map(|s| match s.binary_search_by_key(&(symbol.address as u32), |x| x.address) {
                Ok(idx) => idx,
                Err(idx) => idx - 1,
            })
            .with_context(|| format!("No mapping symbol found before or at '{}'", symbol.name))?;
        let mut mapping = mapping_symbols[first_mapping].mapping;

        let mut mappings_iter =
            mapping_symbols.iter().skip(first_mapping + 1).take_while(|x| x.address < end_addr);
        let mut next_mapping = mappings_iter.next();

        let ins_count = code.len() / mapping.ins_size();
        let mut ops = Vec::<u16>::with_capacity(ins_count);
        let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
        let mut cur_addr = start_addr;

        while cur_addr < end_addr {
            if let Some(next) = next_mapping {
                if cur_addr >= next.address {
                    // Change mapping
                    mapping = next.mapping;
                    next_mapping = mappings_iter.next();
                }
            }
            if code.len() < mapping.ins_size() {
                break;
            }

            let line = obj
                .line_info
                .as_ref()
                .and_then(|map| map.range(..=cur_addr as u64).last().map(|(_, &b)| b));

            let ins = match mapping {
                MappingSymbol::Arm => {
                    let bytes = [code[0], code[1], code[2], code[3]];
                    code = &code[4..];
                    let ins_code = u32::from_le_bytes(bytes);

                    let reloc =
                        section.relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);
                    let ins_code = mask_reloc_from_code(ins_code, reloc)?;

                    let ins = arm::Ins::new(ins_code);
                    let parsed_ins = arm::ParsedIns::parse(ins);

                    let mut reloc_arg = None;
                    if let Some(reloc) = reloc {
                        if let RelocationFlags::Elf { r_type: elf::R_ARM_PC24 } = reloc.flags {
                            reloc_arg = parsed_ins
                                .args
                                .iter()
                                .rposition(|a| matches!(a, arm::Argument::BranchDest(_)));
                        }
                    }

                    let (args, branch_dest) =
                        push_arm_args(&parsed_ins, config, reloc_arg, cur_addr)?;
                    let op = ins.op as u16;
                    let mnemonic = parsed_ins.mnemonic;

                    ObjIns {
                        address: cur_addr as u64,
                        size: mapping.ins_size() as u8,
                        op,
                        mnemonic: mnemonic.to_string(),
                        args,
                        reloc: reloc.cloned(),
                        branch_dest,
                        line,
                        orig: Some(parsed_ins.to_string()),
                    }
                }
                MappingSymbol::Thumb => {
                    let bytes = [code[0], code[1]];
                    code = &code[2..];
                    let ins_code = u16::from_le_bytes(bytes) as u32;

                    let reloc =
                        section.relocations.iter().find(|r| (r.address as u32 & !1) == cur_addr);
                    let ins_code = mask_reloc_from_code(ins_code, reloc)?;

                    let ins = thumb::Ins::new(ins_code);

                    let mut parsed_ins = thumb::ParsedIns::parse(ins);
                    let mut size = 2;
                    let address = cur_addr as u64;
                    if ins.is_half_bl() {
                        cur_addr += 2;
                        let bytes = [code[0], code[1]];
                        code = &code[2..];
                        let second_code = u16::from_le_bytes(bytes) as u32;
                        let reloc = section
                            .relocations
                            .iter()
                            .find(|r| (r.address as u32 & !1) == cur_addr);
                        let second_code = mask_reloc_from_code(second_code, reloc)?;

                        let second_ins = thumb::Ins::new(second_code);
                        let second_ins = thumb::ParsedIns::parse(second_ins);
                        parsed_ins = parsed_ins.combine_bl(&second_ins);
                        size = 4;
                    }

                    let mut reloc_arg = None;
                    if let Some(reloc) = reloc {
                        if let RelocationFlags::Elf { r_type: elf::R_ARM_THM_XPC22 } = reloc.flags {
                            reloc_arg = parsed_ins
                                .args
                                .iter()
                                .rposition(|a| matches!(a, thumb::Argument::BranchDest(_)));
                        }
                    }

                    let (args, branch_dest) =
                        push_thumb_args(&parsed_ins, config, reloc_arg, cur_addr)?;
                    let op = ins.op as u16;
                    let mnemonic = parsed_ins.mnemonic;

                    ObjIns {
                        address,
                        size,
                        op,
                        mnemonic: mnemonic.to_string(),
                        args,
                        reloc: reloc.cloned(),
                        branch_dest,
                        line,
                        orig: Some(parsed_ins.to_string()),
                    }
                }
                MappingSymbol::Data => {
                    let bytes = [code[0], code[1], code[2], code[3]];
                    code = &code[4..];
                    let data = u32::from_le_bytes(bytes);

                    let reloc =
                        section.relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);
                    let data = mask_reloc_from_code(data, reloc)?;

                    let mut args = vec![];
                    if reloc.is_some() {
                        args.push(ObjInsArg::Reloc);
                    } else {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(data as u64)));
                    }

                    ObjIns {
                        address: cur_addr as u64,
                        size: mapping.ins_size() as u8,
                        op: u16::MAX,
                        mnemonic: ".word".to_string(),
                        args,
                        reloc: reloc.cloned(),
                        branch_dest: None,
                        line,
                        orig: None,
                    }
                }
            };

            ops.push(ins.op);
            insts.push(ins);
            cur_addr += mapping.ins_size() as u32;
        }

        Ok(ProcessCodeResult { ops, insts })
    }

    fn implcit_addend(
        &self,
        _section: &ObjSection,
        address: u64,
        reloc: &Relocation,
    ) -> anyhow::Result<i64> {
        bail!("Unsupported ARM implicit relocation {:#x}{:?}", address, reloc.flags())
    }

    fn demangle(&self, name: &str) -> Option<String> {
        cpp_demangle::Symbol::new(name)
            .ok()
            .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
    }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        Cow::Owned(format!("<{flags:?}>"))
    }
}

#[derive(Clone, Copy, Debug)]
struct DisasmMode {
    address: u32,
    mapping: MappingSymbol,
}

impl DisasmMode {
    fn from_symbol<'a>(sym: &Symbol<'a, '_, &'a [u8]>) -> Option<Self> {
        if let Ok(name) = sym.name() {
            MappingSymbol::from_symbol_name(name)
                .map(|mapping| DisasmMode { address: sym.address() as u32, mapping })
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum MappingSymbol {
    Arm,
    Thumb,
    Data,
}

impl MappingSymbol {
    fn ins_size(self) -> usize {
        match self {
            MappingSymbol::Arm => 4,
            MappingSymbol::Thumb => 2,
            MappingSymbol::Data => 4,
        }
    }

    fn from_symbol_name(sym: &str) -> Option<Self> {
        match sym {
            "$a" => Some(Self::Arm),
            "$t" => Some(Self::Thumb),
            "$d" => Some(Self::Data),
            _ => None,
        }
    }
}

fn mask_reloc_from_code(code: u32, reloc: Option<&ObjReloc>) -> Result<u32> {
    if let Some(reloc) = reloc {
        match reloc.flags {
            RelocationFlags::Elf { r_type } => match r_type {
                elf::R_ARM_PC24 => Ok(code & !0xffffff),
                elf::R_ARM_ABS32 => Ok(0),
                elf::R_ARM_THM_PC22 => Ok(code & !0x7ff),
                elf::R_ARM_XPC25 => Ok(code & !0xffffff),
                elf::R_ARM_THM_XPC22 => Ok(code & !0x7ff),
                _ => bail!("Unhandled ELF relocation type {:?}", r_type),
            },
            _ => bail!("Unhandled relocation flags {:?}", reloc.flags),
        }
    } else {
        Ok(code)
    }
}

fn push_arm_args(
    parsed_ins: &arm::ParsedIns,
    config: &DiffObjConfig,
    reloc_arg: Option<usize>,
    cur_addr: u32,
) -> Result<(Vec<ObjInsArg>, Option<u64>)> {
    let mut args = vec![];
    let mut branch_dest = None;
    let mut writeback = false;
    let mut deref = false;
    for (i, arg) in parsed_ins.args_iter().enumerate() {
        // Emit punctuation before separator
        if deref {
            match arg {
                arm::Argument::PostOffset(_)
                | arm::Argument::RegPostOffset(_)
                | arm::Argument::CoOption(_) => {
                    deref = false;
                    args.push(ObjInsArg::PlainText("]".into()));
                    if writeback {
                        writeback = false;
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("!".into())));
                    }
                }
                _ => {}
            }
        }

        if i > 0 {
            args.push(ObjInsArg::PlainText(config.separator().into()));
        }

        if reloc_arg == Some(i) {
            args.push(ObjInsArg::Reloc);
        } else {
            match arg {
                arm::Argument::RegWb(reg) => {
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(reg.to_string().into())));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("!".into())));
                }
                arm::Argument::RegDeref(reg) => {
                    deref = true;
                    args.push(ObjInsArg::PlainText("[".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(reg.to_string().into())));
                }
                arm::Argument::RegDerefWb(reg) => {
                    deref = true;
                    writeback = true;
                    args.push(ObjInsArg::PlainText("[".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(reg.to_string().into())));
                }
                arm::Argument::RegList(reg_list) => {
                    push_reg_list(*reg_list, &mut args, config);
                }
                arm::Argument::RegListC(reg_list) => {
                    push_reg_list(*reg_list, &mut args, config);
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("^".to_string().into())));
                }
                arm::Argument::UImm(value) | arm::Argument::CoOpcode(value) => {
                    args.push(ObjInsArg::PlainText("#".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(*value as u64)));
                }
                arm::Argument::SImm((value, _))
                | arm::Argument::Offset((value, _))
                | arm::Argument::PostOffset((value, _)) => {
                    args.push(ObjInsArg::PlainText("#".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(*value as i64)));
                }
                arm::Argument::BranchDest((value, _)) => {
                    let dest = cur_addr.wrapping_add_signed(*value) as u64;
                    args.push(ObjInsArg::BranchDest(dest));
                    branch_dest = Some(dest);
                }
                arm::Argument::CoOption(value) => {
                    args.push(ObjInsArg::PlainText("{".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(*value as u64)));
                    args.push(ObjInsArg::PlainText("}".into()));
                }
                arm::Argument::CoprocNum(value) => {
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(format!("p{}", value).into())));
                }
                _ => args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(arg.to_string().into()))),
            }
        }
    }
    if deref {
        args.push(ObjInsArg::PlainText("]".into()));
        if writeback {
            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("!".into())));
        }
    }
    Ok((args, branch_dest))
}

fn push_thumb_args(
    parsed_ins: &thumb::ParsedIns,
    config: &DiffObjConfig,
    reloc_arg: Option<usize>,
    cur_addr: u32,
) -> Result<(Vec<ObjInsArg>, Option<u64>)> {
    let mut args = vec![];
    let mut branch_dest = None;
    let mut deref = false;
    for (i, arg) in parsed_ins.args_iter().enumerate() {
        if i > 0 {
            args.push(ObjInsArg::PlainText(config.separator().into()));
        }

        if reloc_arg == Some(i) {
            args.push(ObjInsArg::Reloc);
        } else {
            match arg {
                thumb::Argument::RegWb(reg) => {
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(reg.to_string().into())));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("!".into())));
                }
                thumb::Argument::RegDeref(reg) => {
                    deref = true;
                    args.push(ObjInsArg::PlainText("[".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(reg.to_string().into())));
                }
                thumb::Argument::RegList(reg_list) => {
                    push_reg_list(*reg_list, &mut args, config);
                }
                thumb::Argument::RegListPc(reg_list) => {
                    push_reg_list(
                        reg_list | ((1 << thumb::Reg::Pc as u8) as u32),
                        &mut args,
                        config,
                    );
                }
                thumb::Argument::UImm(value) => {
                    args.push(ObjInsArg::PlainText("#".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(*value as u64)));
                }
                thumb::Argument::SImm((value, _)) | thumb::Argument::Offset((value, _)) => {
                    args.push(ObjInsArg::PlainText("#".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(*value as i64)));
                }
                thumb::Argument::BranchDest((value, _)) => {
                    let dest = cur_addr.wrapping_add_signed(*value) as u64;
                    args.push(ObjInsArg::BranchDest(dest));
                    branch_dest = Some(dest);
                }
                _ => args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(arg.to_string().into()))),
            }
        }
    }
    if deref {
        args.push(ObjInsArg::PlainText("]".into()));
    }
    Ok((args, branch_dest))
}

fn push_reg_list(reg_list: u32, args: &mut Vec<ObjInsArg>, config: &DiffObjConfig) {
    args.push(ObjInsArg::PlainText("{".into()));
    let mut first = true;
    for i in 0..16 {
        if (reg_list & (1 << i)) != 0 {
            if !first {
                args.push(ObjInsArg::PlainText(config.separator().into()));
            }
            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                arm::Reg::parse(i).to_string().into(),
            )));
            first = false;
        }
    }
    args.push(ObjInsArg::PlainText("}".into()));
}

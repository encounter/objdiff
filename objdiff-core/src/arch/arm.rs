use std::{borrow::Cow, collections::HashMap};

use anyhow::{anyhow, bail, Result};
use object::{
    elf, File, Object, ObjectSection, ObjectSymbol, Relocation, RelocationFlags, SectionIndex,
    SectionKind, Symbol,
};
use unarm::{
    args::{Argument, OffsetImm, OffsetReg, Register},
    parse::{ArmVersion, ParseMode, Parser},
    ParsedIns,
};

use crate::{
    arch::{ObjArch, ProcessCodeResult},
    diff::DiffObjConfig,
    obj::{ObjInfo, ObjIns, ObjInsArg, ObjInsArgValue, ObjSection, SymbolRef},
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
        let section = section.ok_or_else(|| anyhow!("Code symbol section not found"))?;
        let code = &section.data
            [symbol.section_address as usize..(symbol.section_address + symbol.size) as usize];

        let start_addr = symbol.address as u32;
        let end_addr = start_addr + symbol.size as u32;

        // Mapping symbols decide what kind of data comes after it. $a for ARM code, $t for Thumb code and $d for data.
        let fallback_mappings =
            [DisasmMode { address: symbol.address as u32, mapping: ParseMode::Arm }];
        let mapping_symbols = self
            .disasm_modes
            .get(&SectionIndex(section.orig_index))
            .map(|x| x.as_slice())
            .unwrap_or(&fallback_mappings);
        let first_mapping_idx =
            match mapping_symbols.binary_search_by_key(&(symbol.address as u32), |x| x.address) {
                Ok(idx) => idx,
                Err(idx) => idx - 1,
            };
        let first_mapping = mapping_symbols[first_mapping_idx].mapping;

        let mut mappings_iter =
            mapping_symbols.iter().skip(first_mapping_idx + 1).take_while(|x| x.address < end_addr);
        let mut next_mapping = mappings_iter.next();

        let ins_count = code.len() / first_mapping.instruction_size();
        let mut ops = Vec::<u16>::with_capacity(ins_count);
        let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
        let mut parser = Parser::new(ArmVersion::V5Te, first_mapping, start_addr, code);

        while let Some((address, op, ins)) = parser.next() {
            if let Some(next) = next_mapping {
                let next_address = parser.address;
                if next_address >= next.address {
                    // Change mapping
                    parser.mode = next.mapping;
                    next_mapping = mappings_iter.next();
                }
            }

            let line = section.line_info.range(..=address as u64).last().map(|(_, &b)| b);

            let reloc =
                section.relocations.iter().find(|r| (r.address as u32 & !1) == address).cloned();

            let mut reloc_arg = None;
            if let Some(reloc) = &reloc {
                match reloc.flags {
                    RelocationFlags::Elf { r_type: elf::R_ARM_THM_XPC22 }
                    | RelocationFlags::Elf { r_type: elf::R_ARM_PC24 } => {
                        reloc_arg =
                            ins.args.iter().rposition(|a| matches!(a, Argument::BranchDest(_)));
                    }
                    _ => (),
                }
            };

            let (args, branch_dest) = if reloc.is_some() && parser.mode == ParseMode::Data {
                (vec![ObjInsArg::Reloc], None)
            } else {
                push_args(&ins, config, reloc_arg, address)?
            };

            ops.push(op.id());
            insts.push(ObjIns {
                address: address as u64,
                size: (parser.address - address) as u8,
                op: op.id(),
                mnemonic: ins.mnemonic.to_string(),
                args,
                reloc,
                branch_dest,
                line,
                formatted: ins.to_string(),
                orig: None,
            });
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
    mapping: ParseMode,
}

impl DisasmMode {
    fn from_symbol<'a>(sym: &Symbol<'a, '_, &'a [u8]>) -> Option<Self> {
        if let Ok(name) = sym.name() {
            ParseMode::from_mapping_symbol(name)
                .map(|mapping| DisasmMode { address: sym.address() as u32, mapping })
        } else {
            None
        }
    }
}

fn push_args(
    parsed_ins: &ParsedIns,
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
                Argument::OffsetImm(OffsetImm { post_indexed: true, value: _ })
                | Argument::OffsetReg(OffsetReg { add: _, post_indexed: true, reg: _ })
                | Argument::CoOption(_) => {
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
                Argument::Reg(reg) => {
                    if reg.deref {
                        deref = true;
                        args.push(ObjInsArg::PlainText("[".into()));
                    }
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(reg.reg.to_string().into())));
                    if reg.writeback {
                        if reg.deref {
                            writeback = true;
                        } else {
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("!".into())));
                        }
                    }
                }
                Argument::RegList(reg_list) => {
                    args.push(ObjInsArg::PlainText("{".into()));
                    let mut first = true;
                    for i in 0..16 {
                        if (reg_list.regs & (1 << i)) != 0 {
                            if !first {
                                args.push(ObjInsArg::PlainText(config.separator().into()));
                            }
                            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                                Register::parse(i).to_string().into(),
                            )));
                            first = false;
                        }
                    }
                    args.push(ObjInsArg::PlainText("}".into()));
                    if reg_list.user_mode {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("^".to_string().into())));
                    }
                }
                Argument::UImm(value) | Argument::CoOpcode(value) => {
                    args.push(ObjInsArg::PlainText("#".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(*value as u64)));
                }
                Argument::SImm(value)
                | Argument::OffsetImm(OffsetImm { post_indexed: _, value }) => {
                    args.push(ObjInsArg::PlainText("#".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(*value as i64)));
                }
                Argument::BranchDest(value) => {
                    let dest = cur_addr.wrapping_add_signed(*value) as u64;
                    args.push(ObjInsArg::BranchDest(dest));
                    branch_dest = Some(dest);
                }
                Argument::CoOption(value) => {
                    args.push(ObjInsArg::PlainText("{".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(*value as u64)));
                    args.push(ObjInsArg::PlainText("}".into()));
                }
                Argument::CoprocNum(value) => {
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(format!("p{}", value).into())));
                }
                Argument::ShiftImm(shift) => {
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(shift.op.to_string().into())));
                    args.push(ObjInsArg::PlainText(" #".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(shift.imm as u64)));
                }
                Argument::ShiftReg(shift) => {
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(shift.op.to_string().into())));
                    args.push(ObjInsArg::PlainText(" ".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(shift.reg.to_string().into())));
                }
                Argument::OffsetReg(offset) => {
                    if !offset.add {
                        args.push(ObjInsArg::PlainText("-".into()));
                    }
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                        offset.reg.to_string().into(),
                    )));
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

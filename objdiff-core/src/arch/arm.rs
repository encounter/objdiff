use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
};

use anyhow::{bail, Result};
use arm_attr::{enums::CpuArch, tag::Tag, BuildAttrs};
use object::{
    elf::{self, SHT_ARM_ATTRIBUTES},
    Endian, File, Object, ObjectSection, ObjectSymbol, Relocation, RelocationFlags, SectionIndex,
    SectionKind, Symbol, SymbolKind,
};
use unarm::{
    args::{Argument, OffsetImm, OffsetReg, Register},
    parse::{ArmVersion, ParseMode, Parser},
    DisplayOptions, ParseFlags, ParsedIns, RegNames,
};

use crate::{
    arch::{ObjArch, ProcessCodeResult},
    diff::{ArmArchVersion, ArmR9Usage, DiffObjConfig},
    obj::{ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjSection},
};

pub struct ObjArchArm {
    /// Maps section index, to list of disasm modes (arm, thumb or data) sorted by address
    disasm_modes: HashMap<SectionIndex, Vec<DisasmMode>>,
    detected_version: Option<ArmVersion>,
    endianness: object::Endianness,
}

impl ObjArchArm {
    pub fn new(file: &File) -> Result<Self> {
        let endianness = file.endianness();
        match file {
            File::Elf32(_) => {
                let disasm_modes = Self::elf_get_mapping_symbols(file);
                let detected_version = Self::elf_detect_arm_version(file)?;
                Ok(Self { disasm_modes, detected_version, endianness })
            }
            _ => bail!("Unsupported file format {:?}", file.format()),
        }
    }

    fn elf_detect_arm_version(file: &File) -> Result<Option<ArmVersion>> {
        // Check ARM attributes
        if let Some(arm_attrs) = file.sections().find(|s| {
            s.kind() == SectionKind::Elf(SHT_ARM_ATTRIBUTES) && s.name() == Ok(".ARM.attributes")
        }) {
            let attr_data = arm_attrs.uncompressed_data()?;
            let build_attrs = BuildAttrs::new(&attr_data, match file.endianness() {
                object::Endianness::Little => arm_attr::Endian::Little,
                object::Endianness::Big => arm_attr::Endian::Big,
            })?;
            for subsection in build_attrs.subsections() {
                let subsection = subsection?;
                if !subsection.is_aeabi() {
                    continue;
                }
                // Only checking first CpuArch tag. Others may exist, but that's very unlikely.
                let cpu_arch = subsection.into_public_tag_iter()?.find_map(|(_, tag)| {
                    if let Tag::CpuArch(cpu_arch) = tag {
                        Some(cpu_arch)
                    } else {
                        None
                    }
                });
                match cpu_arch {
                    Some(CpuArch::V4T) => return Ok(Some(ArmVersion::V4T)),
                    Some(CpuArch::V5TE) => return Ok(Some(ArmVersion::V5Te)),
                    Some(CpuArch::V6K) => return Ok(Some(ArmVersion::V6K)),
                    Some(arch) => bail!("ARM arch {} not supported", arch),
                    None => {}
                };
            }
        }

        Ok(None)
    }

    fn elf_get_mapping_symbols(file: &File) -> HashMap<SectionIndex, Vec<DisasmMode>> {
        file.sections()
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
            .collect()
    }
}

impl ObjArch for ObjArchArm {
    fn symbol_address(&self, symbol: &Symbol) -> u64 {
        let address = symbol.address();
        if symbol.kind() == SymbolKind::Text {
            address & !1
        } else {
            address
        }
    }

    fn process_code(
        &self,
        address: u64,
        code: &[u8],
        section_index: usize,
        relocations: &[ObjReloc],
        line_info: &BTreeMap<u64, u32>,
        config: &DiffObjConfig,
        _sections: &[ObjSection],
    ) -> Result<ProcessCodeResult> {
        let start_addr = address as u32;
        let end_addr = start_addr + code.len() as u32;

        // Mapping symbols decide what kind of data comes after it. $a for ARM code, $t for Thumb code and $d for data.
        let fallback_mappings = [DisasmMode { address: start_addr, mapping: ParseMode::Arm }];
        let mapping_symbols = self
            .disasm_modes
            .get(&SectionIndex(section_index))
            .map(|x| x.as_slice())
            .unwrap_or(&fallback_mappings);
        let first_mapping_idx = mapping_symbols
            .binary_search_by_key(&start_addr, |x| x.address)
            .unwrap_or_else(|idx| idx - 1);
        let first_mapping = mapping_symbols[first_mapping_idx].mapping;

        let mut mappings_iter =
            mapping_symbols.iter().skip(first_mapping_idx + 1).take_while(|x| x.address < end_addr);
        let mut next_mapping = mappings_iter.next();

        let ins_count = code.len() / first_mapping.instruction_size(start_addr);
        let mut ops = Vec::<u16>::with_capacity(ins_count);
        let mut insts = Vec::<ObjIns>::with_capacity(ins_count);

        let version = match config.arm_arch_version {
            ArmArchVersion::Auto => self.detected_version.unwrap_or(ArmVersion::V5Te),
            ArmArchVersion::V4T => ArmVersion::V4T,
            ArmArchVersion::V5TE => ArmVersion::V5Te,
            ArmArchVersion::V6K => ArmVersion::V6K,
        };
        let endian = match self.endianness {
            object::Endianness::Little => unarm::Endian::Little,
            object::Endianness::Big => unarm::Endian::Big,
        };

        let parse_flags = ParseFlags { ual: config.arm_unified_syntax, version };

        let mut parser = Parser::new(first_mapping, start_addr, endian, parse_flags, code);

        let display_options = DisplayOptions {
            reg_names: RegNames {
                av_registers: config.arm_av_registers,
                r9_use: match config.arm_r9_usage {
                    ArmR9Usage::GeneralPurpose => unarm::R9Use::GeneralPurpose,
                    ArmR9Usage::Sb => unarm::R9Use::Pid,
                    ArmR9Usage::Tr => unarm::R9Use::Tls,
                },
                explicit_stack_limit: config.arm_sl_usage,
                frame_pointer: config.arm_fp_usage,
                ip: config.arm_ip_usage,
            },
        };

        while let Some((address, ins, parsed_ins)) = parser.next() {
            if let Some(next) = next_mapping {
                let next_address = parser.address;
                if next_address >= next.address {
                    // Change mapping
                    parser.mode = next.mapping;
                    next_mapping = mappings_iter.next();
                }
            }
            let line = line_info.range(..=address as u64).last().map(|(_, &b)| b);

            let reloc = relocations.iter().find(|r| (r.address as u32 & !1) == address).cloned();

            let mut reloc_arg = None;
            if let Some(reloc) = &reloc {
                match reloc.flags {
                    // Calls
                    RelocationFlags::Elf { r_type: elf::R_ARM_THM_XPC22 }
                    | RelocationFlags::Elf { r_type: elf::R_ARM_THM_PC22 }
                    | RelocationFlags::Elf { r_type: elf::R_ARM_PC24 }
                    | RelocationFlags::Elf { r_type: elf::R_ARM_XPC25 }
                    | RelocationFlags::Elf { r_type: elf::R_ARM_CALL } => {
                        reloc_arg = parsed_ins
                            .args
                            .iter()
                            .rposition(|a| matches!(a, Argument::BranchDest(_)));
                    }
                    // Data
                    RelocationFlags::Elf { r_type: elf::R_ARM_ABS32 } => {
                        reloc_arg =
                            parsed_ins.args.iter().rposition(|a| matches!(a, Argument::UImm(_)));
                    }
                    _ => (),
                }
            };

            let (args, branch_dest) = if reloc.is_some() && parser.mode == ParseMode::Data {
                (vec![ObjInsArg::Reloc], None)
            } else {
                push_args(&parsed_ins, config, reloc_arg, address, display_options)?
            };

            ops.push(ins.opcode_id());
            insts.push(ObjIns {
                address: address as u64,
                size: (parser.address - address) as u8,
                op: ins.opcode_id(),
                mnemonic: Cow::Borrowed(parsed_ins.mnemonic),
                args,
                reloc,
                fake_pool_reloc: None,
                branch_dest,
                line,
                formatted: parsed_ins.display(display_options).to_string(),
                orig: None,
            });
        }

        Ok(ProcessCodeResult { ops, insts })
    }

    fn implcit_addend(
        &self,
        _file: &File<'_>,
        section: &ObjSection,
        address: u64,
        reloc: &Relocation,
    ) -> Result<i64> {
        let address = address as usize;
        Ok(match reloc.flags() {
            // ARM calls
            RelocationFlags::Elf { r_type: elf::R_ARM_PC24 }
            | RelocationFlags::Elf { r_type: elf::R_ARM_XPC25 }
            | RelocationFlags::Elf { r_type: elf::R_ARM_CALL } => {
                let data = section.data[address..address + 4].try_into()?;
                let addend = self.endianness.read_i32_bytes(data);
                let imm24 = addend & 0xffffff;
                (imm24 << 2) << 8 >> 8
            }

            // Thumb calls
            RelocationFlags::Elf { r_type: elf::R_ARM_THM_PC22 }
            | RelocationFlags::Elf { r_type: elf::R_ARM_THM_XPC22 } => {
                let data = section.data[address..address + 2].try_into()?;
                let high = self.endianness.read_i16_bytes(data) as i32;
                let data = section.data[address + 2..address + 4].try_into()?;
                let low = self.endianness.read_i16_bytes(data) as i32;

                let imm22 = ((high & 0x7ff) << 11) | (low & 0x7ff);
                (imm22 << 1) << 9 >> 9
            }

            // Data
            RelocationFlags::Elf { r_type: elf::R_ARM_ABS32 } => {
                let data = section.data[address..address + 4].try_into()?;
                self.endianness.read_i32_bytes(data)
            }

            flags => bail!("Unsupported ARM implicit relocation {flags:?}"),
        } as i64)
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
    display_options: DisplayOptions,
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
                Argument::None => {}
                Argument::Reg(reg) => {
                    if reg.deref {
                        deref = true;
                        args.push(ObjInsArg::PlainText("[".into()));
                    }
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                        reg.reg.display(display_options.reg_names).to_string().into(),
                    )));
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
                                Register::parse(i)
                                    .display(display_options.reg_names)
                                    .to_string()
                                    .into(),
                            )));
                            first = false;
                        }
                    }
                    args.push(ObjInsArg::PlainText("}".into()));
                    if reg_list.user_mode {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("^".to_string().into())));
                    }
                }
                Argument::UImm(value) | Argument::CoOpcode(value) | Argument::SatImm(value) => {
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
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                        shift.reg.display(display_options.reg_names).to_string().into(),
                    )));
                }
                Argument::OffsetReg(offset) => {
                    if !offset.add {
                        args.push(ObjInsArg::PlainText("-".into()));
                    }
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                        offset.reg.display(display_options.reg_names).to_string().into(),
                    )));
                }
                Argument::CpsrMode(mode) => {
                    args.push(ObjInsArg::PlainText("#".into()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(mode.mode as u64)));
                    if mode.writeback {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque("!".into())));
                    }
                }
                Argument::CoReg(_)
                | Argument::StatusReg(_)
                | Argument::StatusMask(_)
                | Argument::Shift(_)
                | Argument::CpsrFlags(_)
                | Argument::Endian(_) => args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                    arg.display(display_options, None).to_string().into(),
                ))),
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

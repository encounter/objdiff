use alloc::{
    borrow::Cow,
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::ops::Range;

use anyhow::{bail, Result};
use arm_attr::{enums::CpuArch, tag::Tag, BuildAttrs};
use object::{elf, Endian as _, Object as _, ObjectSection as _, ObjectSymbol as _};
use unarm::{args, arm, thumb};

use crate::{
    arch::Arch,
    diff::{display::InstructionPart, ArmArchVersion, ArmR9Usage, DiffObjConfig},
    obj::{
        InstructionArg, InstructionArgValue, InstructionRef, RelocationFlags, ResolvedRelocation,
        ScannedInstruction, SymbolFlag, SymbolFlagSet, SymbolKind,
    },
};

#[derive(Debug)]
pub struct ArchArm {
    /// Maps section index, to list of disasm modes (arm, thumb or data) sorted by address
    disasm_modes: BTreeMap<usize, Vec<DisasmMode>>,
    detected_version: Option<unarm::ArmVersion>,
    endianness: object::Endianness,
}

impl ArchArm {
    pub fn new(file: &object::File) -> Result<Self> {
        let endianness = file.endianness();
        match file {
            object::File::Elf32(_) => {
                let disasm_modes = Self::elf_get_mapping_symbols(file);
                let detected_version = Self::elf_detect_arm_version(file)?;
                Ok(Self { disasm_modes, detected_version, endianness })
            }
            _ => bail!("Unsupported file format {:?}", file.format()),
        }
    }

    fn elf_detect_arm_version(file: &object::File) -> Result<Option<unarm::ArmVersion>> {
        // Check ARM attributes
        if let Some(arm_attrs) = file.sections().find(|s| {
            s.kind() == object::SectionKind::Elf(elf::SHT_ARM_ATTRIBUTES)
                && s.name() == Ok(".ARM.attributes")
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
                    Some(CpuArch::V4T) => return Ok(Some(unarm::ArmVersion::V4T)),
                    Some(CpuArch::V5TE) => return Ok(Some(unarm::ArmVersion::V5Te)),
                    Some(CpuArch::V6K) => return Ok(Some(unarm::ArmVersion::V6K)),
                    Some(arch) => bail!("ARM arch {} not supported", arch),
                    None => {}
                };
            }
        }

        Ok(None)
    }

    fn elf_get_mapping_symbols(file: &object::File) -> BTreeMap<usize, Vec<DisasmMode>> {
        file.sections()
            .filter(|s| s.kind() == object::SectionKind::Text)
            .map(|s| {
                let index = s.index();
                let mut mapping_symbols: Vec<_> = file
                    .symbols()
                    .filter(|s| s.section_index().map(|i| i == index).unwrap_or(false))
                    .filter_map(|s| DisasmMode::from_symbol(&s))
                    .collect();
                mapping_symbols.sort_unstable_by_key(|x| x.address);
                (s.index().0, mapping_symbols)
            })
            .collect()
    }

    fn endian(&self) -> unarm::Endian {
        match self.endianness {
            object::Endianness::Little => unarm::Endian::Little,
            object::Endianness::Big => unarm::Endian::Big,
        }
    }

    fn parse_flags(&self, diff_config: &DiffObjConfig) -> unarm::ParseFlags {
        unarm::ParseFlags {
            ual: diff_config.arm_unified_syntax,
            version: match diff_config.arm_arch_version {
                ArmArchVersion::Auto => self.detected_version.unwrap_or(unarm::ArmVersion::V5Te),
                ArmArchVersion::V4t => unarm::ArmVersion::V4T,
                ArmArchVersion::V5te => unarm::ArmVersion::V5Te,
                ArmArchVersion::V6k => unarm::ArmVersion::V6K,
            },
        }
    }

    fn display_options(&self, diff_config: &DiffObjConfig) -> unarm::DisplayOptions {
        unarm::DisplayOptions {
            reg_names: unarm::RegNames {
                av_registers: diff_config.arm_av_registers,
                r9_use: match diff_config.arm_r9_usage {
                    ArmR9Usage::GeneralPurpose => unarm::R9Use::GeneralPurpose,
                    ArmR9Usage::Sb => unarm::R9Use::Pid,
                    ArmR9Usage::Tr => unarm::R9Use::Tls,
                },
                explicit_stack_limit: diff_config.arm_sl_usage,
                frame_pointer: diff_config.arm_fp_usage,
                ip: diff_config.arm_ip_usage,
            },
        }
    }

    fn parse_ins_ref(
        &self,
        ins_ref: InstructionRef,
        code: &[u8],
        diff_config: &DiffObjConfig,
    ) -> Result<(unarm::Ins, unarm::ParsedIns)> {
        let code = match (self.endianness, ins_ref.size) {
            (object::Endianness::Little, 2) => u16::from_le_bytes([code[0], code[1]]) as u32,
            (object::Endianness::Little, 4) => {
                u32::from_le_bytes([code[0], code[1], code[2], code[3]])
            }
            (object::Endianness::Big, 2) => u16::from_be_bytes([code[0], code[1]]) as u32,
            (object::Endianness::Big, 4) => {
                u32::from_be_bytes([code[0], code[1], code[2], code[3]])
            }
            _ => bail!("Invalid instruction size {}", ins_ref.size),
        };
        let (ins, parsed_ins) = if ins_ref.opcode == u16::MAX {
            let mut args = args::Arguments::default();
            args[0] = args::Argument::UImm(code);
            let mnemonic = if ins_ref.size == 4 { ".word" } else { ".hword" };
            (unarm::Ins::Data, unarm::ParsedIns { mnemonic, args })
        } else if ins_ref.opcode & (1 << 15) != 0 {
            let ins = arm::Ins { code, op: arm::Opcode::from(ins_ref.opcode as u8) };
            let parsed = ins.parse(&self.parse_flags(diff_config));
            (unarm::Ins::Arm(ins), parsed)
        } else {
            let ins = thumb::Ins { code, op: thumb::Opcode::from(ins_ref.opcode as u8) };
            let parsed = ins.parse(&self.parse_flags(diff_config));
            if ins.is_half_bl() {
                todo!("Combine thumb BL instructions");
            } else {
                (unarm::Ins::Thumb(ins), parsed)
            }
        };
        Ok((ins, parsed_ins))
    }
}

impl Arch for ArchArm {
    fn scan_instructions(
        &self,
        address: u64,
        code: &[u8],
        section_index: usize,
        diff_config: &DiffObjConfig,
    ) -> Result<Vec<ScannedInstruction>> {
        let start_addr = address as u32;
        let end_addr = start_addr + code.len() as u32;

        // Mapping symbols decide what kind of data comes after it. $a for ARM code, $t for Thumb code and $d for data.
        let fallback_mappings =
            [DisasmMode { address: start_addr, mapping: unarm::ParseMode::Arm }];
        let mapping_symbols = self
            .disasm_modes
            .get(&section_index)
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
        let mut ops = Vec::<ScannedInstruction>::with_capacity(ins_count);

        let endian = self.endian();
        let parse_flags = self.parse_flags(diff_config);
        let mut parser = unarm::Parser::new(first_mapping, start_addr, endian, parse_flags, code);

        while let Some((address, ins, _parsed_ins)) = parser.next() {
            let size = parser.mode.instruction_size(address);
            if let Some(next) = next_mapping {
                let next_address = parser.address;
                if next_address >= next.address {
                    // Change mapping
                    parser.mode = next.mapping;
                    next_mapping = mappings_iter.next();
                }
            }
            let (opcode, branch_dest) = match ins {
                unarm::Ins::Arm(x) => {
                    let opcode = x.op as u16 | (1 << 15);
                    let branch_dest = match x.op {
                        arm::Opcode::B | arm::Opcode::Bl => {
                            address.checked_add_signed(x.field_branch_offset())
                        }
                        arm::Opcode::BlxI => address.checked_add_signed(x.field_blx_offset()),
                        _ => None,
                    };
                    (opcode, branch_dest)
                }
                unarm::Ins::Thumb(x) => {
                    let opcode = x.op as u16;
                    let branch_dest = match x.op {
                        thumb::Opcode::B | thumb::Opcode::Bl => {
                            address.checked_add_signed(x.field_branch_offset_8())
                        }
                        thumb::Opcode::BLong => {
                            address.checked_add_signed(x.field_branch_offset_11())
                        }
                        _ => None,
                    };
                    (opcode, branch_dest)
                }
                unarm::Ins::Data => (u16::MAX, None),
            };
            ops.push(ScannedInstruction {
                ins_ref: InstructionRef { address: address as u64, size: size as u8, opcode },
                branch_dest: branch_dest.map(|x| x as u64),
            });
        }

        Ok(ops)
    }

    fn display_instruction(
        &self,
        ins_ref: InstructionRef,
        code: &[u8],
        relocation: Option<ResolvedRelocation>,
        _function_range: Range<u64>,
        _section_index: usize,
        diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        let (ins, parsed_ins) = self.parse_ins_ref(ins_ref, code, diff_config)?;
        cb(InstructionPart::Opcode(Cow::Borrowed(parsed_ins.mnemonic), ins_ref.opcode))?;
        if ins == unarm::Ins::Data && relocation.is_some() {
            cb(InstructionPart::Arg(InstructionArg::Reloc))?;
        } else {
            push_args(
                &parsed_ins,
                relocation,
                ins_ref.address as u32,
                self.display_options(diff_config),
                cb,
            )?;
        }
        Ok(())
    }

    fn implcit_addend(
        &self,
        _file: &object::File<'_>,
        section: &object::Section,
        address: u64,
        _relocation: &object::Relocation,
        flags: RelocationFlags,
    ) -> Result<i64> {
        let section_data = section.data()?;
        let address = address as usize;
        Ok(match flags {
            // ARM calls
            RelocationFlags::Elf(elf::R_ARM_PC24)
            | RelocationFlags::Elf(elf::R_ARM_XPC25)
            | RelocationFlags::Elf(elf::R_ARM_CALL) => {
                let data = section_data[address..address + 4].try_into()?;
                let addend = self.endianness.read_i32_bytes(data);
                let imm24 = addend & 0xffffff;
                (imm24 << 2) << 8 >> 8
            }

            // Thumb calls
            RelocationFlags::Elf(elf::R_ARM_THM_PC22)
            | RelocationFlags::Elf(elf::R_ARM_THM_XPC22) => {
                let data = section_data[address..address + 2].try_into()?;
                let high = self.endianness.read_i16_bytes(data) as i32;
                let data = section_data[address + 2..address + 4].try_into()?;
                let low = self.endianness.read_i16_bytes(data) as i32;

                let imm22 = ((high & 0x7ff) << 11) | (low & 0x7ff);
                (imm22 << 1) << 9 >> 9
            }

            // Data
            RelocationFlags::Elf(elf::R_ARM_ABS32) => {
                let data = section_data[address..address + 4].try_into()?;
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

    fn get_reloc_byte_size(&self, flags: RelocationFlags) -> usize {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                elf::R_ARM_ABS32 => 4,
                elf::R_ARM_REL32 => 4,
                elf::R_ARM_ABS16 => 2,
                elf::R_ARM_ABS8 => 1,
                _ => 1,
            },
            _ => 1,
        }
    }

    fn symbol_address(&self, address: u64, kind: SymbolKind) -> u64 {
        if kind == SymbolKind::Function {
            address & !1
        } else {
            address
        }
    }

    fn extra_symbol_flags(&self, symbol: &object::Symbol) -> SymbolFlagSet {
        let mut flags = SymbolFlagSet::default();
        if DisasmMode::from_symbol(symbol).is_some() {
            flags |= SymbolFlag::Hidden;
        }
        flags
    }
}

#[derive(Clone, Copy, Debug)]
struct DisasmMode {
    address: u32,
    mapping: unarm::ParseMode,
}

impl DisasmMode {
    fn from_symbol<'a>(sym: &object::Symbol<'a, '_, &'a [u8]>) -> Option<Self> {
        sym.name()
            .ok()
            .and_then(unarm::ParseMode::from_mapping_symbol)
            .map(|mapping| DisasmMode { address: sym.address() as u32, mapping })
    }
}

fn push_args(
    parsed_ins: &unarm::ParsedIns,
    relocation: Option<ResolvedRelocation>,
    cur_addr: u32,
    display_options: unarm::DisplayOptions,
    mut arg_cb: impl FnMut(InstructionPart) -> Result<()>,
) -> Result<()> {
    let reloc_arg = find_reloc_arg(parsed_ins, relocation);
    let mut writeback = false;
    let mut deref = false;
    for (i, arg) in parsed_ins.args_iter().enumerate() {
        // Emit punctuation before separator
        if deref {
            match arg {
                args::Argument::OffsetImm(args::OffsetImm { post_indexed: true, value: _ })
                | args::Argument::OffsetReg(args::OffsetReg {
                    add: _,
                    post_indexed: true,
                    reg: _,
                })
                | args::Argument::CoOption(_) => {
                    deref = false;
                    arg_cb(InstructionPart::Basic("]"))?;
                    if writeback {
                        writeback = false;
                        arg_cb(InstructionPart::Arg(InstructionArg::Value(
                            InstructionArgValue::Opaque("!".into()),
                        )))?;
                    }
                }
                _ => {}
            }
        }

        if i > 0 {
            arg_cb(InstructionPart::Separator)?;
        }

        if reloc_arg == Some(i) {
            arg_cb(InstructionPart::Arg(InstructionArg::Reloc))?;
        } else {
            match arg {
                args::Argument::None => {}
                args::Argument::Reg(reg) => {
                    if reg.deref {
                        deref = true;
                        arg_cb(InstructionPart::Basic("["))?;
                    }
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Opaque(
                            reg.reg.display(display_options.reg_names).to_string().into(),
                        ),
                    )))?;
                    if reg.writeback {
                        if reg.deref {
                            writeback = true;
                        } else {
                            arg_cb(InstructionPart::Arg(InstructionArg::Value(
                                InstructionArgValue::Opaque("!".into()),
                            )))?;
                        }
                    }
                }
                args::Argument::RegList(reg_list) => {
                    arg_cb(InstructionPart::Basic("{"))?;
                    let mut first = true;
                    for i in 0..16 {
                        if (reg_list.regs & (1 << i)) != 0 {
                            if !first {
                                arg_cb(InstructionPart::Separator)?;
                            }
                            arg_cb(InstructionPart::Arg(InstructionArg::Value(
                                InstructionArgValue::Opaque(
                                    args::Register::parse(i)
                                        .display(display_options.reg_names)
                                        .to_string()
                                        .into(),
                                ),
                            )))?;
                            first = false;
                        }
                    }
                    arg_cb(InstructionPart::Basic("}"))?;
                    if reg_list.user_mode {
                        arg_cb(InstructionPart::Arg(InstructionArg::Value(
                            InstructionArgValue::Opaque("^".into()),
                        )))?;
                    }
                }
                args::Argument::UImm(value)
                | args::Argument::CoOpcode(value)
                | args::Argument::SatImm(value) => {
                    arg_cb(InstructionPart::Basic("#"))?;
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Unsigned(*value as u64),
                    )))?;
                }
                args::Argument::SImm(value)
                | args::Argument::OffsetImm(args::OffsetImm { post_indexed: _, value }) => {
                    arg_cb(InstructionPart::Basic("#"))?;
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Signed(*value as i64),
                    )))?;
                }
                args::Argument::BranchDest(value) => {
                    let dest = cur_addr.wrapping_add_signed(*value) as u64;
                    arg_cb(InstructionPart::Arg(InstructionArg::BranchDest(dest)))?;
                }
                args::Argument::CoOption(value) => {
                    arg_cb(InstructionPart::Basic("{"))?;
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Unsigned(*value as u64),
                    )))?;
                    arg_cb(InstructionPart::Basic("}"))?;
                }
                args::Argument::CoprocNum(value) => {
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Opaque(format!("p{}", value).into()),
                    )))?;
                }
                args::Argument::ShiftImm(shift) => {
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Opaque(shift.op.to_string().into()),
                    )))?;
                    arg_cb(InstructionPart::Basic(" #"))?;
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Unsigned(shift.imm as u64),
                    )))?;
                }
                args::Argument::ShiftReg(shift) => {
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Opaque(shift.op.to_string().into()),
                    )))?;
                    arg_cb(InstructionPart::Basic(" "))?;
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Opaque(
                            shift.reg.display(display_options.reg_names).to_string().into(),
                        ),
                    )))?;
                }
                args::Argument::OffsetReg(offset) => {
                    if !offset.add {
                        arg_cb(InstructionPart::Basic("-"))?;
                    }
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Opaque(
                            offset.reg.display(display_options.reg_names).to_string().into(),
                        ),
                    )))?;
                }
                args::Argument::CpsrMode(mode) => {
                    arg_cb(InstructionPart::Basic("#"))?;
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Unsigned(mode.mode as u64),
                    )))?;
                    if mode.writeback {
                        arg_cb(InstructionPart::Arg(InstructionArg::Value(
                            InstructionArgValue::Opaque("!".into()),
                        )))?;
                    }
                }
                args::Argument::CoReg(_)
                | args::Argument::StatusReg(_)
                | args::Argument::StatusMask(_)
                | args::Argument::Shift(_)
                | args::Argument::CpsrFlags(_)
                | args::Argument::Endian(_) => {
                    arg_cb(InstructionPart::Arg(InstructionArg::Value(
                        InstructionArgValue::Opaque(
                            arg.display(display_options, None).to_string().into(),
                        ),
                    )))?;
                }
            }
        }
    }
    if deref {
        arg_cb(InstructionPart::Basic("]"))?;
        if writeback {
            arg_cb(InstructionPart::Arg(InstructionArg::Value(InstructionArgValue::Opaque(
                "!".into(),
            ))))?;
        }
    }
    Ok(())
}

fn find_reloc_arg(
    parsed_ins: &unarm::ParsedIns,
    relocation: Option<ResolvedRelocation>,
) -> Option<usize> {
    if let Some(resolved) = relocation {
        match resolved.relocation.flags {
            // Calls
            RelocationFlags::Elf(elf::R_ARM_THM_XPC22)
            | RelocationFlags::Elf(elf::R_ARM_THM_PC22)
            | RelocationFlags::Elf(elf::R_ARM_PC24)
            | RelocationFlags::Elf(elf::R_ARM_XPC25)
            | RelocationFlags::Elf(elf::R_ARM_CALL) => {
                parsed_ins.args.iter().rposition(|a| matches!(a, args::Argument::BranchDest(_)))
            }
            // Data
            RelocationFlags::Elf(elf::R_ARM_ABS32) => {
                parsed_ins.args.iter().rposition(|a| matches!(a, args::Argument::UImm(_)))
            }
            _ => None,
        }
    } else {
        None
    }
}

use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};

use anyhow::{Result, bail};
use arm_attr::{BuildAttrs, enums::CpuArch, tag::Tag};
use object::{Endian as _, Object as _, ObjectSection as _, ObjectSymbol as _, elf};
use unarm::{args, arm, thumb};

use crate::{
    arch::Arch,
    diff::{ArmArchVersion, ArmR9Usage, DiffObjConfig, display::InstructionPart},
    obj::{
        InstructionRef, Relocation, RelocationFlags, ResolvedInstructionRef, ResolvedRelocation,
        Section, SectionKind, Symbol, SymbolFlag, SymbolFlagSet, SymbolKind,
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
                // The disasm_modes mapping is populated later in the post_init step so that we have access to merged sections.
                let disasm_modes = BTreeMap::new();
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
                    if let Tag::CpuArch(cpu_arch) = tag { Some(cpu_arch) } else { None }
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

    fn get_mapping_symbols(
        sections: &[Section],
        symbols: &[Symbol],
    ) -> BTreeMap<usize, Vec<DisasmMode>> {
        sections
            .iter()
            .enumerate()
            .filter(|(_, section)| section.kind == SectionKind::Code)
            .map(|(index, _)| {
                let mut mapping_symbols: Vec<_> = symbols
                    .iter()
                    .filter(|s| s.section.map(|i| i == index).unwrap_or(false))
                    .filter_map(DisasmMode::from_symbol)
                    .collect();
                mapping_symbols.sort_unstable_by_key(|x| x.address);
                (index, mapping_symbols)
            })
            .collect()
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
        if ins_ref.opcode == thumb::Opcode::BlH as u16 && ins_ref.size == 4 {
            // Special case: combined thumb BL instruction
            let parse_flags = self.parse_flags(diff_config);
            let first_ins = thumb::Ins {
                code: match self.endianness {
                    object::Endianness::Little => u16::from_le_bytes([code[0], code[1]]),
                    object::Endianness::Big => u16::from_be_bytes([code[0], code[1]]),
                } as u32,
                op: thumb::Opcode::BlH,
            };
            let second_ins = thumb::Ins::new(
                match self.endianness {
                    object::Endianness::Little => u16::from_le_bytes([code[2], code[3]]),
                    object::Endianness::Big => u16::from_be_bytes([code[2], code[3]]),
                } as u32,
                &parse_flags,
            );
            let first_parsed = first_ins.parse(&parse_flags);
            let second_parsed = second_ins.parse(&parse_flags);
            return Ok((
                unarm::Ins::Thumb(first_ins),
                first_parsed.combine_thumb_bl(&second_parsed),
            ));
        }

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
            (unarm::Ins::Thumb(ins), parsed)
        };
        Ok((ins, parsed_ins))
    }
}

impl Arch for ArchArm {
    fn post_init(&mut self, sections: &[Section], symbols: &[Symbol]) {
        self.disasm_modes = Self::get_mapping_symbols(sections, symbols);
    }

    fn scan_instructions_internal(
        &self,
        address: u64,
        code: &[u8],
        section_index: usize,
        _relocations: &[Relocation],
        diff_config: &DiffObjConfig,
    ) -> Result<Vec<InstructionRef>> {
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
            .unwrap_or_else(|idx| idx.saturating_sub(1));
        let mut mode = mapping_symbols[first_mapping_idx].mapping;

        let mut mappings_iter = mapping_symbols
            .iter()
            .copied()
            .skip(first_mapping_idx + 1)
            .take_while(|x| x.address < end_addr);
        let mut next_mapping = mappings_iter.next();

        let ins_count = code.len() / mode.instruction_size(start_addr);
        let mut ops = Vec::<InstructionRef>::with_capacity(ins_count);

        let parse_flags = self.parse_flags(diff_config);

        let mut address = start_addr;
        while address < end_addr {
            while let Some(next) = next_mapping.filter(|x| address >= x.address) {
                // Change mapping
                mode = next.mapping;
                next_mapping = mappings_iter.next();
            }

            let mut ins_size = mode.instruction_size(address);
            let data = &code[(address - start_addr) as usize..];
            if data.len() < ins_size {
                // Push the remainder as data
                ops.push(InstructionRef {
                    address: address as u64,
                    size: data.len() as u8,
                    opcode: u16::MAX,
                    branch_dest: None,
                });
                break;
            }
            let code = match (self.endianness, ins_size) {
                (object::Endianness::Little, 2) => u16::from_le_bytes([data[0], data[1]]) as u32,
                (object::Endianness::Little, 4) => {
                    u32::from_le_bytes([data[0], data[1], data[2], data[3]])
                }
                (object::Endianness::Big, 2) => u16::from_be_bytes([data[0], data[1]]) as u32,
                (object::Endianness::Big, 4) => {
                    u32::from_be_bytes([data[0], data[1], data[2], data[3]])
                }
                _ => {
                    // Invalid instruction size
                    ops.push(InstructionRef {
                        address: address as u64,
                        size: ins_size as u8,
                        opcode: u16::MAX,
                        branch_dest: None,
                    });
                    address += ins_size as u32;
                    continue;
                }
            };

            let (opcode, branch_dest) = match mode {
                unarm::ParseMode::Arm => {
                    let ins = arm::Ins::new(code, &parse_flags);
                    let opcode = ins.op as u16 | (1 << 15);
                    let branch_dest = match ins.op {
                        arm::Opcode::B | arm::Opcode::Bl => {
                            address.checked_add_signed(ins.field_branch_offset())
                        }
                        arm::Opcode::BlxI => address.checked_add_signed(ins.field_blx_offset()),
                        _ => None,
                    };
                    (opcode, branch_dest)
                }
                unarm::ParseMode::Thumb => {
                    let ins = thumb::Ins::new(code, &parse_flags);
                    let opcode = ins.op as u16;
                    let branch_dest = match ins.op {
                        thumb::Opcode::B | thumb::Opcode::Bl => {
                            address.checked_add_signed(ins.field_branch_offset_8())
                        }
                        thumb::Opcode::BlH if data.len() >= 4 => {
                            // Combine BL instructions
                            let second_ins = thumb::Ins::new(
                                match self.endianness {
                                    object::Endianness::Little => {
                                        u16::from_le_bytes([data[2], data[3]]) as u32
                                    }
                                    object::Endianness::Big => {
                                        u16::from_be_bytes([data[2], data[3]]) as u32
                                    }
                                },
                                &parse_flags,
                            );
                            if let Some(low) = match second_ins.op {
                                thumb::Opcode::Bl => Some(second_ins.field_low_branch_offset_11()),
                                thumb::Opcode::BlxI => Some(second_ins.field_low_blx_offset_11()),
                                _ => None,
                            } {
                                ins_size = 4;
                                address.checked_add_signed(
                                    (ins.field_high_branch_offset_11() + (low as i32)) << 9 >> 9,
                                )
                            } else {
                                None
                            }
                        }
                        thumb::Opcode::BLong => {
                            address.checked_add_signed(ins.field_branch_offset_11())
                        }
                        _ => None,
                    };
                    (opcode, branch_dest)
                }
                unarm::ParseMode::Data => (u16::MAX, None),
            };

            ops.push(InstructionRef {
                address: address as u64,
                size: ins_size as u8,
                opcode,
                branch_dest: branch_dest.map(|x| x as u64),
            });
            address += ins_size as u32;
        }

        Ok(ops)
    }

    fn display_instruction(
        &self,
        resolved: ResolvedInstructionRef,
        diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        let (ins, parsed_ins) = self.parse_ins_ref(resolved.ins_ref, resolved.code, diff_config)?;
        cb(InstructionPart::opcode(parsed_ins.mnemonic, resolved.ins_ref.opcode))?;
        if ins == unarm::Ins::Data && resolved.relocation.is_some() {
            cb(InstructionPart::reloc())?;
        } else {
            push_args(
                ins,
                &parsed_ins,
                resolved.relocation,
                resolved.ins_ref.address as u32,
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
    ) -> Result<Option<i64>> {
        let section_data = section.data()?;
        let address = address as usize;
        Ok(Some(match flags {
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
        } as i64))
    }

    fn demangle(&self, name: &str) -> Option<String> {
        cpp_demangle::Symbol::new(name)
            .ok()
            .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
    }

    fn reloc_name(&self, flags: RelocationFlags) -> Option<&'static str> {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                elf::R_ARM_NONE => Some("R_ARM_NONE"),
                elf::R_ARM_ABS32 => Some("R_ARM_ABS32"),
                elf::R_ARM_REL32 => Some("R_ARM_REL32"),
                elf::R_ARM_ABS16 => Some("R_ARM_ABS16"),
                elf::R_ARM_ABS8 => Some("R_ARM_ABS8"),
                elf::R_ARM_THM_PC22 => Some("R_ARM_THM_PC22"),
                elf::R_ARM_THM_XPC22 => Some("R_ARM_THM_XPC22"),
                elf::R_ARM_PC24 => Some("R_ARM_PC24"),
                elf::R_ARM_XPC25 => Some("R_ARM_XPC25"),
                elf::R_ARM_CALL => Some("R_ARM_CALL"),
                _ => None,
            },
            _ => None,
        }
    }

    fn data_reloc_size(&self, flags: RelocationFlags) -> usize {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                elf::R_ARM_NONE => 0,
                elf::R_ARM_ABS32 => 4,
                elf::R_ARM_REL32 => 4,
                elf::R_ARM_ABS16 => 2,
                elf::R_ARM_ABS8 => 1,
                elf::R_ARM_THM_PC22 => 4,
                elf::R_ARM_THM_XPC22 => 4,
                elf::R_ARM_PC24 => 4,
                elf::R_ARM_XPC25 => 4,
                elf::R_ARM_CALL => 4,
                _ => 1,
            },
            _ => 1,
        }
    }

    fn symbol_address(&self, address: u64, kind: SymbolKind) -> u64 {
        if kind == SymbolKind::Function { address & !1 } else { address }
    }

    fn extra_symbol_flags(&self, symbol: &object::Symbol) -> SymbolFlagSet {
        let mut flags = SymbolFlagSet::default();
        if DisasmMode::from_object_symbol(symbol).is_some() {
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
    fn from_object_symbol<'a>(sym: &object::Symbol<'a, '_, &'a [u8]>) -> Option<Self> {
        sym.name()
            .ok()
            .and_then(unarm::ParseMode::from_mapping_symbol)
            .map(|mapping| DisasmMode { address: sym.address() as u32, mapping })
    }

    fn from_symbol(sym: &Symbol) -> Option<Self> {
        unarm::ParseMode::from_mapping_symbol(&sym.name)
            .map(|mapping| DisasmMode { address: sym.address as u32, mapping })
    }
}

fn push_args(
    ins: unarm::Ins,
    parsed_ins: &unarm::ParsedIns,
    relocation: Option<ResolvedRelocation>,
    cur_addr: u32,
    display_options: unarm::DisplayOptions,
    mut arg_cb: impl FnMut(InstructionPart) -> Result<()>,
) -> Result<()> {
    let reloc_arg = find_reloc_arg(parsed_ins, relocation);
    let mut writeback = false;
    let mut deref = false;
    for (i, &arg) in parsed_ins.args_iter().enumerate() {
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
                    arg_cb(InstructionPart::basic("]"))?;
                    if writeback {
                        writeback = false;
                        arg_cb(InstructionPart::opaque("!"))?;
                    }
                }
                _ => {}
            }
        }

        if i > 0 {
            arg_cb(InstructionPart::separator())?;
        }

        if reloc_arg == Some(i) {
            arg_cb(InstructionPart::reloc())?;
        } else {
            match arg {
                args::Argument::None => {}
                args::Argument::Reg(reg) => {
                    if reg.deref {
                        deref = true;
                        arg_cb(InstructionPart::basic("["))?;
                    }
                    arg_cb(InstructionPart::opaque(
                        reg.reg.display(display_options.reg_names).to_string(),
                    ))?;
                    if reg.writeback {
                        if reg.deref {
                            writeback = true;
                        } else {
                            arg_cb(InstructionPart::opaque("!"))?;
                        }
                    }
                }
                args::Argument::RegList(reg_list) => {
                    arg_cb(InstructionPart::basic("{"))?;
                    let mut first = true;
                    for i in 0..16 {
                        if (reg_list.regs & (1 << i)) != 0 {
                            if !first {
                                arg_cb(InstructionPart::separator())?;
                            }
                            arg_cb(InstructionPart::opaque(
                                args::Register::parse(i)
                                    .display(display_options.reg_names)
                                    .to_string(),
                            ))?;
                            first = false;
                        }
                    }
                    arg_cb(InstructionPart::basic("}"))?;
                    if reg_list.user_mode {
                        arg_cb(InstructionPart::opaque("^"))?;
                    }
                }
                args::Argument::UImm(value)
                | args::Argument::CoOpcode(value)
                | args::Argument::SatImm(value) => {
                    arg_cb(InstructionPart::basic("#"))?;
                    arg_cb(InstructionPart::unsigned(value))?;
                }
                args::Argument::SImm(value)
                | args::Argument::OffsetImm(args::OffsetImm { post_indexed: _, value }) => {
                    arg_cb(InstructionPart::basic("#"))?;
                    arg_cb(InstructionPart::signed(value))?;
                }
                args::Argument::BranchDest(value) => {
                    arg_cb(InstructionPart::branch_dest(cur_addr.wrapping_add_signed(value)))?;
                }
                args::Argument::CoOption(value) => {
                    arg_cb(InstructionPart::basic("{"))?;
                    arg_cb(InstructionPart::unsigned(value))?;
                    arg_cb(InstructionPart::basic("}"))?;
                }
                args::Argument::CoprocNum(value) => {
                    arg_cb(InstructionPart::opaque(format!("p{value}")))?;
                }
                args::Argument::ShiftImm(shift) => {
                    arg_cb(InstructionPart::opaque(shift.op.to_string()))?;
                    arg_cb(InstructionPart::basic(" #"))?;
                    arg_cb(InstructionPart::unsigned(shift.imm))?;
                }
                args::Argument::ShiftReg(shift) => {
                    arg_cb(InstructionPart::opaque(shift.op.to_string()))?;
                    arg_cb(InstructionPart::basic(" "))?;
                    arg_cb(InstructionPart::opaque(
                        shift.reg.display(display_options.reg_names).to_string(),
                    ))?;
                }
                args::Argument::OffsetReg(offset) => {
                    if !offset.add {
                        arg_cb(InstructionPart::basic("-"))?;
                    }
                    arg_cb(InstructionPart::opaque(
                        offset.reg.display(display_options.reg_names).to_string(),
                    ))?;
                }
                args::Argument::CpsrMode(mode) => {
                    arg_cb(InstructionPart::basic("#"))?;
                    arg_cb(InstructionPart::unsigned(mode.mode))?;
                    if mode.writeback {
                        arg_cb(InstructionPart::opaque("!"))?;
                    }
                }
                args::Argument::CoReg(_)
                | args::Argument::StatusReg(_)
                | args::Argument::StatusMask(_)
                | args::Argument::Shift(_)
                | args::Argument::CpsrFlags(_)
                | args::Argument::Endian(_) => {
                    arg_cb(InstructionPart::opaque(
                        arg.display(display_options, None).to_string(),
                    ))?;
                }
            }
        }
    }
    if deref {
        arg_cb(InstructionPart::basic("]"))?;
        if writeback {
            arg_cb(InstructionPart::opaque("!"))?;
        }
    }

    let branch_dest = get_pc_relative_load_address(ins, cur_addr);
    if let Some(branch_dest) = branch_dest {
        arg_cb(InstructionPart::basic(" (->"))?;
        arg_cb(InstructionPart::branch_dest(branch_dest))?;
        arg_cb(InstructionPart::basic(")"))?;
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

fn get_pc_relative_load_address(ins: unarm::Ins, address: u32) -> Option<u32> {
    match ins {
        unarm::Ins::Arm(ins)
            if ins.op == arm::Opcode::Ldr
                && ins.modifier_addr_ldr_str() == arm::AddrLdrStr::Imm
                && ins.field_rn_deref().reg == args::Register::Pc =>
        {
            let offset = ins.field_offset_12().value;
            Some(address.wrapping_add_signed(offset + 8))
        }
        unarm::Ins::Thumb(ins) if ins.op == thumb::Opcode::LdrPc => {
            let offset = ins.field_rel_immed_8().value;
            Some((address & !3).wrapping_add_signed(offset + 4))
        }
        _ => None,
    }
}

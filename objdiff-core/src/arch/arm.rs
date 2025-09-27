use alloc::{collections::BTreeMap, vec::Vec};
use core::fmt::Write;
use std::borrow::Cow;

use anyhow::{Result, bail};
use arm_attr::{BuildAttrs, enums::CpuArch, tag::Tag};
use object::{Endian as _, Object as _, ObjectSection as _, ObjectSymbol as _, elf};
use unarm::parse_thumb;

use crate::{
    arch::{Arch, OPCODE_DATA, OPCODE_INVALID, RelocationOverride, RelocationOverrideTarget},
    diff::{ArmArchVersion, ArmR9Usage, DiffObjConfig, display::InstructionPart},
    obj::{
        InstructionRef, Relocation, RelocationFlags, ResolvedInstructionRef, Section, SectionKind,
        Symbol, SymbolFlag, SymbolFlagSet, SymbolKind,
    },
};

#[derive(Debug)]
pub struct ArchArm {
    /// Maps section index, to list of disasm modes (arm, thumb or data) sorted by address
    disasm_modes: BTreeMap<usize, Vec<DisasmMode>>,
    detected_version: Option<unarm::Version>,
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

    fn elf_detect_arm_version(file: &object::File) -> Result<Option<unarm::Version>> {
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
                    Some(CpuArch::V4) => return Ok(Some(unarm::Version::V4)),
                    Some(CpuArch::V4T) => return Ok(Some(unarm::Version::V4T)),
                    Some(CpuArch::V5TE) => return Ok(Some(unarm::Version::V5Te)),
                    Some(CpuArch::V5TEJ) => return Ok(Some(unarm::Version::V5Tej)),
                    Some(CpuArch::V6) => return Ok(Some(unarm::Version::V6)),
                    Some(CpuArch::V6K) => return Ok(Some(unarm::Version::V6K)),
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

    fn unarm_options(&self, diff_config: &DiffObjConfig) -> unarm::Options {
        unarm::Options {
            version: match diff_config.arm_arch_version {
                ArmArchVersion::Auto => self.detected_version.unwrap_or(unarm::Version::V5Te),
                ArmArchVersion::V4 => unarm::Version::V4,
                ArmArchVersion::V4t => unarm::Version::V4T,
                ArmArchVersion::V5t => unarm::Version::V5T,
                ArmArchVersion::V5te => unarm::Version::V5Te,
                ArmArchVersion::V5tej => unarm::Version::V5Tej,
                ArmArchVersion::V6 => unarm::Version::V6,
                ArmArchVersion::V6k => unarm::Version::V6K,
            },
            extensions: unarm::Extensions::all(), // TODO: Add checkboxes for extensions
            av: diff_config.arm_av_registers,
            r9_use: match diff_config.arm_r9_usage {
                ArmR9Usage::GeneralPurpose => unarm::R9Use::R9,
                ArmR9Usage::Sb => unarm::R9Use::Sb,
                ArmR9Usage::Tr => unarm::R9Use::Tr,
            },
            sl: diff_config.arm_sl_usage,
            fp: diff_config.arm_fp_usage,
            ip: diff_config.arm_ip_usage,
            ual: diff_config.arm_unified_syntax,
        }
    }

    fn parse_ins_ref(
        &self,
        ins_ref: InstructionRef,
        code: &[u8],
        diff_config: &DiffObjConfig,
    ) -> Result<unarm::Ins> {
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

        let thumb = ins_ref.opcode & (1 << 15) == 0;
        let options = self.unarm_options(diff_config);

        // TODO: Optimize parsing by providing the opcode discriminant
        let ins = if ins_ref.opcode == OPCODE_DATA {
            match ins_ref.size {
                4 => unarm::Ins::Word(code),
                2 => unarm::Ins::HalfWord(code as u16),
                _ => bail!("Invalid data size {}", ins_ref.size),
            }
        } else if thumb {
            let (ins, _size) = unarm::parse_thumb(code, ins_ref.address as u32, &options);
            ins
        } else {
            unarm::parse_arm(code, ins_ref.address as u32, &options)
        };
        Ok(ins)
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

        let min_ins_size = if mode == unarm::ParseMode::Thumb { 2 } else { 4 };
        let ins_count = code.len() / min_ins_size;
        let mut ops = Vec::<InstructionRef>::with_capacity(ins_count);

        let options = self.unarm_options(diff_config);

        let mut address = start_addr;
        while address < end_addr {
            while let Some(next) = next_mapping.filter(|x| address >= x.address) {
                // Change mapping
                mode = next.mapping;
                next_mapping = mappings_iter.next();
            }

            let data = &code[(address - start_addr) as usize..];
            if data.len() < min_ins_size {
                // Push the remainder as data
                ops.push(InstructionRef {
                    address: address as u64,
                    size: data.len() as u8,
                    opcode: OPCODE_DATA,
                    branch_dest: None,
                });
                break;
            }

            // Check how many bytes we can/should read
            let num_code_bytes = if data.len() >= 4 {
                // Read 4 bytes even for Thumb, as the parser will determine if it's a 2 or 4 byte instruction
                4
            } else if mode != unarm::ParseMode::Arm {
                2
            } else {
                // Invalid instruction size
                ops.push(InstructionRef {
                    address: address as u64,
                    size: min_ins_size as u8,
                    opcode: OPCODE_INVALID,
                    branch_dest: None,
                });
                address += min_ins_size as u32;
                continue;
            };

            let code = match num_code_bytes {
                4 => match self.endianness {
                    object::Endianness::Little => {
                        u32::from_le_bytes([data[0], data[1], data[2], data[3]])
                    }
                    object::Endianness::Big => {
                        u32::from_be_bytes([data[0], data[1], data[2], data[3]])
                    }
                },
                2 => match self.endianness {
                    object::Endianness::Little => u16::from_le_bytes([data[0], data[1]]) as u32,
                    object::Endianness::Big => u16::from_be_bytes([data[0], data[1]]) as u32,
                },
                _ => unreachable!(),
            };

            let (opcode, ins, ins_size) = match mode {
                unarm::ParseMode::Arm => {
                    let ins = unarm::parse_arm(code, address, &options);
                    let opcode = ins.discriminant() | (1 << 15);
                    (opcode, ins, 4)
                }
                unarm::ParseMode::Thumb => {
                    let (ins, size) = parse_thumb(code, address, &options);
                    let opcode = ins.discriminant();
                    (opcode, ins, size)
                }
                unarm::ParseMode::Data => (
                    OPCODE_DATA,
                    if num_code_bytes == 4 {
                        unarm::Ins::Word(code)
                    } else {
                        unarm::Ins::HalfWord(code as u16)
                    },
                    num_code_bytes,
                ),
            };

            let branch_dest = match ins {
                unarm::Ins::B { target, .. }
                | unarm::Ins::Bl { target, .. }
                | unarm::Ins::Blx { target: unarm::BlxTarget::Direct(target), .. } => {
                    Some(target.addr)
                }
                _ => None,
            };

            ops.push(InstructionRef {
                address: address as u64,
                size: ins_size as u8,
                opcode,
                branch_dest: branch_dest.map(|x| x as u64),
            });
            address += ins_size;
        }

        Ok(ops)
    }

    fn display_instruction(
        &self,
        resolved: ResolvedInstructionRef,
        diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        let ins = self.parse_ins_ref(resolved.ins_ref, resolved.code, diff_config)?;

        let options = self.unarm_options(diff_config);
        let mut string_fmt = unarm::StringFormatter::new(&options);
        ins.write_opcode(&mut string_fmt)?;
        let opcode = string_fmt.into_string();
        cb(InstructionPart::opcode(opcode, resolved.ins_ref.opcode))?;

        let mut args_formatter = ArgsFormatter { options: &options, cb, resolved: &resolved };
        ins.write_params(&mut args_formatter)?;
        Ok(())
    }

    fn relocation_override(
        &self,
        _file: &object::File<'_>,
        section: &object::Section,
        address: u64,
        relocation: &object::Relocation,
    ) -> Result<Option<RelocationOverride>> {
        match relocation.flags() {
            // Handle ELF implicit relocations
            object::RelocationFlags::Elf { r_type } => {
                if relocation.has_implicit_addend() {
                    let section_data = section.data()?;
                    let address = address as usize;
                    let addend = match r_type {
                        // ARM calls
                        elf::R_ARM_PC24 | elf::R_ARM_XPC25 | elf::R_ARM_CALL => {
                            let data = section_data[address..address + 4].try_into()?;
                            let addend = self.endianness.read_i32_bytes(data);
                            let imm24 = addend & 0xffffff;
                            (imm24 << 2) << 8 >> 8
                        }

                        // Thumb calls
                        elf::R_ARM_THM_PC22 | elf::R_ARM_THM_XPC22 => {
                            let data = section_data[address..address + 2].try_into()?;
                            let high = self.endianness.read_i16_bytes(data) as i32;
                            let data = section_data[address + 2..address + 4].try_into()?;
                            let low = self.endianness.read_i16_bytes(data) as i32;

                            let imm22 = ((high & 0x7ff) << 11) | (low & 0x7ff);
                            (imm22 << 1) << 9 >> 9
                        }

                        // Data
                        elf::R_ARM_ABS32 => {
                            let data = section_data[address..address + 4].try_into()?;
                            self.endianness.read_i32_bytes(data)
                        }

                        flags => bail!("Unsupported ARM implicit relocation {flags:?}"),
                    };
                    Ok(Some(RelocationOverride {
                        target: RelocationOverrideTarget::Keep,
                        addend: addend as i64,
                    }))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
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

    fn infer_function_size(
        &self,
        symbol: &Symbol,
        section: &Section,
        mut next_address: u64,
    ) -> Result<u64> {
        // Trim any trailing 4-byte zeroes from the end (padding)
        while next_address >= symbol.address + 4
            && let Some(data) = section.data_range(next_address - 4, 4)
            && data == [0u8; 4]
        {
            next_address -= 4;
        }
        Ok(next_address.saturating_sub(symbol.address))
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

pub struct ArgsFormatter<'a> {
    options: &'a unarm::Options,
    cb: &'a mut dyn FnMut(InstructionPart) -> Result<()>,
    resolved: &'a ResolvedInstructionRef<'a>,
}

impl ArgsFormatter<'_> {
    fn write(&mut self, part: InstructionPart) -> core::fmt::Result {
        (self.cb)(part).map_err(|_| core::fmt::Error)
    }
}

impl Write for ArgsFormatter<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result { self.write(InstructionPart::basic(s)) }
}

impl unarm::Write for ArgsFormatter<'_> {
    fn options(&self) -> &unarm::Options { self.options }

    fn write_ins(&mut self, ins: &unarm::Ins) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        ins.write_opcode(&mut string_fmt)?;
        let opcode = string_fmt.into_string();
        self.write(InstructionPart::Opcode(Cow::Owned(opcode), self.resolved.ins_ref.opcode))?;
        ins.write_params(self)
    }

    fn write_separator(&mut self) -> core::fmt::Result { self.write(InstructionPart::separator()) }

    fn write_uimm(&mut self, uimm: u32) -> core::fmt::Result {
        if let Some(resolved) = self.resolved.relocation
            && let RelocationFlags::Elf(elf::R_ARM_ABS32) = resolved.relocation.flags
        {
            return self.write(InstructionPart::reloc());
        }
        self.write(InstructionPart::unsigned(uimm))
    }

    fn write_simm(&mut self, simm: i32) -> core::fmt::Result {
        self.write(InstructionPart::signed(simm))
    }

    fn write_branch_target(&mut self, branch_target: unarm::BranchTarget) -> core::fmt::Result {
        if let Some(resolved) = self.resolved.relocation {
            match resolved.relocation.flags {
                RelocationFlags::Elf(elf::R_ARM_THM_XPC22)
                | RelocationFlags::Elf(elf::R_ARM_THM_PC22)
                | RelocationFlags::Elf(elf::R_ARM_PC24)
                | RelocationFlags::Elf(elf::R_ARM_XPC25)
                | RelocationFlags::Elf(elf::R_ARM_CALL) => {
                    return self.write(InstructionPart::reloc());
                }
                _ => {}
            }
        }
        self.write(InstructionPart::branch_dest(branch_target.addr))
    }

    fn write_reg(&mut self, reg: unarm::Reg) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        reg.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_status_reg(&mut self, status_reg: unarm::StatusReg) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        status_reg.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_status_fields(&mut self, status_fields: unarm::StatusFields) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        status_fields.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_shift_op(&mut self, shift_op: unarm::ShiftOp) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        shift_op.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_coproc(&mut self, coproc: unarm::Coproc) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        coproc.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_co_reg(&mut self, co_reg: unarm::CoReg) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        co_reg.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_aif_flags(&mut self, aif_flags: unarm::AifFlags) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        aif_flags.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_endianness(&mut self, endianness: unarm::Endianness) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        endianness.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_sreg(&mut self, sreg: unarm::Sreg) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        sreg.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_dreg(&mut self, dreg: unarm::Dreg) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        dreg.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_fpscr(&mut self, fpscr: unarm::Fpscr) -> core::fmt::Result {
        let mut string_fmt = unarm::StringFormatter::new(self.options);
        fpscr.write(&mut string_fmt)?;
        self.write(InstructionPart::opaque(string_fmt.into_string()))?;
        Ok(())
    }

    fn write_addr_ldr_str(&mut self, addr_ldr_str: unarm::AddrLdrStr) -> core::fmt::Result {
        addr_ldr_str.write(self)?;
        if let unarm::AddrLdrStr::Pre {
            rn: unarm::Reg::Pc,
            offset: unarm::LdrStrOffset::Imm(offset),
            ..
        } = addr_ldr_str
        {
            let thumb = self.resolved.ins_ref.opcode & (1 << 15) == 0;
            let pc_offset = if thumb { 4 } else { 8 };
            let pc = (self.resolved.ins_ref.address as u32 & !3) + pc_offset;
            self.write(InstructionPart::basic(" (->"))?;
            self.write(InstructionPart::branch_dest(pc.wrapping_add(offset as u32)))?;
            self.write(InstructionPart::basic(")"))?;
        }
        Ok(())
    }
}

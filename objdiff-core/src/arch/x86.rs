use alloc::{boxed::Box, format, string::String, vec::Vec};
use core::cmp::Ordering;

use anyhow::{Context, Result, anyhow, bail};
use iced_x86::{
    Decoder, DecoderOptions, DecoratorKind, FormatterOutput, FormatterTextKind, GasFormatter,
    Instruction, IntelFormatter, MasmFormatter, NasmFormatter, NumberKind, OpKind, Register,
};
use object::{Endian as _, Object as _, ObjectSection as _, pe};

use crate::{
    arch::Arch,
    diff::{DiffObjConfig, X86Formatter, display::InstructionPart},
    obj::{
        InstructionRef, Relocation, RelocationFlags, ResolvedInstructionRef, ScannedInstruction,
    },
};

#[derive(Debug)]
pub struct ArchX86 {
    arch: Architecture,
    endianness: object::Endianness,
}

#[derive(Debug)]
enum Architecture {
    X86,
    X86_64,
}

impl ArchX86 {
    pub fn new(object: &object::File) -> Result<Self> {
        let arch = match object.architecture() {
            object::Architecture::I386 => Architecture::X86,
            object::Architecture::X86_64 => Architecture::X86_64,
            _ => bail!("Unsupported architecture for ArchX86: {:?}", object.architecture()),
        };
        Ok(Self { arch, endianness: object.endianness() })
    }

    fn decoder<'a>(&self, code: &'a [u8], address: u64) -> Decoder<'a> {
        Decoder::with_ip(
            match self.arch {
                Architecture::X86 => 32,
                Architecture::X86_64 => 64,
            },
            code,
            address,
            DecoderOptions::NONE,
        )
    }

    fn formatter(&self, diff_config: &DiffObjConfig) -> Box<dyn iced_x86::Formatter> {
        let mut formatter: Box<dyn iced_x86::Formatter> = match diff_config.x86_formatter {
            X86Formatter::Intel => Box::new(IntelFormatter::new()),
            X86Formatter::Gas => Box::new(GasFormatter::new()),
            X86Formatter::Nasm => Box::new(NasmFormatter::new()),
            X86Formatter::Masm => Box::new(MasmFormatter::new()),
        };
        formatter.options_mut().set_space_after_operand_separator(diff_config.space_between_args);
        formatter
    }

    fn reloc_size(&self, flags: RelocationFlags) -> Option<usize> {
        match self.arch {
            Architecture::X86 => match flags {
                RelocationFlags::Coff(typ) => match typ {
                    pe::IMAGE_REL_I386_DIR16 | pe::IMAGE_REL_I386_REL16 => Some(2),
                    pe::IMAGE_REL_I386_DIR32 | pe::IMAGE_REL_I386_REL32 => Some(4),
                    _ => None,
                },
                _ => None,
            },
            Architecture::X86_64 => match flags {
                RelocationFlags::Coff(typ) => match typ {
                    pe::IMAGE_REL_AMD64_ADDR32NB | pe::IMAGE_REL_AMD64_REL32 => Some(4),
                    pe::IMAGE_REL_AMD64_ADDR64 => Some(8),
                    _ => None,
                },
                _ => None,
            },
        }
    }
}

const DATA_OPCODE: u16 = u16::MAX - 1;

impl Arch for ArchX86 {
    fn scan_instructions(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        relocations: &[Relocation],
        _diff_config: &DiffObjConfig,
    ) -> Result<Vec<ScannedInstruction>> {
        let mut out = Vec::with_capacity(code.len() / 2);
        let mut decoder = self.decoder(code, address);
        let mut instruction = Instruction::default();
        let mut reloc_iter = relocations.iter().peekable();
        'outer: while decoder.can_decode() {
            let address = decoder.ip();
            while let Some(reloc) = reloc_iter.peek() {
                match reloc.address.cmp(&address) {
                    Ordering::Less => {
                        reloc_iter.next();
                    }
                    Ordering::Equal => {
                        // If the instruction starts at a relocation, it's inline data
                        let size = self.reloc_size(reloc.flags).with_context(|| {
                            format!("Unsupported inline x86 relocation {:?}", reloc.flags)
                        })?;
                        if decoder.set_position(decoder.position() + size).is_ok() {
                            decoder.set_ip(address + size as u64);
                            out.push(ScannedInstruction {
                                ins_ref: InstructionRef {
                                    address,
                                    size: size as u8,
                                    opcode: DATA_OPCODE,
                                },
                                branch_dest: None,
                            });
                            reloc_iter.next();
                            continue 'outer;
                        }
                    }
                    Ordering::Greater => break,
                }
            }
            decoder.decode_out(&mut instruction);
            let branch_dest = match instruction.op0_kind() {
                OpKind::NearBranch16 => Some(instruction.near_branch16() as u64),
                OpKind::NearBranch32 => Some(instruction.near_branch32() as u64),
                OpKind::NearBranch64 => Some(instruction.near_branch64()),
                _ => None,
            };
            out.push(ScannedInstruction {
                ins_ref: InstructionRef {
                    address,
                    size: instruction.len() as u8,
                    opcode: instruction.mnemonic() as u16,
                },
                branch_dest,
            });
        }
        Ok(out)
    }

    fn display_instruction(
        &self,
        resolved: ResolvedInstructionRef,
        diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        if resolved.ins_ref.opcode == DATA_OPCODE {
            let (mnemonic, imm) = match resolved.ins_ref.size {
                2 => (".word", self.endianness.read_u16_bytes(resolved.code.try_into()?) as u64),
                4 => (".dword", self.endianness.read_u32_bytes(resolved.code.try_into()?) as u64),
                _ => bail!("Unsupported x86 inline data size {}", resolved.ins_ref.size),
            };
            cb(InstructionPart::opcode(mnemonic, DATA_OPCODE))?;
            if resolved.relocation.is_some() {
                cb(InstructionPart::reloc())?;
            } else {
                cb(InstructionPart::unsigned(imm))?;
            }
            return Ok(());
        }

        let mut decoder = self.decoder(resolved.code, resolved.ins_ref.address);
        let mut formatter = self.formatter(diff_config);
        let mut instruction = Instruction::default();
        decoder.decode_out(&mut instruction);

        // Determine where to insert relocation in instruction output.
        // We replace the immediate or displacement with a placeholder value since the formatter
        // doesn't provide enough information to know which number is the displacement inside a
        // memory operand.
        let mut reloc_replace = None;
        if let Some(reloc) = resolved.relocation {
            const PLACEHOLDER: u64 = 0x7BDE3E7D; // chosen by fair dice roll. guaranteed to be random.
            let reloc_offset = reloc.relocation.address - resolved.ins_ref.address;
            let reloc_size = self.reloc_size(reloc.relocation.flags).unwrap_or(usize::MAX);
            let offsets = decoder.get_constant_offsets(&instruction);
            if reloc_offset == offsets.displacement_offset() as u64
                && reloc_size == offsets.displacement_size()
            {
                instruction.set_memory_displacement64(PLACEHOLDER);
                // Formatter always writes the displacement as Int32
                reloc_replace = Some((OpKind::Memory, 4, PLACEHOLDER));
            } else if reloc_offset == offsets.immediate_offset() as u64
                && reloc_size == offsets.immediate_size()
            {
                let is_branch = matches!(
                    instruction.op0_kind(),
                    OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
                );
                let op_kind = if is_branch {
                    instruction.op0_kind()
                } else {
                    match reloc_size {
                        2 => OpKind::Immediate16,
                        4 => OpKind::Immediate32,
                        8 => OpKind::Immediate64,
                        _ => OpKind::default(),
                    }
                };
                if is_branch {
                    instruction.set_near_branch64(PLACEHOLDER);
                } else {
                    instruction.set_immediate32(PLACEHOLDER as u32);
                }
                reloc_replace = Some((op_kind, reloc_size, PLACEHOLDER));
            }
        }

        let mut output =
            InstructionFormatterOutput { cb, reloc_replace, error: None, skip_next: false };
        formatter.format(&instruction, &mut output);
        if let Some(error) = output.error.take() {
            return Err(error);
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
        match self.arch {
            Architecture::X86 => match flags {
                RelocationFlags::Coff(pe::IMAGE_REL_I386_DIR32 | pe::IMAGE_REL_I386_REL32) => {
                    let data =
                        section.data()?[address as usize..address as usize + 4].try_into()?;
                    Ok(self.endianness.read_i32_bytes(data) as i64)
                }
                flags => bail!("Unsupported x86 implicit relocation {flags:?}"),
            },
            Architecture::X86_64 => match flags {
                RelocationFlags::Coff(pe::IMAGE_REL_AMD64_ADDR32NB | pe::IMAGE_REL_AMD64_REL32) => {
                    let data =
                        section.data()?[address as usize..address as usize + 4].try_into()?;
                    Ok(self.endianness.read_i32_bytes(data) as i64)
                }
                RelocationFlags::Coff(pe::IMAGE_REL_AMD64_ADDR64) => {
                    let data =
                        section.data()?[address as usize..address as usize + 8].try_into()?;
                    Ok(self.endianness.read_i64_bytes(data))
                }
                flags => bail!("Unsupported x86-64 implicit relocation {flags:?}"),
            },
        }
    }

    fn demangle(&self, name: &str) -> Option<String> {
        if name.starts_with('?') {
            msvc_demangler::demangle(name, msvc_demangler::DemangleFlags::llvm()).ok()
        } else {
            cpp_demangle::Symbol::new(name)
                .ok()
                .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
        }
    }

    fn reloc_name(&self, flags: RelocationFlags) -> Option<&'static str> {
        match self.arch {
            Architecture::X86 => match flags {
                RelocationFlags::Coff(typ) => match typ {
                    pe::IMAGE_REL_I386_DIR32 => Some("IMAGE_REL_I386_DIR32"),
                    pe::IMAGE_REL_I386_REL32 => Some("IMAGE_REL_I386_REL32"),
                    _ => None,
                },
                _ => None,
            },
            Architecture::X86_64 => match flags {
                RelocationFlags::Coff(typ) => match typ {
                    pe::IMAGE_REL_AMD64_ADDR64 => Some("IMAGE_REL_AMD64_ADDR64"),
                    pe::IMAGE_REL_AMD64_ADDR32NB => Some("IMAGE_REL_AMD64_ADDR32NB"),
                    pe::IMAGE_REL_AMD64_REL32 => Some("IMAGE_REL_AMD64_REL32"),
                    _ => None,
                },
                _ => None,
            },
        }
    }

    fn data_reloc_size(&self, flags: RelocationFlags) -> usize {
        self.reloc_size(flags).unwrap_or(1)
    }
}

struct InstructionFormatterOutput<'a> {
    cb: &'a mut dyn FnMut(InstructionPart<'_>) -> Result<()>,
    reloc_replace: Option<(OpKind, usize, u64)>,
    error: Option<anyhow::Error>,
    skip_next: bool,
}

impl InstructionFormatterOutput<'_> {
    fn push_signed(&mut self, mut value: i64) {
        if self.error.is_some() {
            return;
        }
        // The formatter writes the '-' operator and then gives us a negative value,
        // so convert it to a positive value to avoid double negatives
        if value < 0 {
            value = value.wrapping_abs();
        }
        if let Err(e) = (self.cb)(InstructionPart::signed(value)) {
            self.error = Some(e);
        }
    }
}

impl FormatterOutput for InstructionFormatterOutput<'_> {
    fn write(&mut self, text: &str, kind: FormatterTextKind) {
        if self.error.is_some() {
            return;
        }
        // Skip whitespace after the mnemonic
        if self.skip_next {
            self.skip_next = false;
            if kind == FormatterTextKind::Text && text == " " {
                return;
            }
        }
        match kind {
            FormatterTextKind::Text | FormatterTextKind::Punctuation => {
                if let Err(e) = (self.cb)(InstructionPart::basic(text)) {
                    self.error = Some(e);
                }
            }
            FormatterTextKind::Prefix
            | FormatterTextKind::Keyword
            | FormatterTextKind::Operator => {
                if let Err(e) = (self.cb)(InstructionPart::opaque(text)) {
                    self.error = Some(e);
                }
            }
            _ => self.error = Some(anyhow!("x86: Unsupported FormatterTextKind {:?}", kind)),
        }
    }

    fn write_mnemonic(&mut self, instruction: &Instruction, text: &str) {
        if self.error.is_some() {
            return;
        }
        if let Err(e) = (self.cb)(InstructionPart::opcode(text, instruction.mnemonic() as u16)) {
            self.error = Some(e);
        }
        // Skip whitespace after the mnemonic
        self.skip_next = true;
    }

    fn write_number(
        &mut self,
        instruction: &Instruction,
        _operand: u32,
        instruction_operand: Option<u32>,
        _text: &str,
        value: u64,
        number_kind: NumberKind,
        kind: FormatterTextKind,
    ) {
        if self.error.is_some() {
            return;
        }

        if let (Some(operand), Some((target_op_kind, reloc_size, target_value))) =
            (instruction_operand, self.reloc_replace)
        {
            #[allow(clippy::match_like_matches_macro)]
            if instruction.op_kind(operand) == target_op_kind
                && match (number_kind, reloc_size) {
                    (NumberKind::Int8 | NumberKind::UInt8, 1)
                    | (NumberKind::Int16 | NumberKind::UInt16, 2)
                    | (NumberKind::Int32 | NumberKind::UInt32, 4)
                    | (NumberKind::Int64 | NumberKind::UInt64, 4) // x86_64
                    | (NumberKind::Int64 | NumberKind::UInt64, 8) => true,
                    _ => false,
                }
                && value == target_value
            {
                if let Err(e) = (self.cb)(InstructionPart::reloc()) {
                    self.error = Some(e);
                }
                return;
            }
        }

        if let FormatterTextKind::LabelAddress | FormatterTextKind::FunctionAddress = kind {
            if let Err(e) = (self.cb)(InstructionPart::branch_dest(value)) {
                self.error = Some(e);
            }
            return;
        }

        match number_kind {
            NumberKind::Int8 => self.push_signed(value as i8 as i64),
            NumberKind::Int16 => self.push_signed(value as i16 as i64),
            NumberKind::Int32 => self.push_signed(value as i32 as i64),
            NumberKind::Int64 => self.push_signed(value as i64),
            NumberKind::UInt8 | NumberKind::UInt16 | NumberKind::UInt32 | NumberKind::UInt64 => {
                if let Err(e) = (self.cb)(InstructionPart::unsigned(value)) {
                    self.error = Some(e);
                }
            }
        }
    }

    fn write_decorator(
        &mut self,
        _instruction: &Instruction,
        _operand: u32,
        _instruction_operand: Option<u32>,
        text: &str,
        _decorator: DecoratorKind,
    ) {
        if self.error.is_some() {
            return;
        }
        if let Err(e) = (self.cb)(InstructionPart::basic(text)) {
            self.error = Some(e);
        }
    }

    fn write_register(
        &mut self,
        _instruction: &Instruction,
        _operand: u32,
        _instruction_operand: Option<u32>,
        text: &str,
        _register: Register,
    ) {
        if self.error.is_some() {
            return;
        }
        if let Err(e) = (self.cb)(InstructionPart::opaque(text)) {
            self.error = Some(e);
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::obj::{Relocation, ResolvedRelocation};

    #[test]
    fn test_scan_instructions() {
        let arch = ArchX86 { arch: Architecture::X86, endianness: object::Endianness::Little };
        let code = [
            0xc7, 0x85, 0x68, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0x8b, 0x04, 0x85, 0x00,
            0x00, 0x00, 0x00,
        ];
        let scanned = arch.scan_instructions(0, &code, 0, &[], &DiffObjConfig::default()).unwrap();
        assert_eq!(scanned.len(), 2);
        assert_eq!(scanned[0].ins_ref.address, 0);
        assert_eq!(scanned[0].ins_ref.size, 10);
        assert_eq!(scanned[0].ins_ref.opcode, iced_x86::Mnemonic::Mov as u16);
        assert_eq!(scanned[0].branch_dest, None);
        assert_eq!(scanned[1].ins_ref.address, 10);
        assert_eq!(scanned[1].ins_ref.size, 7);
        assert_eq!(scanned[1].ins_ref.opcode, iced_x86::Mnemonic::Mov as u16);
        assert_eq!(scanned[1].branch_dest, None);
    }

    #[test]
    fn test_process_instruction() {
        let arch = ArchX86 { arch: Architecture::X86, endianness: object::Endianness::Little };
        let code = [0xc7, 0x85, 0x68, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00];
        let opcode = iced_x86::Mnemonic::Mov as u16;
        let mut parts = Vec::new();
        arch.display_instruction(
            ResolvedInstructionRef {
                ins_ref: InstructionRef { address: 0x1234, size: 10, opcode },
                code: &code,
                ..Default::default()
            },
            &DiffObjConfig::default(),
            &mut |part| {
                parts.push(part.into_static());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(parts, &[
            InstructionPart::opcode("mov", opcode),
            InstructionPart::opaque("dword"),
            InstructionPart::basic(" "),
            InstructionPart::opaque("ptr"),
            InstructionPart::basic(" "),
            InstructionPart::basic("["),
            InstructionPart::opaque("ebp"),
            InstructionPart::opaque("-"),
            InstructionPart::signed(152i64),
            InstructionPart::basic("]"),
            InstructionPart::basic(","),
            InstructionPart::basic(" "),
            InstructionPart::unsigned(0u64),
        ]);
    }

    #[test]
    fn test_process_instruction_with_reloc_1() {
        let arch = ArchX86 { arch: Architecture::X86, endianness: object::Endianness::Little };
        let code = [0xc7, 0x85, 0x68, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00];
        let opcode = iced_x86::Mnemonic::Mov as u16;
        let mut parts = Vec::new();
        arch.display_instruction(
            ResolvedInstructionRef {
                ins_ref: InstructionRef { address: 0x1234, size: 10, opcode },
                code: &code,
                relocation: Some(ResolvedRelocation {
                    relocation: &Relocation {
                        flags: RelocationFlags::Coff(pe::IMAGE_REL_I386_DIR32),
                        address: 0x1234 + 6,
                        target_symbol: 0,
                        addend: 0,
                    },
                    symbol: &Default::default(),
                }),
                ..Default::default()
            },
            &DiffObjConfig::default(),
            &mut |part| {
                parts.push(part.into_static());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(parts, &[
            InstructionPart::opcode("mov", opcode),
            InstructionPart::opaque("dword"),
            InstructionPart::basic(" "),
            InstructionPart::opaque("ptr"),
            InstructionPart::basic(" "),
            InstructionPart::basic("["),
            InstructionPart::opaque("ebp"),
            InstructionPart::opaque("-"),
            InstructionPart::signed(152i64),
            InstructionPart::basic("]"),
            InstructionPart::basic(","),
            InstructionPart::basic(" "),
            InstructionPart::reloc(),
        ]);
    }

    #[test]
    fn test_process_instruction_with_reloc_2() {
        let arch = ArchX86 { arch: Architecture::X86, endianness: object::Endianness::Little };
        let code = [0x8b, 0x04, 0x85, 0x00, 0x00, 0x00, 0x00];
        let opcode = iced_x86::Mnemonic::Mov as u16;
        let mut parts = Vec::new();
        arch.display_instruction(
            ResolvedInstructionRef {
                ins_ref: InstructionRef { address: 0x1234, size: 7, opcode },
                code: &code,
                relocation: Some(ResolvedRelocation {
                    relocation: &Relocation {
                        flags: RelocationFlags::Coff(pe::IMAGE_REL_I386_DIR32),
                        address: 0x1234 + 3,
                        target_symbol: 0,
                        addend: 0,
                    },
                    symbol: &Default::default(),
                }),
                ..Default::default()
            },
            &DiffObjConfig::default(),
            &mut |part| {
                parts.push(part.into_static());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(parts, &[
            InstructionPart::opcode("mov", opcode),
            InstructionPart::opaque("eax"),
            InstructionPart::basic(","),
            InstructionPart::basic(" "),
            InstructionPart::basic("["),
            InstructionPart::opaque("eax"),
            InstructionPart::opaque("*"),
            InstructionPart::signed(4),
            InstructionPart::opaque("+"),
            InstructionPart::reloc(),
            InstructionPart::basic("]"),
        ]);
    }

    #[test]
    fn test_process_instruction_with_reloc_3() {
        let arch = ArchX86 { arch: Architecture::X86, endianness: object::Endianness::Little };
        let code = [0xe8, 0x00, 0x00, 0x00, 0x00];
        let opcode = iced_x86::Mnemonic::Call as u16;
        let mut parts = Vec::new();
        arch.display_instruction(
            ResolvedInstructionRef {
                ins_ref: InstructionRef { address: 0x1234, size: 5, opcode },
                code: &code,
                relocation: Some(ResolvedRelocation {
                    relocation: &Relocation {
                        flags: RelocationFlags::Coff(pe::IMAGE_REL_I386_REL32),
                        address: 0x1234 + 1,
                        target_symbol: 0,
                        addend: 0,
                    },
                    symbol: &Default::default(),
                }),
                ..Default::default()
            },
            &DiffObjConfig::default(),
            &mut |part| {
                parts.push(part.into_static());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(parts, &[InstructionPart::opcode("call", opcode), InstructionPart::reloc()]);
    }

    #[test]
    fn test_process_instruction_with_reloc_4() {
        let arch = ArchX86 { arch: Architecture::X86, endianness: object::Endianness::Little };
        let code = [0x8b, 0x15, 0xa4, 0x21, 0x7e, 0x00];
        let opcode = iced_x86::Mnemonic::Mov as u16;
        let mut parts = Vec::new();
        arch.display_instruction(
            ResolvedInstructionRef {
                ins_ref: InstructionRef { address: 0x1234, size: 6, opcode },
                code: &code,
                relocation: Some(ResolvedRelocation {
                    relocation: &Relocation {
                        flags: RelocationFlags::Coff(pe::IMAGE_REL_I386_DIR32),
                        address: 0x1234 + 2,
                        target_symbol: 0,
                        addend: 0,
                    },
                    symbol: &Default::default(),
                }),
                ..Default::default()
            },
            &DiffObjConfig::default(),
            &mut |part| {
                parts.push(part.into_static());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(parts, &[
            InstructionPart::opcode("mov", opcode),
            InstructionPart::opaque("edx"),
            InstructionPart::basic(","),
            InstructionPart::basic(" "),
            InstructionPart::basic("["),
            InstructionPart::reloc(),
            InstructionPart::basic("]"),
        ]);
    }

    #[test]
    fn test_process_x86_64_instruction_with_reloc_1() {
        let arch = ArchX86 { arch: Architecture::X86_64, endianness: object::Endianness::Little };
        let code = [0x48, 0x8b, 0x05, 0x00, 0x00, 0x00, 0x00];
        let opcode = iced_x86::Mnemonic::Mov as u16;
        let mut parts = Vec::new();
        arch.display_instruction(
            ResolvedInstructionRef {
                ins_ref: InstructionRef { address: 0x1234, size: 7, opcode },
                code: &code,
                relocation: Some(ResolvedRelocation {
                    relocation: &Relocation {
                        flags: RelocationFlags::Coff(pe::IMAGE_REL_AMD64_REL32),
                        address: 0x1234 + 3,
                        target_symbol: 0,
                        addend: 0,
                    },
                    symbol: &Default::default(),
                }),
                ..Default::default()
            },
            &DiffObjConfig::default(),
            &mut |part| {
                parts.push(part.into_static());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(parts, &[
            InstructionPart::opcode("mov", opcode),
            InstructionPart::opaque("rax"),
            InstructionPart::basic(","),
            InstructionPart::basic(" "),
            InstructionPart::basic("["),
            InstructionPart::reloc(),
            InstructionPart::basic("]"),
        ]);
    }

    #[test]
    fn test_process_x86_64_instruction_with_reloc_2() {
        let arch = ArchX86 { arch: Architecture::X86_64, endianness: object::Endianness::Little };
        let code = [0xe8, 0x00, 0x00, 0x00, 0x00];
        let opcode = iced_x86::Mnemonic::Call as u16;
        let mut parts = Vec::new();
        arch.display_instruction(
            ResolvedInstructionRef {
                ins_ref: InstructionRef { address: 0x1234, size: 5, opcode },
                code: &code,
                relocation: Some(ResolvedRelocation {
                    relocation: &Relocation {
                        flags: RelocationFlags::Coff(pe::IMAGE_REL_AMD64_REL32),
                        address: 0x1234 + 1,
                        target_symbol: 0,
                        addend: 0,
                    },
                    symbol: &Default::default(),
                }),
                ..Default::default()
            },
            &DiffObjConfig::default(),
            &mut |part| {
                parts.push(part.into_static());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(parts, &[InstructionPart::opcode("call", opcode), InstructionPart::reloc()]);
    }
}

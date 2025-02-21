use alloc::{
    borrow::Cow,
    boxed::Box,
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use std::ops::Range;

use anyhow::{anyhow, bail, ensure, Result};
use iced_x86::{
    Decoder, DecoderOptions, DecoratorKind, FormatterOutput, FormatterTextKind, GasFormatter,
    Instruction, IntelFormatter, MasmFormatter, NasmFormatter, NumberKind, OpKind, PrefixKind,
    Register,
};
use object::{pe, Endian as _, Object as _, ObjectSection as _};

use crate::{
    arch::Arch,
    diff::{display::InstructionPart, DiffObjConfig, X86Formatter},
    obj::{
        InstructionArg, InstructionArgValue, InstructionRef, ParsedInstruction, RelocationFlags,
        ResolvedRelocation, ScannedInstruction,
    },
};

#[derive(Debug)]
pub struct ArchX86 {
    bits: u32,
    endianness: object::Endianness,
}

impl ArchX86 {
    pub fn new(object: &object::File) -> Result<Self> {
        Ok(Self { bits: if object.is_64() { 64 } else { 32 }, endianness: object.endianness() })
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
}

impl Arch for ArchX86 {
    fn scan_instructions(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        _diff_config: &DiffObjConfig,
    ) -> Result<Vec<ScannedInstruction>> {
        let mut out = Vec::with_capacity(code.len() / 2);
        let mut decoder = Decoder::with_ip(self.bits, code, address, DecoderOptions::NONE);
        let mut instruction = Instruction::default();
        while decoder.can_decode() {
            decoder.decode_out(&mut instruction);
            // TODO is this right?
            let branch_dest = match instruction.op0_kind() {
                OpKind::NearBranch16 => Some(instruction.near_branch16() as u64),
                OpKind::NearBranch32 => Some(instruction.near_branch32() as u64),
                OpKind::NearBranch64 => Some(instruction.near_branch64()),
                _ => None,
            };
            out.push(ScannedInstruction {
                ins_ref: InstructionRef {
                    address: instruction.ip(),
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
        ins_ref: InstructionRef,
        code: &[u8],
        relocation: Option<ResolvedRelocation>,
        function_range: Range<u64>,
        section_index: usize,
        diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        todo!()
    }

    fn process_code(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        relocations: &[ObjReloc],
        line_info: &BTreeMap<u64, u32>,
        config: &DiffObjConfig,
    ) -> Result<ProcessCodeResult> {
        let mut result = ProcessCodeResult { ops: Vec::new(), insts: Vec::new() };
        let mut decoder = Decoder::with_ip(self.bits, code, address, DecoderOptions::NONE);
        let mut formatter = self.formatter(config);

        let mut output = InstructionFormatterOutput {
            formatted: String::new(),
            ins: ObjIns {
                address: 0,
                size: 0,
                op: 0,
                mnemonic: Cow::Borrowed("<invalid>"),
                args: vec![],
                reloc: None,
                branch_dest: None,
                line: None,
                formatted: String::new(),
                orig: None,
            },
            error: None,
            ins_operands: vec![],
        };
        let mut instruction = Instruction::default();
        while decoder.can_decode() {
            decoder.decode_out(&mut instruction);

            let address = instruction.ip();
            let op = instruction.mnemonic() as u16;
            let reloc = relocations
                .iter()
                .find(|r| r.address >= address && r.address < address + instruction.len() as u64);
            let line = line_info.range(..=address).last().map(|(_, &b)| b);
            output.ins = ObjIns {
                address,
                size: instruction.len() as u8,
                op,
                mnemonic: Cow::Borrowed("<invalid>"),
                args: vec![],
                reloc: reloc.cloned(),
                branch_dest: None,
                line,
                formatted: String::new(),
                orig: None,
            };
            // Run the formatter, which will populate output.ins
            formatter.format(&instruction, &mut output);
            if let Some(error) = output.error.take() {
                return Err(error);
            }
            ensure!(output.ins_operands.len() == output.ins.args.len());
            output.ins.formatted.clone_from(&output.formatted);

            // Make sure we've put the relocation somewhere in the instruction
            if reloc.is_some()
                && !output.ins.args.iter().any(|a| matches!(a, InstructionArg::Reloc))
            {
                let mut found = replace_arg(
                    OpKind::Memory,
                    InstructionArg::Reloc,
                    &mut output.ins.args,
                    &instruction,
                    &output.ins_operands,
                )?;
                if !found {
                    found = replace_arg(
                        OpKind::Immediate32,
                        InstructionArg::Reloc,
                        &mut output.ins.args,
                        &instruction,
                        &output.ins_operands,
                    )?;
                }
                ensure!(found, "x86: Failed to find operand for Absolute relocation");
            }
            if reloc.is_some()
                && !output.ins.args.iter().any(|a| matches!(a, InstructionArg::Reloc))
            {
                bail!("Failed to find relocation in instruction");
            }

            result.ops.push(op);
            result.insts.push(output.ins.clone());

            // Clear for next iteration
            output.formatted.clear();
            output.ins_operands.clear();
        }
        Ok(result)
    }

    fn implcit_addend(
        &self,
        _file: &object::File<'_>,
        section: &object::Section,
        address: u64,
        _relocation: &object::Relocation,
        flags: RelocationFlags,
    ) -> Result<i64> {
        match flags {
            RelocationFlags::Coff(pe::IMAGE_REL_I386_DIR32 | pe::IMAGE_REL_I386_REL32) => {
                let data = section.data()[address as usize..address as usize + 4].try_into()?;
                Ok(self.endianness.read_i32_bytes(data) as i64)
            }
            flags => bail!("Unsupported x86 implicit relocation {flags:?}"),
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

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        match flags {
            RelocationFlags::Coff(typ) => match typ {
                pe::IMAGE_REL_I386_DIR32 => Cow::Borrowed("IMAGE_REL_I386_DIR32"),
                pe::IMAGE_REL_I386_REL32 => Cow::Borrowed("IMAGE_REL_I386_REL32"),
                _ => Cow::Owned(format!("<{flags:?}>")),
            },
            _ => Cow::Owned(format!("<{flags:?}>")),
        }
    }

    fn get_reloc_byte_size(&self, flags: RelocationFlags) -> usize {
        match flags {
            RelocationFlags::Coff(typ) => match typ {
                pe::IMAGE_REL_I386_DIR16 => 2,
                pe::IMAGE_REL_I386_REL16 => 2,
                pe::IMAGE_REL_I386_DIR32 => 4,
                pe::IMAGE_REL_I386_REL32 => 4,
                _ => 1,
            },
            _ => 1,
        }
    }
}

fn replace_arg(
    from: OpKind,
    to: InstructionArg,
    args: &mut [InstructionArg],
    instruction: &Instruction,
    ins_operands: &[Option<u32>],
) -> Result<bool> {
    let mut replace = None;
    for i in 0..instruction.op_count() {
        let op_kind = instruction.op_kind(i);
        if op_kind == from {
            replace = Some(i);
            break;
        }
    }
    if let Some(i) = replace {
        for (j, arg) in args.iter_mut().enumerate() {
            if ins_operands[j] == Some(i) {
                *arg = to;
                return Ok(true);
            }
        }
    }
    Ok(false)
}

struct InstructionFormatterOutput {
    formatted: String,
    ins: ParsedInstruction,
    error: Option<anyhow::Error>,
    ins_operands: Vec<Option<u32>>,
}

impl InstructionFormatterOutput {
    fn push_signed(&mut self, value: i64) {
        // The formatter writes the '-' operator and then gives us a negative value,
        // so convert it to a positive value to avoid double negatives
        if value < 0
            && matches!(self.ins.args.last(), Some(InstructionArg::Value(InstructionArgValue::Opaque(v))) if v == "-")
        {
            self.ins
                .args
                .push(InstructionArg::Value(InstructionArgValue::Signed(value.wrapping_abs())));
        } else {
            self.ins.args.push(InstructionArg::Value(InstructionArgValue::Signed(value)));
        }
    }
}

impl FormatterOutput for InstructionFormatterOutput {
    fn write(&mut self, text: &str, kind: FormatterTextKind) {
        self.formatted.push_str(text);
        // Skip whitespace after the mnemonic
        if self.ins.args.is_empty() && kind == FormatterTextKind::Text {
            return;
        }
        self.ins_operands.push(None);
        match kind {
            FormatterTextKind::Text | FormatterTextKind::Punctuation => {
                self.ins.args.push(InstructionArg::PlainText(text.to_string().into()));
            }
            FormatterTextKind::Keyword | FormatterTextKind::Operator => {
                self.ins.args.push(InstructionArg::Value(InstructionArgValue::Opaque(
                    text.to_string().into(),
                )));
            }
            _ => {
                if self.error.is_none() {
                    self.error = Some(anyhow!("x86: Unsupported FormatterTextKind {:?}", kind));
                }
            }
        }
    }

    fn write_prefix(&mut self, _instruction: &Instruction, text: &str, _prefix: PrefixKind) {
        self.formatted.push_str(text);
        self.ins_operands.push(None);
        self.ins
            .args
            .push(InstructionArg::Value(InstructionArgValue::Opaque(text.to_string().into())));
    }

    fn write_mnemonic(&mut self, _instruction: &Instruction, text: &str) {
        self.formatted.push_str(text);
        self.ins.mnemonic = Cow::Owned(text.to_string());
    }

    fn write_number(
        &mut self,
        _instruction: &Instruction,
        _operand: u32,
        instruction_operand: Option<u32>,
        text: &str,
        value: u64,
        number_kind: NumberKind,
        kind: FormatterTextKind,
    ) {
        self.formatted.push_str(text);
        self.ins_operands.push(instruction_operand);

        // Handle relocations
        match kind {
            FormatterTextKind::LabelAddress => {
                if let Some(reloc) = self.ins.reloc.as_ref() {
                    if matches!(
                        reloc.flags,
                        RelocationFlags::Coff(pe::IMAGE_REL_I386_DIR32 | pe::IMAGE_REL_I386_REL32)
                    ) {
                        self.ins.args.push(InstructionArg::Reloc);
                        return;
                    } else if self.error.is_none() {
                        self.error = Some(anyhow!(
                            "x86: Unsupported LabelAddress relocation flags {:?}",
                            reloc.flags
                        ));
                    }
                }
                self.ins.args.push(InstructionArg::BranchDest(value));
                self.ins.branch_dest = Some(value);
                return;
            }
            FormatterTextKind::FunctionAddress => {
                if let Some(reloc) = self.ins.reloc.as_ref() {
                    if matches!(reloc.flags, RelocationFlags::Coff(pe::IMAGE_REL_I386_REL32)) {
                        self.ins.args.push(InstructionArg::Reloc);
                        return;
                    } else if self.error.is_none() {
                        self.error = Some(anyhow!(
                            "x86: Unsupported FunctionAddress relocation flags {:?}",
                            reloc.flags
                        ));
                    }
                }
            }
            _ => {}
        }

        match number_kind {
            NumberKind::Int8 => {
                self.push_signed(value as i8 as i64);
            }
            NumberKind::Int16 => {
                self.push_signed(value as i16 as i64);
            }
            NumberKind::Int32 => {
                self.push_signed(value as i32 as i64);
            }
            NumberKind::Int64 => {
                self.push_signed(value as i64);
            }
            NumberKind::UInt8 | NumberKind::UInt16 | NumberKind::UInt32 | NumberKind::UInt64 => {
                self.ins.args.push(InstructionArg::Value(InstructionArgValue::Unsigned(value)));
            }
        }
    }

    fn write_decorator(
        &mut self,
        _instruction: &Instruction,
        _operand: u32,
        instruction_operand: Option<u32>,
        text: &str,
        _decorator: DecoratorKind,
    ) {
        self.formatted.push_str(text);
        self.ins_operands.push(instruction_operand);
        self.ins.args.push(InstructionArg::PlainText(text.to_string().into()));
    }

    fn write_register(
        &mut self,
        _instruction: &Instruction,
        _operand: u32,
        instruction_operand: Option<u32>,
        text: &str,
        _register: Register,
    ) {
        self.formatted.push_str(text);
        self.ins_operands.push(instruction_operand);
        self.ins
            .args
            .push(InstructionArg::Value(InstructionArgValue::Opaque(text.to_string().into())));
    }
}

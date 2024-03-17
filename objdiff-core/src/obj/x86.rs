use std::collections::BTreeMap;

use anyhow::{anyhow, bail, ensure, Result};
use iced_x86::{
    Decoder, DecoderOptions, DecoratorKind, Formatter, FormatterOutput, FormatterTextKind,
    GasFormatter, Instruction, IntelFormatter, MasmFormatter, NasmFormatter, NumberKind, OpKind,
    PrefixKind, Register, SymbolResult,
};

use crate::{
    diff::{DiffObjConfig, ProcessCodeResult},
    obj::{ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjRelocKind},
};

#[derive(Debug, Copy, Clone, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum X86Formatter {
    #[default]
    Intel,
    Gas,
    Nasm,
    Masm,
}

pub fn process_code(
    config: &DiffObjConfig,
    data: &[u8],
    bitness: u32,
    start_address: u64,
    relocs: &[ObjReloc],
    line_info: &Option<BTreeMap<u64, u64>>,
) -> Result<ProcessCodeResult> {
    let mut result = ProcessCodeResult { ops: Vec::new(), insts: Vec::new() };
    let mut decoder = Decoder::with_ip(bitness, data, start_address, DecoderOptions::NONE);
    let mut formatter: Box<dyn Formatter> = match config.x86_formatter {
        X86Formatter::Intel => Box::new(IntelFormatter::new()),
        X86Formatter::Gas => Box::new(GasFormatter::new()),
        X86Formatter::Nasm => Box::new(NasmFormatter::new()),
        X86Formatter::Masm => Box::new(MasmFormatter::new()),
    };
    formatter.options_mut().set_space_after_operand_separator(config.space_between_args);

    let mut output = InstructionFormatterOutput {
        formatted: String::new(),
        ins: ObjIns {
            address: 0,
            size: 0,
            op: 0,
            mnemonic: "".to_string(),
            args: vec![],
            reloc: None,
            branch_dest: None,
            line: None,
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
        let reloc = relocs
            .iter()
            .find(|r| r.address >= address && r.address < address + instruction.len() as u64);
        output.ins = ObjIns {
            address,
            size: instruction.len() as u8,
            op,
            mnemonic: "".to_string(),
            args: vec![],
            reloc: reloc.cloned(),
            branch_dest: None,
            line: line_info.as_ref().and_then(|m| m.get(&address).cloned()),
            orig: None,
        };
        // Run the formatter, which will populate output.ins
        formatter.format(&instruction, &mut output);
        if let Some(error) = output.error.take() {
            return Err(error);
        }
        ensure!(output.ins_operands.len() == output.ins.args.len());
        output.ins.orig = Some(output.formatted.clone());

        // print!("{:016X} ", instruction.ip());
        // let start_index = (instruction.ip() - address) as usize;
        // let instr_bytes = &data[start_index..start_index + instruction.len()];
        // for b in instr_bytes.iter() {
        //     print!("{:02X}", b);
        // }
        // if instr_bytes.len() < 32 {
        //     for _ in 0..32 - instr_bytes.len() {
        //         print!("  ");
        //     }
        // }
        // println!(" {}", output.formatted);
        //
        // if let Some(reloc) = reloc {
        //     println!("\tReloc: {:?}", reloc);
        // }
        //
        // for i in 0..instruction.op_count() {
        //     let kind = instruction.op_kind(i);
        //     print!("{:?} ", kind);
        // }
        // println!();

        // Make sure we've put the relocation somewhere in the instruction
        if reloc.is_some() && !output.ins.args.iter().any(|a| matches!(a, ObjInsArg::Reloc)) {
            let mut found = replace_arg(
                OpKind::Memory,
                ObjInsArg::Reloc,
                &mut output.ins.args,
                &instruction,
                &output.ins_operands,
            )?;
            if !found {
                found = replace_arg(
                    OpKind::Immediate32,
                    ObjInsArg::Reloc,
                    &mut output.ins.args,
                    &instruction,
                    &output.ins_operands,
                )?;
            }
            ensure!(found, "x86: Failed to find operand for Absolute relocation");
        }
        if reloc.is_some() && !output.ins.args.iter().any(|a| matches!(a, ObjInsArg::Reloc)) {
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

fn replace_arg(
    from: OpKind,
    to: ObjInsArg,
    args: &mut [ObjInsArg],
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
    ins: ObjIns,
    error: Option<anyhow::Error>,
    ins_operands: Vec<Option<u32>>,
}

impl InstructionFormatterOutput {
    fn push_signed(&mut self, value: i64) {
        // The formatter writes the '-' operator and then gives us a negative value,
        // so convert it to a positive value to avoid double negatives
        if value < 0
            && matches!(self.ins.args.last(), Some(ObjInsArg::Arg(ObjInsArgValue::Opaque(v))) if v == "-")
        {
            self.ins.args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(value.wrapping_abs())));
        } else {
            self.ins.args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(value)));
        }
    }
}

impl FormatterOutput for InstructionFormatterOutput {
    fn write(&mut self, text: &str, kind: FormatterTextKind) {
        // log::debug!("write {} {:?}", text, kind);
        self.formatted.push_str(text);
        // Skip whitespace after the mnemonic
        if self.ins.args.is_empty() && kind == FormatterTextKind::Text {
            return;
        }
        self.ins_operands.push(None);
        match kind {
            FormatterTextKind::Text | FormatterTextKind::Punctuation => {
                self.ins.args.push(ObjInsArg::PlainText(text.to_string()));
            }
            FormatterTextKind::Keyword | FormatterTextKind::Operator => {
                self.ins.args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(text.to_string())));
            }
            _ => {
                if self.error.is_none() {
                    self.error = Some(anyhow!("x86: Unsupported FormatterTextKind {:?}", kind));
                }
            }
        }
    }

    fn write_prefix(&mut self, _instruction: &Instruction, text: &str, _prefix: PrefixKind) {
        // log::debug!("write_prefix {} {:?}", text, prefix);
        self.formatted.push_str(text);
        self.ins_operands.push(None);
        self.ins.args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(text.to_string())));
    }

    fn write_mnemonic(&mut self, _instruction: &Instruction, text: &str) {
        // log::debug!("write_mnemonic {}", text);
        self.formatted.push_str(text);
        self.ins.mnemonic = text.to_string();
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
        // log::debug!("write_number {} {:?} {} {} {:?} {:?}", operand, instruction_operand, text, value, number_kind, kind);
        self.formatted.push_str(text);
        self.ins_operands.push(instruction_operand);

        // Handle relocations
        match kind {
            FormatterTextKind::LabelAddress => {
                if let Some(reloc) = self.ins.reloc.as_ref() {
                    if reloc.kind == ObjRelocKind::Absolute {
                        self.ins.args.push(ObjInsArg::Reloc);
                        return;
                    } else if self.error.is_none() {
                        self.error = Some(anyhow!(
                            "x86: Unsupported LabelAddress relocation kind {:?}",
                            reloc.kind
                        ));
                    }
                }
                self.ins.args.push(ObjInsArg::BranchDest(value));
                self.ins.branch_dest = Some(value);
                return;
            }
            FormatterTextKind::FunctionAddress => {
                if let Some(reloc) = self.ins.reloc.as_ref() {
                    if reloc.kind == ObjRelocKind::X86PcRel32 {
                        self.ins.args.push(ObjInsArg::Reloc);
                        return;
                    } else if self.error.is_none() {
                        self.error = Some(anyhow!(
                            "x86: Unsupported FunctionAddress relocation kind {:?}",
                            reloc.kind
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
                self.ins.args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(value)));
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
        // log::debug!("write_decorator {} {:?} {} {:?}", operand, instruction_operand, text, decorator);
        self.formatted.push_str(text);
        self.ins_operands.push(instruction_operand);
        self.ins.args.push(ObjInsArg::PlainText(text.to_string()));
    }

    fn write_register(
        &mut self,
        _instruction: &Instruction,
        _operand: u32,
        instruction_operand: Option<u32>,
        text: &str,
        _register: Register,
    ) {
        // log::debug!("write_register {} {:?} {} {:?}", operand, instruction_operand, text, register);
        self.formatted.push_str(text);
        self.ins_operands.push(instruction_operand);
        self.ins.args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(text.to_string())));
    }

    fn write_symbol(
        &mut self,
        _instruction: &Instruction,
        _operand: u32,
        _instruction_operand: Option<u32>,
        _address: u64,
        _symbol: &SymbolResult<'_>,
    ) {
        if self.error.is_none() {
            self.error = Some(anyhow!("x86: Unsupported write_symbol"));
        }
    }
}

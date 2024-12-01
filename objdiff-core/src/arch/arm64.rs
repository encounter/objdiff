use std::{borrow::Cow, cmp::Ordering, collections::BTreeMap};

use anyhow::{bail, Result};
use object::{elf, File, Relocation, RelocationFlags};
use yaxpeax_arch::{Arch, Decoder, Reader, U8Reader};
use yaxpeax_arm::armv8::a64::{
    ARMv8, DecodeError, InstDecoder, Instruction, Opcode, Operand, SIMDSizeCode, ShiftStyle,
    SizeCode,
};

use crate::{
    arch::{ObjArch, ProcessCodeResult},
    diff::DiffObjConfig,
    obj::{ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjSection},
};

pub struct ObjArchArm64 {}

impl ObjArchArm64 {
    pub fn new(_file: &File) -> Result<Self> { Ok(Self {}) }
}

impl ObjArch for ObjArchArm64 {
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
        let start_address = address;
        let end_address = address + code.len() as u64;
        let ins_count = code.len() / 4;

        let mut ops = Vec::with_capacity(ins_count);
        let mut insts = Vec::with_capacity(ins_count);

        let mut reader = U8Reader::new(code);
        let decoder = InstDecoder::default();
        let mut ins = Instruction::default();
        loop {
            // This is ridiculous...
            let address =
                start_address + <U8Reader<'_> as Reader<<ARMv8 as Arch>::Address, <ARMv8 as Arch>::Word>>::total_offset(&mut reader);
            match decoder.decode_into(&mut ins, &mut reader) {
                Ok(()) => {}
                Err(e) => match e {
                    DecodeError::ExhaustedInput => break,
                    DecodeError::InvalidOpcode
                    | DecodeError::InvalidOperand
                    | DecodeError::IncompleteDecoder => {
                        ops.push(u16::MAX);
                        insts.push(ObjIns {
                            address,
                            size: 4,
                            op: u16::MAX,
                            mnemonic: Cow::Borrowed("<invalid>"),
                            args: vec![],
                            reloc: None,
                            fake_pool_reloc: None,
                            branch_dest: None,
                            line: None,
                            formatted: "".to_string(),
                            orig: None,
                        });
                        continue;
                    }
                },
            }

            let line = line_info.range(..=address).last().map(|(_, &b)| b);
            let reloc = relocations.iter().find(|r| (r.address & !3) == address).cloned();

            let mut args = vec![];
            let mut ctx = DisplayCtx {
                address,
                section_index,
                start_address,
                end_address,
                reloc: reloc.as_ref(),
                config,
                branch_dest: None,
            };
            // Simplify instruction and process args
            let mnemonic = display_instruction(&mut args, &ins, &mut ctx);

            // Format the instruction without simplification
            let mut orig = ins.opcode.to_string();
            for (i, o) in ins.operands.iter().enumerate() {
                if let Operand::Nothing = o {
                    break;
                }
                if i == 0 {
                    orig.push(' ');
                } else {
                    orig.push_str(", ");
                }
                orig.push_str(o.to_string().as_str());
            }

            if let Some(reloc) = &reloc {
                if !args.iter().any(|a| matches!(a, ObjInsArg::Reloc)) {
                    args.push(ObjInsArg::PlainText(Cow::Borrowed(" <unhandled relocation>")));
                    log::warn!(
                        "Unhandled ARM64 relocation {:?}: {} @ {:#X}",
                        reloc.flags,
                        orig,
                        address
                    );
                }
            };

            let op = opcode_to_u16(ins.opcode);
            ops.push(op);
            let branch_dest = ctx.branch_dest;
            insts.push(ObjIns {
                address,
                size: 4,
                op,
                mnemonic: Cow::Borrowed(mnemonic),
                args,
                reloc,
                fake_pool_reloc: None,
                branch_dest,
                line,
                formatted: ins.to_string(),
                orig: Some(orig),
            });
        }

        Ok(ProcessCodeResult { ops, insts })
    }

    fn implcit_addend(
        &self,
        _file: &File<'_>,
        _section: &ObjSection,
        address: u64,
        reloc: &Relocation,
    ) -> Result<i64> {
        bail!("Unsupported ARM64 implicit relocation {:#x}:{:?}", address, reloc.flags())
    }

    fn demangle(&self, name: &str) -> Option<String> {
        cpp_demangle::Symbol::new(name)
            .ok()
            .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
    }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        match flags {
            RelocationFlags::Elf { r_type: elf::R_AARCH64_ADR_PREL_PG_HI21 } => {
                Cow::Borrowed("R_AARCH64_ADR_PREL_PG_HI21")
            }
            RelocationFlags::Elf { r_type: elf::R_AARCH64_ADD_ABS_LO12_NC } => {
                Cow::Borrowed("R_AARCH64_ADD_ABS_LO12_NC")
            }
            RelocationFlags::Elf { r_type: elf::R_AARCH64_JUMP26 } => {
                Cow::Borrowed("R_AARCH64_JUMP26")
            }
            RelocationFlags::Elf { r_type: elf::R_AARCH64_CALL26 } => {
                Cow::Borrowed("R_AARCH64_CALL26")
            }
            RelocationFlags::Elf { r_type: elf::R_AARCH64_LDST32_ABS_LO12_NC } => {
                Cow::Borrowed("R_AARCH64_LDST32_ABS_LO12_NC")
            }
            RelocationFlags::Elf { r_type: elf::R_AARCH64_ADR_GOT_PAGE } => {
                Cow::Borrowed("R_AARCH64_ADR_GOT_PAGE")
            }
            RelocationFlags::Elf { r_type: elf::R_AARCH64_LD64_GOT_LO12_NC } => {
                Cow::Borrowed("R_AARCH64_LD64_GOT_LO12_NC")
            }
            _ => Cow::Owned(format!("<{flags:?}>")),
        }
    }
}

struct DisplayCtx<'a> {
    address: u64,
    section_index: usize,
    start_address: u64,
    end_address: u64,
    reloc: Option<&'a ObjReloc>,
    config: &'a DiffObjConfig,
    branch_dest: Option<u64>,
}

// Source: https://github.com/iximeow/yaxpeax-arm/blob/716a6e3fc621f5fe3300f3309e56943b8e1e65ad/src/armv8/a64.rs#L317
// License: 0BSD
// Reworked for more structured output. The library only gives us a Display impl, and no way to
// capture any of this information, so it needs to be reimplemented here.
fn display_instruction(
    args: &mut Vec<ObjInsArg>,
    ins: &Instruction,
    ctx: &mut DisplayCtx,
) -> &'static str {
    let mnemonic = match ins.opcode {
        Opcode::Invalid => return "<invalid>",
        Opcode::UDF => "udf",
        Opcode::MOVN => {
            let imm = if let Operand::ImmShift(imm, shift) = ins.operands[1] {
                !((imm as u64) << shift)
            } else {
                unreachable!("movn operand 1 is always ImmShift");
            };
            let imm = if let Operand::Register(size, _) = ins.operands[0] {
                if size == SizeCode::W {
                    imm as u32 as u64
                } else {
                    imm
                }
            } else {
                unreachable!("movn operand 0 is always Register");
            };
            push_operand(args, &ins.operands[0], ctx);
            push_separator(args, ctx.config);
            push_unsigned(args, imm);
            return "mov";
        }
        Opcode::MOVK => "movk",
        Opcode::MOVZ => {
            let imm = if let Operand::ImmShift(imm, shift) = ins.operands[1] {
                (imm as u64) << shift
            } else {
                unreachable!("movz operand is always ImmShift");
            };
            let imm = if let Operand::Register(size, _) = ins.operands[0] {
                if size == SizeCode::W {
                    imm as u32 as u64
                } else {
                    imm
                }
            } else {
                unreachable!("movz operand 0 is always Register");
            };
            push_operand(args, &ins.operands[0], ctx);
            push_separator(args, ctx.config);
            push_unsigned(args, imm);
            return "mov";
        }
        Opcode::ADC => "adc",
        Opcode::ADCS => "adcs",
        Opcode::SBC => {
            if let Operand::Register(_, 31) = ins.operands[1] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "ngc";
            } else {
                "sbc"
            }
        }
        Opcode::SBCS => {
            if let Operand::Register(_, 31) = ins.operands[1] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "ngcs";
            } else {
                "sbcs"
            }
        }
        Opcode::AND => "and",
        Opcode::ORR => {
            if let Operand::Register(_, 31) = ins.operands[1] {
                if let Operand::Immediate(0) = ins.operands[2] {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    return "mov";
                } else if let Operand::RegShift(style, amt, size, r) = ins.operands[2] {
                    if style == ShiftStyle::LSL && amt == 0 {
                        push_operand(args, &ins.operands[0], ctx);
                        push_separator(args, ctx.config);
                        push_register(args, size, r, false);
                        return "mov";
                    }
                } else {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return "mov";
                }
            } else if ins.operands[1] == ins.operands[2] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                return "mov";
            }
            "orr"
        }
        Opcode::ORN => {
            if let Operand::Register(_, 31) = ins.operands[1] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "mvn";
            }
            "orn"
        }
        Opcode::EOR => "eor",
        Opcode::EON => "eon",
        Opcode::BIC => "bic",
        Opcode::BICS => "bics",
        Opcode::ANDS => {
            if let Operand::Register(_, 31) = ins.operands[0] {
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "tst";
            }
            "ands"
        }
        Opcode::ADDS => {
            if let Operand::Register(_, 31) = ins.operands[0] {
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "cmn";
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = ins.operands[2] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_register(args, size, reg, false);
                return "adds";
            }
            "adds"
        }
        Opcode::ADD => {
            if let Operand::Immediate(0) = ins.operands[2] {
                if let Operand::RegisterOrSP(size, 31) = ins.operands[0] {
                    push_register(args, size, 31, true);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    return "mov";
                } else if let Operand::RegisterOrSP(size, 31) = ins.operands[1] {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_register(args, size, 31, true);
                    return "mov";
                }
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = ins.operands[2] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_register(args, size, reg, false);
                return "add";
            }
            "add"
        }
        Opcode::SUBS => {
            if let Operand::Register(_, 31) = ins.operands[0] {
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "cmp";
            } else if let Operand::Register(_, 31) = ins.operands[1] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "negs";
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = ins.operands[2] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_register(args, size, reg, false);
                return "subs";
            }
            "subs"
        }
        Opcode::SUB => {
            if let Operand::Register(_, 31) = ins.operands[1] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "neg";
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = ins.operands[2] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_register(args, size, reg, false);
                return "sub";
            }
            "sub"
        }
        Opcode::BFM => {
            if let (Operand::Immediate(immr), Operand::Immediate(imms)) =
                (ins.operands[2], ins.operands[3])
            {
                if imms < immr {
                    if let Operand::Register(sz, rn) = ins.operands[1] {
                        let width = imms + 1;
                        let lsb = if sz == SizeCode::W {
                            ((-(immr as i8)) as u8) & 0x1f
                        } else {
                            ((-(immr as i8)) as u8) & 0x3f
                        };
                        return if rn == 31 {
                            push_operand(args, &ins.operands[0], ctx);
                            push_separator(args, ctx.config);
                            push_unsigned(args, lsb as u64);
                            push_separator(args, ctx.config);
                            push_unsigned(args, width as u64);
                            "bfc"
                        } else {
                            push_operand(args, &ins.operands[0], ctx);
                            push_separator(args, ctx.config);
                            push_operand(args, &ins.operands[1], ctx);
                            push_separator(args, ctx.config);
                            push_unsigned(args, lsb as u64);
                            push_separator(args, ctx.config);
                            push_unsigned(args, width as u64);
                            "bfi"
                        };
                    }
                } else {
                    // bfxil
                    let lsb = immr;
                    let width = imms + 1 - lsb;
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    push_separator(args, ctx.config);
                    push_unsigned(args, lsb as u64);
                    push_separator(args, ctx.config);
                    push_unsigned(args, width as u64);
                    return "bfxil";
                }
            }
            "bfm"
        }
        Opcode::UBFM => {
            // TODO: handle ubfx alias
            if let (
                Operand::Register(SizeCode::W, _),
                Operand::Register(SizeCode::W, _),
                Operand::Immediate(0),
            ) = (ins.operands[0], ins.operands[1], ins.operands[2])
            {
                if let Operand::Immediate(7) = ins.operands[3] {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    return "uxtb";
                } else if let Operand::Immediate(15) = ins.operands[3] {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    return "uxth";
                }
            }
            if let Operand::Immediate(imms) = ins.operands[3] {
                let size = if let Operand::Register(size, _) = ins.operands[0] {
                    size
                } else {
                    unreachable!("operand 0 is a register");
                };
                match (imms, size) {
                    (63, SizeCode::X) | (31, SizeCode::W) => {
                        push_operand(args, &ins.operands[0], ctx);
                        push_separator(args, ctx.config);
                        push_operand(args, &ins.operands[1], ctx);
                        push_separator(args, ctx.config);
                        push_operand(args, &ins.operands[2], ctx);
                        return "lsr";
                    }
                    _ => {
                        let size = if size == SizeCode::X { 64 } else { 32 };
                        let immr = if let Operand::Immediate(immr) = ins.operands[2] {
                            immr
                        } else {
                            unreachable!("operand 3 is a register");
                        };
                        if imms + 1 == immr {
                            push_operand(args, &ins.operands[0], ctx);
                            push_separator(args, ctx.config);
                            push_operand(args, &ins.operands[1], ctx);
                            push_separator(args, ctx.config);
                            push_unsigned(args, (size - imms - 1) as u64);
                            return "lsl";
                        }
                        if imms < immr {
                            push_operand(args, &ins.operands[0], ctx);
                            push_separator(args, ctx.config);
                            push_operand(args, &ins.operands[1], ctx);
                            push_separator(args, ctx.config);
                            push_unsigned(args, (size - immr) as u64);
                            push_separator(args, ctx.config);
                            push_unsigned(args, (imms + 1) as u64);
                            return "ubfiz";
                        }
                    }
                }
            }
            // `ubfm` is never actually displayed: in the remaining case, it is always aliased to `ubfx`
            let width = if let (Operand::Immediate(lsb), Operand::Immediate(width)) =
                (ins.operands[2], ins.operands[3])
            {
                Operand::Immediate(width - lsb + 1)
            } else {
                unreachable!("last two operands of ubfm are always immediates");
            };
            push_operand(args, &ins.operands[0], ctx);
            push_separator(args, ctx.config);
            push_operand(args, &ins.operands[1], ctx);
            push_separator(args, ctx.config);
            push_operand(args, &ins.operands[2], ctx);
            push_separator(args, ctx.config);
            push_operand(args, &width, ctx);
            return "ubfx";
        }
        Opcode::SBFM => {
            if let Operand::Immediate(63) = ins.operands[3] {
                if let Operand::Register(SizeCode::X, _) = ins.operands[0] {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return "asr";
                }
            }
            if let Operand::Immediate(31) = ins.operands[3] {
                if let Operand::Register(SizeCode::W, _) = ins.operands[0] {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return "asr";
                }
            }
            if let Operand::Immediate(0) = ins.operands[2] {
                let newsrc = if let Operand::Register(_size, srcnum) = ins.operands[1] {
                    Operand::Register(SizeCode::W, srcnum)
                } else {
                    unreachable!("operand 1 is always a register");
                };
                if let Operand::Immediate(7) = ins.operands[3] {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &newsrc, ctx);
                    return "sxtb";
                } else if let Operand::Immediate(15) = ins.operands[3] {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &newsrc, ctx);
                    return "sxth";
                } else if let Operand::Immediate(31) = ins.operands[3] {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &newsrc, ctx);
                    return "sxtw";
                }
            }
            if let (Operand::Immediate(imms), Operand::Immediate(immr)) =
                (ins.operands[2], ins.operands[3])
            {
                if immr < imms {
                    let size = if let Operand::Register(size, _) = ins.operands[0] {
                        if size == SizeCode::W {
                            32
                        } else {
                            64
                        }
                    } else {
                        unreachable!("operand 0 is always a register");
                    };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    push_separator(args, ctx.config);
                    push_unsigned(args, (size - imms) as u64);
                    push_separator(args, ctx.config);
                    push_unsigned(args, (immr + 1) as u64);
                    return "sbfiz";
                }
            }
            // `sbfm` is never actually displayed: in the remaining case, it is always aliased to `sbfx`
            let width = if let (Operand::Immediate(lsb), Operand::Immediate(width)) =
                (ins.operands[2], ins.operands[3])
            {
                Operand::Immediate(width - lsb + 1)
            } else {
                unreachable!("last two operands of sbfm are always immediates");
            };
            push_operand(args, &ins.operands[0], ctx);
            push_separator(args, ctx.config);
            push_operand(args, &ins.operands[1], ctx);
            push_separator(args, ctx.config);
            push_operand(args, &ins.operands[2], ctx);
            push_separator(args, ctx.config);
            push_operand(args, &width, ctx);
            return "sbfx";
        }
        Opcode::ADR => "adr",
        Opcode::ADRP => "adrp",
        Opcode::EXTR => {
            if let (Operand::Register(_, rn), Operand::Register(_, rm)) =
                (ins.operands[1], ins.operands[2])
            {
                if rn == rm {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[3], ctx);
                    return "ror";
                }
            }
            "extr"
        }
        Opcode::LDAR => "ldar",
        Opcode::LDLAR => "ldlar",
        Opcode::LDARB => "ldarb",
        Opcode::LDLARB => "ldlarb",
        Opcode::LDAXRB => "ldaxrb",
        Opcode::LDARH => "ldarh",
        Opcode::LDLARH => "ldlarh",
        Opcode::LDAXP => "ldaxp",
        Opcode::LDAXR => "ldaxr",
        Opcode::LDAXRH => "ldaxrh",
        Opcode::LDP => "ldp",
        Opcode::LDPSW => "ldpsw",
        Opcode::LDR => "ldr",
        Opcode::LDRB => "ldrb",
        Opcode::LDRSB => "ldrsb",
        Opcode::LDRSW => "ldrsw",
        Opcode::LDRSH => "ldrsh",
        Opcode::LDRH => "ldrh",
        Opcode::LDTR => "ldtr",
        Opcode::LDTRB => "ldtrb",
        Opcode::LDTRH => "ldtrh",
        Opcode::LDTRSB => "ldtrsb",
        Opcode::LDTRSH => "ldtrsh",
        Opcode::LDTRSW => "ldtrsw",
        Opcode::LDUR => "ldur",
        Opcode::LDURB => "ldurb",
        Opcode::LDURSB => "ldursb",
        Opcode::LDURSW => "ldursw",
        Opcode::LDURSH => "ldursh",
        Opcode::LDURH => "ldurh",
        Opcode::LDXP => "ldxp",
        Opcode::LDXR => "ldxr",
        Opcode::LDXRB => "ldxrb",
        Opcode::LDXRH => "ldxrh",
        Opcode::STLR => "stlr",
        Opcode::STLRB => "stlrb",
        Opcode::STLRH => "stlrh",
        Opcode::STLXP => "stlxp",
        Opcode::STLLRB => "stllrb",
        Opcode::STLLRH => "stllrh",
        Opcode::STLLR => "stllr",
        Opcode::STLXR => "stlxr",
        Opcode::STLXRB => "stlxrb",
        Opcode::STLXRH => "stlxrh",
        Opcode::STP => "stp",
        Opcode::STR => "str",
        Opcode::STTR => "sttr",
        Opcode::STTRB => "sttrb",
        Opcode::STTRH => "sttrh",
        Opcode::STRB => "strb",
        Opcode::STRH => "strh",
        Opcode::STRW => "strw",
        Opcode::STUR => "stur",
        Opcode::STURB => "sturb",
        Opcode::STURH => "sturh",
        Opcode::STXP => "stxp",
        Opcode::STXR => "stxr",
        Opcode::STXRB => "stxrb",
        Opcode::STXRH => "stxrh",
        Opcode::TBZ => "tbz",
        Opcode::TBNZ => "tbnz",
        Opcode::CBZ => "cbz",
        Opcode::CBNZ => "cbnz",
        Opcode::B => "b",
        Opcode::BR => "br",
        Opcode::Bcc(cond) => match cond {
            0b0000 => "b.eq",
            0b0001 => "b.ne",
            0b0010 => "b.hs",
            0b0011 => "b.lo",
            0b0100 => "b.mi",
            0b0101 => "b.pl",
            0b0110 => "b.vs",
            0b0111 => "b.vc",
            0b1000 => "b.hi",
            0b1001 => "b.ls",
            0b1010 => "b.ge",
            0b1011 => "b.lt",
            0b1100 => "b.gt",
            0b1101 => "b.le",
            0b1110 => "b.al",
            0b1111 => "b.nv",
            _ => return "<invalid>",
        },
        Opcode::BL => "bl",
        Opcode::BLR => "blr",
        Opcode::SVC => "svc",
        Opcode::HVC => "hvc",
        Opcode::SMC => "smc",
        Opcode::BRK => "brk",
        Opcode::HLT => "hlt",
        Opcode::DCPS1 => "dcps1",
        Opcode::DCPS2 => "dcps2",
        Opcode::DCPS3 => "dcps3",
        Opcode::RET => {
            if let Operand::Register(SizeCode::X, 30) = ins.operands[0] {
                // C5.6.148:  Defaults to X30 if absent.
                return "ret";
            }
            "ret"
        }
        Opcode::ERET => "eret",
        Opcode::DRPS => "drps",
        Opcode::MSR => "msr",
        Opcode::MRS => "mrs",
        Opcode::SYS(ops) => {
            push_unsigned(args, ops.op1() as u64);
            push_separator(args, ctx.config);
            push_operand(args, &ins.operands[1], ctx);
            push_separator(args, ctx.config);
            push_operand(args, &ins.operands[2], ctx);
            push_separator(args, ctx.config);
            push_unsigned(args, ops.op2() as u64);
            push_separator(args, ctx.config);
            push_operand(args, &ins.operands[0], ctx);
            return "sys";
        }
        Opcode::SYSL(ops) => {
            push_operand(args, &ins.operands[2], ctx);
            push_separator(args, ctx.config);
            push_unsigned(args, ops.op1() as u64);
            push_separator(args, ctx.config);
            push_operand(args, &ins.operands[0], ctx);
            push_separator(args, ctx.config);
            push_operand(args, &ins.operands[1], ctx);
            push_separator(args, ctx.config);
            push_unsigned(args, ops.op2() as u64);
            return "sysl";
        }
        Opcode::ISB => {
            // the default/reserved/expected value for the immediate in `isb` is `0b1111`.
            if let Operand::Imm16(15) = ins.operands[0] {
                return "isb";
            }
            "isb"
        }
        Opcode::DSB(option) => {
            push_barrier(args, option);
            return "dsb";
        }
        Opcode::DMB(option) => {
            push_barrier(args, option);
            return "dmb";
        }
        Opcode::SB => "sb",
        Opcode::SSSB => "sssb",
        Opcode::HINT => {
            if let (Operand::ControlReg(crn), Operand::Immediate(op2)) =
                (ins.operands[0], ins.operands[1])
            {
                let hint_num = (crn << 3) | op2 as u16;
                return match hint_num & 0b111111 {
                    0 => "nop",
                    1 => "yield",
                    2 => "wfe",
                    3 => "wfi",
                    4 => "sev",
                    0x10 => "esb",
                    0x11 => {
                        push_opaque(args, "csync");
                        "psb"
                    }
                    0x12 => {
                        push_opaque(args, "csync");
                        "tsb"
                    }
                    0x14 => "csdb",
                    0x15 => "sevl",
                    _ => {
                        push_unsigned(args, hint_num as u64);
                        "hint"
                    }
                };
            }
            "hint"
        }
        Opcode::CLREX => "clrex",
        Opcode::CSEL => "csel",
        Opcode::CSNEG => {
            if let (
                Operand::Register(_size, rn),
                Operand::Register(_, rm),
                Operand::ConditionCode(cond),
            ) = (ins.operands[1], ins.operands[2], ins.operands[3])
            {
                if cond < 0b1110 && rn == rm {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    push_separator(args, ctx.config);
                    push_condition_code(args, cond ^ 0x01);
                    return "cneg";
                }
            } else {
                unreachable!("operands 2 and 3 are always registers");
            }
            "csneg"
        }
        Opcode::CSINC => {
            if let (
                Operand::Register(_, n),
                Operand::Register(_, m),
                Operand::ConditionCode(cond),
            ) = (ins.operands[1], ins.operands[2], ins.operands[3])
            {
                if n == m && cond < 0b1110 {
                    return if n == 31 {
                        push_operand(args, &ins.operands[0], ctx);
                        push_separator(args, ctx.config);
                        push_condition_code(args, cond ^ 0x01);
                        "cset"
                    } else {
                        push_operand(args, &ins.operands[0], ctx);
                        push_separator(args, ctx.config);
                        push_operand(args, &ins.operands[1], ctx);
                        push_separator(args, ctx.config);
                        push_condition_code(args, cond ^ 0x01);
                        "cinc"
                    };
                }
            }
            "csinc"
        }
        Opcode::CSINV => {
            if let (
                Operand::Register(_, n),
                Operand::Register(_, m),
                Operand::ConditionCode(cond),
            ) = (ins.operands[1], ins.operands[2], ins.operands[3])
            {
                if n == m && n != 31 && cond < 0b1110 {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    push_separator(args, ctx.config);
                    push_condition_code(args, cond ^ 0x01);
                    return "cinv";
                } else if n == m && n == 31 && cond < 0b1110 {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_condition_code(args, cond ^ 0x01);
                    return "csetm";
                }
            }
            "csinv"
        }
        Opcode::CCMN => "ccmn",
        Opcode::CCMP => "ccmp",
        Opcode::RBIT => "rbit",
        Opcode::REV16 => "rev16",
        Opcode::REV => "rev",
        Opcode::REV32 => "rev32",
        Opcode::CLZ => "clz",
        Opcode::CLS => "cls",
        Opcode::MADD => {
            if let Operand::Register(_, 31) = ins.operands[3] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "mul";
            }
            "madd"
        }
        Opcode::MSUB => {
            if let Operand::Register(_, 31) = ins.operands[3] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "mneg";
            }
            "msub"
        }
        Opcode::SMADDL => {
            if let Operand::Register(_, 31) = ins.operands[3] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "smull";
            }
            "smaddl"
        }
        Opcode::SMSUBL => {
            if let Operand::Register(_, 31) = ins.operands[3] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "smnegl";
            }
            "smsubl"
        }
        Opcode::SMULH => "smulh",
        Opcode::UMADDL => {
            if let Operand::Register(_, 31) = ins.operands[3] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "umull";
            }
            "umaddl"
        }
        Opcode::UMSUBL => {
            if let Operand::Register(_, 31) = ins.operands[3] {
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "umnegl";
            }
            "umsubl"
        }
        Opcode::UMULH => "umulh",
        Opcode::UDIV => "udiv",
        Opcode::SDIV => "sdiv",
        // lslv == lsl (register) and, quoth the manual, `lsl is always the preferred disassembly`.
        Opcode::LSLV => "lsl",
        // lsrv == lsr (register) and, quoth the manual, `lsr is always the preferred disassembly`.
        Opcode::LSRV => "lsr",
        // asrv == asr (register) and, quoth the manual, `asr is always the preferred disassembly`.
        Opcode::ASRV => "asr",
        // rorv == ror (register) and, quoth the manual, `ror is always the preferred disassembly`.
        Opcode::RORV => "ror",
        Opcode::CRC32B => "crc32b",
        Opcode::CRC32H => "crc32h",
        Opcode::CRC32W => "crc32w",
        Opcode::CRC32X => "crc32x",
        Opcode::CRC32CB => "crc32cb",
        Opcode::CRC32CH => "crc32ch",
        Opcode::CRC32CW => "crc32cw",
        Opcode::CRC32CX => "crc32cx",
        Opcode::STNP => "stnp",
        Opcode::LDNP => "ldnp",
        Opcode::ST1 => "st1",
        Opcode::ST2 => "st2",
        Opcode::ST3 => "st3",
        Opcode::ST4 => "st4",
        Opcode::LD1 => "ld1",
        Opcode::LD2 => "ld2",
        Opcode::LD3 => "ld3",
        Opcode::LD4 => "ld4",
        Opcode::LD1R => "ld1r",
        Opcode::LD2R => "ld2r",
        Opcode::LD3R => "ld3r",
        Opcode::LD4R => "ld4r",
        Opcode::FMADD => "fmadd",
        Opcode::FMSUB => "fmsub",
        Opcode::FNMADD => "fnmadd",
        Opcode::FNMSUB => "fnmsub",
        Opcode::SCVTF => "scvtf",
        Opcode::UCVTF => "ucvtf",
        Opcode::FCVTZS => "fcvtzs",
        Opcode::FCVTZU => "fcvtzu",
        Opcode::FMOV => "fmov",
        Opcode::FABS => "fabs",
        Opcode::FNEG => "fneg",
        Opcode::FSQRT => "fsqrt",
        Opcode::FRINTN => "frintn",
        Opcode::FRINTP => "frintp",
        Opcode::FRINTM => "frintm",
        Opcode::FRINTZ => "frintz",
        Opcode::FRINTA => "frinta",
        Opcode::FRINTX => "frintx",
        Opcode::FRINTI => "frinti",
        Opcode::FRINT32Z => "frint32z",
        Opcode::FRINT32X => "frint32x",
        Opcode::FRINT64Z => "frint64z",
        Opcode::FRINT64X => "frint64x",
        Opcode::BFCVT => "bfcvt",
        Opcode::FCVT => "fcvt",
        Opcode::FCMP => "fcmp",
        Opcode::FCMPE => "fcmpe",
        Opcode::FMUL => "fmul",
        Opcode::FDIV => "fdiv",
        Opcode::FADD => "fadd",
        Opcode::FSUB => "fsub",
        Opcode::FMAX => "fmax",
        Opcode::FMIN => "fmin",
        Opcode::FMAXNM => "fmaxnm",
        Opcode::FMINNM => "fminnm",
        Opcode::FNMUL => "fnmul",
        Opcode::FCSEL => "fcsel",
        Opcode::FCCMP => "fccmp",
        Opcode::FCCMPE => "fccmpe",
        Opcode::FMULX => "fmulx",
        Opcode::FMLSL => "fmlsl",
        Opcode::FMLAL => "fmlal",
        Opcode::SQRDMLSH => "sqrdmlsh",
        Opcode::UDOT => "udot",
        Opcode::SQRDMLAH => "sqrdmlah",
        Opcode::UMULL => "umull",
        Opcode::UMULL2 => "umull2",
        Opcode::UMLSL => "umlsl",
        Opcode::UMLSL2 => "umlsl2",
        Opcode::MLS => "mls",
        Opcode::UMLAL => "umlal",
        Opcode::UMLAL2 => "umlal2",
        Opcode::MLA => "mla",
        Opcode::SDOT => "sdot",
        Opcode::SQDMULH => "sqdmulh",
        Opcode::SQDMULL => "sqdmull",
        Opcode::SQDMULL2 => "sqdmull2",
        Opcode::SMULL => "smull",
        Opcode::SMULL2 => "smull2",
        Opcode::MUL => "mul",
        Opcode::SQDMLSL => "sqdmlsl",
        Opcode::SQDMLSL2 => "sqdmlsl2",
        Opcode::SMLSL => "smlsl",
        Opcode::SMLSL2 => "smlsl2",
        Opcode::SQDMLAL => "sqdmlal",
        Opcode::SQDMLAL2 => "sqdmlal2",
        Opcode::SMLAL => "smlal",
        Opcode::SMLAL2 => "smlal2",
        Opcode::SQRDMULH => "sqrdmulh",
        Opcode::FCMLA => "fcmla",
        Opcode::SSHR => "sshr",
        Opcode::SSRA => "ssra",
        Opcode::SRSHR => "srshr",
        Opcode::SRSRA => "srsra",
        Opcode::SHL => "shl",
        Opcode::SQSHL => "sqshl",
        Opcode::SHRN => "shrn",
        Opcode::SHRN2 => "shrn2",
        Opcode::RSHRN => "rshrn",
        Opcode::SQSHRN => "sqshrn",
        Opcode::SQRSHRN => "sqrshrn",
        Opcode::SSHLL => "sshll",
        Opcode::USHLL => "ushll",
        Opcode::USHR => "ushr",
        Opcode::USRA => "usra",
        Opcode::URSHR => "urshr",
        Opcode::URSRA => "ursra",
        Opcode::SRI => "sri",
        Opcode::SLI => "sli",
        Opcode::SQSHLU => "sqshlu",
        Opcode::UQSHL => "uqshl",
        Opcode::SQSHRUN => "sqshrun",
        Opcode::SQRSHRUN => "sqrshrun",
        Opcode::UQSHRN => "uqshrn",
        Opcode::UQRSHRN => "uqrshrn",
        Opcode::MOVI => "movi",
        Opcode::MVNI => "mvni",
        Opcode::SHADD => "shadd",
        Opcode::SQADD => "sqadd",
        Opcode::SRHADD => "srhadd",
        Opcode::SHSUB => "shsub",
        Opcode::SQSUB => "sqsub",
        Opcode::CMGT => "cmgt",
        Opcode::CMGE => "cmge",
        Opcode::SSHL => "sshl",
        Opcode::SRSHL => "srshl",
        Opcode::SQRSHL => "sqrshl",
        Opcode::SMAX => "smax",
        Opcode::SMIN => "smin",
        Opcode::SABD => "sabd",
        Opcode::SABA => "saba",
        Opcode::CMTST => "cmtst",
        Opcode::SMAXP => "smaxp",
        Opcode::SMINP => "sminp",
        Opcode::ADDP => "addp",
        Opcode::UHADD => "uhadd",
        Opcode::UQADD => "uqadd",
        Opcode::URHADD => "urhadd",
        Opcode::UHSUB => "uhsub",
        Opcode::UQSUB => "uqsub",
        Opcode::CMHI => "cmhi",
        Opcode::CMHS => "cmhs",
        Opcode::USHL => "ushl",
        Opcode::URSHL => "urshl",
        Opcode::UQRSHL => "uqrshl",
        Opcode::UMAX => "umax",
        Opcode::UMIN => "umin",
        Opcode::UABD => "uabd",
        Opcode::UABA => "uaba",
        Opcode::CMEQ => "cmeq",
        Opcode::PMUL => "pmul",
        Opcode::UMAXP => "umaxp",
        Opcode::UMINP => "uminp",
        Opcode::FMLA => "fmla",
        Opcode::FCMEQ => "fcmeq",
        Opcode::FRECPS => "frecps",
        Opcode::BSL => "bsl",
        Opcode::BIT => "bit",
        Opcode::BIF => "bif",
        Opcode::FMAXNMP => "fmaxnmp",
        Opcode::FMINMNP => "fminmnp",
        Opcode::FADDP => "faddp",
        Opcode::FCMGE => "fcmge",
        Opcode::FACGE => "facge",
        Opcode::FMAXP => "fmaxp",
        Opcode::SADDL => "saddl",
        Opcode::SADDL2 => "saddl2",
        Opcode::SADDW => "saddw",
        Opcode::SADDW2 => "saddw2",
        Opcode::SSUBL => "ssubl",
        Opcode::SSUBL2 => "ssubl2",
        Opcode::SSUBW => "ssubw",
        Opcode::SSUBW2 => "ssubw2",
        Opcode::ADDHN => "addhn",
        Opcode::ADDHN2 => "addhn2",
        Opcode::SABAL => "sabal",
        Opcode::SABAL2 => "sabal2",
        Opcode::SUBHN => "subhn",
        Opcode::SUBHN2 => "subhn2",
        Opcode::SABDL => "sabdl",
        Opcode::SABDL2 => "sabdl2",
        Opcode::PMULL => "pmull",
        Opcode::PMULL2 => "pmull2",
        Opcode::UADDL => "uaddl",
        Opcode::UADDL2 => "uaddl2",
        Opcode::UADDW => "uaddw",
        Opcode::UADDW2 => "uaddw2",
        Opcode::USUBL => "usubl",
        Opcode::USUBL2 => "usubl2",
        Opcode::USUBW => "usubw",
        Opcode::USUBW2 => "usubw2",
        Opcode::RADDHN => "raddhn",
        Opcode::RADDHN2 => "raddhn2",
        Opcode::RSUBHN => "rsubhn",
        Opcode::RSUBHN2 => "rsubhn2",
        Opcode::UABAL => "uabal",
        Opcode::UABAL2 => "uabal2",
        Opcode::UABDL => "uabdl",
        Opcode::UABDL2 => "uabdl2",
        Opcode::REV64 => "rev64",
        Opcode::SADDLP => "saddlp",
        Opcode::SUQADD => "suqadd",
        Opcode::CNT => "cnt",
        Opcode::SADALP => "sadalp",
        Opcode::SQABS => "sqabs",
        Opcode::CMLT => "cmlt",
        Opcode::ABS => "abs",
        Opcode::XTN => "xtn",
        Opcode::XTN2 => "xtn2",
        Opcode::SQXTN => "sqxtn",
        Opcode::SQXTN2 => "sqxtn2",
        Opcode::FCVTN => "fcvtn",
        Opcode::FCVTN2 => "fcvtn2",
        Opcode::FCMGT => "fcmgt",
        Opcode::FCVTL => "fcvtl",
        Opcode::FCVTL2 => "fcvtl2",
        Opcode::FCVTNS => "fcvtns",
        Opcode::FCVTPS => "fcvtps",
        Opcode::FCVTMS => "fcvtms",
        Opcode::FCVTAS => "fcvtas",
        Opcode::URECPE => "urecpe",
        Opcode::FRECPE => "frecpe",
        Opcode::UADDLP => "uaddlp",
        Opcode::USQADD => "usqadd",
        Opcode::UADALP => "uadalp",
        Opcode::SQNEG => "sqneg",
        Opcode::CMLE => "cmle",
        Opcode::NEG => "neg",
        Opcode::SQXTUN => "sqxtun",
        Opcode::SQXTUN2 => "sqxtun2",
        Opcode::SHLL => "shll",
        Opcode::SHLL2 => "shll2",
        Opcode::UQXTN => "uqxtn",
        Opcode::UQXTN2 => "uqxtn2",
        Opcode::FCVTXN => "fcvtxn",
        Opcode::FCVTXN2 => "fcvtxn2",
        Opcode::FCVTNU => "fcvtnu",
        Opcode::FCVTMU => "fcvtmu",
        Opcode::FCVTAU => "fcvtau",
        // `ins (element)` and `ins (general)` both have `mov` as an alias. manual reports that `mov` is the preferred disassembly.
        Opcode::INS => "mov",
        Opcode::EXT => "ext",
        Opcode::DUP => {
            if let Operand::Register(_, _) = ins.operands[1] {
                "dup"
            } else {
                "mov"
            }
        }
        Opcode::UZP1 => "uzp1",
        Opcode::TRN1 => "trn1",
        Opcode::ZIP1 => "zip1",
        Opcode::UZP2 => "uzp2",
        Opcode::TRN2 => "trn2",
        Opcode::ZIP2 => "zip2",
        Opcode::SMOV => "smov",
        Opcode::UMOV => {
            if let (
                Operand::Register(reg_sz, _),
                Operand::SIMDRegisterElementsLane(_, _, elem_sz, _),
            ) = (ins.operands[0], ins.operands[1])
            {
                if (reg_sz == SizeCode::W && elem_sz == SIMDSizeCode::S)
                    || (reg_sz == SizeCode::X && elem_sz == SIMDSizeCode::D)
                {
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    return "mov";
                }
            }
            "umov"
        }
        Opcode::SQSHRN2 => "sqshrn2",
        Opcode::SQRSHRN2 => "sqrshrn2",
        Opcode::SQSHRUN2 => "sqshrun2",
        Opcode::UQSHRN2 => "uqshrn2",
        Opcode::UQRSHRN2 => "uqrshrn2",
        Opcode::FMLS => "fmls",
        Opcode::FRECPX => "frecpx",
        Opcode::FRSQRTE => "frsqrte",
        Opcode::FCVTPU => "fcvtpu",
        Opcode::FCMLT => "fcmlt",
        Opcode::FCMLE => "fcmle",
        Opcode::FMAXNMV => "fmaxnmv",
        Opcode::FMINNMV => "fminnmv",
        Opcode::FMAXV => "fmaxv",
        Opcode::FMINV => "fminv",
        Opcode::UADDLV => "uaddlv",
        Opcode::SADDLV => "saddlv",
        Opcode::UMAXV => "umaxv",
        Opcode::SMAXV => "smaxv",
        Opcode::UMINV => "uminv",
        Opcode::SMINV => "sminv",
        Opcode::ADDV => "addv",
        Opcode::FRSQRTS => "frsqrts",
        Opcode::FMINNMP => "fminnmp",
        Opcode::FMLAL2 => "fmlal2",
        Opcode::FMLSL2 => "fmlsl2",
        Opcode::FABD => "fabd",
        Opcode::FACGT => "facgt",
        Opcode::FMINP => "fminp",
        Opcode::FJCVTZS => "fjcvtzs",
        Opcode::URSQRTE => "ursqrte",
        Opcode::PRFM => "prfm",
        Opcode::PRFUM => "prfum",
        Opcode::AESE => "aese",
        Opcode::AESD => "aesd",
        Opcode::AESMC => "aesmc",
        Opcode::AESIMC => "aesimc",
        Opcode::SHA1H => "sha1h",
        Opcode::SHA1SU1 => "sha1su1",
        Opcode::SHA256SU0 => "sha256su0",
        Opcode::SM3TT1A => "sm3tt1a",
        Opcode::SM3TT1B => "sm3tt1b",
        Opcode::SM3TT2A => "sm3tt2a",
        Opcode::SM3TT2B => "sm3tt2b",
        Opcode::SHA512H => "sha512h",
        Opcode::SHA512H2 => "sha512h2",
        Opcode::SHA512SU1 => "sha512su1",
        Opcode::RAX1 => "rax1",
        Opcode::SM3PARTW1 => "sm3partw1",
        Opcode::SM3PARTW2 => "sm3partw2",
        Opcode::SM4EKEY => "sm4ekey",
        Opcode::BCAX => "bcax",
        Opcode::SM3SS1 => "sm3ss1",
        Opcode::SHA512SU0 => "sha512su0",
        Opcode::SM4E => "sm4e",
        Opcode::EOR3 => "eor3",
        Opcode::XAR => "xar",
        Opcode::LDRAA => "ldraa",
        Opcode::LDRAB => "ldrab",
        Opcode::LDAPR => "ldapr",
        Opcode::LDAPRH => "ldaprh",
        Opcode::LDAPRB => "ldaprb",
        Opcode::SWP(ar) => {
            if ar == 0 {
                "swp"
            } else if ar == 0b01 {
                "swpl"
            } else if ar == 0b10 {
                "swpa"
            } else {
                "swpal"
            }
        }
        Opcode::SWPB(ar) => {
            if ar == 0 {
                "swpb"
            } else if ar == 0b01 {
                "swplb"
            } else if ar == 0b10 {
                "swpab"
            } else {
                "swpalb"
            }
        }
        Opcode::SWPH(ar) => {
            if ar == 0 {
                "swph"
            } else if ar == 0b01 {
                "swplh"
            } else if ar == 0b10 {
                "swpah"
            } else {
                "swpalh"
            }
        }
        Opcode::LDADDB(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "staddb" } else { "staddlb" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldaddb"
            } else if ar == 0b01 {
                "ldaddlb"
            } else if ar == 0b10 {
                "ldaddab"
            } else {
                "ldaddalb"
            }
        }
        Opcode::LDCLRB(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stclrb" } else { "stclrlb" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldclrb"
            } else if ar == 0b01 {
                "ldclrlb"
            } else if ar == 0b10 {
                "ldclrab"
            } else {
                "ldclralb"
            }
        }
        Opcode::LDEORB(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "steorb" } else { "steorlb" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldeorb"
            } else if ar == 0b01 {
                "ldeorlb"
            } else if ar == 0b10 {
                "ldeorab"
            } else {
                "ldeoralb"
            }
        }
        Opcode::LDSETB(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stsetb" } else { "stsetlb" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldsetb"
            } else if ar == 0b01 {
                "ldsetlb"
            } else if ar == 0b10 {
                "ldsetab"
            } else {
                "ldsetalb"
            }
        }
        Opcode::LDSMAXB(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stsmaxb" } else { "stsmaxlb" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldsmaxb"
            } else if ar == 0b01 {
                "ldsmaxlb"
            } else if ar == 0b10 {
                "ldsmaxab"
            } else {
                "ldsmaxalb"
            }
        }
        Opcode::LDSMINB(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stsminb" } else { "stsminlb" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldsminb"
            } else if ar == 0b01 {
                "ldsminlb"
            } else if ar == 0b10 {
                "ldsminab"
            } else {
                "ldsminalb"
            }
        }
        Opcode::LDUMAXB(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stumaxb" } else { "stumaxlb" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldumaxb"
            } else if ar == 0b01 {
                "ldumaxlb"
            } else if ar == 0b10 {
                "ldumaxab"
            } else {
                "ldumaxalb"
            }
        }
        Opcode::LDUMINB(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stuminb" } else { "stuminlb" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            // write!(fmt, "{}", self.opcode)?;
            if ar == 0 {
                "lduminb"
            } else if ar == 0b01 {
                "lduminlb"
            } else if ar == 0b10 {
                "lduminab"
            } else {
                "lduminalb"
            }
        }
        Opcode::LDADDH(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "staddh" } else { "staddlh" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldaddh"
            } else if ar == 0b01 {
                "ldaddlh"
            } else if ar == 0b10 {
                "ldaddah"
            } else {
                "ldaddalh"
            }
        }
        Opcode::LDCLRH(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stclrh" } else { "stclrlh" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldclrh"
            } else if ar == 0b01 {
                "ldclrlh"
            } else if ar == 0b10 {
                "ldclrah"
            } else {
                "ldclralh"
            }
        }
        Opcode::LDEORH(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "steorh" } else { "steorlh" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldeorh"
            } else if ar == 0b01 {
                "ldeorlh"
            } else if ar == 0b10 {
                "ldeorah"
            } else {
                "ldeoralh"
            }
        }
        Opcode::LDSETH(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stseth" } else { "stsetlh" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldseth"
            } else if ar == 0b01 {
                "ldsetlh"
            } else if ar == 0b10 {
                "ldsetah"
            } else {
                "ldsetalh"
            }
        }
        Opcode::LDSMAXH(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stsmaxh" } else { "stsmaxlh" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldsmaxh"
            } else if ar == 0b01 {
                "ldsmaxlh"
            } else if ar == 0b10 {
                "ldsmaxah"
            } else {
                "ldsmaxalh"
            }
        }
        Opcode::LDSMINH(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stsminh" } else { "stsminlh" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldsminh"
            } else if ar == 0b01 {
                "ldsminlh"
            } else if ar == 0b10 {
                "ldsminah"
            } else {
                "ldsminalh"
            }
        }
        Opcode::LDUMAXH(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stumaxh" } else { "stumaxlh" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldumaxh"
            } else if ar == 0b01 {
                "ldumaxlh"
            } else if ar == 0b10 {
                "ldumaxah"
            } else {
                "ldumaxalh"
            }
        }
        Opcode::LDUMINH(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stuminh" } else { "stuminlh" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "lduminh"
            } else if ar == 0b01 {
                "lduminlh"
            } else if ar == 0b10 {
                "lduminah"
            } else {
                "lduminalh"
            }
        }
        Opcode::LDADD(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stadd" } else { "staddl" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldadd"
            } else if ar == 0b01 {
                "ldaddl"
            } else if ar == 0b10 {
                "ldadda"
            } else {
                "ldaddal"
            }
        }
        Opcode::LDCLR(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stclr" } else { "stclrl" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldclr"
            } else if ar == 0b01 {
                "ldclrl"
            } else if ar == 0b10 {
                "ldclra"
            } else {
                "ldclral"
            }
        }
        Opcode::LDEOR(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "steor" } else { "steorl" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldeor"
            } else if ar == 0b01 {
                "ldeorl"
            } else if ar == 0b10 {
                "ldeora"
            } else {
                "ldeoral"
            }
        }
        Opcode::LDSET(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stset" } else { "stsetl" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldset"
            } else if ar == 0b01 {
                "ldsetl"
            } else if ar == 0b10 {
                "ldseta"
            } else {
                "ldsetal"
            }
        }
        Opcode::LDSMAX(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stsmax" } else { "stsmaxl" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldsmax"
            } else if ar == 0b01 {
                "ldsmaxl"
            } else if ar == 0b10 {
                "ldsmaxa"
            } else {
                "ldsmaxal"
            }
        }
        Opcode::LDSMIN(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stsmin" } else { "stsminl" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldsmin"
            } else if ar == 0b01 {
                "ldsminl"
            } else if ar == 0b10 {
                "ldsmina"
            } else {
                "ldsminal"
            }
        }
        Opcode::LDUMAX(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stumax" } else { "stumaxl" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldumax"
            } else if ar == 0b01 {
                "ldumaxl"
            } else if ar == 0b10 {
                "ldumaxa"
            } else {
                "ldumaxal"
            }
        }
        Opcode::LDUMIN(ar) => {
            if let Operand::Register(_, rt) = ins.operands[1] {
                if rt == 31 && ar & 0b10 == 0b00 {
                    let inst = if ar & 0b01 == 0b00 { "stumin" } else { "stuminl" };
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return inst;
                }
            }
            if ar == 0 {
                "ldumin"
            } else if ar == 0b01 {
                "lduminl"
            } else if ar == 0b10 {
                "ldumina"
            } else {
                "lduminal"
            }
        }
        Opcode::CAS(ar) => {
            if ar == 0 {
                "cas"
            } else if ar == 0b01 {
                "casl"
            } else if ar == 0b10 {
                "casa"
            } else {
                "casal"
            }
        }
        Opcode::CASH(ar) => {
            if ar == 0 {
                "cash"
            } else if ar == 0b01 {
                "caslh"
            } else if ar == 0b10 {
                "casah"
            } else {
                "casalh"
            }
        }
        Opcode::CASB(ar) => {
            if ar == 0 {
                "casb"
            } else if ar == 0b01 {
                "caslb"
            } else if ar == 0b10 {
                "casab"
            } else {
                "casalb"
            }
        }
        Opcode::CASP(ar) => {
            if ar == 0 {
                "casp"
            } else if ar == 0b01 {
                "caspl"
            } else if ar == 0b10 {
                "caspa"
            } else {
                "caspal"
            }
        }
        Opcode::TBL => "tbl",
        Opcode::TBX => "tbx",
        Opcode::FCADD => "fcadd",
        Opcode::LDGM => "ldgm",
        Opcode::LDG => "ldm",
        Opcode::STGM => "stgm",
        Opcode::STZGM => "stzgm",
        Opcode::STG => "stg",
        Opcode::STZG => "stzg",
        Opcode::ST2G => "st2g",
        Opcode::STZ2G => "stz2g",
        Opcode::LDAPUR => "ldapur",
        Opcode::LDAPURB => "ldapurb",
        Opcode::LDAPURH => "ldapurh",
        Opcode::LDAPURSB => "ldapursb",
        Opcode::LDAPURSH => "ldapursh",
        Opcode::LDAPURSW => "ldapursw",
        Opcode::STLUR => "stlur",
        Opcode::STLURB => "stlurb",
        Opcode::STLURH => "stlurh",
        Opcode::SETF8 => "setf8",
        Opcode::SETF16 => "setf16",
        Opcode::RMIF => "rmif",
        // `This instruction is used by the alias MVN. The alias is always the preferred disassembly.`
        Opcode::NOT => "mvn",
        Opcode::RSHRN2 => "rshrn2",
        Opcode::SQRSHRUN2 => "sqrshrun2",
        Opcode::USHLL2 => "ushll2",
        Opcode::SSHLL2 => "sshll2",
        Opcode::SHA1C => "sha1c",
        Opcode::SHA1P => "sha1p",
        Opcode::SHA1M => "sha1m",
        Opcode::SHA1SU0 => "sha1su0",
        Opcode::SHA256H => "sha256h",
        Opcode::SHA256H2 => "sha256h2",
        Opcode::SHA256SU1 => "sha256su1",
        Opcode::BLRAA => "blraa",
        Opcode::BLRAAZ => "blraaz",
        Opcode::BLRAB => "blrab",
        Opcode::BLRABZ => "blrabz",
        Opcode::BRAA => "braa",
        Opcode::BRAAZ => "braaz",
        Opcode::BRAB => "brab",
        Opcode::BRABZ => "brabz",
        Opcode::ERETAA => "eretaa",
        Opcode::ERETAB => "eretab",
        Opcode::RETAA => "retaa",
        Opcode::RETAB => "retab",
        Opcode::PACIA => "pacia",
        Opcode::PACIB => "pacib",
        Opcode::PACDA => "pacda",
        Opcode::PACDB => "pacdb",
        Opcode::AUTIA => "autia",
        Opcode::AUTIB => "autib",
        Opcode::AUTDA => "autda",
        Opcode::AUTDB => "autdb",
        Opcode::PACIZA => "paciza",
        Opcode::PACIZB => "pacizb",
        Opcode::PACDZA => "pacdza",
        Opcode::PACDZB => "pacdzb",
        Opcode::AUTIZA => "autiza",
        Opcode::AUTIZB => "autizb",
        Opcode::AUTDZA => "autdza",
        Opcode::AUTDZB => "autdzb",
        Opcode::XPACI => "xpaci",
        Opcode::XPACD => "xpacd",
        Opcode::PACGA => "pacga",
        Opcode::GMI => "gmi",
        Opcode::IRG => "irg",
        Opcode::SUBP => "subp",
        Opcode::SUBPS => "subps",
    };

    // Regular logic for formatting the operands
    for (i, o) in ins.operands.iter().enumerate() {
        if let Operand::Nothing = o {
            break;
        }
        if i > 0 {
            push_separator(args, ctx.config);
        }
        push_operand(args, o, ctx);
    }
    mnemonic
}

const REG_NAMES_X: [&str; 31] = [
    "x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9", "x10", "x11", "x12", "x13", "x14",
    "x15", "x16", "x17", "x18", "x19", "x20", "x21", "x22", "x23", "x24", "x25", "x26", "x27",
    "x28", "x29", "x30",
];

const REG_NAMES_W: [&str; 31] = [
    "w0", "w1", "w2", "w3", "w4", "w5", "w6", "w7", "w8", "w9", "w10", "w11", "w12", "w13", "w14",
    "w15", "w16", "w17", "w18", "w19", "w20", "w21", "w22", "w23", "w24", "w25", "w26", "w27",
    "w28", "w29", "w30",
];

fn reg_name(size: SizeCode, reg: u16, sp: bool) -> &'static str {
    match reg.cmp(&31) {
        Ordering::Less => match size {
            SizeCode::X => REG_NAMES_X[reg as usize],
            SizeCode::W => REG_NAMES_W[reg as usize],
        },
        Ordering::Equal => {
            if sp {
                match size {
                    SizeCode::X => "sp",
                    SizeCode::W => "wsp",
                }
            } else {
                match size {
                    SizeCode::X => "xzr",
                    SizeCode::W => "wzr",
                }
            }
        }
        Ordering::Greater => "<invalid>",
    }
}

fn shift_style(style: ShiftStyle) -> &'static str {
    match style {
        ShiftStyle::LSL => "lsl",
        ShiftStyle::LSR => "lsr",
        ShiftStyle::ASR => "asr",
        ShiftStyle::ROR => "ror",
        ShiftStyle::UXTB => "uxtb",
        ShiftStyle::UXTH => "uxth",
        ShiftStyle::UXTW => "uxtw",
        ShiftStyle::UXTX => "uxtx",
        ShiftStyle::SXTB => "sxtb",
        ShiftStyle::SXTH => "sxth",
        ShiftStyle::SXTW => "sxtw",
        ShiftStyle::SXTX => "sxtx",
    }
}

fn condition_code(cond: u8) -> &'static str {
    match cond {
        0b0000 => "eq",
        0b0010 => "hs",
        0b0100 => "mi",
        0b0110 => "vs",
        0b1000 => "hi",
        0b1010 => "ge",
        0b1100 => "gt",
        0b1110 => "al",
        0b0001 => "ne",
        0b0011 => "lo",
        0b0101 => "pl",
        0b0111 => "vc",
        0b1001 => "ls",
        0b1011 => "lt",
        0b1101 => "le",
        0b1111 => "nv",
        _ => "<invalid>",
    }
}

#[inline]
fn push_register(args: &mut Vec<ObjInsArg>, size: SizeCode, reg: u16, sp: bool) {
    push_opaque(args, reg_name(size, reg, sp));
}

#[inline]
fn push_shift(args: &mut Vec<ObjInsArg>, style: ShiftStyle, amount: u8) {
    push_opaque(args, shift_style(style));
    if amount != 0 {
        push_plain(args, " ");
        push_unsigned(args, amount as u64);
    }
}

#[inline]
fn push_condition_code(args: &mut Vec<ObjInsArg>, cond: u8) {
    push_opaque(args, condition_code(cond));
}

fn push_barrier(args: &mut Vec<ObjInsArg>, option: u8) {
    match option {
        0b0001 => push_opaque(args, "oshld"),
        0b0010 => push_opaque(args, "oshst"),
        0b0011 => push_opaque(args, "osh"),
        0b0101 => push_opaque(args, "nshld"),
        0b0110 => push_opaque(args, "nshst"),
        0b0111 => push_opaque(args, "nsh"),
        0b1001 => push_opaque(args, "ishld"),
        0b1010 => push_opaque(args, "ishst"),
        0b1011 => push_opaque(args, "ish"),
        0b1101 => push_opaque(args, "ld"),
        0b1110 => push_opaque(args, "st"),
        0b1111 => push_opaque(args, "sy"),
        _ => push_unsigned(args, option as u64),
    }
}

#[inline]
fn push_opaque(args: &mut Vec<ObjInsArg>, text: &'static str) {
    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(Cow::Borrowed(text))));
}

#[inline]
fn push_plain(args: &mut Vec<ObjInsArg>, text: &'static str) {
    args.push(ObjInsArg::PlainText(Cow::Borrowed(text)));
}

#[inline]
fn push_separator(args: &mut Vec<ObjInsArg>, config: &DiffObjConfig) {
    args.push(ObjInsArg::PlainText(Cow::Borrowed(config.separator())));
}

#[inline]
fn push_unsigned(args: &mut Vec<ObjInsArg>, v: u64) {
    push_plain(args, "#");
    args.push(ObjInsArg::Arg(ObjInsArgValue::Unsigned(v)));
}

#[inline]
fn push_signed(args: &mut Vec<ObjInsArg>, v: i64) {
    push_plain(args, "#");
    args.push(ObjInsArg::Arg(ObjInsArgValue::Signed(v)));
}

/// Relocations that appear in Operand::PCOffset.
fn is_pc_offset_reloc(reloc: Option<&ObjReloc>) -> Option<&ObjReloc> {
    if let Some(reloc) = reloc {
        if let RelocationFlags::Elf {
            r_type:
                elf::R_AARCH64_ADR_PREL_PG_HI21
                | elf::R_AARCH64_JUMP26
                | elf::R_AARCH64_CALL26
                | elf::R_AARCH64_ADR_GOT_PAGE,
        } = reloc.flags
        {
            return Some(reloc);
        }
    }
    None
}

/// Relocations that appear in Operand::Immediate.
fn is_imm_reloc(reloc: Option<&ObjReloc>) -> bool {
    matches!(reloc, Some(reloc) if matches!(reloc.flags, RelocationFlags::Elf {
        r_type: elf::R_AARCH64_ADD_ABS_LO12_NC,
    }))
}

/// Relocations that appear in Operand::RegPreIndex/RegPostIndex.
fn is_reg_index_reloc(reloc: Option<&ObjReloc>) -> bool {
    matches!(reloc, Some(reloc) if matches!(reloc.flags, RelocationFlags::Elf {
        r_type: elf::R_AARCH64_LDST32_ABS_LO12_NC | elf::R_AARCH64_LD64_GOT_LO12_NC,
    }))
}

fn push_operand(args: &mut Vec<ObjInsArg>, o: &Operand, ctx: &mut DisplayCtx) {
    match o {
        Operand::Nothing => unreachable!(),
        Operand::PCOffset(off) => {
            if let Some(reloc) = is_pc_offset_reloc(ctx.reloc) {
                let target_address = reloc.target.address.checked_add_signed(reloc.addend);
                if reloc.target.orig_section_index == Some(ctx.section_index)
                    && matches!(target_address, Some(addr) if addr > ctx.start_address && addr < ctx.end_address)
                {
                    let dest = target_address.unwrap();
                    push_plain(args, "$");
                    args.push(ObjInsArg::BranchDest(dest));
                    ctx.branch_dest = Some(dest);
                } else {
                    args.push(ObjInsArg::Reloc);
                }
            } else {
                let dest = ctx.address.saturating_add_signed(*off);
                push_plain(args, "$");
                args.push(ObjInsArg::BranchDest(dest));
                ctx.branch_dest = Some(dest);
            }
        }
        Operand::Immediate(imm) => {
            if is_imm_reloc(ctx.reloc) {
                args.push(ObjInsArg::Reloc);
            } else {
                push_unsigned(args, *imm as u64);
            }
        }
        Operand::Imm64(imm) => {
            push_unsigned(args, *imm);
        }
        Operand::Imm16(imm) => {
            push_unsigned(args, *imm as u64);
        }
        Operand::Register(size, reg) => {
            push_register(args, *size, *reg, false);
        }
        Operand::RegisterPair(size, reg) => {
            push_register(args, *size, *reg, false);
            push_separator(args, ctx.config);
            push_register(args, *size, *reg + 1, false);
        }
        Operand::RegisterOrSP(size, reg) => {
            push_register(args, *size, *reg, true);
        }
        Operand::ConditionCode(cond) => match cond {
            0b0000 => push_opaque(args, "eq"),
            0b0010 => push_opaque(args, "hs"),
            0b0100 => push_opaque(args, "mi"),
            0b0110 => push_opaque(args, "vs"),
            0b1000 => push_opaque(args, "hi"),
            0b1010 => push_opaque(args, "ge"),
            0b1100 => push_opaque(args, "gt"),
            0b1110 => push_opaque(args, "al"),
            0b0001 => push_opaque(args, "ne"),
            0b0011 => push_opaque(args, "lo"),
            0b0101 => push_opaque(args, "pl"),
            0b0111 => push_opaque(args, "vc"),
            0b1001 => push_opaque(args, "ls"),
            0b1011 => push_opaque(args, "lt"),
            0b1101 => push_opaque(args, "le"),
            0b1111 => push_opaque(args, "nv"),
            _ => unreachable!(),
        },
        Operand::ImmShift(i, shift) => {
            push_unsigned(args, *i as u64);
            if *shift > 0 {
                push_separator(args, ctx.config);
                push_opaque(args, "lsl");
                push_plain(args, " ");
                push_unsigned(args, *shift as u64);
            }
        }
        Operand::ImmShiftMSL(i, shift) => {
            push_unsigned(args, *i as u64);
            if *shift > 0 {
                push_separator(args, ctx.config);
                push_opaque(args, "msl");
                push_plain(args, " ");
                push_unsigned(args, *shift as u64);
            }
        }
        Operand::RegShift(shift_type, amount, size, reg) => match size {
            SizeCode::X => {
                push_register(args, SizeCode::X, *reg, false);
                if (*shift_type == ShiftStyle::LSL || *shift_type == ShiftStyle::UXTX)
                    && *amount == 0
                {
                    // pass
                } else {
                    push_separator(args, ctx.config);
                    push_shift(args, *shift_type, *amount);
                }
            }
            SizeCode::W => {
                push_register(args, SizeCode::W, *reg, false);
                if *shift_type == ShiftStyle::LSL && *amount == 0 {
                    // pass
                } else {
                    push_separator(args, ctx.config);
                    push_shift(args, *shift_type, *amount);
                }
            }
        },
        Operand::RegRegOffset(reg, index_reg, index_size, extend, amount) => {
            push_plain(args, "[");
            push_register(args, SizeCode::X, *reg, true);
            push_separator(args, ctx.config);
            push_register(args, *index_size, *index_reg, false);
            if extend == &ShiftStyle::LSL && *amount == 0 {
                // pass
            } else if ((extend == &ShiftStyle::UXTW && index_size == &SizeCode::W)
                || (extend == &ShiftStyle::UXTX && index_size == &SizeCode::X))
                && *amount == 0
            {
                push_separator(args, ctx.config);
                push_shift(args, *extend, 0);
            } else {
                push_separator(args, ctx.config);
                push_shift(args, *extend, *amount);
            }
            push_plain(args, "]");
        }
        Operand::RegPreIndex(reg, offset, wback_bit) => {
            push_plain(args, "[");
            push_register(args, SizeCode::X, *reg, true);
            if is_reg_index_reloc(ctx.reloc) {
                push_separator(args, ctx.config);
                args.push(ObjInsArg::Reloc);
            } else if *offset != 0 || *wback_bit {
                push_separator(args, ctx.config);
                push_signed(args, *offset as i64);
            }
            push_plain(args, "]");
            if *wback_bit {
                push_plain(args, "!");
            }
        }
        Operand::RegPostIndex(reg, offset) => {
            push_plain(args, "[");
            push_register(args, SizeCode::X, *reg, true);
            push_plain(args, "]");
            push_separator(args, ctx.config);
            if is_reg_index_reloc(ctx.reloc) {
                args.push(ObjInsArg::Reloc);
            } else {
                push_signed(args, *offset as i64);
            }
        }
        Operand::RegPostIndexReg(reg, offset_reg) => {
            push_plain(args, "[");
            push_register(args, SizeCode::X, *reg, true);
            push_plain(args, "]");
            push_separator(args, ctx.config);
            // TODO does 31 have to be handled separate?
            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(Cow::Owned(format!(
                "x{}",
                offset_reg
            )))));
        }
        // Fall back to original logic
        Operand::SIMDRegister(_, _)
        | Operand::SIMDRegisterElements(_, _, _)
        | Operand::SIMDRegisterElementsLane(_, _, _, _)
        | Operand::SIMDRegisterElementsMultipleLane(_, _, _, _, _)
        | Operand::SIMDRegisterGroup(_, _, _, _)
        | Operand::SIMDRegisterGroupLane(_, _, _, _)
        | Operand::ImmediateDouble(_)
        | Operand::PrefetchOp(_)
        | Operand::SystemReg(_)
        | Operand::ControlReg(_)
        | Operand::PstateField(_) => {
            args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(Cow::Owned(o.to_string()))));
        }
    }
}

// Opcode is #[repr(u16)], but the tuple variants negate that, so we have to do this instead.
fn opcode_to_u16(opcode: Opcode) -> u16 {
    match opcode {
        Opcode::Invalid => u16::MAX,
        Opcode::UDF => 0,
        Opcode::MOVN => 1,
        Opcode::MOVK => 2,
        Opcode::MOVZ => 3,
        Opcode::ADC => 4,
        Opcode::ADCS => 5,
        Opcode::SBC => 6,
        Opcode::SBCS => 7,
        Opcode::AND => 8,
        Opcode::ORR => 9,
        Opcode::ORN => 10,
        Opcode::EOR => 11,
        Opcode::EON => 12,
        Opcode::BIC => 13,
        Opcode::BICS => 14,
        Opcode::ANDS => 15,
        Opcode::ADDS => 16,
        Opcode::ADD => 17,
        Opcode::SUBS => 18,
        Opcode::SUB => 19,
        Opcode::BFM => 20,
        Opcode::UBFM => 21,
        Opcode::SBFM => 22,
        Opcode::ADR => 23,
        Opcode::ADRP => 24,
        Opcode::EXTR => 25,
        Opcode::LDAR => 26,
        Opcode::LDLAR => 27,
        Opcode::LDARB => 28,
        Opcode::LDLARB => 29,
        Opcode::LDAXRB => 30,
        Opcode::LDARH => 31,
        Opcode::LDLARH => 32,
        Opcode::LDAXP => 33,
        Opcode::LDAXR => 34,
        Opcode::LDAXRH => 35,
        Opcode::LDP => 36,
        Opcode::LDPSW => 37,
        Opcode::LDR => 38,
        Opcode::LDRB => 39,
        Opcode::LDRSB => 40,
        Opcode::LDRSW => 41,
        Opcode::LDRSH => 42,
        Opcode::LDRH => 43,
        Opcode::LDTR => 44,
        Opcode::LDTRB => 45,
        Opcode::LDTRH => 46,
        Opcode::LDTRSB => 47,
        Opcode::LDTRSH => 48,
        Opcode::LDTRSW => 49,
        Opcode::LDUR => 50,
        Opcode::LDURB => 51,
        Opcode::LDURSB => 52,
        Opcode::LDURSW => 53,
        Opcode::LDURSH => 54,
        Opcode::LDURH => 55,
        Opcode::LDXP => 56,
        Opcode::LDXR => 57,
        Opcode::LDXRB => 58,
        Opcode::LDXRH => 59,
        Opcode::STLR => 60,
        Opcode::STLLR => 61,
        Opcode::STLRB => 62,
        Opcode::STLLRB => 63,
        Opcode::STLRH => 64,
        Opcode::STLLRH => 65,
        Opcode::STLXP => 66,
        Opcode::STLXR => 67,
        Opcode::STLXRB => 68,
        Opcode::STLXRH => 69,
        Opcode::STP => 70,
        Opcode::STR => 71,
        Opcode::STTR => 72,
        Opcode::STTRB => 73,
        Opcode::STTRH => 74,
        Opcode::STRB => 75,
        Opcode::STRH => 76,
        Opcode::STRW => 77,
        Opcode::STUR => 78,
        Opcode::STURB => 79,
        Opcode::STURH => 80,
        Opcode::STXP => 81,
        Opcode::STXR => 82,
        Opcode::STXRB => 83,
        Opcode::STXRH => 84,
        Opcode::TBZ => 85,
        Opcode::TBNZ => 86,
        Opcode::CBZ => 87,
        Opcode::CBNZ => 88,
        Opcode::B => 89,
        Opcode::BR => 90,
        Opcode::Bcc(_) => 91,
        Opcode::BL => 92,
        Opcode::BLR => 93,
        Opcode::SVC => 94,
        Opcode::HVC => 95,
        Opcode::SMC => 96,
        Opcode::BRK => 97,
        Opcode::HLT => 98,
        Opcode::DCPS1 => 99,
        Opcode::DCPS2 => 100,
        Opcode::DCPS3 => 101,
        Opcode::RET => 102,
        Opcode::ERET => 103,
        Opcode::DRPS => 104,
        Opcode::MSR => 105,
        Opcode::MRS => 106,
        Opcode::SYS(_) => 107,
        Opcode::SYSL(_) => 108,
        Opcode::ISB => 109,
        Opcode::DSB(_) => 110,
        Opcode::DMB(_) => 111,
        Opcode::SB => 112,
        Opcode::SSSB => 113,
        Opcode::HINT => 114,
        Opcode::CLREX => 115,
        Opcode::CSEL => 116,
        Opcode::CSNEG => 117,
        Opcode::CSINC => 118,
        Opcode::CSINV => 119,
        Opcode::CCMN => 120,
        Opcode::CCMP => 121,
        Opcode::RBIT => 122,
        Opcode::REV16 => 123,
        Opcode::REV => 124,
        Opcode::REV32 => 125,
        Opcode::CLZ => 126,
        Opcode::CLS => 127,
        Opcode::MADD => 128,
        Opcode::MSUB => 129,
        Opcode::SMADDL => 130,
        Opcode::SMSUBL => 131,
        Opcode::SMULH => 132,
        Opcode::UMADDL => 133,
        Opcode::UMSUBL => 134,
        Opcode::UMULH => 135,
        Opcode::UDIV => 136,
        Opcode::SDIV => 137,
        Opcode::LSLV => 138,
        Opcode::LSRV => 139,
        Opcode::ASRV => 140,
        Opcode::RORV => 141,
        Opcode::CRC32B => 142,
        Opcode::CRC32H => 143,
        Opcode::CRC32W => 144,
        Opcode::CRC32X => 145,
        Opcode::CRC32CB => 146,
        Opcode::CRC32CH => 147,
        Opcode::CRC32CW => 148,
        Opcode::CRC32CX => 149,
        Opcode::STNP => 150,
        Opcode::LDNP => 151,
        Opcode::ST1 => 152,
        Opcode::ST2 => 153,
        Opcode::ST3 => 154,
        Opcode::ST4 => 155,
        Opcode::LD1 => 156,
        Opcode::LD2 => 157,
        Opcode::LD3 => 158,
        Opcode::LD4 => 159,
        Opcode::LD1R => 160,
        Opcode::LD2R => 161,
        Opcode::LD3R => 162,
        Opcode::LD4R => 163,
        Opcode::FMADD => 164,
        Opcode::FMSUB => 165,
        Opcode::FNMADD => 166,
        Opcode::FNMSUB => 167,
        Opcode::SCVTF => 168,
        Opcode::UCVTF => 169,
        Opcode::FCVTZS => 170,
        Opcode::FCVTZU => 171,
        Opcode::FMOV => 172,
        Opcode::FABS => 173,
        Opcode::FNEG => 174,
        Opcode::FSQRT => 175,
        Opcode::FRINTN => 176,
        Opcode::FRINTP => 177,
        Opcode::FRINTM => 178,
        Opcode::FRINTZ => 179,
        Opcode::FRINTA => 180,
        Opcode::FRINTX => 181,
        Opcode::FRINTI => 182,
        Opcode::FRINT32Z => 183,
        Opcode::FRINT32X => 184,
        Opcode::FRINT64Z => 185,
        Opcode::FRINT64X => 186,
        Opcode::BFCVT => 187,
        Opcode::FCVT => 188,
        Opcode::FCMP => 189,
        Opcode::FCMPE => 190,
        Opcode::FMUL => 191,
        Opcode::FDIV => 192,
        Opcode::FADD => 193,
        Opcode::FSUB => 194,
        Opcode::FMAX => 195,
        Opcode::FMIN => 196,
        Opcode::FMAXNM => 197,
        Opcode::FMINNM => 198,
        Opcode::FNMUL => 199,
        Opcode::FCSEL => 200,
        Opcode::FCCMP => 201,
        Opcode::FCCMPE => 202,
        Opcode::FMULX => 203,
        Opcode::FMLSL => 204,
        Opcode::FMLAL => 205,
        Opcode::SQRDMLSH => 206,
        Opcode::UDOT => 207,
        Opcode::SQRDMLAH => 208,
        Opcode::UMULL => 209,
        Opcode::UMULL2 => 210,
        Opcode::UMLSL => 211,
        Opcode::UMLSL2 => 212,
        Opcode::MLS => 213,
        Opcode::UMLAL => 214,
        Opcode::UMLAL2 => 215,
        Opcode::MLA => 216,
        Opcode::SDOT => 217,
        Opcode::SQDMULH => 218,
        Opcode::SQDMULL => 219,
        Opcode::SQDMULL2 => 220,
        Opcode::SMULL => 221,
        Opcode::SMULL2 => 222,
        Opcode::MUL => 223,
        Opcode::SQDMLSL => 224,
        Opcode::SQDMLSL2 => 225,
        Opcode::SMLSL => 226,
        Opcode::SMLSL2 => 227,
        Opcode::SQDMLAL => 228,
        Opcode::SQDMLAL2 => 229,
        Opcode::SMLAL => 230,
        Opcode::SMLAL2 => 231,
        Opcode::SQRDMULH => 232,
        Opcode::FCMLA => 233,
        Opcode::SSHR => 234,
        Opcode::SSRA => 235,
        Opcode::SRSHR => 236,
        Opcode::SRSRA => 237,
        Opcode::SHL => 238,
        Opcode::SQSHL => 239,
        Opcode::SHRN => 240,
        Opcode::RSHRN => 241,
        Opcode::SQSHRN => 242,
        Opcode::SQRSHRN => 243,
        Opcode::SSHLL => 244,
        Opcode::USHR => 245,
        Opcode::USRA => 246,
        Opcode::URSHR => 247,
        Opcode::URSRA => 248,
        Opcode::SRI => 249,
        Opcode::SLI => 250,
        Opcode::SQSHLU => 251,
        Opcode::UQSHL => 252,
        Opcode::SQSHRUN => 253,
        Opcode::SQRSHRUN => 254,
        Opcode::UQSHRN => 255,
        Opcode::UQRSHRN => 256,
        Opcode::USHLL => 257,
        Opcode::MOVI => 258,
        Opcode::MVNI => 259,
        Opcode::SHADD => 260,
        Opcode::SQADD => 261,
        Opcode::SRHADD => 262,
        Opcode::SHSUB => 263,
        Opcode::SQSUB => 264,
        Opcode::CMGT => 265,
        Opcode::CMGE => 266,
        Opcode::SSHL => 267,
        Opcode::SRSHL => 268,
        Opcode::SQRSHL => 269,
        Opcode::SMAX => 270,
        Opcode::SMIN => 271,
        Opcode::SABD => 272,
        Opcode::SABA => 273,
        Opcode::CMTST => 274,
        Opcode::SMAXP => 275,
        Opcode::SMINP => 276,
        Opcode::ADDP => 277,
        Opcode::UHADD => 278,
        Opcode::UQADD => 279,
        Opcode::URHADD => 280,
        Opcode::UHSUB => 281,
        Opcode::UQSUB => 282,
        Opcode::CMHI => 283,
        Opcode::CMHS => 284,
        Opcode::USHL => 285,
        Opcode::URSHL => 286,
        Opcode::UQRSHL => 287,
        Opcode::UMAX => 288,
        Opcode::UMIN => 289,
        Opcode::UABD => 290,
        Opcode::UABA => 291,
        Opcode::CMEQ => 292,
        Opcode::PMUL => 293,
        Opcode::UMAXP => 294,
        Opcode::UMINP => 295,
        Opcode::FMLA => 296,
        Opcode::FCMEQ => 297,
        Opcode::FRECPS => 298,
        Opcode::BSL => 299,
        Opcode::BIT => 300,
        Opcode::BIF => 301,
        Opcode::FMAXNMP => 302,
        Opcode::FMINMNP => 303,
        Opcode::FADDP => 304,
        Opcode::FCMGE => 305,
        Opcode::FACGE => 306,
        Opcode::FMAXP => 307,
        Opcode::SADDL => 308,
        Opcode::SADDL2 => 309,
        Opcode::SADDW => 310,
        Opcode::SADDW2 => 311,
        Opcode::SSUBL => 312,
        Opcode::SSUBL2 => 313,
        Opcode::SSUBW => 314,
        Opcode::SSUBW2 => 315,
        Opcode::ADDHN => 316,
        Opcode::ADDHN2 => 317,
        Opcode::SABAL => 318,
        Opcode::SABAL2 => 319,
        Opcode::SUBHN => 320,
        Opcode::SUBHN2 => 321,
        Opcode::SABDL => 322,
        Opcode::SABDL2 => 323,
        Opcode::PMULL => 324,
        Opcode::PMULL2 => 325,
        Opcode::UADDL => 326,
        Opcode::UADDL2 => 327,
        Opcode::UADDW => 328,
        Opcode::UADDW2 => 329,
        Opcode::USUBL => 330,
        Opcode::USUBL2 => 331,
        Opcode::USUBW => 332,
        Opcode::USUBW2 => 333,
        Opcode::RADDHN => 334,
        Opcode::RADDHN2 => 335,
        Opcode::RSUBHN => 336,
        Opcode::RSUBHN2 => 337,
        Opcode::UABAL => 338,
        Opcode::UABAL2 => 339,
        Opcode::UABDL => 340,
        Opcode::UABDL2 => 341,
        Opcode::REV64 => 342,
        Opcode::SADDLP => 343,
        Opcode::SUQADD => 344,
        Opcode::CNT => 345,
        Opcode::SADALP => 346,
        Opcode::SQABS => 347,
        Opcode::CMLT => 348,
        Opcode::ABS => 349,
        Opcode::XTN => 350,
        Opcode::XTN2 => 351,
        Opcode::SQXTN => 352,
        Opcode::SQXTN2 => 353,
        Opcode::FCVTN => 354,
        Opcode::FCVTN2 => 355,
        Opcode::FCMGT => 356,
        Opcode::FCVTL => 357,
        Opcode::FCVTL2 => 358,
        Opcode::FCVTNS => 359,
        Opcode::FCVTPS => 360,
        Opcode::FCVTMS => 361,
        Opcode::FCVTAS => 362,
        Opcode::URECPE => 363,
        Opcode::FRECPE => 364,
        Opcode::UADDLP => 365,
        Opcode::USQADD => 366,
        Opcode::UADALP => 367,
        Opcode::SQNEG => 368,
        Opcode::CMLE => 369,
        Opcode::NEG => 370,
        Opcode::SQXTUN => 371,
        Opcode::SQXTUN2 => 372,
        Opcode::SHLL => 373,
        Opcode::SHLL2 => 374,
        Opcode::UQXTN => 375,
        Opcode::UQXTN2 => 376,
        Opcode::FCVTXN => 377,
        Opcode::FCVTXN2 => 378,
        Opcode::FCVTNU => 379,
        Opcode::FCVTMU => 380,
        Opcode::FCVTAU => 381,
        Opcode::INS => 382,
        Opcode::EXT => 383,
        Opcode::DUP => 384,
        Opcode::UZP1 => 385,
        Opcode::TRN1 => 386,
        Opcode::ZIP1 => 387,
        Opcode::UZP2 => 388,
        Opcode::TRN2 => 389,
        Opcode::ZIP2 => 390,
        Opcode::SMOV => 391,
        Opcode::UMOV => 392,
        Opcode::SQSHRN2 => 393,
        Opcode::SQRSHRN2 => 394,
        Opcode::SQSHRUN2 => 395,
        Opcode::UQSHRN2 => 396,
        Opcode::UQRSHRN2 => 397,
        Opcode::FMLS => 398,
        Opcode::FRECPX => 399,
        Opcode::FRSQRTE => 400,
        Opcode::FCVTPU => 401,
        Opcode::FCMLT => 402,
        Opcode::FCMLE => 403,
        Opcode::FMAXNMV => 404,
        Opcode::FMINNMV => 405,
        Opcode::FMAXV => 406,
        Opcode::FMINV => 407,
        Opcode::UADDLV => 408,
        Opcode::SADDLV => 409,
        Opcode::UMAXV => 410,
        Opcode::SMAXV => 411,
        Opcode::UMINV => 412,
        Opcode::SMINV => 413,
        Opcode::ADDV => 414,
        Opcode::FRSQRTS => 415,
        Opcode::FMINNMP => 416,
        Opcode::FMLAL2 => 417,
        Opcode::FMLSL2 => 418,
        Opcode::FABD => 419,
        Opcode::FACGT => 420,
        Opcode::FMINP => 421,
        Opcode::FJCVTZS => 422,
        Opcode::URSQRTE => 423,
        Opcode::PRFM => 424,
        Opcode::PRFUM => 425,
        Opcode::AESE => 426,
        Opcode::AESD => 427,
        Opcode::AESMC => 428,
        Opcode::AESIMC => 429,
        Opcode::SHA1H => 430,
        Opcode::SHA1SU1 => 431,
        Opcode::SHA256SU0 => 432,
        Opcode::SM3TT1A => 433,
        Opcode::SM3TT1B => 434,
        Opcode::SM3TT2A => 435,
        Opcode::SM3TT2B => 436,
        Opcode::SHA512H => 437,
        Opcode::SHA512H2 => 438,
        Opcode::SHA512SU1 => 439,
        Opcode::RAX1 => 440,
        Opcode::SM3PARTW1 => 441,
        Opcode::SM3PARTW2 => 442,
        Opcode::SM4EKEY => 443,
        Opcode::BCAX => 444,
        Opcode::SM3SS1 => 445,
        Opcode::SHA512SU0 => 446,
        Opcode::SM4E => 447,
        Opcode::EOR3 => 448,
        Opcode::XAR => 449,
        Opcode::LDRAA => 450,
        Opcode::LDRAB => 451,
        Opcode::LDAPR => 452,
        Opcode::LDAPRH => 453,
        Opcode::LDAPRB => 454,
        Opcode::SWP(_) => 455,
        Opcode::SWPB(_) => 456,
        Opcode::SWPH(_) => 457,
        Opcode::LDADDB(_) => 458,
        Opcode::LDCLRB(_) => 459,
        Opcode::LDEORB(_) => 460,
        Opcode::LDSETB(_) => 461,
        Opcode::LDSMAXB(_) => 462,
        Opcode::LDSMINB(_) => 463,
        Opcode::LDUMAXB(_) => 464,
        Opcode::LDUMINB(_) => 465,
        Opcode::LDADDH(_) => 466,
        Opcode::LDCLRH(_) => 467,
        Opcode::LDEORH(_) => 468,
        Opcode::LDSETH(_) => 469,
        Opcode::LDSMAXH(_) => 470,
        Opcode::LDSMINH(_) => 471,
        Opcode::LDUMAXH(_) => 472,
        Opcode::LDUMINH(_) => 473,
        Opcode::LDADD(_) => 474,
        Opcode::LDCLR(_) => 475,
        Opcode::LDEOR(_) => 476,
        Opcode::LDSET(_) => 477,
        Opcode::LDSMAX(_) => 478,
        Opcode::LDSMIN(_) => 479,
        Opcode::LDUMAX(_) => 480,
        Opcode::LDUMIN(_) => 481,
        Opcode::CAS(_) => 482,
        Opcode::CASH(_) => 483,
        Opcode::CASB(_) => 484,
        Opcode::CASP(_) => 485,
        Opcode::TBL => 486,
        Opcode::TBX => 487,
        Opcode::FCADD => 488,
        Opcode::LDGM => 489,
        Opcode::LDG => 490,
        Opcode::STGM => 491,
        Opcode::STZGM => 492,
        Opcode::STG => 493,
        Opcode::STZG => 494,
        Opcode::ST2G => 495,
        Opcode::STZ2G => 496,
        Opcode::LDAPUR => 497,
        Opcode::LDAPURB => 498,
        Opcode::LDAPURH => 499,
        Opcode::LDAPURSB => 500,
        Opcode::LDAPURSH => 501,
        Opcode::LDAPURSW => 502,
        Opcode::STLUR => 503,
        Opcode::STLURB => 504,
        Opcode::STLURH => 505,
        Opcode::SETF8 => 506,
        Opcode::SETF16 => 507,
        Opcode::RMIF => 508,
        Opcode::NOT => 509,
        Opcode::RSHRN2 => 510,
        Opcode::SQRSHRUN2 => 511,
        Opcode::USHLL2 => 512,
        Opcode::SSHLL2 => 513,
        Opcode::SHA1C => 514,
        Opcode::SHA1P => 515,
        Opcode::SHA1M => 516,
        Opcode::SHA1SU0 => 517,
        Opcode::SHA256H => 518,
        Opcode::SHA256H2 => 519,
        Opcode::SHA256SU1 => 520,
        Opcode::SHRN2 => 521,
        Opcode::BLRAA => 522,
        Opcode::BLRAAZ => 523,
        Opcode::BLRAB => 524,
        Opcode::BLRABZ => 525,
        Opcode::BRAA => 526,
        Opcode::BRAAZ => 527,
        Opcode::BRAB => 528,
        Opcode::BRABZ => 529,
        Opcode::RETAA => 530,
        Opcode::RETAB => 531,
        Opcode::ERETAA => 532,
        Opcode::ERETAB => 533,
        Opcode::PACIA => 534,
        Opcode::PACIB => 535,
        Opcode::PACDA => 536,
        Opcode::PACDB => 537,
        Opcode::AUTIA => 538,
        Opcode::AUTIB => 539,
        Opcode::AUTDA => 540,
        Opcode::AUTDB => 541,
        Opcode::PACIZA => 542,
        Opcode::PACIZB => 543,
        Opcode::PACDZA => 544,
        Opcode::PACDZB => 545,
        Opcode::AUTIZA => 546,
        Opcode::AUTIZB => 547,
        Opcode::AUTDZA => 548,
        Opcode::AUTDZB => 549,
        Opcode::XPACI => 550,
        Opcode::XPACD => 551,
        Opcode::PACGA => 552,
        Opcode::GMI => 553,
        Opcode::IRG => 554,
        Opcode::SUBP => 555,
        Opcode::SUBPS => 556,
    }
}

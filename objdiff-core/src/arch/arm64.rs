use std::{borrow::Cow, cmp::Ordering, collections::BTreeMap};

use anyhow::Result;
use object::{elf, File, Relocation, RelocationFlags};
use yaxpeax_arch::{Arch, Decoder, Reader, U8Reader};
use yaxpeax_arm::armv8::a64::{
    ARMv8, DecodeError, InstDecoder, Instruction, Opcode, Operand, ShiftStyle, SizeCode,
};

use crate::{
    arch::{ObjArch, ProcessCodeResult},
    diff::DiffObjConfig,
    obj::{ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjSection},
};

pub struct ObjArchArm64 {}

impl ObjArchArm64 {
    pub fn new(file: &File) -> Result<Self> { Ok(Self {}) }
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
    ) -> Result<ProcessCodeResult> {
        let start_address = address;
        let end_address = address + code.len() as u64;
        let ins_count = code.len() / 4;

        let mut ops = Vec::with_capacity(ins_count);
        let mut insts = Vec::with_capacity(ins_count);

        let mut reader = U8Reader::new(code);
        let mut decoder = InstDecoder::default();
        let mut ins = Instruction::default();
        loop {
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

            let mut branch_dest = None;

            let op = opcode_to_u16(ins.opcode);

            let mut args = vec![];
            let mut ctx = DisplayCtx {
                address,
                section_index,
                start_address,
                end_address,
                reloc: reloc.as_ref(),
                config,
                branch_dest: &mut branch_dest,
            };
            // for (i, o) in ins.operands.iter().enumerate() {
            //     match o {
            //         Operand::Nothing => break,
            //         _ => {}
            //     }

            //     if i > 0 {
            //         push_separator(&mut args, config);
            //     }

            //     push_operand(&mut args, o, &mut ctx);
            // }
            let mnemonic = display_instruction(&mut args, &ins, &mut ctx);

            // if let Some(reloc) = &reloc {
            //     match reloc.flags {
            //         RelocationFlags::Elf { r_type: elf::R_AARCH64_CALL26 } => {}
            //         RelocationFlags::Elf { r_type: elf::R_AARCH64_ADR_PREL_PG_HI21 } => {
            //             if let Some(arg) = args
            //                 .iter_mut()
            //                 .rfind(|a| matches!(a, ObjInsArg::Arg(ObjInsArgValue::Unsigned(_))))
            //             {
            //                 *arg = ObjInsArg::Reloc;
            //             }
            //         }
            //         RelocationFlags::Elf { r_type: elf::R_AARCH64_ADD_ABS_LO12_NC } => {
            //             if let Some(arg) = args
            //                 .iter_mut()
            //                 .rfind(|a| matches!(a, ObjInsArg::Arg(ObjInsArgValue::Unsigned(_))))
            //             {
            //                 *arg = ObjInsArg::Reloc;
            //             }
            //         }
            //         _ => (),
            //     }
            // };

            ops.push(op);
            insts.push(ObjIns {
                address,
                size: 4,
                op,
                mnemonic: Cow::Borrowed(mnemonic),
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
        file: &File<'_>,
        section: &ObjSection,
        address: u64,
        reloc: &Relocation,
    ) -> anyhow::Result<i64> {
        todo!()
    }

    fn demangle(&self, name: &str) -> Option<String> {
        cpp_demangle::Symbol::new(name)
            .ok()
            .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
    }

    fn display_reloc(&self, flags: RelocationFlags) -> Cow<'static, str> {
        match flags {
            RelocationFlags::Elf { r_type: elf::R_AARCH64_CALL26 } => {
                Cow::Borrowed("R_AARCH64_CALL26")
            }
            RelocationFlags::Elf { r_type: elf::R_AARCH64_ADR_PREL_PG_HI21 } => {
                Cow::Borrowed("R_AARCH64_ADR_PREL_PG_HI21")
            }
            RelocationFlags::Elf { r_type: elf::R_AARCH64_ADD_ABS_LO12_NC } => {
                Cow::Borrowed("R_AARCH64_ADD_ABS_LO12_NC")
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
    branch_dest: &'a mut Option<u64>,
}

fn display_instruction(
    args: &mut Vec<ObjInsArg>,
    ins: &Instruction,
    ctx: &mut DisplayCtx,
) -> &'static str {
    let mnemonic = match ins.opcode {
        Opcode::Invalid => {
            return "<invalid>";
        }
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
                // return write!(fmt, "ngc {}, {}", ins.operands[0], ins.operands[2]);
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
                // return write!(fmt, "ngc {}, {}", ins.operands[0], ins.operands[2]);
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
            /*

            if let Operand::Register(_, 31) = self.operands[1] {
                if let Operand::Immediate(0) = self.operands[2] {
                    return write!(fmt, "mov {}, {}", self.operands[0], self.operands[1]);
                } else if let Operand::RegShift(style, amt, size, r) = self.operands[2] {
                    if style == ShiftStyle::LSL && amt == 0 {
                        return write!(fmt, "mov {}, {}", self.operands[0], Operand::Register(size, r));
                    }
                } else {
                    return write!(fmt, "mov {}, {}", self.operands[0], self.operands[2]);
                }
            } else if self.operands[1] == self.operands[2] {
                return write!(fmt, "mov {}, {}", self.operands[0], self.operands[1]);
            }
            write!(fmt, "orr")?; */

            if let Operand::Register(_, 31) = ins.operands[1] {
                if let Operand::Immediate(0) = ins.operands[2] {
                    // return write!(fmt, "mov {}, {}", ins.operands[0], ins.operands[1]);
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    return "mov";
                } else if let Operand::RegShift(style, amt, size, r) = ins.operands[2] {
                    if style == ShiftStyle::LSL && amt == 0 {
                        // return write!(fmt, "mov {}, {}", ins.operands[0], Operand::Register(size, r));
                        push_operand(args, &ins.operands[0], ctx);
                        push_separator(args, ctx.config);
                        push_register(args, size, r, false);
                        return "mov";
                    }
                } else {
                    // return write!(fmt, "mov {}, {}", ins.operands[0], ins.operands[2]);
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    return "mov";
                }
            } else if ins.operands[1] == ins.operands[2] {
                // return write!(fmt, "mov {}, {}", ins.operands[0], ins.operands[1]);
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                return "mov";
            }
            // write!(fmt, "orr")?;
            "orr"
        }
        Opcode::ORN => {
            /*

            if let Operand::Register(_, 31) = self.operands[1] {
                return write!(fmt, "mvn {}, {}", self.operands[0], self.operands[2]);
            }
            write!(fmt, "orn")?; */
            if let Operand::Register(_, 31) = ins.operands[1] {
                // return write!(fmt, "mvn {}, {}", ins.operands[0], ins.operands[2]);
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "mvn";
            }
            // write!(fmt, "orn")?;
            "orn"
        }
        Opcode::EOR => "eor",
        Opcode::EON => "eon",
        Opcode::BIC => "bic",
        Opcode::BICS => "bics",
        Opcode::ANDS => {
            if let Operand::Register(_, 31) = ins.operands[0] {
                // return write!(fmt, "tst {}, {}", ins.operands[1], ins.operands[2]);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "tst";
            }
            "ands"
        }
        Opcode::ADDS => {
            /*

            if let Operand::Register(_, 31) = self.operands[0] {
                return write!(fmt, "cmn {}, {}", self.operands[1], self.operands[2]);
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = self.operands[2] {
                return write!(fmt, "adds {}, {}, {}", self.operands[0], self.operands[1], Operand::Register(size, reg));
            }
            write!(fmt, "adds")?; */
            if let Operand::Register(_, 31) = ins.operands[0] {
                // return write!(fmt, "cmn {}, {}", ins.operands[1], ins.operands[2]);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "cmn";
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = ins.operands[2] {
                // return write!(fmt, "adds {}, {}, {}", self.operands[0], self.operands[1], Operand::Register(size, reg));
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_register(args, size, reg, false);
                return "adds";
            }
            // write!(fmt, "adds")?;
            "adds"
        }
        Opcode::ADD => {
            /*

                            if let Operand::Immediate(0) = self.operands[2] {
                                if let Operand::RegisterOrSP(_, 31) = self.operands[0] {
                                    return write!(fmt, "mov {}, {}", self.operands[0], self.operands[1]);
                                } else if let Operand::RegisterOrSP(_, 31) = self.operands[1] {
                                    return write!(fmt, "mov {}, {}", self.operands[0], self.operands[1]);
                                }
            // oh. add-with-zr does not alias mov
            //                } else if let Operand::Register(_, 31) = self.operands[1] {
            //                    return write!(fmt, "mov {}, {}", self.operands[0], self.operands[2]);
                            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = self.operands[2] {
                                return write!(fmt, "add {}, {}, {}", self.operands[0], self.operands[1], Operand::Register(size, reg));
                            }
                            write!(fmt, "add")?; */

            if let Operand::Immediate(0) = ins.operands[2] {
                if let Operand::RegisterOrSP(size, 31) = ins.operands[0] {
                    // return write!(fmt, "mov {}, {}", self.operands[0], self.operands[1]);
                    push_register(args, size, 31, true);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    return "mov";
                } else if let Operand::RegisterOrSP(size, 31) = ins.operands[1] {
                    // return write!(fmt, "mov {}, {}", self.operands[0], self.operands[1]);
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_register(args, size, 31, true);
                    return "mov";
                }
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = ins.operands[2] {
                // return write!(fmt, "add {}, {}, {}", self.operands[0], self.operands[1], Operand::Register(size, reg));
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_register(args, size, reg, false);
                return "add";
            }
            // write!(fmt, "add")?;
            "add"
        }
        Opcode::SUBS => {
            /*

            if let Operand::Register(_, 31) = self.operands[0] {
                return write!(fmt, "cmp {}, {}", self.operands[1], self.operands[2])
            } else if let Operand::Register(_, 31) = self.operands[1] {
                return write!(fmt, "negs {}, {}", self.operands[0], self.operands[2])
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = self.operands[2] {
                return write!(fmt, "subs {}, {}, {}", self.operands[0], self.operands[1], Operand::Register(size, reg));
            }
            write!(fmt, "subs")?; */
            if let Operand::Register(_, 31) = ins.operands[0] {
                // return write!(fmt, "cmp {}, {}", ins.operands[1], ins.operands[2])
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "cmp";
            } else if let Operand::Register(_, 31) = ins.operands[1] {
                // return write!(fmt, "negs {}, {}", ins.operands[0], ins.operands[2])
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "negs";
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = ins.operands[2] {
                // return write!(fmt, "subs {}, {}, {}", ins.operands[0], ins.operands[1], Operand::Register(size, reg));
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_register(args, size, reg, false);
                return "subs";
            }
            // write!(fmt, "subs")?;
            "subs"
        }
        Opcode::SUB => {
            /*

            if let Operand::Register(_, 31) = self.operands[1] {
                return write!(fmt, "neg {}, {}", self.operands[0], self.operands[2])
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = self.operands[2] {
                return write!(fmt, "sub {}, {}, {}", self.operands[0], self.operands[1], Operand::Register(size, reg));
            }
            write!(fmt, "sub")?; */

            if let Operand::Register(_, 31) = ins.operands[1] {
                // return write!(fmt, "neg {}, {}", ins.operands[0], ins.operands[2])
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[2], ctx);
                return "neg";
            } else if let Operand::RegShift(ShiftStyle::LSL, 0, size, reg) = ins.operands[2] {
                // return write!(fmt, "sub {}, {}, {}", ins.operands[0], ins.operands[1], Operand::Register(size, reg));
                push_operand(args, &ins.operands[0], ctx);
                push_separator(args, ctx.config);
                push_operand(args, &ins.operands[1], ctx);
                push_separator(args, ctx.config);
                push_register(args, size, reg, false);
                return "sub";
            }
            // write!(fmt, "sub")?;
            "sub"
        }
        Opcode::BFM => {
            /*

               if let (Operand::Immediate(immr), Operand::Immediate(imms)) = (self.operands[2], self.operands[3]) {
                   if imms < immr {
                       if let Operand::Register(sz, rn) = self.operands[1] {
                           let width = imms + 1;
                           let lsb = if sz == SizeCode::W {
                               ((-(immr as i8)) as u8) & 0x1f
                           } else {
                               ((-(immr as i8)) as u8) & 0x3f
                           };
                           if rn == 31 {
                               return write!(fmt, "bfc {}, #{:#x}, #{:#x}", self.operands[0], lsb, width);
                           } else {
                               return write!(fmt, "bfi {}, {}, #{:#x}, #{:#x}", self.operands[0], self.operands[1], lsb, width);
                           }
                       }
                   } else {
                       // bfxil
                       let lsb = immr;
                       let width = imms + 1 - lsb;
                       return write!(fmt, "bfxil {}, {}, #{:#x}, #{:#x}", self.operands[0], self.operands[1], lsb, width);
                   }
               }
            */

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
                        if rn == 31 {
                            // return write!(fmt, "bfc {}, #{:#x}, #{:#x}", ins.operands[0], lsb, width);
                            push_operand(args, &ins.operands[0], ctx);
                            push_separator(args, ctx.config);
                            push_unsigned(args, lsb as u64);
                            push_separator(args, ctx.config);
                            push_unsigned(args, width as u64);
                            return "bfc";
                        } else {
                            // return write!(fmt, "bfi {}, {}, #{:#x}, #{:#x}", ins.operands[0], ins.operands[1], lsb, width);
                            push_operand(args, &ins.operands[0], ctx);
                            push_separator(args, ctx.config);
                            push_operand(args, &ins.operands[1], ctx);
                            push_separator(args, ctx.config);
                            push_unsigned(args, lsb as u64);
                            push_separator(args, ctx.config);
                            push_unsigned(args, width as u64);
                            return "bfi";
                        }
                    }
                } else {
                    // bfxil
                    let lsb = immr;
                    let width = imms + 1 - lsb;
                    // return write!(fmt, "bfxil {}, {}, #{:#x}, #{:#x}", ins.operands[0], ins.operands[1], lsb, width);
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
                    // return write!(fmt, "uxtb {}, {}", ins.operands[0], ins.operands[1]);
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[1], ctx);
                    return "uxtb";
                } else if let Operand::Immediate(15) = ins.operands[3] {
                    // return write!(fmt, "uxth {}, {}", ins.operands[0], ins.operands[1]);
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
                        // return write!(fmt, "lsr {}, {}, {}", ins.operands[0], ins.operands[1], ins.operands[2]);
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
                            // return write!(fmt, "lsl {}, {}, #{:#x}", ins.operands[0], ins.operands[1], size - imms - 1);
                            push_operand(args, &ins.operands[0], ctx);
                            push_separator(args, ctx.config);
                            push_operand(args, &ins.operands[1], ctx);
                            push_separator(args, ctx.config);
                            push_unsigned(args, (size - imms - 1) as u64);
                            return "lsl";
                        }
                        if imms < immr {
                            // return write!(fmt, "ubfiz {}, {}, #{:#x}, #{:#x}",
                            //     ins.operands[0],
                            //     ins.operands[1],
                            //     size - immr,
                            //     imms + 1,
                            // );
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
            // return write!(fmt, "ubfx {}, {}, {}, {}", ins.operands[0], ins.operands[1], ins.operands[2], width);
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
                    // return write!(fmt, "asr {}, {}, {}", ins.operands[0], ins.operands[1], ins.operands[2]);
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
                    // return write!(fmt, "asr {}, {}, {}", ins.operands[0], ins.operands[1], ins.operands[2]);
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
                    // return write!(fmt, "sxtb {}, {}", ins.operands[0], newsrc);
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &newsrc, ctx);
                    return "sxtb";
                } else if let Operand::Immediate(15) = ins.operands[3] {
                    // return write!(fmt, "sxth {}, {}", ins.operands[0], newsrc);
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &newsrc, ctx);
                    return "sxth";
                } else if let Operand::Immediate(31) = ins.operands[3] {
                    // return write!(fmt, "sxtw {}, {}", ins.operands[0], newsrc);
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
                    // return write!(fmt, "sbfiz {}, {}, #{:#x}, #{:#x}",
                    //     ins.operands[0],
                    //     ins.operands[1],
                    //     size - imms,
                    //     immr + 1,
                    // );
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
            // return write!(fmt, "sbfx {}, {}, {}, {}", ins.operands[0], ins.operands[1], ins.operands[2], width);
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
            if let (Operand::Register(_, Rn), Operand::Register(_, Rm)) =
                (ins.operands[1], ins.operands[2])
            {
                if Rn == Rm {
                    // return write!(fmt, "ror {}, {}, {}", ins.operands[0], ins.operands[2], ins.operands[3]);
                    push_operand(args, &ins.operands[0], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[2], ctx);
                    push_separator(args, ctx.config);
                    push_operand(args, &ins.operands[3], ctx);
                    return "ror";
                }
            }
            // write!(fmt, "extr")?;
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
        // Opcode::Bcc(_) => {} TODO
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
            /*
            
                write!(fmt, "ret")?;
                if let Operand::Register(SizeCode::X, 30) = self.operands[0] {
                    // C5.6.148:  Defaults to X30 if absent.
                    // so ret x30 is probably expected to be read as just `ret`
                    return Ok(());
                } */
            
                if let Operand::Register(SizeCode::X, 30) = ins.operands[0] {
                    return "ret";
                }
                "ret"
        }
        Opcode::ERET => "eret",
        Opcode::DRPS => "drps",
        Opcode::MSR => "msr",
        Opcode::MRS => "mrs",
        Opcode::SYS(ops) => {
            // return write!(fmt, "sys #{:#x}, {}, {}, #{:#x}, {}",
            //     ops.op1(),
            //     self.operands[1],
            //     self.operands[2],
            //     ops.op2(),
            //     self.operands[0],
            // );
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
            // return write!(fmt, "sysl {}, #{:#x}, {}, {}, #{:#x}",
            //     self.operands[2],
            //     ops.op1(),
            //     self.operands[0],
            //     self.operands[1],
            //     ops.op2(),
            // );
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
            return "dsb";
        }
        Opcode::DMB(option) => {
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
            return "dmb";
        }
        Opcode::SB => "sb",
        Opcode::SSSB => "sssb",
        // Opcode::HINT => {}
        // Opcode::CLREX => {}
        // Opcode::CSEL => {}
        // Opcode::CSNEG => {}
        // Opcode::CSINC => {}
        // Opcode::CSINV => {}
        // Opcode::CCMN => {}
        // Opcode::CCMP => {}
        // Opcode::RBIT => {}
        // Opcode::REV16 => {}
        // Opcode::REV => {}
        // Opcode::REV32 => {}
        // Opcode::CLZ => {}
        // Opcode::CLS => {}
        // Opcode::MADD => {}
        // Opcode::MSUB => {}
        // Opcode::SMADDL => {}
        // Opcode::SMSUBL => {}
        // Opcode::SMULH => {}
        // Opcode::UMADDL => {}
        // Opcode::UMSUBL => {}
        // Opcode::UMULH => {}
        // Opcode::UDIV => {}
        // Opcode::SDIV => {}
        // Opcode::LSLV => {}
        // Opcode::LSRV => {}
        // Opcode::ASRV => {}
        // Opcode::RORV => {}
        // Opcode::CRC32B => {}
        // Opcode::CRC32H => {}
        // Opcode::CRC32W => {}
        // Opcode::CRC32X => {}
        // Opcode::CRC32CB => {}
        // Opcode::CRC32CH => {}
        // Opcode::CRC32CW => {}
        // Opcode::CRC32CX => {}
        // Opcode::STNP => {}
        // Opcode::LDNP => {}
        // Opcode::ST1 => {}
        // Opcode::ST2 => {}
        // Opcode::ST3 => {}
        // Opcode::ST4 => {}
        // Opcode::LD1 => {}
        // Opcode::LD2 => {}
        // Opcode::LD3 => {}
        // Opcode::LD4 => {}
        // Opcode::LD1R => {}
        // Opcode::LD2R => {}
        // Opcode::LD3R => {}
        // Opcode::LD4R => {}
        // Opcode::FMADD => {}
        // Opcode::FMSUB => {}
        // Opcode::FNMADD => {}
        // Opcode::FNMSUB => {}
        // Opcode::SCVTF => {}
        // Opcode::UCVTF => {}
        // Opcode::FCVTZS => {}
        // Opcode::FCVTZU => {}
        // Opcode::FMOV => {}
        // Opcode::FABS => {}
        // Opcode::FNEG => {}
        // Opcode::FSQRT => {}
        // Opcode::FRINTN => {}
        // Opcode::FRINTP => {}
        // Opcode::FRINTM => {}
        // Opcode::FRINTZ => {}
        // Opcode::FRINTA => {}
        // Opcode::FRINTX => {}
        // Opcode::FRINTI => {}
        // Opcode::FRINT32Z => {}
        // Opcode::FRINT32X => {}
        // Opcode::FRINT64Z => {}
        // Opcode::FRINT64X => {}
        // Opcode::BFCVT => {}
        // Opcode::FCVT => {}
        // Opcode::FCMP => {}
        // Opcode::FCMPE => {}
        // Opcode::FMUL => {}
        // Opcode::FDIV => {}
        // Opcode::FADD => {}
        // Opcode::FSUB => {}
        // Opcode::FMAX => {}
        // Opcode::FMIN => {}
        // Opcode::FMAXNM => {}
        // Opcode::FMINNM => {}
        // Opcode::FNMUL => {}
        // Opcode::FCSEL => {}
        // Opcode::FCCMP => {}
        // Opcode::FCCMPE => {}
        // Opcode::FMULX => {}
        // Opcode::FMLSL => {}
        // Opcode::FMLAL => {}
        // Opcode::SQRDMLSH => {}
        // Opcode::UDOT => {}
        // Opcode::SQRDMLAH => {}
        // Opcode::UMULL => {}
        // Opcode::UMULL2 => {}
        // Opcode::UMLSL => {}
        // Opcode::UMLSL2 => {}
        // Opcode::MLS => {}
        // Opcode::UMLAL => {}
        // Opcode::UMLAL2 => {}
        // Opcode::MLA => {}
        // Opcode::SDOT => {}
        // Opcode::SQDMULH => {}
        // Opcode::SQDMULL => {}
        // Opcode::SQDMULL2 => {}
        // Opcode::SMULL => {}
        // Opcode::SMULL2 => {}
        // Opcode::MUL => {}
        // Opcode::SQDMLSL => {}
        // Opcode::SQDMLSL2 => {}
        // Opcode::SMLSL => {}
        // Opcode::SMLSL2 => {}
        // Opcode::SQDMLAL => {}
        // Opcode::SQDMLAL2 => {}
        // Opcode::SMLAL => {}
        // Opcode::SMLAL2 => {}
        // Opcode::SQRDMULH => {}
        // Opcode::FCMLA => {}
        // Opcode::SSHR => {}
        // Opcode::SSRA => {}
        // Opcode::SRSHR => {}
        // Opcode::SRSRA => {}
        // Opcode::SHL => {}
        // Opcode::SQSHL => {}
        // Opcode::SHRN => {}
        // Opcode::RSHRN => {}
        // Opcode::SQSHRN => {}
        // Opcode::SQRSHRN => {}
        // Opcode::SSHLL => {}
        // Opcode::USHR => {}
        // Opcode::USRA => {}
        // Opcode::URSHR => {}
        // Opcode::URSRA => {}
        // Opcode::SRI => {}
        // Opcode::SLI => {}
        // Opcode::SQSHLU => {}
        // Opcode::UQSHL => {}
        // Opcode::SQSHRUN => {}
        // Opcode::SQRSHRUN => {}
        // Opcode::UQSHRN => {}
        // Opcode::UQRSHRN => {}
        // Opcode::USHLL => {}
        // Opcode::MOVI => {}
        // Opcode::MVNI => {}
        // Opcode::SHADD => {}
        // Opcode::SQADD => {}
        // Opcode::SRHADD => {}
        // Opcode::SHSUB => {}
        // Opcode::SQSUB => {}
        // Opcode::CMGT => {}
        // Opcode::CMGE => {}
        // Opcode::SSHL => {}
        // Opcode::SRSHL => {}
        // Opcode::SQRSHL => {}
        // Opcode::SMAX => {}
        // Opcode::SMIN => {}
        // Opcode::SABD => {}
        // Opcode::SABA => {}
        // Opcode::CMTST => {}
        // Opcode::SMAXP => {}
        // Opcode::SMINP => {}
        // Opcode::ADDP => {}
        // Opcode::UHADD => {}
        // Opcode::UQADD => {}
        // Opcode::URHADD => {}
        // Opcode::UHSUB => {}
        // Opcode::UQSUB => {}
        // Opcode::CMHI => {}
        // Opcode::CMHS => {}
        // Opcode::USHL => {}
        // Opcode::URSHL => {}
        // Opcode::UQRSHL => {}
        // Opcode::UMAX => {}
        // Opcode::UMIN => {}
        // Opcode::UABD => {}
        // Opcode::UABA => {}
        // Opcode::CMEQ => {}
        // Opcode::PMUL => {}
        // Opcode::UMAXP => {}
        // Opcode::UMINP => {}
        // Opcode::FMLA => {}
        // Opcode::FCMEQ => {}
        // Opcode::FRECPS => {}
        // Opcode::BSL => {}
        // Opcode::BIT => {}
        // Opcode::BIF => {}
        // Opcode::FMAXNMP => {}
        // Opcode::FMINMNP => {}
        // Opcode::FADDP => {}
        // Opcode::FCMGE => {}
        // Opcode::FACGE => {}
        // Opcode::FMAXP => {}
        // Opcode::SADDL => {}
        // Opcode::SADDL2 => {}
        // Opcode::SADDW => {}
        // Opcode::SADDW2 => {}
        // Opcode::SSUBL => {}
        // Opcode::SSUBL2 => {}
        // Opcode::SSUBW => {}
        // Opcode::SSUBW2 => {}
        // Opcode::ADDHN => {}
        // Opcode::ADDHN2 => {}
        // Opcode::SABAL => {}
        // Opcode::SABAL2 => {}
        // Opcode::SUBHN => {}
        // Opcode::SUBHN2 => {}
        // Opcode::SABDL => {}
        // Opcode::SABDL2 => {}
        // Opcode::PMULL => {}
        // Opcode::PMULL2 => {}
        // Opcode::UADDL => {}
        // Opcode::UADDL2 => {}
        // Opcode::UADDW => {}
        // Opcode::UADDW2 => {}
        // Opcode::USUBL => {}
        // Opcode::USUBL2 => {}
        // Opcode::USUBW => {}
        // Opcode::USUBW2 => {}
        // Opcode::RADDHN => {}
        // Opcode::RADDHN2 => {}
        // Opcode::RSUBHN => {}
        // Opcode::RSUBHN2 => {}
        // Opcode::UABAL => {}
        // Opcode::UABAL2 => {}
        // Opcode::UABDL => {}
        // Opcode::UABDL2 => {}
        // Opcode::REV64 => {}
        // Opcode::SADDLP => {}
        // Opcode::SUQADD => {}
        // Opcode::CNT => {}
        // Opcode::SADALP => {}
        // Opcode::SQABS => {}
        // Opcode::CMLT => {}
        // Opcode::ABS => {}
        // Opcode::XTN => {}
        // Opcode::XTN2 => {}
        // Opcode::SQXTN => {}
        // Opcode::SQXTN2 => {}
        // Opcode::FCVTN => {}
        // Opcode::FCVTN2 => {}
        // Opcode::FCMGT => {}
        // Opcode::FCVTL => {}
        // Opcode::FCVTL2 => {}
        // Opcode::FCVTNS => {}
        // Opcode::FCVTPS => {}
        // Opcode::FCVTMS => {}
        // Opcode::FCVTAS => {}
        // Opcode::URECPE => {}
        // Opcode::FRECPE => {}
        // Opcode::UADDLP => {}
        // Opcode::USQADD => {}
        // Opcode::UADALP => {}
        // Opcode::SQNEG => {}
        // Opcode::CMLE => {}
        // Opcode::NEG => {}
        // Opcode::SQXTUN => {}
        // Opcode::SQXTUN2 => {}
        // Opcode::SHLL => {}
        // Opcode::SHLL2 => {}
        // Opcode::UQXTN => {}
        // Opcode::UQXTN2 => {}
        // Opcode::FCVTXN => {}
        // Opcode::FCVTXN2 => {}
        // Opcode::FCVTNU => {}
        // Opcode::FCVTMU => {}
        // Opcode::FCVTAU => {}
        // Opcode::INS => {}
        // Opcode::EXT => {}
        // Opcode::DUP => {}
        // Opcode::UZP1 => {}
        // Opcode::TRN1 => {}
        // Opcode::ZIP1 => {}
        // Opcode::UZP2 => {}
        // Opcode::TRN2 => {}
        // Opcode::ZIP2 => {}
        // Opcode::SMOV => {}
        // Opcode::UMOV => {}
        // Opcode::SQSHRN2 => {}
        // Opcode::SQRSHRN2 => {}
        // Opcode::SQSHRUN2 => {}
        // Opcode::UQSHRN2 => {}
        // Opcode::UQRSHRN2 => {}
        // Opcode::FMLS => {}
        // Opcode::FRECPX => {}
        // Opcode::FRSQRTE => {}
        // Opcode::FCVTPU => {}
        // Opcode::FCMLT => {}
        // Opcode::FCMLE => {}
        // Opcode::FMAXNMV => {}
        // Opcode::FMINNMV => {}
        // Opcode::FMAXV => {}
        // Opcode::FMINV => {}
        // Opcode::UADDLV => {}
        // Opcode::SADDLV => {}
        // Opcode::UMAXV => {}
        // Opcode::SMAXV => {}
        // Opcode::UMINV => {}
        // Opcode::SMINV => {}
        // Opcode::ADDV => {}
        // Opcode::FRSQRTS => {}
        // Opcode::FMINNMP => {}
        // Opcode::FMLAL2 => {}
        // Opcode::FMLSL2 => {}
        // Opcode::FABD => {}
        // Opcode::FACGT => {}
        // Opcode::FMINP => {}
        // Opcode::FJCVTZS => {}
        // Opcode::URSQRTE => {}
        // Opcode::PRFM => {}
        // Opcode::PRFUM => {}
        // Opcode::AESE => {}
        // Opcode::AESD => {}
        // Opcode::AESMC => {}
        // Opcode::AESIMC => {}
        // Opcode::SHA1H => {}
        // Opcode::SHA1SU1 => {}
        // Opcode::SHA256SU0 => {}
        // Opcode::SM3TT1A => {}
        // Opcode::SM3TT1B => {}
        // Opcode::SM3TT2A => {}
        // Opcode::SM3TT2B => {}
        // Opcode::SHA512H => {}
        // Opcode::SHA512H2 => {}
        // Opcode::SHA512SU1 => {}
        // Opcode::RAX1 => {}
        // Opcode::SM3PARTW1 => {}
        // Opcode::SM3PARTW2 => {}
        // Opcode::SM4EKEY => {}
        // Opcode::BCAX => {}
        // Opcode::SM3SS1 => {}
        // Opcode::SHA512SU0 => {}
        // Opcode::SM4E => {}
        // Opcode::EOR3 => {}
        // Opcode::XAR => {}
        // Opcode::LDRAA => {}
        // Opcode::LDRAB => {}
        // Opcode::LDAPR => {}
        // Opcode::LDAPRH => {}
        // Opcode::LDAPRB => {}
        // Opcode::SWP(_) => {}
        // Opcode::SWPB(_) => {}
        // Opcode::SWPH(_) => {}
        // Opcode::LDADDB(_) => {}
        // Opcode::LDCLRB(_) => {}
        // Opcode::LDEORB(_) => {}
        // Opcode::LDSETB(_) => {}
        // Opcode::LDSMAXB(_) => {}
        // Opcode::LDSMINB(_) => {}
        // Opcode::LDUMAXB(_) => {}
        // Opcode::LDUMINB(_) => {}
        // Opcode::LDADDH(_) => {}
        // Opcode::LDCLRH(_) => {}
        // Opcode::LDEORH(_) => {}
        // Opcode::LDSETH(_) => {}
        // Opcode::LDSMAXH(_) => {}
        // Opcode::LDSMINH(_) => {}
        // Opcode::LDUMAXH(_) => {}
        // Opcode::LDUMINH(_) => {}
        // Opcode::LDADD(_) => {}
        // Opcode::LDCLR(_) => {}
        // Opcode::LDEOR(_) => {}
        // Opcode::LDSET(_) => {}
        // Opcode::LDSMAX(_) => {}
        // Opcode::LDSMIN(_) => {}
        // Opcode::LDUMAX(_) => {}
        // Opcode::LDUMIN(_) => {}
        // Opcode::CAS(_) => {}
        // Opcode::CASH(_) => {}
        // Opcode::CASB(_) => {}
        // Opcode::CASP(_) => {}
        // Opcode::TBL => {}
        // Opcode::TBX => {}
        // Opcode::FCADD => {}
        // Opcode::LDGM => {}
        // Opcode::LDG => {}
        // Opcode::STGM => {}
        // Opcode::STZGM => {}
        // Opcode::STG => {}
        // Opcode::STZG => {}
        // Opcode::ST2G => {}
        // Opcode::STZ2G => {}
        // Opcode::LDAPUR => {}
        // Opcode::LDAPURB => {}
        // Opcode::LDAPURH => {}
        // Opcode::LDAPURSB => {}
        // Opcode::LDAPURSH => {}
        // Opcode::LDAPURSW => {}
        // Opcode::STLUR => {}
        // Opcode::STLURB => {}
        // Opcode::STLURH => {}
        // Opcode::SETF8 => {}
        // Opcode::SETF16 => {}
        // Opcode::RMIF => {}
        // Opcode::NOT => {}
        // Opcode::RSHRN2 => {}
        // Opcode::SQRSHRUN2 => {}
        // Opcode::USHLL2 => {}
        // Opcode::SSHLL2 => {}
        // Opcode::SHA1C => {}
        // Opcode::SHA1P => {}
        // Opcode::SHA1M => {}
        // Opcode::SHA1SU0 => {}
        // Opcode::SHA256H => {}
        // Opcode::SHA256H2 => {}
        // Opcode::SHA256SU1 => {}
        // Opcode::SHRN2 => {}
        // Opcode::BLRAA => {}
        // Opcode::BLRAAZ => {}
        // Opcode::BLRAB => {}
        // Opcode::BLRABZ => {}
        // Opcode::BRAA => {}
        // Opcode::BRAAZ => {}
        // Opcode::BRAB => {}
        // Opcode::BRABZ => {}
        // Opcode::RETAA => {}
        // Opcode::RETAB => {}
        // Opcode::ERETAA => {}
        // Opcode::ERETAB => {}
        // Opcode::PACIA => {}
        // Opcode::PACIB => {}
        // Opcode::PACDA => {}
        // Opcode::PACDB => {}
        // Opcode::AUTIA => {}
        // Opcode::AUTIB => {}
        // Opcode::AUTDA => {}
        // Opcode::AUTDB => {}
        // Opcode::PACIZA => {}
        // Opcode::PACIZB => {}
        // Opcode::PACDZA => {}
        // Opcode::PACDZB => {}
        // Opcode::AUTIZA => {}
        // Opcode::AUTIZB => {}
        // Opcode::AUTDZA => {}
        // Opcode::AUTDZB => {}
        // Opcode::XPACI => {}
        // Opcode::XPACD => {}
        // Opcode::PACGA => {}
        // Opcode::GMI => {}
        // Opcode::IRG => {}
        // Opcode::SUBP => {}
        // Opcode::SUBPS => {}
        _ => {
            // format!("{:?}", ins.opcode)
            "TODO"
        }
    };

    for (i, o) in ins.operands.iter().enumerate() {
        match o {
            Operand::Nothing => break,
            _ => {}
        }

        if i > 0 {
            push_separator(args, ctx.config);
        }

        push_operand(args, o, ctx);
    }

    mnemonic
}

const REG_NAMES_X: [&'static str; 31] = [
    "x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9", "x10", "x11", "x12", "x13", "x14",
    "x15", "x16", "x17", "x18", "x19", "x20", "x21", "x22", "x23", "x24", "x25", "x26", "x27",
    "x28", "x29", "x30",
];

const REG_NAMES_W: [&'static str; 31] = [
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

#[inline]
fn push_register(args: &mut Vec<ObjInsArg>, size: SizeCode, reg: u16, sp: bool) {
    push_opaque(args, reg_name(size, reg, sp));
}

#[inline]
fn push_shift(args: &mut Vec<ObjInsArg>, style: ShiftStyle) {
    push_opaque(args, shift_style(style));
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

fn push_operand(args: &mut Vec<ObjInsArg>, o: &Operand, ctx: &mut DisplayCtx) {
    match o {
        Operand::Nothing => unreachable!(),
        Operand::PCOffset(off) => {
            if let Some(reloc) = ctx.reloc.as_ref() {
                let target_address = reloc.target.address.checked_add_signed(reloc.addend);
                if reloc.target.orig_section_index == Some(ctx.section_index)
                    && matches!(target_address, Some(addr) if addr > ctx.start_address && addr < ctx.end_address)
                {
                    let target_address = target_address.unwrap();
                    push_plain(args, "$");
                    args.push(ObjInsArg::BranchDest(target_address));
                    *ctx.branch_dest = Some(target_address);
                } else {
                    args.push(ObjInsArg::Reloc);
                }
            } else {
                let dest = ctx.address.saturating_add_signed(*off);
                push_plain(args, "$");
                args.push(ObjInsArg::BranchDest(dest));
                *ctx.branch_dest = Some(dest);
            }
        }
        Operand::Immediate(imm) => {
            if let Some(reloc) = ctx.reloc.as_ref() {
                match reloc.flags {
                    RelocationFlags::Elf { r_type: elf::R_AARCH64_ADD_ABS_LO12_NC } => {
                        args.push(ObjInsArg::Reloc);
                    }
                    _ => {
                        push_unsigned(args, *imm as u64);
                    }
                }
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
                push_plain(args, "lsl ");
                push_unsigned(args, *shift as u64);
            }
        }
        Operand::ImmShiftMSL(i, shift) => {
            push_unsigned(args, *i as u64);
            if *shift > 0 {
                push_separator(args, ctx.config);
                push_plain(args, "msl ");
                push_unsigned(args, *shift as u64);
            }
        }
        Operand::RegShift(shift_type, amount, size, reg) => match size {
            SizeCode::X => {
                if (*shift_type == ShiftStyle::LSL || *shift_type == ShiftStyle::UXTX)
                    && *amount == 0
                {
                    push_register(args, SizeCode::X, *reg, false);
                } else if *amount != 0 {
                    push_register(args, SizeCode::X, *reg, false);
                    push_separator(args, ctx.config);
                    push_shift(args, *shift_type);
                    push_separator(args, ctx.config);
                    push_unsigned(args, *amount as u64);
                } else {
                    push_register(args, SizeCode::X, *reg, false);
                    push_separator(args, ctx.config);
                    push_shift(args, *shift_type);
                }
            }
            SizeCode::W => {
                if *shift_type == ShiftStyle::LSL && *amount == 0 {
                    push_register(args, SizeCode::W, *reg, false);
                } else if *amount != 0 {
                    push_register(args, SizeCode::W, *reg, false);
                    push_separator(args, ctx.config);
                    push_shift(args, *shift_type);
                    push_separator(args, ctx.config);
                    push_unsigned(args, *amount as u64);
                } else {
                    push_register(args, SizeCode::W, *reg, false);
                    push_separator(args, ctx.config);
                    push_shift(args, *shift_type);
                }
            }
        },
        Operand::RegRegOffset(reg, index_reg, index_size, extend, amount) => {
            if extend == &ShiftStyle::LSL && *amount == 0 {
                push_plain(args, "[");
                push_register(args, SizeCode::X, *reg, true);
                push_separator(args, ctx.config);
                push_register(args, *index_size, *index_reg, false);
                push_plain(args, "]");
            } else if ((extend == &ShiftStyle::UXTW && index_size == &SizeCode::W)
                || (extend == &ShiftStyle::UXTX && index_size == &SizeCode::X))
                && *amount == 0
            {
                push_plain(args, "[");
                push_register(args, SizeCode::X, *reg, true);
                push_separator(args, ctx.config);
                push_register(args, *index_size, *index_reg, false);
                push_separator(args, ctx.config);
                push_shift(args, *extend);
                push_plain(args, "]");
            } else {
                push_plain(args, "[");
                push_register(args, SizeCode::X, *reg, true);
                push_separator(args, ctx.config);
                push_register(args, *index_size, *index_reg, false);
                push_separator(args, ctx.config);
                push_shift(args, *extend);
                push_separator(args, ctx.config);
                push_unsigned(args, *amount as u64);
                push_plain(args, "]");
            }
        }
        Operand::RegPreIndex(reg, offset, wback_bit) => {
            if *offset != 0 || *wback_bit {
                push_plain(args, "[");
                push_register(args, SizeCode::X, *reg, true);
                push_separator(args, ctx.config);
                push_signed(args, *offset as i64);
                push_plain(args, "]");
                if *wback_bit {
                    push_plain(args, "!");
                }
            } else {
                push_plain(args, "[");
                push_register(args, SizeCode::X, *reg, true);
                push_plain(args, "]");
            }
        }
        Operand::RegPostIndex(reg, offset) => {
            push_plain(args, "[");
            push_register(args, SizeCode::X, *reg, true);
            push_plain(args, "]");
            push_separator(args, ctx.config);
            push_signed(args, *offset as i64);
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

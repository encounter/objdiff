use alloc::{format, string::String, vec::Vec};

use anyhow::{Result, bail};
use object::elf;

use crate::{
    arch::Arch,
    diff::{DiffObjConfig, display::InstructionPart},
    obj::{
        InstructionRef, Relocation, RelocationFlags, ResolvedInstructionRef, ScannedInstruction,
    },
};

#[derive(Debug)]
pub struct ArchSuperH {}

impl ArchSuperH {
    pub fn new(_file: &object::File) -> Result<Self> { Ok(Self {}) }
}

static REG_NAMES: [&str; 16] = [
    "r0", "r1", "r2", "r3", "r4", "r5", "r6", "r7", "r8", "r9", "r10", "r11", "r12", "r13", "r14",
    "r15",
];

enum Ops {
    // match_ni_f
    AddImmRn,
    MovImmRn,
    // match_i_f
    AndBImmAtR0Gbr,
    OrBImmAtR0Gbr,
    TstBImmAtR0Gbr,
    XorBImmAtR0Gbr,
    AndImmR0,
    CmpEqImmR0,
    OrImmR0,
    TstImmR0,
    XorImmR0,
    Trapa,

    // match_nd8_f
    MovWAtDispPcRn,
    MovLAtDispPcRn,

    // match_d12_f
    Bra,
    Bsr,

    // match_d_f
    MovBR0AtDispGbr,
    MovWR0AtDispGbr,
    MovLR0AtDispGbr,
    MovBAtDispGbrR0,
    MovWAtDispGbrR0,
    MovLAtDispGbrR0,
    Bf,
    Bfs,
    Bt,
    Bts,

    // match_nmd_f
    MovLRmAtDispRn,
    MovLRDispRmRn,

    // match_ff00
    MovBAtDispRnR0,
    MovWAtDispRnR0,
    MovBR0AtDispRn,
    MovWR0AtDispRn,

    // match_f00f
    AddRmRn,
    AddcRmRn,
    AddvRmRn,
    AndRmRn,
    CmpEqRmRn,
    CmpHsRmRn,
    CmpGeRmRn,
    CmpHiRmRn,
    CmpGtRmRn,
    CmpStrRmRn,
    Div1RmRn,
    Div0sRmRn,
    DmulsLRmRn,
    DmuluLRmRn,
    ExtsBRmRn,
    ExtsWRmRn,
    ExtuBRmRn,
    ExtuWRmRn,
    MovRmRn,
    MulLRmRn,
    MulsRmRn,
    MuluRmRn,
    NegRmRn,
    NegcRmRn,
    NotRmRn,
    OrRmRn,
    SubRmRn,
    SubcRmRn,
    SubvRmRn,
    SwapBRmRn,
    SwapWRmRn,
    TstRmRn,
    XorRmRn,
    XtrctRmRn,
    MovBRmAtRn,
    MovWRmAtRn,
    MovLRmAtRn,
    MovBAtRmRn,
    MovWAtRmRn,
    MovLAtRmRn,
    MacLAtRmIncAtRnInc,
    MacWAtRmIncAtRnInc,
    MovBAtRmIncRn,
    MovWAtRmIncRn,
    MovLAtRmIncRn,
    MovBRmAtDecRn,
    MovWRmAtDecRn,
    MovLRmAtDecRn,
    MovBRmAtR0Rn,
    MovWRmAtR0Rn,
    MovLRmAtR0Rn,
    MovBAtR0RmRn,
    MovWAtR0RmRn,
    MovLAtR0RmRn,

    // match_f0ff
    CmpPlRn,
    CmpPzRn,
    DtRn,
    MovtRn,
    RotlRn,
    RotrRn,
    RotclRn,
    RotcrRn,
    ShalRn,
    SharRn,
    ShllRn,
    ShlrRn,
    Shll2Rn,
    Shlr2Rn,
    Shll8Rn,
    Shlr8Rn,
    Shll16Rn,
    Shlr16Rn,
    StcSrRn,
    StcGbrRn,
    StcVbrRn,
    StsMachRn,
    StsMaclRn,
    StsPrRn,
    TasB,
    StcLSrAtDecrementRn,
    StcLGbrAtDecrementRn,
    StcLVbrAtDecrementRn,
    StsLMachAtDecrementRn,
    StsLMaclAtDecrementRn,
    StsLPrAtDecrementRn,
    LdcRnSr,
    LdcRnGbr,
    LdcRnVbr,
    LdsRnMach,
    LdsRnMacl,
    LdsRnPr,
    JmpAtRn,
    JsrAtRn,
    LdcLAtRnIncSr,
    LdcLAtRnIncGbr,
    LdcLAtRnIncVbr,
    LdsLAtRnIncMach,
    LdsLAtRnIncMacl,
    LdsLAtRnIncPr,
    BrafRn,
    BsrfRn,

    //sh2_disasm
    Clrt,
    Clrmac,
    Div0u,
    Nop,
    Rte,
    Rts,
    Sett,
    Sleep,
}
fn match_ni_f(
    _v_addr: u32,
    op: u16,
    _mode: bool,
    parts: &mut Vec<InstructionPart>,
    _resolved: &ResolvedInstructionRef,
    _branch_dest: &mut Option<u64>,
) {
    match op & 0xf000 {
        0x7000 => {
            // ADD #imm,Rn
            let reg = REG_NAMES[((op >> 8) & 0xf) as usize];
            parts.push(InstructionPart::opcode("add", Ops::AddImmRn as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic(reg));
        }
        0xe000 => {
            // MOV #imm,Rn
            let reg = REG_NAMES[((op >> 8) & 0xf) as usize];
            parts.push(InstructionPart::opcode("mov", Ops::MovImmRn as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg));
        }
        _ => {
            parts.push(InstructionPart::basic(".word 0x"));
            parts.push(InstructionPart::basic(format!("{:04X}", op)));
            parts.push(InstructionPart::basic(" /* unknown instruction */"));
        }
    }
}

fn match_i_f(
    v_addr: u32,
    op: u16,
    mode: bool,
    parts: &mut Vec<InstructionPart>,
    resolved: &ResolvedInstructionRef,
    branch_dest: &mut Option<u64>,
) {
    match op & 0xff00 {
        0xcd00 => {
            // AND.B #imm,@(R0,GBR)
            parts.push(InstructionPart::opcode("and.b", Ops::AndBImmAtR0Gbr as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
        }
        0xcf00 => {
            // OR.B #imm,@(R0,GBR)
            parts.push(InstructionPart::opcode("or.b", Ops::OrBImmAtR0Gbr as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
        }
        0xcc00 => {
            // TST.B #imm,@(R0,GBR)
            parts.push(InstructionPart::opcode("tst.b", Ops::TstBImmAtR0Gbr as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
        }
        0xce00 => {
            // XOR.B #imm,@(R0,GBR)
            parts.push(InstructionPart::opcode("xor.b", Ops::XorBImmAtR0Gbr as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
        }
        0xc900 => {
            // AND #imm, R0
            parts.push(InstructionPart::opcode("and", Ops::AndImmR0 as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0x8800 => {
            parts.push(InstructionPart::opcode("cmp/eq", Ops::CmpEqImmR0 as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0xcb00 => {
            parts.push(InstructionPart::opcode("or", Ops::OrImmR0 as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0xc800 => {
            parts.push(InstructionPart::opcode("tst", Ops::TstImmR0 as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0xca00 => {
            parts.push(InstructionPart::opcode("xor", Ops::XorImmR0 as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0xc300 => {
            parts.push(InstructionPart::opcode("trapa", Ops::Trapa as u16));
            parts.push(InstructionPart::basic("#"));
            parts.push(InstructionPart::unsigned(op & 0xff));
        }
        _ => match_ni_f(v_addr, op, mode, parts, resolved, branch_dest),
    }
}

fn match_nd8_f(
    v_addr: u32,
    op: u16,
    mode: bool,
    parts: &mut Vec<InstructionPart>,
    resolved: &ResolvedInstructionRef,
    branch_dest: &mut Option<u64>,
) {
    match op & 0xf000 {
        0x9000 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWAtDispPcRn as u16));
            parts.push(InstructionPart::basic("@("));
            if resolved.relocation.is_some() {
                parts.push(InstructionPart::reloc());
            } else {
                parts.push(InstructionPart::unsigned((op & 0xff) * 2 + 4));
            }
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("pc"));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(format!("r{}", (op >> 8) & 0xf)));
        }
        0xd000 => {
            let mut target_a = (op as u32 & 0xff) * 4 + 4;
            let test = (op as u32 & 0xff) * 4 + 4 + v_addr;

            if (test & 3) == 2 {
                target_a -= 2;
            }
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLAtDispPcRn as u16));
            parts.push(InstructionPart::basic("@("));
            if resolved.relocation.is_some() {
                parts.push(InstructionPart::reloc());
            } else {
                parts.push(InstructionPart::unsigned(target_a));
            }
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("pc"));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(format!("r{}", (op >> 8) & 0xf)));
        }
        _ => match_i_f(v_addr, op, mode, parts, resolved, branch_dest),
    }
}

fn match_d12_f(
    v_addr: u32,
    op: u16,
    mode: bool,
    parts: &mut Vec<InstructionPart>,
    resolved: &ResolvedInstructionRef,
    branch_dest: &mut Option<u64>,
) {
    match op & 0xf000 {
        0xa000 => {
            if (op & 0x800) == 0x800 {
                let addr = ((op as u32 & 0xfff) + 0xfffff000)
                    .wrapping_mul(2)
                    .wrapping_add(v_addr)
                    .wrapping_add(4);
                *branch_dest = Some(addr as u64);
                if resolved.relocation.is_some() {
                    // Use the label
                    parts.push(InstructionPart::opcode("bra", Ops::Bra as u16));
                    parts.push(InstructionPart::reloc());
                } else {
                    // use an address
                    parts.push(InstructionPart::opcode("bra", Ops::Bra as u16));
                    parts.push(InstructionPart::unsigned(addr));
                }
            } else {
                let addr = (op as u32 & 0xfff) * 2 + v_addr + 4;
                *branch_dest = Some(addr as u64);

                if resolved.relocation.is_some() {
                    // Use the label
                    parts.push(InstructionPart::opcode("bra", Ops::Bra as u16));
                    parts.push(InstructionPart::reloc());
                } else {
                    // use an address
                    parts.push(InstructionPart::opcode("bra", Ops::Bra as u16));
                    parts.push(InstructionPart::unsigned(addr));
                }
            }
        }
        0xb000 => {
            if (op & 0x800) == 0x800 {
                let addr =
                    ((op as u32 & 0xfff) + 0xfffff000).wrapping_mul(2).wrapping_add(v_addr) + 4;
                *branch_dest = Some(addr as u64);
                if resolved.relocation.is_some() {
                    // Use the label
                    parts.push(InstructionPart::opcode("bsr", Ops::Bsr as u16));
                    parts.push(InstructionPart::reloc());
                } else {
                    // use an address
                    parts.push(InstructionPart::opcode("bsr", Ops::Bsr as u16));
                    parts.push(InstructionPart::unsigned(addr));
                }
            } else {
                let addr = (op as u32 & 0xfff) * 2 + v_addr + 4;
                *branch_dest = Some(addr as u64);
                if resolved.relocation.is_some() {
                    // Use the label
                    parts.push(InstructionPart::opcode("bsr", Ops::Bsr as u16));
                    parts.push(InstructionPart::reloc());
                } else {
                    // use an address
                    parts.push(InstructionPart::opcode("bsr", Ops::Bsr as u16));
                    parts.push(InstructionPart::unsigned(addr));
                }
            }
        }
        _ => match_nd8_f(v_addr, op, mode, parts, resolved, branch_dest),
    }
}

fn match_d_f(
    v_addr: u32,
    op: u16,
    mode: bool,
    parts: &mut Vec<InstructionPart>,
    resolved: &ResolvedInstructionRef,
    branch_dest: &mut Option<u64>,
) {
    match op & 0xff00 {
        0xc000 => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBR0AtDispGbr as u16));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
        }
        0xc100 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWR0AtDispGbr as u16));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::unsigned((op & 0xff) * 2));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
        }
        0xc200 => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLR0AtDispGbr as u16));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::unsigned((op & 0xff) * 4));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
        }
        0xc400 => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBAtDispGbrR0 as u16));
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::unsigned(op & 0xff));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0xc500 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWAtDispGbrR0 as u16));
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::unsigned((op & 0xff) * 2));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0xc600 => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLAtDispGbrR0 as u16));
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::unsigned((op & 0xff) * 4));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0x8b00 => {
            let addr: u32 = if (op & 0x80) == 0x80 {
                (((op as u32 & 0xff).wrapping_add(0xffffff00)).wrapping_mul(2))
                    .wrapping_add(v_addr)
                    .wrapping_add(4)
            } else {
                ((op as u32 & 0xff) * 2).wrapping_add(v_addr).wrapping_add(4)
            };
            *branch_dest = Some(addr as u64);
            parts.push(InstructionPart::opcode("bf", Ops::Bf as u16));
            if resolved.relocation.is_some() {
                parts.push(InstructionPart::reloc());
            } else {
                parts.push(InstructionPart::unsigned(addr));
            }
        }
        0x8f00 => {
            let addr = if (op & 0x80) == 0x80 {
                (((op as u32 & 0xff).wrapping_add(0xffffff00)).wrapping_mul(2))
                    .wrapping_add(v_addr)
                    .wrapping_add(4)
            } else {
                ((op as u32 & 0xff) * 2).wrapping_add(v_addr).wrapping_add(4)
            };
            *branch_dest = Some(addr as u64);
            parts.push(InstructionPart::opcode("bf.s", Ops::Bfs as u16));
            if resolved.relocation.is_some() {
                parts.push(InstructionPart::reloc());
            } else {
                parts.push(InstructionPart::unsigned(addr));
            }
        }
        0x8900 => {
            let addr = if (op & 0x80) == 0x80 {
                (((op as u32 & 0xff).wrapping_add(0xffffff00)).wrapping_mul(2))
                    .wrapping_add(v_addr)
                    .wrapping_add(4)
            } else {
                ((op as u32 & 0xff) * 2).wrapping_add(v_addr).wrapping_add(4)
            };
            *branch_dest = Some(addr as u64);
            parts.push(InstructionPart::opcode("bt", Ops::Bt as u16));
            if resolved.relocation.is_some() {
                parts.push(InstructionPart::reloc());
            } else {
                parts.push(InstructionPart::unsigned(addr));
            }
        }
        0x8d00 => {
            let addr = if (op & 0x80) == 0x80 {
                (((op as u32 & 0xff).wrapping_add(0xffffff00)).wrapping_mul(2))
                    .wrapping_add(v_addr)
                    .wrapping_add(4)
            } else {
                ((op as u32 & 0xff) * 2).wrapping_add(v_addr).wrapping_add(4)
            };
            *branch_dest = Some(addr as u64);
            parts.push(InstructionPart::opcode("bt.s", Ops::Bts as u16));
            if resolved.relocation.is_some() {
                parts.push(InstructionPart::reloc());
            } else {
                parts.push(InstructionPart::unsigned(addr));
            }
        }
        _ => match_d12_f(v_addr, op, mode, parts, resolved, branch_dest),
    }
}

fn match_nmd_f(
    v_addr: u32,
    op: u16,
    mode: bool,
    parts: &mut Vec<InstructionPart>,
    resolved: &ResolvedInstructionRef,
    branch_dest: &mut Option<u64>,
) {
    let reg_m = REG_NAMES[((op >> 4) & 0xf) as usize];
    let reg_n = REG_NAMES[((op >> 8) & 0xf) as usize];
    match op & 0xf000 {
        0x1000 => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLRmAtDispRn as u16));
            parts.push(InstructionPart::basic(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::unsigned((op & 0xf) * 4));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic(reg_n));
            parts.push(InstructionPart::basic(")"));
        }
        0x5000 => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLRDispRmRn as u16));
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::unsigned((op & 0xf) * 4));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic(reg_m));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic(reg_n));
        }
        _ => match_d_f(v_addr, op, mode, parts, resolved, branch_dest),
    }
}
fn match_ff00(
    v_addr: u32,
    op: u16,
    mode: bool,
    parts: &mut Vec<InstructionPart>,
    resolved: &ResolvedInstructionRef,
    branch_dest: &mut Option<u64>,
) {
    match op & 0xff00 {
        0x8400 => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBAtDispRnR0 as u16));
            parts.push(InstructionPart::basic("@("));
            if (op & 0x100) == 0x100 {
                parts.push(InstructionPart::unsigned((op & 0xf) * 2));
            } else {
                parts.push(InstructionPart::unsigned(op & 0xf));
            }
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(REG_NAMES[((op >> 4) & 0xf) as usize]));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0x8500 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWAtDispRnR0 as u16));
            parts.push(InstructionPart::basic("@("));
            if (op & 0x100) == 0x100 {
                parts.push(InstructionPart::unsigned((op & 0xf) * 2));
            } else {
                parts.push(InstructionPart::unsigned(op & 0xf));
            }
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(REG_NAMES[((op >> 4) & 0xf) as usize]));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("r0"));
        }
        0x8000 => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBR0AtDispRn as u16));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            if (op & 0x100) == 0x100 {
                parts.push(InstructionPart::unsigned((op & 0xf) * 2));
            } else {
                parts.push(InstructionPart::unsigned(op & 0xf));
            }
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(REG_NAMES[((op >> 4) & 0xf) as usize]));
            parts.push(InstructionPart::basic(")"));
        }
        0x8100 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWR0AtDispRn as u16));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            if (op & 0x100) == 0x100 {
                parts.push(InstructionPart::unsigned((op & 0xf) * 2));
            } else {
                parts.push(InstructionPart::unsigned(op & 0xf));
            }
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(REG_NAMES[((op >> 4) & 0xf) as usize]));
            parts.push(InstructionPart::basic(")"));
        }
        _ => match_nmd_f(v_addr, op, mode, parts, resolved, branch_dest),
    }
}

fn match_f00f(
    v_addr: u32,
    op: u16,
    mode: bool,
    parts: &mut Vec<InstructionPart>,
    resolved: &ResolvedInstructionRef,
    branch_dest: &mut Option<u64>,
) {
    let reg_m = REG_NAMES[((op >> 4) & 0xf) as usize];
    let reg_n = REG_NAMES[((op >> 8) & 0xf) as usize];

    match op & 0xf00f {
        0x300c => {
            parts.push(InstructionPart::opcode("add", Ops::AddRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x300e => {
            parts.push(InstructionPart::opcode("addc", Ops::AddcRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x300f => {
            parts.push(InstructionPart::opcode("addv", Ops::AddvRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x2009 => {
            parts.push(InstructionPart::opcode("and", Ops::AndRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x3000 => {
            parts.push(InstructionPart::opcode("cmp/eq", Ops::CmpEqRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x3002 => {
            parts.push(InstructionPart::opcode("cmp/hs", Ops::CmpHsRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x3003 => {
            parts.push(InstructionPart::opcode("cmp/ge", Ops::CmpGeRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x3006 => {
            parts.push(InstructionPart::opcode("cmp/hi", Ops::CmpHiRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x3007 => {
            parts.push(InstructionPart::opcode("cmp/gt", Ops::CmpGtRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x200c => {
            parts.push(InstructionPart::opcode("cmp/str", Ops::CmpStrRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x3004 => {
            parts.push(InstructionPart::opcode("div1", Ops::Div1RmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x2007 => {
            parts.push(InstructionPart::opcode("div0s", Ops::Div0sRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x300d => {
            parts.push(InstructionPart::opcode("dmuls.l", Ops::DmulsLRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x3005 => {
            parts.push(InstructionPart::opcode("dmulu.l", Ops::DmuluLRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x600e => {
            parts.push(InstructionPart::opcode("exts.b", Ops::ExtsBRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x600f => {
            parts.push(InstructionPart::opcode("exts.w", Ops::ExtsWRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x600c => {
            parts.push(InstructionPart::opcode("extu.b", Ops::ExtuBRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x600d => {
            parts.push(InstructionPart::opcode("extu.w", Ops::ExtuWRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x6003 => {
            parts.push(InstructionPart::opcode("mov", Ops::MovRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x0007 => {
            parts.push(InstructionPart::opcode("mul.l", Ops::MulLRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x200f => {
            parts.push(InstructionPart::opcode("muls", Ops::MulsRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x200e => {
            parts.push(InstructionPart::opcode("mulu", Ops::MuluRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x600b => {
            parts.push(InstructionPart::opcode("neg", Ops::NegRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x600a => {
            parts.push(InstructionPart::opcode("negc", Ops::NegcRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x6007 => {
            parts.push(InstructionPart::opcode("not", Ops::NotRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x200b => {
            parts.push(InstructionPart::opcode("or", Ops::OrRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x3008 => {
            parts.push(InstructionPart::opcode("sub", Ops::SubRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x300a => {
            parts.push(InstructionPart::opcode("subc", Ops::SubcRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x300b => {
            parts.push(InstructionPart::opcode("subv", Ops::SubvRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x6008 => {
            parts.push(InstructionPart::opcode("swap.b", Ops::SwapBRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x6009 => {
            parts.push(InstructionPart::opcode("swap.w", Ops::SwapWRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x2008 => {
            parts.push(InstructionPart::opcode("tst", Ops::TstRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x200a => {
            parts.push(InstructionPart::opcode("xor", Ops::XorRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x200d => {
            parts.push(InstructionPart::opcode("xtrct", Ops::XtrctRmRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x2000 => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBRmAtRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x2001 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWRmAtRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x2002 => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLRmAtRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x6000 => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBAtRmRn as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x6001 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWAtRmRn as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x6002 => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLAtRmRn as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x000f => {
            parts.push(InstructionPart::opcode("mac.l", Ops::MacLAtRmIncAtRnInc as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_n));
            parts.push(InstructionPart::basic("+"));
        }
        0x400f => {
            parts.push(InstructionPart::opcode("mac.w", Ops::MacWAtRmIncAtRnInc as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_n));
            parts.push(InstructionPart::basic("+"));
        }
        0x6004 => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBAtRmIncRn as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x6005 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWAtRmIncRn as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x6006 => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLAtRmIncRn as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x2004 => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBRmAtDecRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@-"));
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x2005 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWRmAtDecRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@-"));
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x2006 => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLRmAtDecRn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@-"));
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x0004 => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBRmAtR0Rn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
            parts.push(InstructionPart::basic(")"));
        }
        0x0005 => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWRmAtR0Rn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
            parts.push(InstructionPart::basic(")"));
        }
        0x0006 => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLRmAtR0Rn as u16));
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
            parts.push(InstructionPart::basic(")"));
        }
        0x000c => {
            parts.push(InstructionPart::opcode("mov.b", Ops::MovBAtR0RmRn as u16));
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x000d => {
            parts.push(InstructionPart::opcode("mov.w", Ops::MovWAtR0RmRn as u16));
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        0x000e => {
            parts.push(InstructionPart::opcode("mov.l", Ops::MovLAtR0RmRn as u16));
            parts.push(InstructionPart::basic("@("));
            parts.push(InstructionPart::opaque("r0"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_m));
            parts.push(InstructionPart::basic(")"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg_n));
        }
        _ => match_ff00(v_addr, op, mode, parts, resolved, branch_dest),
    }
}

fn match_f0ff(
    v_addr: u32,
    op: u16,
    mode: bool,
    parts: &mut Vec<InstructionPart>,
    resolved: &ResolvedInstructionRef,
    branch_dest: &mut Option<u64>,
) {
    let reg = REG_NAMES[((op >> 8) & 0xf) as usize];
    match op & 0xf0ff {
        0x4015 => {
            // CMP/PL Rn
            parts.push(InstructionPart::opcode("cmp/pl", Ops::CmpPlRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4011 => {
            // CMP/PZ Rn
            parts.push(InstructionPart::opcode("cmp/pz", Ops::CmpPzRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4010 => {
            // DT Rn
            parts.push(InstructionPart::opcode("dt", Ops::DtRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x0029 => {
            // MOVT Rn
            parts.push(InstructionPart::opcode("movt", Ops::MovtRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4004 => {
            // ROTL Rn
            parts.push(InstructionPart::opcode("rotl", Ops::RotlRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4005 => {
            // ROTR Rn
            parts.push(InstructionPart::opcode("rotr", Ops::RotrRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4024 => {
            // ROTCL Rn
            parts.push(InstructionPart::opcode("rotcl", Ops::RotclRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4025 => {
            // ROTCR Rn
            parts.push(InstructionPart::opcode("rotcr", Ops::RotcrRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4020 => {
            // SHAL Rn
            parts.push(InstructionPart::opcode("shal", Ops::ShalRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4021 => {
            // SHAR Rn
            parts.push(InstructionPart::opcode("shar", Ops::SharRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4000 => {
            // SHLL Rn
            parts.push(InstructionPart::opcode("shll", Ops::ShllRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4001 => {
            // SHLR Rn
            parts.push(InstructionPart::opcode("shlr", Ops::ShlrRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4008 => {
            // SHLL2 Rn
            parts.push(InstructionPart::opcode("shll2", Ops::Shll2Rn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4009 => {
            // SHLR2 Rn
            parts.push(InstructionPart::opcode("shlr2", Ops::Shlr2Rn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4018 => {
            // SHLL8 Rn
            parts.push(InstructionPart::opcode("shll8", Ops::Shll8Rn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4019 => {
            // SHLR8 Rn
            parts.push(InstructionPart::opcode("shlr8", Ops::Shlr8Rn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4028 => {
            // SHLL16 Rn
            parts.push(InstructionPart::opcode("shll16", Ops::Shll16Rn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4029 => {
            // SHLR16 Rn
            parts.push(InstructionPart::opcode("shlr16", Ops::Shlr16Rn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x0002 => {
            // STC SR,Rn
            parts.push(InstructionPart::opcode("stc", Ops::StcSrRn as u16));
            parts.push(InstructionPart::opaque("sr"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg));
        }
        0x0012 => {
            // STC GBR,Rn
            parts.push(InstructionPart::opcode("stc", Ops::StcGbrRn as u16));
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg));
        }
        0x0022 => {
            // STC VBR,Rn
            parts.push(InstructionPart::opcode("stc", Ops::StcVbrRn as u16));
            parts.push(InstructionPart::opaque("vbr"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg));
        }
        0x000a => {
            // STS MACH,Rn
            parts.push(InstructionPart::opcode("sts", Ops::StsMachRn as u16));
            parts.push(InstructionPart::opaque("mach"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg));
        }
        0x001a => {
            // STS MACL,Rn
            parts.push(InstructionPart::opcode("sts", Ops::StsMaclRn as u16));
            parts.push(InstructionPart::opaque("macl"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg));
        }
        0x002a => {
            // STS PR,Rn
            parts.push(InstructionPart::opcode("sts", Ops::StsPrRn as u16));
            parts.push(InstructionPart::opaque("pr"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque(reg));
        }
        0x401b => {
            // TAS.B
            parts.push(InstructionPart::opcode("tas.b", Ops::TasB as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4003 => {
            parts.push(InstructionPart::opcode("stc.l", Ops::StcLSrAtDecrementRn as u16));
            parts.push(InstructionPart::opaque("sr"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@-"));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4013 => {
            parts.push(InstructionPart::opcode("stc.l", Ops::StcLGbrAtDecrementRn as u16));
            parts.push(InstructionPart::opaque("gbr"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@-"));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4023 => {
            parts.push(InstructionPart::opcode("stc.l", Ops::StcLVbrAtDecrementRn as u16));
            parts.push(InstructionPart::opaque("vbr"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@-"));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4002 => {
            parts.push(InstructionPart::opcode("sts.l", Ops::StsLMachAtDecrementRn as u16));
            parts.push(InstructionPart::opaque("mach"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@-"));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4012 => {
            parts.push(InstructionPart::opcode("sts.l", Ops::StsLMaclAtDecrementRn as u16));
            parts.push(InstructionPart::opaque("macl"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@-"));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4022 => {
            parts.push(InstructionPart::opcode("sts.l", Ops::StsLPrAtDecrementRn as u16));
            parts.push(InstructionPart::opaque("pr"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::basic("@-"));
            parts.push(InstructionPart::opaque(reg));
        }
        0x400e => {
            parts.push(InstructionPart::opcode("ldc", Ops::LdcRnSr as u16));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("sr"));
        }
        0x401e => {
            parts.push(InstructionPart::opcode("ldc", Ops::LdcRnGbr as u16));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
        }
        0x402e => {
            parts.push(InstructionPart::opcode("ldc", Ops::LdcRnVbr as u16));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("vbr"));
        }
        0x400a => {
            parts.push(InstructionPart::opcode("lds", Ops::LdsRnMach as u16));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("mach"));
        }
        0x401a => {
            parts.push(InstructionPart::opcode("lds", Ops::LdsRnMacl as u16));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("macl"));
        }
        0x402a => {
            parts.push(InstructionPart::opcode("lds", Ops::LdsRnPr as u16));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("pr"));
        }
        0x402b => {
            parts.push(InstructionPart::opcode("jmp", Ops::JmpAtRn as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg));
        }
        0x400b => {
            parts.push(InstructionPart::opcode("jsr", Ops::JsrAtRn as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg));
        }
        0x4007 => {
            parts.push(InstructionPart::opcode("ldc.l", Ops::LdcLAtRnIncSr as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("sr"));
        }
        0x4017 => {
            parts.push(InstructionPart::opcode("ldc.l", Ops::LdcLAtRnIncGbr as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("gbr"));
        }
        0x4027 => {
            parts.push(InstructionPart::opcode("ldc.l", Ops::LdcLAtRnIncVbr as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("vbr"));
        }
        0x4006 => {
            parts.push(InstructionPart::opcode("lds.l", Ops::LdsLAtRnIncMach as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("mach"));
        }
        0x4016 => {
            parts.push(InstructionPart::opcode("lds.l", Ops::LdsLAtRnIncMacl as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("macl"));
        }
        0x4026 => {
            parts.push(InstructionPart::opcode("lds.l", Ops::LdsLAtRnIncPr as u16));
            parts.push(InstructionPart::basic("@"));
            parts.push(InstructionPart::opaque(reg));
            parts.push(InstructionPart::basic("+"));
            parts.push(InstructionPart::separator());
            parts.push(InstructionPart::opaque("pr"));
        }
        0x0023 => {
            parts.push(InstructionPart::opcode("braf", Ops::BrafRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        0x0003 => {
            parts.push(InstructionPart::opcode("bsrf", Ops::BsrfRn as u16));
            parts.push(InstructionPart::opaque(reg));
        }
        _ => {
            match_f00f(v_addr, op, mode, parts, resolved, branch_dest);
        }
    }
}

fn sh2_disasm(
    v_addr: u32,
    op: u16,
    mode: bool,
    parts: &mut Vec<InstructionPart>,
    resolved: &ResolvedInstructionRef,
    branch_dest: &mut Option<u64>,
) {
    match op & 0xffff {
        0x0008 => parts.push(InstructionPart::opcode("clrt", Ops::Clrt as u16)),
        0x0028 => parts.push(InstructionPart::opcode("clrmac", Ops::Clrmac as u16)),
        0x0019 => parts.push(InstructionPart::opcode("div0u", Ops::Div0u as u16)),
        0x0009 => parts.push(InstructionPart::opcode("nop", Ops::Nop as u16)),
        0x002b => parts.push(InstructionPart::opcode("rte", Ops::Rte as u16)),
        0x000b => parts.push(InstructionPart::opcode("rts", Ops::Rts as u16)),
        0x0018 => parts.push(InstructionPart::opcode("sett", Ops::Sett as u16)),
        0x001b => parts.push(InstructionPart::opcode("sleep", Ops::Sleep as u16)),
        _ => {
            match_f0ff(v_addr, op, mode, parts, resolved, branch_dest);
        }
    }
}

impl Arch for ArchSuperH {
    fn scan_instructions(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        _relocations: &[Relocation],
        _diff_config: &DiffObjConfig,
    ) -> Result<Vec<ScannedInstruction>> {
        let mut ops = Vec::<ScannedInstruction>::with_capacity(code.len() / 4);
        let mut offset = address;
        for chunk in code.chunks_exact(2) {
            let opcode = u16::from_be_bytes(chunk.try_into().unwrap());
            let mut parts: Vec<InstructionPart> = vec![];
            let resolved: ResolvedInstructionRef = Default::default();
            let mut branch_dest: Option<u64> = None;
            sh2_disasm(
                offset.try_into().unwrap(),
                opcode.into(),
                true,
                &mut parts,
                &resolved,
                &mut branch_dest,
            );
            let ins_ref = InstructionRef { address: offset.try_into().unwrap(), size: 2, opcode };
            ops.push(ScannedInstruction { ins_ref, branch_dest });
            offset += 2;
        }

        Ok(ops)
    }

    fn display_instruction(
        &self,
        resolved: ResolvedInstructionRef,
        _diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        let opcode = u16::from_be_bytes(resolved.code.try_into().unwrap());
        let mut parts: Vec<InstructionPart> = vec![];
        let mut branch_dest: Option<u64> = None;
        sh2_disasm(0, opcode.into(), true, &mut parts, &resolved, &mut branch_dest);

        for part in parts {
            cb(part)?;
        }

        Ok(())
    }

    fn implcit_addend(
        &self,
        _file: &object::File<'_>,
        _section: &object::Section,
        address: u64,
        _relocation: &object::Relocation,
        flags: RelocationFlags,
    ) -> Result<i64> {
        bail!("Unsupported SuperH implicit relocation {:#x}:{:?}", address, flags)
    }

    fn demangle(&self, name: &str) -> Option<String> {
        cpp_demangle::Symbol::new(name)
            .ok()
            .and_then(|s| s.demangle(&cpp_demangle::DemangleOptions::default()).ok())
    }

    fn reloc_name(&self, flags: RelocationFlags) -> Option<&'static str> {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                elf::R_SH_NONE => Some("R_SH_NONE"),
                elf::R_SH_DIR32 => Some("R_SH_DIR32"),
                elf::R_SH_REL32 => Some("R_SH_REL32"),
                elf::R_SH_DIR8WPN => Some("R_SH_DIR8WPN"),
                elf::R_SH_IND12W => Some("R_SH_IND12W"),
                elf::R_SH_DIR8WPL => Some("R_SH_DIR8WPL"),
                elf::R_SH_DIR8WPZ => Some("R_SH_DIR8WPZ"),
                elf::R_SH_DIR8BP => Some("R_SH_DIR8BP"),
                elf::R_SH_DIR8W => Some("R_SH_DIR8W"),
                elf::R_SH_DIR8L => Some("R_SH_DIR8L"),
                elf::R_SH_SWITCH16 => Some("R_SH_SWITCH16"),
                elf::R_SH_SWITCH32 => Some("R_SH_SWITCH32"),
                elf::R_SH_USES => Some("R_SH_USES"),
                elf::R_SH_COUNT => Some("R_SH_COUNT"),
                elf::R_SH_ALIGN => Some("R_SH_ALIGN"),
                elf::R_SH_CODE => Some("R_SH_CODE"),
                elf::R_SH_DATA => Some("R_SH_DATA"),
                elf::R_SH_LABEL => Some("R_SH_LABEL"),
                elf::R_SH_SWITCH8 => Some("R_SH_SWITCH8"),
                elf::R_SH_GNU_VTINHERIT => Some("R_SH_GNU_VTINHERIT"),
                elf::R_SH_GNU_VTENTRY => Some("R_SH_GNU_VTENTRY"),
                elf::R_SH_TLS_GD_32 => Some("R_SH_TLS_GD_32"),
                elf::R_SH_TLS_LD_32 => Some("R_SH_TLS_LD_32"),
                elf::R_SH_TLS_LDO_32 => Some("R_SH_TLS_LDO_32"),
                elf::R_SH_TLS_IE_32 => Some("R_SH_TLS_IE_32"),
                elf::R_SH_TLS_LE_32 => Some("R_SH_TLS_LE_32"),
                elf::R_SH_TLS_DTPMOD32 => Some("R_SH_TLS_DTPMOD32"),
                elf::R_SH_TLS_DTPOFF32 => Some("R_SH_TLS_DTPOFF32"),
                elf::R_SH_TLS_TPOFF32 => Some("R_SH_TLS_TPOFF32"),
                elf::R_SH_GOT32 => Some("R_SH_GOT32"),
                elf::R_SH_PLT32 => Some("R_SH_PLT32"),
                elf::R_SH_COPY => Some("R_SH_COPY"),
                elf::R_SH_GLOB_DAT => Some("R_SH_GLOB_DAT"),
                elf::R_SH_JMP_SLOT => Some("R_SH_JMP_SLOT"),
                elf::R_SH_RELATIVE => Some("R_SH_RELATIVE"),
                elf::R_SH_GOTOFF => Some("R_SH_GOTOFF"),
                elf::R_SH_GOTPC => Some("R_SH_GOTPC"),
                _ => None,
            },
            _ => None,
        }
    }

    fn data_reloc_size(&self, flags: RelocationFlags) -> usize {
        match flags {
            RelocationFlags::Elf(r_type) => match r_type {
                // ???
                _ => 1,
            },
            _ => 1,
        }
    }
}

#[cfg(test)]
mod test {
    use std::fmt::{self, Display};

    use super::*;
    use crate::obj::InstructionArg;

    impl<'a> Display for InstructionPart<'a> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                InstructionPart::Basic(s) => write!(f, "{}", s),
                InstructionPart::Opcode(s, _o) => write!(f, "{} ", s),
                InstructionPart::Arg(arg) => write!(f, "{}", arg),
                InstructionPart::Separator => write!(f, ", "),
            }
        }
    }

    impl<'a> Display for InstructionArg<'a> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                InstructionArg::Value(v) => write!(f, "{}", v),
                InstructionArg::BranchDest(v) => write!(f, "{}", v),
                InstructionArg::Reloc => write!(f, "reloc"),
            }
        }
    }

    #[test]
    fn test_sh2_display_instruction_basic_ops() {
        let arch = ArchSuperH {};
        let ops: [(u16, &str); 8] = [
            (0x0008, "clrt "),
            (0x0028, "clrmac "),
            (0x0019, "div0u "),
            (0x0009, "nop "),
            (0x002b, "rte "),
            (0x000b, "rts "),
            (0x0018, "sett "),
            (0x001b, "sleep "),
        ];

        for (opcode, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: 0x1000, size: 2, opcode: opcode as u16 },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_display_instruction_f0ff_ops() {
        let arch = ArchSuperH {};
        let ops: [(u16, &str); 49] = [
            (0x4015, "cmp/pl r0"),
            (0x4115, "cmp/pl r1"),
            (0x4215, "cmp/pl r2"),
            (0x4315, "cmp/pl r3"),
            (0x4011, "cmp/pz r0"),
            (0x4010, "dt r0"),
            (0x0029, "movt r0"),
            (0x4004, "rotl r0"),
            (0x4005, "rotr r0"),
            (0x4024, "rotcl r0"),
            (0x4025, "rotcr r0"),
            (0x4020, "shal r0"),
            (0x4021, "shar r0"),
            (0x4000, "shll r0"),
            (0x4001, "shlr r0"),
            (0x4008, "shll2 r0"),
            (0x4009, "shlr2 r0"),
            (0x4018, "shll8 r0"),
            (0x4019, "shlr8 r0"),
            (0x4028, "shll16 r0"),
            (0x4029, "shlr16 r0"),
            (0x0002, "stc sr, r0"),
            (0x0012, "stc gbr, r0"),
            (0x0022, "stc vbr, r0"),
            (0x000a, "sts mach, r0"),
            (0x001a, "sts macl, r0"),
            (0x402a, "lds r0, pr"),
            (0x401b, "tas.b r0"),
            (0x4003, "stc.l sr, @-r0"),
            (0x4013, "stc.l gbr, @-r0"),
            (0x4023, "stc.l vbr, @-r0"),
            (0x4002, "sts.l mach, @-r0"),
            (0x4012, "sts.l macl, @-r0"),
            (0x4022, "sts.l pr, @-r0"),
            (0x400e, "ldc r0, sr"),
            (0x401e, "ldc r0, gbr"),
            (0x402e, "ldc r0, vbr"),
            (0x400a, "lds r0, mach"),
            (0x401a, "lds r0, macl"),
            (0x402b, "jmp @r0"),
            (0x400b, "jsr @r0"),
            (0x4007, "ldc.l @r0+, sr"),
            (0x4017, "ldc.l @r0+, gbr"),
            (0x4027, "ldc.l @r0+, vbr"),
            (0x4006, "lds.l @r0+, mach"),
            (0x4016, "lds.l @r0+, macl"),
            (0x4026, "lds.l @r0+, pr"),
            (0x0023, "braf r0"),
            (0x0003, "bsrf r0"),
        ];

        for (opcode, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: 0x1000, size: 2, opcode: opcode as u16 },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_display_instructions_f00f() {
        let arch = ArchSuperH {};
        let ops: [(u16, &str); 54] = [
            (0x300c, "add r0, r0"),
            (0x300e, "addc r0, r0"),
            (0x300f, "addv r0, r0"),
            (0x2009, "and r0, r0"),
            (0x3000, "cmp/eq r0, r0"),
            (0x3002, "cmp/hs r0, r0"),
            (0x3003, "cmp/ge r0, r0"),
            (0x3006, "cmp/hi r0, r0"),
            (0x3007, "cmp/gt r0, r0"),
            (0x200c, "cmp/str r0, r0"),
            (0x3004, "div1 r0, r0"),
            (0x2007, "div0s r0, r0"),
            (0x300d, "dmuls.l r0, r0"),
            (0x3005, "dmulu.l r0, r0"),
            (0x600e, "exts.b r0, r0"),
            (0x600f, "exts.w r0, r0"),
            (0x600c, "extu.b r0, r0"),
            (0x600d, "extu.w r0, r0"),
            (0x6003, "mov r0, r0"),
            (0x0007, "mul.l r0, r0"),
            (0x200f, "muls r0, r0"),
            (0x200e, "mulu r0, r0"),
            (0x600b, "neg r0, r0"),
            (0x600a, "negc r0, r0"),
            (0x6007, "not r0, r0"),
            (0x200b, "or r0, r0"),
            (0x3008, "sub r0, r0"),
            (0x300a, "subc r0, r0"),
            (0x300b, "subv r0, r0"),
            (0x6008, "swap.b r0, r0"),
            (0x6009, "swap.w r0, r0"),
            (0x2008, "tst r0, r0"),
            (0x200a, "xor r0, r0"),
            (0x200d, "xtrct r0, r0"),
            (0x2000, "mov.b r0, @r0"),
            (0x2001, "mov.w r0, @r0"),
            (0x2002, "mov.l r0, @r0"),
            (0x6000, "mov.b @r0, r0"),
            (0x6001, "mov.w @r0, r0"),
            (0x6002, "mov.l @r0, r0"),
            (0x000f, "mac.l @r0+, @r0+"),
            (0x400f, "mac.w @r0+, @r0+"),
            (0x6004, "mov.b @r0+, r0"),
            (0x6005, "mov.w @r0+, r0"),
            (0x6006, "mov.l @r0+, r0"),
            (0x2004, "mov.b r0, @-r0"),
            (0x2005, "mov.w r0, @-r0"),
            (0x2006, "mov.l r0, @-r0"),
            (0x0004, "mov.b r0, @(r0, r0)"),
            (0x0005, "mov.w r0, @(r0, r0)"),
            (0x0006, "mov.l r0, @(r0, r0)"),
            (0x000c, "mov.b @(r0, r0), r0"),
            (0x000d, "mov.w @(r0, r0), r0"),
            (0x000e, "mov.l @(r0, r0), r0"),
        ];

        for (opcode, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: 0x1000, size: 2, opcode: opcode as u16 },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_display_instruction_mov_immediate_offset() {
        let arch = ArchSuperH {};
        let ops: [(u16, &str); 8] = [
            (0x8000, "mov.b r0, @(0x0, r0)"),
            (0x8011, "mov.b r0, @(0x1, r1)"),
            (0x8102, "mov.w r0, @(0x4, r0)"),
            (0x8113, "mov.w r0, @(0x6, r1)"),
            (0x8404, "mov.b @(0x4, r0), r0"),
            (0x8415, "mov.b @(0x5, r1), r0"),
            (0x8506, "mov.w @(0xc, r0), r0"),
            (0x8517, "mov.w @(0xe, r1), r0"),
        ];

        for (opcode, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: 0x1000, size: 2, opcode },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_display_instruction_gbr_and_branches() {
        let arch = ArchSuperH {};
        let ops: &[(u16, u32, &str)] = &[
            (0xc000, 0x0000, "mov.b r0, @(0x0, gbr)"),
            (0xc07f, 0x0000, "mov.b r0, @(0x7f, gbr)"),
            (0xc100, 0x0000, "mov.w r0, @(0x0, gbr)"),
            (0xc17f, 0x0000, "mov.w r0, @(0xfe, gbr)"),
            (0xc200, 0x0000, "mov.l r0, @(0x0, gbr)"),
            (0xc27f, 0x0000, "mov.l r0, @(0x1fc, gbr)"),
            (0xc400, 0x0000, "mov.b @(0x0, gbr), r0"),
            (0xc47f, 0x0000, "mov.b @(0x7f, gbr), r0"),
            (0xc500, 0x0000, "mov.w @(0x0, gbr), r0"),
            (0xc57f, 0x0000, "mov.w @(0xfe, gbr), r0"),
            (0xc600, 0x0000, "mov.l @(0x0, gbr), r0"),
            (0xc67f, 0x0000, "mov.l @(0x1fc, gbr), r0"),
            (0x8b20, 0x1000, "bf 0x44"),
            (0x8b80, 0x1000, "bf 0xffffff04"),
            (0x8f10, 0x2000, "bf.s 0x24"),
            (0x8f90, 0x2000, "bf.s 0xffffff24"),
            (0x8904, 0x3000, "bt 0xc"),
            (0x8980, 0x3000, "bt 0xffffff04"),
            (0x8d04, 0x4000, "bt.s 0xc"),
            (0x8d80, 0x4000, "bt.s 0xffffff04"),
        ];

        for &(opcode, addr, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: addr as u64, size: 2, opcode },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_display_instruction_mov_l() {
        let arch = ArchSuperH {};
        let ops: &[(u16, u32, &str)] = &[
            // mov.l rX, @(0xXXX, rY)
            (0x1000, 0x0000, "mov.l r0, @(0x0, r0)"),
            (0x1001, 0x0000, "mov.l r0, @(0x4, r0)"),
            (0x100f, 0x0000, "mov.l r0, @(0x3c, r0)"),
            (0x101f, 0x0000, "mov.l r1, @(0x3c, r0)"),
            // mov.l @(0xXXX, rY), rX
            (0x5000, 0x0000, "mov.l @(0x0, r0), r0"),
        ];

        for &(opcode, addr, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: addr as u64, size: 2, opcode },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_display_instruction_bra_bsr() {
        let arch: ArchSuperH = ArchSuperH {};
        let ops: &[(u16, u32, &str)] = &[
            // bra
            (0xa000, 0x0000, "bra 0x4"),
            (0xa001, 0x0000, "bra 0x6"),
            (0xa800, 0x0000, "bra 0xfffff004"),
            (0xa801, 0x0000, "bra 0xfffff006"),
            // bsr
            (0xb000, 0x0000, "bsr 0x4"),
            (0xb001, 0x0000, "bsr 0x6"),
            (0xb800, 0x0000, "bsr 0xfffff004"),
            (0xb801, 0x0000, "bsr 0xfffff006"),
        ];

        for &(opcode, addr, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: addr as u64, size: 2, opcode },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_display_instruction_operations() {
        let arch = ArchSuperH {};
        let ops: &[(u16, u32, &str)] = &[
            (0xcdff, 0x0000, "and.b #0xff, @(r0, gbr)"),
            (0xcfff, 0x0000, "or.b #0xff, @(r0, gbr)"),
            (0xccff, 0x0000, "tst.b #0xff, @(r0, gbr)"),
            (0xceff, 0x0000, "xor.b #0xff, @(r0, gbr)"),
            (0xc9ff, 0x0000, "and #0xff, r0"),
            (0x88ff, 0x0000, "cmp/eq #0xff, r0"),
            (0xcbff, 0x0000, "or #0xff, r0"),
            (0xc8ff, 0x0000, "tst #0xff, r0"),
            (0xcaff, 0x0000, "xor #0xff, r0"),
            (0xc3ff, 0x0000, "trapa #0xff"),
        ];

        for &(opcode, addr, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: addr as u64, size: 2, opcode },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_add_mov_unknown_instructions() {
        let arch = ArchSuperH {};
        let ops: &[(u16, u32, &str)] = &[
            (0x70FF, 0x0000, "add #0xff, r0"),
            (0xe0FF, 0x0000, "mov #0xff, r0"),
            (0x0000, 0x0000, ".word 0x0000 /* unknown instruction */"),
        ];

        for &(opcode, addr, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: addr as u64, size: 2, opcode },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_mov_instructions_with_labels() {
        let arch = ArchSuperH {};
        let ops: &[(u16, u32, &str)] =
            &[(0x9000, 0x0000, "mov.w @(0x4, pc), r0"), (0xd000, 0x0000, "mov.l @(0x4, pc), r0")];

        for &(opcode, addr, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef { address: addr as u64, size: 2, opcode },
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

            let joined_str: String = parts.iter().map(|part| format!("{}", part)).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }
}

use alloc::{collections::BTreeMap, format, string::String, vec, vec::Vec};

use anyhow::Result;
use object::elf;

use crate::{
    arch::{Arch, superh::disasm::sh2_disasm},
    diff::{DiffObjConfig, display::InstructionPart},
    obj::{InstructionRef, Relocation, RelocationFlags, ResolvedInstructionRef},
};

pub mod disasm;

#[derive(Debug)]
pub struct ArchSuperH {}

impl ArchSuperH {
    pub fn new(_file: &object::File) -> Result<Self> { Ok(Self {}) }
}

struct DataInfo {
    address: u64,
    size: u32,
}

impl Arch for ArchSuperH {
    fn scan_instructions_internal(
        &self,
        address: u64,
        code: &[u8],
        _section_index: usize,
        _relocations: &[Relocation],
        _diff_config: &DiffObjConfig,
    ) -> Result<Vec<InstructionRef>> {
        let mut ops = Vec::<InstructionRef>::with_capacity(code.len() / 2);
        let mut offset = address;

        for chunk in code.chunks_exact(2) {
            let opcode = u16::from_be_bytes(chunk.try_into().unwrap());
            let mut parts: Vec<InstructionPart> = vec![];
            let resolved: ResolvedInstructionRef = Default::default();
            let mut branch_dest: Option<u64> = None;
            sh2_disasm(
                offset.try_into().unwrap(),
                opcode,
                true,
                &mut parts,
                &resolved,
                &mut branch_dest,
            );

            let opcode_enum: u16 = match parts.first() {
                Some(InstructionPart::Opcode(_, val)) => *val,
                _ => 0,
            };
            ops.push(InstructionRef { address: offset, size: 2, opcode: opcode_enum, branch_dest });
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

        sh2_disasm(0, opcode, true, &mut parts, &resolved, &mut branch_dest);

        if let Some(symbol_data) =
            resolved.section.data_range(resolved.symbol.address, resolved.symbol.size as usize)
        {
            // scan for data
            // map of instruction offsets to data target offsets
            let mut data_offsets = BTreeMap::<u64, DataInfo>::new();

            let mut pos: u64 = 0;
            for chunk in symbol_data.chunks_exact(2) {
                let opcode = u16::from_be_bytes(chunk.try_into().unwrap());
                // mov.w
                if (opcode & 0xf000) == 0x9000 {
                    let target = (opcode as u64 & 0xff) * 2 + 4 + pos;
                    let data_info = DataInfo { address: target, size: 2 };
                    data_offsets.insert(pos, data_info);
                }
                // mov.l
                else if (opcode & 0xf000) == 0xd000 {
                    let target = ((opcode as u64 & 0xff) * 4 + 4 + pos) & 0xfffffffc;
                    let data_info = DataInfo { address: target, size: 4 };
                    data_offsets.insert(pos, data_info);
                }
                pos += 2;
            }

            let pos = resolved.ins_ref.address - resolved.symbol.address;

            // add the data info
            if let Some(value) = data_offsets.get(&pos) {
                if value.size == 2 && value.address as usize + 1 < symbol_data.len() {
                    let data = u16::from_be_bytes(
                        symbol_data[value.address as usize..value.address as usize + 2]
                            .try_into()
                            .unwrap(),
                    );
                    parts.push(InstructionPart::basic(" /* "));
                    parts.push(InstructionPart::basic("0x"));
                    parts.push(InstructionPart::basic(format!("{data:04X}")));
                    parts.push(InstructionPart::basic(" */"));
                } else if value.size == 4 && value.address as usize + 3 < symbol_data.len() {
                    let data = u32::from_be_bytes(
                        symbol_data[value.address as usize..value.address as usize + 4]
                            .try_into()
                            .unwrap(),
                    );
                    parts.push(InstructionPart::basic(" /* "));
                    parts.push(InstructionPart::basic("0x"));
                    parts.push(InstructionPart::basic(format!("{data:08X}")));
                    parts.push(InstructionPart::basic(" */"));
                }
            }
        }

        for part in parts {
            cb(part)?;
        }

        Ok(())
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
            RelocationFlags::Elf(elf::R_SH_DIR32) => 4,
            RelocationFlags::Elf(_) => 1,
            _ => 1,
        }
    }
}

#[cfg(test)]
mod test {
    use std::fmt::{self, Display};

    use super::*;
    use crate::obj::{InstructionArg, Section, SectionData, Symbol};

    impl Display for InstructionPart<'_> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                InstructionPart::Basic(s) => f.write_str(s),
                InstructionPart::Opcode(s, _o) => write!(f, "{s} "),
                InstructionPart::Arg(arg) => write!(f, "{arg}"),
                InstructionPart::Separator => f.write_str(", "),
            }
        }
    }

    impl Display for InstructionArg<'_> {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            match self {
                InstructionArg::Value(v) => write!(f, "{v}"),
                InstructionArg::BranchDest(v) => write!(f, "{v}"),
                InstructionArg::Reloc => f.write_str("reloc"),
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
                    ins_ref: InstructionRef { address: 0x1000, size: 2, opcode, branch_dest: None },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
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
                    ins_ref: InstructionRef { address: 0x1000, size: 2, opcode, branch_dest: None },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
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
                    ins_ref: InstructionRef { address: 0x1000, size: 2, opcode, branch_dest: None },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
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
                    ins_ref: InstructionRef { address: 0x1000, size: 2, opcode, branch_dest: None },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
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
                    ins_ref: InstructionRef {
                        address: addr as u64,
                        size: 2,
                        opcode,
                        branch_dest: None,
                    },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
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
                    ins_ref: InstructionRef {
                        address: addr as u64,
                        size: 2,
                        opcode,
                        branch_dest: None,
                    },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
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
                    ins_ref: InstructionRef {
                        address: addr as u64,
                        size: 2,
                        opcode,
                        branch_dest: None,
                    },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
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
                    ins_ref: InstructionRef {
                        address: addr as u64,
                        size: 2,
                        opcode,
                        branch_dest: None,
                    },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_sh2_add_mov_unknown_instructions() {
        let arch = ArchSuperH {};
        let ops: &[(u16, u32, &str)] = &[
            (0x70FF, 0x0000, "add #0xff, r0"),
            (0xE0FF, 0x0000, "mov #0xff, r0"),
            (0x0000, 0x0000, ".word 0x0000 /* unknown instruction */"),
        ];

        for &(opcode, addr, expected_str) in ops {
            let code = opcode.to_be_bytes();
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef {
                        address: addr as u64,
                        size: 2,
                        opcode,
                        branch_dest: None,
                    },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
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
                    ins_ref: InstructionRef {
                        address: addr as u64,
                        size: 2,
                        opcode,
                        branch_dest: None,
                    },
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

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_func_0606_f378_mov_w_data_labeling() {
        let arch = ArchSuperH {};
        let ops: &[(u16, u32, &str)] = &[(0x9000, 0x0606F378, "mov.w @(0x4, pc), r0 /* 0x00B0 */")];

        let mut code = Vec::new();
        code.extend_from_slice(&0x9000_u16.to_be_bytes());
        code.extend_from_slice(&0x0009_u16.to_be_bytes());
        code.extend_from_slice(&0x00B0_u16.to_be_bytes());

        for &(opcode, addr, expected_str) in ops {
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef {
                        address: addr as u64,
                        size: 2,
                        opcode,
                        branch_dest: None,
                    },
                    code: &opcode.to_be_bytes(),
                    symbol: &Symbol {
                        address: 0x0606F378, // func base address
                        size: code.len() as u64,
                        ..Default::default()
                    },
                    section: &Section {
                        address: 0x0606F378,
                        size: code.len() as u64,
                        data: SectionData(code.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                &DiffObjConfig::default(),
                &mut |part| {
                    parts.push(part.into_static());
                    Ok(())
                },
            )
            .unwrap();

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }

    #[test]
    fn test_func_0606_f378_mov_l_data_labeling() {
        let arch = ArchSuperH {};
        let ops: &[(u16, u32, &str)] =
            &[(0xd000, 0x0606F378, "mov.l @(0x4, pc), r0 /* 0x00B000B0 */")];

        let mut code = Vec::new();
        code.extend_from_slice(&0xd000_u16.to_be_bytes());
        code.extend_from_slice(&0x0009_u16.to_be_bytes());
        code.extend_from_slice(&0x00B0_u16.to_be_bytes());
        code.extend_from_slice(&0x00B0_u16.to_be_bytes());

        for &(opcode, addr, expected_str) in ops {
            let mut parts = Vec::new();

            arch.display_instruction(
                ResolvedInstructionRef {
                    ins_ref: InstructionRef {
                        address: addr as u64,
                        size: 2,
                        opcode,
                        branch_dest: None,
                    },
                    code: &opcode.to_be_bytes(),
                    symbol: &Symbol {
                        address: 0x0606F378, // func base address
                        size: code.len() as u64,
                        ..Default::default()
                    },
                    section: &Section {
                        address: 0x0606F378,
                        size: code.len() as u64,
                        data: SectionData(code.clone()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                &DiffObjConfig::default(),
                &mut |part| {
                    parts.push(part.into_static());
                    Ok(())
                },
            )
            .unwrap();

            let joined_str: String = parts.iter().map(<_>::to_string).collect();
            assert_eq!(joined_str, expected_str.to_string());
        }
    }
}

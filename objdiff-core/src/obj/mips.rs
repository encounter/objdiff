use std::collections::BTreeMap;

use anyhow::Result;
use rabbitizer::{config, Abi, InstrCategory, Instruction, OperandType};

use crate::{
    diff::{DiffObjConfig, ProcessCodeResult},
    obj::{ObjIns, ObjInsArg, ObjInsArgValue, ObjReloc, ObjRelocKind},
};

fn configure_rabbitizer() {
    unsafe {
        config::RabbitizerConfig_Cfg.reg_names.fpr_abi_names = Abi::O32;
    }
}

pub fn process_code(
    config: &DiffObjConfig,
    data: &[u8],
    start_address: u64,
    end_address: u64,
    relocs: &[ObjReloc],
    line_info: &Option<BTreeMap<u64, u64>>,
) -> Result<ProcessCodeResult> {
    configure_rabbitizer();

    let ins_count = data.len() / 4;
    let mut ops = Vec::<u16>::with_capacity(ins_count);
    let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
    let mut cur_addr = start_address as u32;
    for chunk in data.chunks_exact(4) {
        let reloc = relocs.iter().find(|r| (r.address as u32 & !3) == cur_addr);
        let code = u32::from_be_bytes(chunk.try_into()?);
        let instruction = Instruction::new(code, cur_addr, InstrCategory::CPU);

        let op = instruction.unique_id as u16;
        ops.push(op);

        let mnemonic = instruction.opcode_name().to_string();
        let is_branch = instruction.is_branch();
        let branch_offset = instruction.branch_offset();
        let branch_dest = if is_branch {
            cur_addr.checked_add_signed(branch_offset).map(|a| a as u64)
        } else {
            None
        };

        let operands = instruction.get_operands_slice();
        let mut args = Vec::with_capacity(operands.len() + 1);
        for (idx, op) in operands.iter().enumerate() {
            if idx > 0 {
                if config.space_between_args {
                    args.push(ObjInsArg::PlainText(", ".to_string()));
                } else {
                    args.push(ObjInsArg::PlainText(",".to_string()));
                }
            }

            match op {
                OperandType::cpu_immediate
                | OperandType::cpu_label
                | OperandType::cpu_branch_target_label => {
                    if let Some(branch_dest) = branch_dest {
                        args.push(ObjInsArg::BranchDest(branch_dest));
                    } else if let Some(reloc) = reloc {
                        if matches!(&reloc.target_section, Some(s) if s == ".text")
                            && reloc.target.address > start_address
                            && reloc.target.address < end_address
                        {
                            args.push(ObjInsArg::BranchDest(reloc.target.address));
                        } else {
                            push_reloc(&mut args, reloc);
                        }
                    } else {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                            op.disassemble(&instruction, None),
                        )));
                    }
                }
                OperandType::cpu_immediate_base => {
                    if let Some(reloc) = reloc {
                        push_reloc(&mut args, reloc);
                    } else {
                        args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                            OperandType::cpu_immediate.disassemble(&instruction, None),
                        )));
                    }
                    args.push(ObjInsArg::PlainText("(".to_string()));
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                        OperandType::cpu_rs.disassemble(&instruction, None),
                    )));
                    args.push(ObjInsArg::PlainText(")".to_string()));
                }
                _ => {
                    args.push(ObjInsArg::Arg(ObjInsArgValue::Opaque(
                        op.disassemble(&instruction, None),
                    )));
                }
            }
        }
        let line = line_info
            .as_ref()
            .and_then(|map| map.range(..=cur_addr as u64).last().map(|(_, &b)| b));
        insts.push(ObjIns {
            address: cur_addr as u64,
            size: 4,
            op,
            mnemonic,
            args,
            reloc: reloc.cloned(),
            branch_dest,
            line,
            orig: None,
        });
        cur_addr += 4;
    }
    Ok(ProcessCodeResult { ops, insts })
}

fn push_reloc(args: &mut Vec<ObjInsArg>, reloc: &ObjReloc) {
    match reloc.kind {
        ObjRelocKind::MipsHi16 => {
            args.push(ObjInsArg::PlainText("%hi(".to_string()));
            args.push(ObjInsArg::Reloc);
            args.push(ObjInsArg::PlainText(")".to_string()));
        }
        ObjRelocKind::MipsLo16 => {
            args.push(ObjInsArg::PlainText("%lo(".to_string()));
            args.push(ObjInsArg::Reloc);
            args.push(ObjInsArg::PlainText(")".to_string()));
        }
        ObjRelocKind::MipsGot16 => {
            args.push(ObjInsArg::PlainText("%got(".to_string()));
            args.push(ObjInsArg::Reloc);
            args.push(ObjInsArg::PlainText(")".to_string()));
        }
        ObjRelocKind::MipsCall16 => {
            args.push(ObjInsArg::PlainText("%call16(".to_string()));
            args.push(ObjInsArg::Reloc);
            args.push(ObjInsArg::PlainText(")".to_string()));
        }
        ObjRelocKind::MipsGpRel16 => {
            args.push(ObjInsArg::PlainText("%gp_rel(".to_string()));
            args.push(ObjInsArg::Reloc);
            args.push(ObjInsArg::PlainText(")".to_string()));
        }
        ObjRelocKind::Mips26 => {
            args.push(ObjInsArg::Reloc);
        }
        ObjRelocKind::MipsGpRel32 => {
            todo!("unimplemented: mips gp_rel32");
        }
        kind => panic!("Unsupported MIPS relocation kind: {:?}", kind),
    }
}

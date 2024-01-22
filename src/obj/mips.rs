use std::collections::BTreeMap;

use anyhow::Result;
use rabbitizer::{config, Abi, InstrCategory, Instruction, OperandType};

use crate::{
    diff::ProcessCodeResult,
    obj::{ObjIns, ObjInsArg, ObjReloc},
};

fn configure_rabbitizer() {
    unsafe {
        config::RabbitizerConfig_Cfg.reg_names.fpr_abi_names = Abi::O32;
    }
}

pub fn process_code(
    data: &[u8],
    start_address: u64,
    end_address: u64,
    relocs: &[ObjReloc],
    line_info: &Option<BTreeMap<u64, u64>>,
) -> Result<ProcessCodeResult> {
    configure_rabbitizer();

    let ins_count = data.len() / 4;
    let mut ops = Vec::<u8>::with_capacity(ins_count);
    let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
    let mut cur_addr = start_address as u32;
    for chunk in data.chunks_exact(4) {
        let reloc = relocs.iter().find(|r| (r.address as u32 & !3) == cur_addr);
        let code = u32::from_be_bytes(chunk.try_into()?);
        let instruction = Instruction::new(code, cur_addr, InstrCategory::CPU);

        let op = instruction.unique_id as u8;
        ops.push(op);

        let mnemonic = instruction.opcode_name().to_string();
        let is_branch = instruction.is_branch();
        let branch_offset = instruction.branch_offset();
        let branch_dest =
            if is_branch { Some((cur_addr as i32 + branch_offset) as u32) } else { None };

        let operands = instruction.get_operands_slice();
        let mut args = Vec::with_capacity(operands.len() + 1);
        for op in operands {
            match op {
                OperandType::cpu_immediate
                | OperandType::cpu_label
                | OperandType::cpu_branch_target_label => {
                    if is_branch {
                        args.push(ObjInsArg::BranchOffset(branch_offset));
                    } else if let Some(reloc) = reloc {
                        if matches!(&reloc.target_section, Some(s) if s == ".text")
                            && reloc.target.address > start_address
                            && reloc.target.address < end_address
                        {
                            // Inter-function reloc, convert to branch offset
                            args.push(ObjInsArg::BranchOffset(
                                reloc.target.address as i32 - cur_addr as i32,
                            ));
                        } else {
                            args.push(ObjInsArg::Reloc);
                        }
                    } else {
                        args.push(ObjInsArg::MipsArg(op.disassemble(&instruction, None)));
                    }
                }
                OperandType::cpu_immediate_base => {
                    if reloc.is_some() {
                        args.push(ObjInsArg::RelocWithBase);
                    } else {
                        args.push(ObjInsArg::MipsArgWithBase(
                            OperandType::cpu_immediate.disassemble(&instruction, None),
                        ));
                    }
                    args.push(ObjInsArg::MipsArg(
                        OperandType::cpu_rs.disassemble(&instruction, None),
                    ));
                }
                _ => {
                    args.push(ObjInsArg::MipsArg(op.disassemble(&instruction, None)));
                }
            }
        }
        let line = line_info
            .as_ref()
            .and_then(|map| map.range(..=cur_addr as u64).last().map(|(_, &b)| b));
        insts.push(ObjIns {
            address: cur_addr,
            code,
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

use std::collections::BTreeMap;

use anyhow::Result;
use rabbitizer::{config, Abi, Instruction, InstrCategory, OperandType};

use crate::obj::{ObjIns, ObjInsArg, ObjReloc};

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
    line_info: &Option<BTreeMap<u32, u32>>,
) -> Result<(Vec<u8>, Vec<ObjIns>)> {
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
        let args = instruction
            .get_operands_slice()
            .iter()
            .map(|op| match op {
                OperandType::cpu_immediate | OperandType::cpu_label | OperandType::cpu_branch_target_label => {
                    if is_branch {
                        ObjInsArg::BranchOffset(branch_offset)
                    } else if let Some(reloc) = reloc {
                        if matches!(&reloc.target_section, Some(s) if s == ".text")
                            && reloc.target.address > start_address
                            && reloc.target.address < end_address
                        {
                            // Inter-function reloc, convert to branch offset
                            ObjInsArg::BranchOffset(reloc.target.address as i32 - cur_addr as i32)
                        } else {
                            ObjInsArg::Reloc
                        }
                    } else {
                        ObjInsArg::MipsArg(op.disassemble(&instruction, None))
                    }
                }
                OperandType::cpu_immediate_base => {
                    if reloc.is_some() {
                        ObjInsArg::RelocWithBase
                    } else {
                        ObjInsArg::MipsArg(op.disassemble(&instruction, None))
                    }
                }
                _ => ObjInsArg::MipsArg(op.disassemble(&instruction, None)),
            })
            .collect();
        let line =
            line_info.as_ref().and_then(|map| map.range(..=cur_addr).last().map(|(_, &b)| b));
        insts.push(ObjIns {
            address: cur_addr,
            code,
            op,
            mnemonic,
            args,
            reloc: reloc.cloned(),
            branch_dest,
            line,
        });
        cur_addr += 4;
    }
    Ok((ops, insts))
}

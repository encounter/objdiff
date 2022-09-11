use anyhow::Result;
use rabbitizer::{config_set_register_fpr_abi_names, Abi, Instruction, SimpleOperandType};

use crate::obj::{ObjIns, ObjInsArg, ObjReloc};

pub fn process_code(
    data: &[u8],
    start_address: u64,
    end_address: u64,
    relocs: &[ObjReloc],
) -> Result<(Vec<u8>, Vec<ObjIns>)> {
    config_set_register_fpr_abi_names(Abi::RABBITIZER_ABI_O32);

    let ins_count = data.len() / 4;
    let mut ops = Vec::<u8>::with_capacity(ins_count);
    let mut insts = Vec::<ObjIns>::with_capacity(ins_count);
    let mut cur_addr = start_address as u32;
    for chunk in data.chunks_exact(4) {
        let reloc = relocs.iter().find(|r| (r.address as u32 & !3) == cur_addr);
        let code = u32::from_be_bytes(chunk.try_into()?);
        let mut instruction = Instruction::new(code, cur_addr);

        let op = instruction.instr_id() as u8;
        ops.push(op);

        let mnemonic = instruction.instr_id().get_opcode_name().unwrap_or_default().to_string();
        let is_branch = instruction.is_branch();
        let branch_offset = instruction.branch_offset();
        let branch_dest =
            if is_branch { Some((cur_addr as i32 + branch_offset) as u32) } else { None };
        let args = instruction
            .simple_operands()
            .iter()
            .map(|op| match op.kind {
                SimpleOperandType::Imm | SimpleOperandType::Label => {
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
                        ObjInsArg::MipsArg(op.disassembled.clone())
                    }
                }
                SimpleOperandType::ImmBase => {
                    if reloc.is_some() {
                        ObjInsArg::RelocWithBase
                    } else {
                        ObjInsArg::MipsArg(op.disassembled.clone())
                    }
                }
                _ => ObjInsArg::MipsArg(op.disassembled.clone()),
            })
            .collect();
        insts.push(ObjIns {
            address: cur_addr,
            code,
            op,
            mnemonic,
            args,
            reloc: reloc.cloned(),
            branch_dest,
        });
        cur_addr += 4;
    }
    Ok((ops, insts))
}

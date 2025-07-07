use alloc::{
    boxed::Box,
    collections::{BTreeMap, BTreeSet},
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::{
    ffi::CStr,
    ops::{Index, IndexMut},
};

use itertools::Itertools;
use ppc750cl::Simm;

use crate::{
    arch::DataType,
    obj::{FlowAnalysisResult, FlowAnalysisValue, Object, Relocation, Symbol},
    util::{RawDouble, RawFloat},
};

fn is_store_instruction(op: ppc750cl::Opcode) -> bool {
    use ppc750cl::Opcode;
    matches!(
        op,
        Opcode::Stbux
            | Opcode::Stbx
            | Opcode::Stfdux
            | Opcode::Stfdx
            | Opcode::Stfiwx
            | Opcode::Stfsux
            | Opcode::Stfsx
            | Opcode::Sthbrx
            | Opcode::Sthux
            | Opcode::Sthx
            | Opcode::Stswi
            | Opcode::Stswx
            | Opcode::Stwbrx
            | Opcode::Stwcx_
            | Opcode::Stwux
            | Opcode::Stwx
            | Opcode::Stwu
            | Opcode::Stb
            | Opcode::Stbu
            | Opcode::Sth
            | Opcode::Sthu
            | Opcode::Stmw
            | Opcode::Stfs
            | Opcode::Stfsu
            | Opcode::Stfd
            | Opcode::Stfdu
    )
}

pub fn guess_data_type_from_load_store_inst_op(inst_op: ppc750cl::Opcode) -> Option<DataType> {
    use ppc750cl::Opcode;
    match inst_op {
        Opcode::Lbz | Opcode::Lbzu | Opcode::Lbzux | Opcode::Lbzx => Some(DataType::Int8),
        Opcode::Lhz | Opcode::Lhzu | Opcode::Lhzux | Opcode::Lhzx => Some(DataType::Int16),
        Opcode::Lha | Opcode::Lhau | Opcode::Lhaux | Opcode::Lhax => Some(DataType::Int16),
        Opcode::Lwz | Opcode::Lwzu | Opcode::Lwzux | Opcode::Lwzx => Some(DataType::Int32),
        Opcode::Lfs | Opcode::Lfsu | Opcode::Lfsux | Opcode::Lfsx => Some(DataType::Float),
        Opcode::Lfd | Opcode::Lfdu | Opcode::Lfdux | Opcode::Lfdx => Some(DataType::Double),

        Opcode::Stb | Opcode::Stbu | Opcode::Stbux | Opcode::Stbx => Some(DataType::Int8),
        Opcode::Sth | Opcode::Sthu | Opcode::Sthux | Opcode::Sthx => Some(DataType::Int16),
        Opcode::Stw | Opcode::Stwu | Opcode::Stwux | Opcode::Stwx => Some(DataType::Int32),
        Opcode::Stfs | Opcode::Stfsu | Opcode::Stfsux | Opcode::Stfsx => Some(DataType::Float),
        Opcode::Stfd | Opcode::Stfdu | Opcode::Stfdux | Opcode::Stfdx => Some(DataType::Double),
        _ => None,
    }
}

#[derive(Default, PartialEq, Eq, Copy, Clone, Debug, PartialOrd, Ord)]
enum RegisterContent {
    #[default]
    Unknown,
    Variable, // Multiple potential values
    FloatConstant(RawFloat),
    DoubleConstant(RawDouble),
    IntConstant(i32),
    InputRegister(u8),
    Symbol(usize),
}

impl core::fmt::Display for RegisterContent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RegisterContent::Unknown => write!(f, "unknown"),
            RegisterContent::Variable => write!(f, "variable"),
            RegisterContent::IntConstant(i) =>
            // -i is safe because it's at most a 16 bit constant in the i32
            {
                if *i >= 0 {
                    write!(f, "0x{i:x}")
                } else {
                    write!(f, "-0x{:x}", -i)
                }
            }
            RegisterContent::FloatConstant(RawFloat(fp)) => write!(f, "{fp:?}f"),
            RegisterContent::DoubleConstant(RawDouble(fp)) => write!(f, "{fp:?}d"),
            RegisterContent::InputRegister(p) => write!(f, "input{p}"),
            RegisterContent::Symbol(_u) => write!(f, "relocation"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Ord, PartialOrd)]
struct RegisterState {
    gpr: [RegisterContent; 32],
    fpr: [RegisterContent; 32],
}

impl RegisterState {
    fn new() -> Self {
        RegisterState { gpr: [RegisterContent::Unknown; 32], fpr: [RegisterContent::Unknown; 32] }
    }

    // During a function call, these registers must be assumed trashed.
    fn clear_volatile(&mut self) {
        self[ppc750cl::GPR(0)] = RegisterContent::Unknown;
        for i in 0..=13 {
            self[ppc750cl::GPR(i)] = RegisterContent::Unknown;
        }
        for i in 0..=13 {
            self[ppc750cl::FPR(i)] = RegisterContent::Unknown;
        }
    }

    // Mark potential input values.
    // Subsequent flow analysis will "realize" that they are not actually inputs if
    // they get overwritten with another value before getting read.
    fn set_potential_inputs(&mut self) {
        for g_reg in 3..=13 {
            self[ppc750cl::GPR(g_reg)] = RegisterContent::InputRegister(g_reg);
        }
        for f_reg in 1..=13 {
            self[ppc750cl::FPR(f_reg)] = RegisterContent::InputRegister(f_reg);
        }
    }

    // If the there is no value, we can take the new known value.
    // If there's a known value different than the new value, the content
    // must is variable.
    // Returns whether the current value was updated.
    fn unify_values(current: &mut RegisterContent, new: &RegisterContent) -> bool {
        if *current == *new {
            false
        } else if *current == RegisterContent::Unknown {
            *current = *new;
            true
        } else if *current == RegisterContent::Variable {
            // Already variable
            false
        } else {
            *current = RegisterContent::Variable;
            true
        }
    }

    // Unify currently known register contents in a give situation with new
    // information about the register contents in that situation.
    // Currently unknown register contents can be filled, but if there are
    // conflicting contents, we go back to unknown.
    fn unify(&mut self, other: &RegisterState) -> bool {
        let mut updated = false;
        for i in 0..32 {
            updated |= Self::unify_values(&mut self.gpr[i], &other.gpr[i]);
            updated |= Self::unify_values(&mut self.fpr[i], &other.fpr[i]);
        }
        updated
    }
}

impl Index<ppc750cl::GPR> for RegisterState {
    type Output = RegisterContent;

    fn index(&self, gpr: ppc750cl::GPR) -> &Self::Output { &self.gpr[gpr.0 as usize] }
}
impl IndexMut<ppc750cl::GPR> for RegisterState {
    fn index_mut(&mut self, gpr: ppc750cl::GPR) -> &mut Self::Output {
        &mut self.gpr[gpr.0 as usize]
    }
}

impl Index<ppc750cl::FPR> for RegisterState {
    type Output = RegisterContent;

    fn index(&self, fpr: ppc750cl::FPR) -> &Self::Output { &self.fpr[fpr.0 as usize] }
}
impl IndexMut<ppc750cl::FPR> for RegisterState {
    fn index_mut(&mut self, fpr: ppc750cl::FPR) -> &mut Self::Output {
        &mut self.fpr[fpr.0 as usize]
    }
}

fn execute_instruction(
    registers: &mut RegisterState,
    op: &ppc750cl::Opcode,
    args: &ppc750cl::Arguments,
) {
    use ppc750cl::{Argument, GPR, Opcode};
    match (op, args[0], args[1], args[2]) {
        (Opcode::Or, Argument::GPR(a), Argument::GPR(b), Argument::GPR(c)) => {
            // Move is implemented as or with self for ints
            if b == c {
                registers[a] = registers[b];
            } else {
                registers[a] = RegisterContent::Unknown;
            }
        }
        (Opcode::Fmr, Argument::FPR(a), Argument::FPR(b), _) => {
            registers[a] = registers[b];
        }
        (Opcode::Addi, Argument::GPR(a), Argument::GPR(GPR(0)), Argument::Simm(c)) => {
            // Load immidiate implemented as addi with addend = r0
            // Let Addi with other addends fall through to the case which
            // overwrites the destination
            registers[a] = RegisterContent::IntConstant(c.0 as i32);
        }
        (Opcode::Bcctr, _, _, _) => {
            // Called a function pointer, may have erased volatile registers
            registers.clear_volatile();
        }
        (Opcode::B, _, _, _) => {
            if get_branch_offset(args) == 0 {
                // Call to another function
                registers.clear_volatile();
            }
        }
        (
            Opcode::Stbu | Opcode::Sthu | Opcode::Stwu | Opcode::Stfsu | Opcode::Stfdu,
            _,
            _,
            Argument::GPR(rel),
        ) => {
            // Storing with update, clear updated register (third arg)
            registers[rel] = RegisterContent::Unknown;
        }
        (
            Opcode::Stbux | Opcode::Sthux | Opcode::Stwux | Opcode::Stfsux | Opcode::Stfdux,
            _,
            Argument::GPR(rel),
            _,
        ) => {
            // Storing indexed with update, clear updated register (second arg)
            registers[rel] = RegisterContent::Unknown;
        }
        (Opcode::Lmw, Argument::GPR(target), _, _) => {
            // `lmw` overwrites all registers from rd to r31.
            for reg in target.0..31 {
                registers[GPR(reg)] = RegisterContent::Unknown;
            }
        }
        (_, Argument::GPR(a), _, _) => {
            // Store instructions don't modify the GPR
            if !is_store_instruction(*op) {
                // Other operations which write to GPR a
                registers[a] = RegisterContent::Unknown;
            }
        }
        (_, Argument::FPR(a), _, _) => {
            // Store instructions don't modify the FPR
            if !is_store_instruction(*op) {
                // Other operations which write to FPR a
                registers[a] = RegisterContent::Unknown;
            }
        }
        (_, _, _, _) => {}
    }
}

fn get_branch_offset(args: &ppc750cl::Arguments) -> i32 {
    for arg in args.iter() {
        match arg {
            ppc750cl::Argument::BranchDest(dest) => return dest.0 / 4,
            ppc750cl::Argument::None => break,
            _ => {}
        }
    }
    0
}

#[derive(Debug, Default)]
struct PPCFlowAnalysisResult {
    argument_contents: BTreeMap<(u64, u8), FlowAnalysisValue>,
}

impl PPCFlowAnalysisResult {
    fn set_argument_value_at_address(
        &mut self,
        address: u64,
        argument: u8,
        value: FlowAnalysisValue,
    ) {
        self.argument_contents.insert((address, argument), value);
    }

    fn new() -> Self { PPCFlowAnalysisResult { argument_contents: Default::default() } }
}

impl FlowAnalysisResult for PPCFlowAnalysisResult {
    fn get_argument_value_at_address(
        &self,
        address: u64,
        argument: u8,
    ) -> Option<&FlowAnalysisValue> {
        self.argument_contents.get(&(address, argument))
    }
}

fn clamp_text_length(s: String, max: usize) -> String {
    if s.len() <= max { s } else { format!("{}…", s.chars().take(max - 3).collect::<String>()) }
}

fn get_register_content_from_reloc(
    reloc: &Relocation,
    obj: &Object,
    op: ppc750cl::Opcode,
) -> RegisterContent {
    if let Some(bytes) = obj.symbol_data(reloc.target_symbol) {
        match guess_data_type_from_load_store_inst_op(op) {
            Some(DataType::Float) => {
                RegisterContent::FloatConstant(RawFloat(match obj.endianness {
                    object::Endianness::Little => {
                        f32::from_le_bytes(bytes.try_into().unwrap_or([0; 4]))
                    }
                    object::Endianness::Big => {
                        f32::from_be_bytes(bytes.try_into().unwrap_or([0; 4]))
                    }
                }))
            }
            Some(DataType::Double) => {
                RegisterContent::DoubleConstant(RawDouble(match obj.endianness {
                    object::Endianness::Little => {
                        f64::from_le_bytes(bytes.try_into().unwrap_or([0; 8]))
                    }
                    object::Endianness::Big => {
                        f64::from_be_bytes(bytes.try_into().unwrap_or([0; 8]))
                    }
                }))
            }
            _ => RegisterContent::Symbol(reloc.target_symbol),
        }
    } else {
        RegisterContent::Symbol(reloc.target_symbol)
    }
}

// Executing op with args at cur_address, update current_state with symbols that
// come from relocations. That is, references to globals, floating point
// constants, string constants, etc.
fn fill_registers_from_relocation(
    reloc: &Relocation,
    current_state: &mut RegisterState,
    obj: &Object,
    op: ppc750cl::Opcode,
    args: &ppc750cl::Arguments,
) {
    // Only update the register state for loads. We may store to a reloc
    // address but that doesn't update register contents.
    if !is_store_instruction(op) {
        match (op, args[0]) {
            // Everything else is a load of some sort
            (_, ppc750cl::Argument::GPR(gpr)) => {
                current_state[gpr] = get_register_content_from_reloc(reloc, obj, op);
            }
            (_, ppc750cl::Argument::FPR(fpr)) => {
                current_state[fpr] = get_register_content_from_reloc(reloc, obj, op);
            }
            _ => {}
        }
    }
}

// Special helper fragments generated by MWCC.
// See: https://github.com/encounter/decomp-toolkit/blob/main/src/analysis/pass.rs
const SLEDS: [&str; 6] = ["_savefpr_", "_restfpr_", "_savegpr_", "_restgpr_", "_savev", "_restv"];

fn is_sled_function(name: &str) -> bool { SLEDS.iter().any(|sled| name.starts_with(sled)) }

pub fn ppc_data_flow_analysis(
    obj: &Object,
    func_symbol: &Symbol,
    code: &[u8],
    relocations: &[Relocation],
) -> Box<dyn FlowAnalysisResult> {
    use alloc::collections::VecDeque;

    use ppc750cl::InsIter;
    let instructions = InsIter::new(code, func_symbol.address as u32)
        .map(|(_addr, ins)| (ins.op, ins.basic().args))
        .collect_vec();

    let func_address = func_symbol.address;

    // Get initial register values from function parameters
    let mut initial_register_state = RegisterState::new();
    initial_register_state.set_potential_inputs();

    let mut execution_queue = VecDeque::<(usize, RegisterState)>::new();
    execution_queue.push_back((0, initial_register_state));

    // Execute the instructions against abstract data
    let mut failsafe_counter = 0;
    let mut taken_branches = BTreeSet::<(usize, RegisterState)>::new();
    let mut register_state_at = Vec::<RegisterState>::new();
    let mut completed_first_pass = false;
    register_state_at.resize_with(instructions.len(), RegisterState::new);
    while let Some((mut index, mut current_state)) = execution_queue.pop_front() {
        while let Some((op, args)) = instructions.get(index) {
            // Record the state at this index
            // If recording does not result in any changes to the known values
            // we're done, because the subsequent values are a function of the
            // current values so we'll get the same result as the last time
            // we went down this path.
            // Don't break out if we haven't even completed the first pass
            // through the function though.
            if !register_state_at[index].unify(&current_state) && completed_first_pass {
                break;
            }

            // Get symbol used in this instruction
            let cur_addr = (func_address as u32) + ((index * 4) as u32);
            let reloc = relocations.iter().find(|r| (r.address as u32 & !3) == cur_addr);

            // Is this a branch to a compiler generated helper? These helpers
            // do not trash registers like normal function calls, so we don't
            // want to treat this as normal execution.
            let symbol = reloc.and_then(|r| obj.symbols.get(r.target_symbol));
            let is_sled_invocation = symbol.is_some_and(|x| is_sled_function(&x.name));

            // Execute the instruction to update the state
            // Since sled invocations are only used to save / restore registers
            // as part of prelude / cleanup in a function call we don't have to
            // do any execution for them.
            if !is_sled_invocation {
                execute_instruction(&mut current_state, op, args);
            }

            // Fill in register state coming from relocations at this line. This
            // handles references to global variables, floating point constants,
            // etc.
            if let Some(reloc) = reloc {
                fill_registers_from_relocation(reloc, &mut current_state, obj, *op, args);
            }

            // Add conditional branches to execution queue
            // Only take a given (address, register state) combination once. If
            // the known register state is different we have to take the branch
            // again to stabilize the known values for backwards branches.
            if op == &ppc750cl::Opcode::Bc {
                let branch_state = (index, current_state.clone());
                if !taken_branches.contains(&branch_state) {
                    let offset = get_branch_offset(args);
                    let target_index = ((index as i32) + offset) as usize;
                    execution_queue.push_back((target_index, current_state.clone()));
                    taken_branches.insert(branch_state);

                    // We should never hit this case, but avoid getting stuck in
                    // an infinite loop if we hit some kind of bad behavior.
                    failsafe_counter += 1;
                    if failsafe_counter > 256 {
                        //println!("Analysis of {} failed to stabilize", func_symbol.name);
                        return Box::new(PPCFlowAnalysisResult::new());
                    }
                }
            }

            // Update index
            if op == &ppc750cl::Opcode::B {
                // Unconditional branch
                let offset = get_branch_offset(args);
                if offset > 0 {
                    // Jump table or branch to over else clause.
                    index += offset as usize;
                } else if offset == 0 {
                    // Function call with relocation. We'll return to
                    // the next instruction.
                    index += 1;
                } else {
                    // Unconditional branch (E.g.: loop { ... })
                    // Also some compilations of loops put the conditional at
                    // the end and B to it for the check of the first iteration.
                    let branch_state = (index, current_state.clone());
                    if taken_branches.contains(&branch_state) {
                        break;
                    }
                    taken_branches.insert(branch_state);
                    index = ((index as i32) + offset) as usize;
                }
            } else {
                // Normal execution of next instruction
                index += 1;
            }
        }

        // Mark that we've completed at least one pass over the function, at
        // this point we can break out if the code we're running doesn't change
        // any register outcomes.
        completed_first_pass = true;
    }

    // Store the relevant data flow values for simplified instructions
    generate_flow_analysis_result(obj, func_address, code, register_state_at, relocations)
}

fn get_string_data(obj: &Object, symbol_index: usize, offset: Simm) -> Option<&str> {
    if let Some(sym) = obj.symbols.get(symbol_index) {
        if sym.name.starts_with("@stringBase") && offset.0 != 0 {
            if let Some(data) = obj.symbol_data(symbol_index) {
                let bytes = &data[offset.0 as usize..];
                if let Ok(Ok(str)) = CStr::from_bytes_until_nul(bytes).map(|x| x.to_str()) {
                    return Some(str);
                }
            }
        }
    }
    None
}

// Write the relevant part of the flow analysis out into the FlowAnalysisResult
// the rest of the application will use to query results of the flow analysis.
// Flow analysis will compute the known contents of every register at every
// line, but we only need to record the values of registers that are actually
// referenced at each line.
fn generate_flow_analysis_result(
    obj: &Object,
    base_address: u64,
    code: &[u8],
    register_state_at: Vec<RegisterState>,
    relocations: &[Relocation],
) -> Box<PPCFlowAnalysisResult> {
    use ppc750cl::{Argument, InsIter};
    let mut analysis_result = PPCFlowAnalysisResult::new();
    let default_register_state = RegisterState::new();
    for (addr, ins) in InsIter::new(code, 0) {
        let ins_address = base_address + (addr as u64);
        let index = addr / 4;
        let ppc750cl::ParsedIns { mnemonic: _, args } = ins.simplified();

        // If we're already showing relocations on a line don't also show data flow
        let reloc = relocations.iter().find(|r| (r.address & !3) == ins_address);

        // Special case to show float and double constants on the line where
        // they are being loaded.
        // We need to do this before we break out on showing relocations in the
        // subsequent if statement.
        if let (ppc750cl::Opcode::Lfs | ppc750cl::Opcode::Lfd, Some(reloc)) = (ins.op, reloc) {
            let content = get_register_content_from_reloc(reloc, obj, ins.op);
            if matches!(
                content,
                RegisterContent::FloatConstant(_) | RegisterContent::DoubleConstant(_)
            ) {
                analysis_result.set_argument_value_at_address(
                    ins_address,
                    1,
                    FlowAnalysisValue::Text(content.to_string()),
                );

                // Don't need to show any other data flow if we're showing that
                continue;
            }
        }

        // Special case to show string constants on the line where they are
        // being indexed to. This will typically be "addi t, stringbase, offset"
        let registers = register_state_at.get(index as usize).unwrap_or(&default_register_state);
        if let (ppc750cl::Opcode::Addi, Argument::GPR(rel), Argument::Simm(offset)) =
            (ins.op, args[1], args[2])
        {
            if let RegisterContent::Symbol(sym_index) = registers[rel] {
                if let Some(str) = get_string_data(obj, sym_index, offset) {
                    // Show the string constant in the analysis result
                    let formatted = format!("\"{str}\"");
                    analysis_result.set_argument_value_at_address(
                        ins_address,
                        2,
                        FlowAnalysisValue::Text(clamp_text_length(formatted, 20)),
                    );
                    // Don't continue, we want to show the stringbase value as well
                }
            }
        }

        let is_store = is_store_instruction(ins.op);
        for (arg_index, arg) in args.into_iter().enumerate() {
            // Hacky shorthand for determining which arguments are sources,
            // We only want to show data flow for source registers, not target
            // registers. Technically there are some non-"st_" operations which
            // read from their first argument but they're rare.
            if (arg_index == 0) && !is_store {
                continue;
            }

            let content = match arg {
                Argument::GPR(gpr) => Some(registers[gpr]),
                Argument::FPR(fpr) => Some(registers[fpr]),
                _ => None,
            };
            let analysis_value = match content {
                Some(RegisterContent::Symbol(s)) => {
                    if reloc.is_none() {
                        // Only symbols if there isn't already a relocation, because
                        // code other than the data flow analysis will be showing
                        // the symbol for a relocation on the line it is for. If we
                        // also showed it as data flow analysis value we would be
                        // showing redundant information.
                        obj.symbols.get(s).map(|sym| {
                            FlowAnalysisValue::Text(clamp_text_length(
                                sym.demangled_name.as_ref().unwrap_or(&sym.name).clone(),
                                20,
                            ))
                        })
                    } else {
                        None
                    }
                }
                Some(RegisterContent::InputRegister(reg)) => {
                    let reg_name = match arg {
                        Argument::GPR(_) => format!("in_r{reg}"),
                        Argument::FPR(_) => format!("in_f{reg}"),
                        _ => panic!("Register content should only be in a register"),
                    };
                    Some(FlowAnalysisValue::Text(reg_name))
                }
                Some(RegisterContent::Unknown) | Some(RegisterContent::Variable) => None,
                Some(value) => Some(FlowAnalysisValue::Text(value.to_string())),
                None => None,
            };
            if let Some(analysis_value) = analysis_value {
                analysis_result.set_argument_value_at_address(
                    ins_address,
                    arg_index as u8,
                    analysis_value,
                );
            }
        }
    }

    Box::new(analysis_result)
}

use alloc::{
    borrow::Cow,
    boxed::Box,
    format,
    string::{String, ToString},
    vec::Vec,
};
use core::{
    any::Any,
    fmt::{self, Debug},
};

use anyhow::{Result, bail};
use object::Endian as _;

use crate::{
    diff::{
        DiffObjConfig, DiffSide,
        display::{ContextItem, HoverItem, InstructionPart},
    },
    obj::{
        FlowAnalysisResult, InstructionArg, InstructionRef, Object, ParsedInstruction, Relocation,
        RelocationFlags, ResolvedInstructionRef, ResolvedSymbol, Section, Symbol, SymbolFlagSet,
        SymbolKind,
    },
    util::ReallySigned,
};

#[cfg(feature = "arm")]
pub mod arm;
#[cfg(feature = "arm64")]
pub mod arm64;
#[cfg(feature = "mips")]
pub mod mips;
#[cfg(feature = "ppc")]
pub mod ppc;
#[cfg(feature = "superh")]
pub mod superh;
#[cfg(feature = "x86")]
pub mod x86;

pub const OPCODE_INVALID: u16 = u16::MAX;
pub const OPCODE_DATA: u16 = u16::MAX - 1;

const SUPPORTED_ENCODINGS: [(&encoding_rs::Encoding, &str); 7] = [
    (encoding_rs::UTF_8, "UTF-8"),
    (encoding_rs::SHIFT_JIS, "Shift JIS"),
    (encoding_rs::UTF_16BE, "UTF-16BE"),
    (encoding_rs::UTF_16LE, "UTF-16LE"),
    (encoding_rs::WINDOWS_1252, "Windows-1252"),
    (encoding_rs::EUC_JP, "EUC-JP"),
    (encoding_rs::BIG5, "Big5"),
];

/// Represents the type of data associated with an instruction
#[derive(PartialEq)]
pub enum DataType {
    Int8,
    Int16,
    Int32,
    Int64,
    Float,
    Double,
    Bytes,
    String,
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DataType::Int8 => "Int8",
            DataType::Int16 => "Int16",
            DataType::Int32 => "Int32",
            DataType::Int64 => "Int64",
            DataType::Float => "Float",
            DataType::Double => "Double",
            DataType::Bytes => "Bytes",
            DataType::String => "String",
        })
    }
}

impl DataType {
    pub fn display_labels(&self, endian: object::Endianness, bytes: &[u8]) -> Vec<String> {
        let mut strs = Vec::new();
        for (literal, label_override, _escape_string) in self.display_literals(endian, bytes) {
            let label = label_override.unwrap_or_else(|| self.to_string());
            strs.push(format!("{label}: {literal:?}"))
        }
        strs
    }

    pub fn display_literals(
        &self,
        endian: object::Endianness,
        bytes: &[u8],
    ) -> Vec<(String, Option<String>, bool)> {
        let mut strs = Vec::new();
        if self.required_len().is_some_and(|l| bytes.len() < l) {
            log::warn!(
                "Failed to display a symbol value for a symbol whose size is too small for instruction referencing it."
            );
            return strs;
        }
        let mut bytes = bytes;
        if self.required_len().is_some_and(|l| bytes.len() > l) {
            // If the symbol's size is larger a single instance of this data type, we take just the
            // bytes necessary for one of them in order to display the first element of the array.
            bytes = &bytes[0..self.required_len().unwrap()];
            // TODO: Attempt to interpret large symbols as arrays of a smaller type and show all
            // elements of the array instead. https://github.com/encounter/objdiff/issues/124
            // However, note that the stride of an array can not always be determined just by the
            // data type guessed by the single instruction accessing it. There can also be arrays of
            // structs that contain multiple elements of different types, so if other elements after
            // the first one were to be displayed in this manner, they may be inaccurate.
        }

        match self {
            DataType::Int8 => {
                let i = i8::from_ne_bytes(bytes.try_into().unwrap());
                strs.push((format!("{i:#x}"), None, false));

                if i < 0 {
                    strs.push((format!("{:#x}", ReallySigned(i)), None, false));
                }
            }
            DataType::Int16 => {
                let i = endian.read_i16_bytes(bytes.try_into().unwrap());
                strs.push((format!("{i:#x}"), None, false));

                if i < 0 {
                    strs.push((format!("{:#x}", ReallySigned(i)), None, false));
                }
            }
            DataType::Int32 => {
                let i = endian.read_i32_bytes(bytes.try_into().unwrap());
                strs.push((format!("{i:#x}"), None, false));

                if i < 0 {
                    strs.push((format!("{:#x}", ReallySigned(i)), None, false));
                }
            }
            DataType::Int64 => {
                let i = endian.read_i64_bytes(bytes.try_into().unwrap());
                strs.push((format!("{i:#x}"), None, false));

                if i < 0 {
                    strs.push((format!("{:#x}", ReallySigned(i)), None, false));
                }
            }
            DataType::Float => {
                let bytes: [u8; 4] = bytes.try_into().unwrap();
                strs.push((
                    format!("{:?}f", match endian {
                        object::Endianness::Little => f32::from_le_bytes(bytes),
                        object::Endianness::Big => f32::from_be_bytes(bytes),
                    }),
                    None,
                    false,
                ));
            }
            DataType::Double => {
                let bytes: [u8; 8] = bytes.try_into().unwrap();
                strs.push((
                    format!("{:?}", match endian {
                        object::Endianness::Little => f64::from_le_bytes(bytes),
                        object::Endianness::Big => f64::from_be_bytes(bytes),
                    }),
                    None,
                    false,
                ));
            }
            DataType::Bytes => {
                strs.push((format!("{bytes:#?}"), None, false));
            }
            DataType::String => {
                if let Some(nul_idx) = bytes.iter().position(|&c| c == b'\0') {
                    let str_bytes = &bytes[..nul_idx];
                    // Special case to display (ASCII) as the label for ASCII-only strings.
                    let (cow, _, had_errors) = encoding_rs::UTF_8.decode(str_bytes);
                    if !had_errors && cow.is_ascii() {
                        strs.push((format!("{cow}"), Some("ASCII".into()), true));
                    }
                    for (encoding, encoding_name) in SUPPORTED_ENCODINGS {
                        let (cow, _, had_errors) = encoding.decode(str_bytes);
                        // Avoid showing ASCII-only strings more than once if the encoding is ASCII-compatible.
                        if !had_errors && (!encoding.is_ascii_compatible() || !cow.is_ascii()) {
                            strs.push((format!("{cow}"), Some(encoding_name.into()), true));
                        }
                    }
                }
            }
        }

        strs
    }

    fn required_len(&self) -> Option<usize> {
        match self {
            DataType::Int8 => Some(1),
            DataType::Int16 => Some(2),
            DataType::Int32 => Some(4),
            DataType::Int64 => Some(8),
            DataType::Float => Some(4),
            DataType::Double => Some(8),
            DataType::Bytes => None,
            DataType::String => None,
        }
    }
}

impl dyn Arch {
    /// Generate a list of instructions references (offset, size, opcode) from the given code.
    ///
    /// See [`scan_instructions_internal`] for more details.
    pub fn scan_instructions(
        &self,
        resolved: ResolvedSymbol,
        diff_config: &DiffObjConfig,
    ) -> Result<Vec<InstructionRef>> {
        let mut result = self.scan_instructions_internal(
            resolved.symbol.address,
            resolved.data,
            resolved.section_index,
            &resolved.section.relocations,
            diff_config,
        )?;

        let function_start = resolved.symbol.address;
        let function_end = function_start + resolved.symbol.size;

        // Remove any branch destinations that are outside the function range
        for ins in result.iter_mut() {
            if let Some(branch_dest) = ins.branch_dest
                && (branch_dest < function_start || branch_dest >= function_end)
            {
                ins.branch_dest = None;
            }
        }

        // Resolve relocation targets within the same function to branch destinations
        let mut ins_iter = result.iter_mut().peekable();
        'outer: for reloc in resolved
            .section
            .relocations
            .iter()
            .skip_while(|r| r.address < function_start)
            .take_while(|r| r.address < function_end)
        {
            let ins = loop {
                let Some(ins) = ins_iter.peek_mut() else {
                    break 'outer;
                };
                if reloc.address < ins.address {
                    continue 'outer;
                }
                let ins = ins_iter.next().unwrap();
                if reloc.address >= ins.address && reloc.address < ins.address + ins.size as u64 {
                    break ins;
                }
            };
            // Clear existing branch destination for instructions with relocations
            ins.branch_dest = None;
            let Some(target) = resolved.obj.symbols.get(reloc.target_symbol) else {
                continue;
            };
            if target.section != Some(resolved.section_index) {
                continue;
            }
            let Some(target_address) = target.address.checked_add_signed(reloc.addend) else {
                continue;
            };
            // If the target address is within the function range, set it as a branch destination
            if target_address >= function_start && target_address < function_end {
                ins.branch_dest = Some(target_address);
            }
        }

        Ok(result)
    }

    /// Parse an instruction to gather its mnemonic and arguments for more detailed comparison.
    ///
    /// This is called only when we need to compare the arguments of an instruction.
    pub fn process_instruction(
        &self,
        resolved: ResolvedInstructionRef,
        diff_config: &DiffObjConfig,
    ) -> Result<ParsedInstruction> {
        let mut mnemonic = None;
        let mut args = Vec::with_capacity(8);
        let mut relocation_emitted = false;
        self.display_instruction(resolved, diff_config, &mut |part| {
            match part {
                InstructionPart::Opcode(m, _) => mnemonic = Some(Cow::Owned(m.into_owned())),
                InstructionPart::Arg(arg) => {
                    if arg == InstructionArg::Reloc {
                        relocation_emitted = true;
                        // If the relocation was resolved to a branch destination, emit that instead.
                        if let Some(dest) = resolved.ins_ref.branch_dest {
                            args.push(InstructionArg::BranchDest(dest));
                            return Ok(());
                        }
                    }
                    args.push(arg.into_static());
                }
                _ => {}
            }
            Ok(())
        })?;
        // If the instruction has a relocation, but we didn't format it in the display, add it to
        // the end of the arguments list.
        if resolved.relocation.is_some() && !relocation_emitted {
            args.push(InstructionArg::Reloc);
        }
        Ok(ParsedInstruction {
            ins_ref: resolved.ins_ref,
            mnemonic: mnemonic.unwrap_or_default(),
            args,
        })
    }
}

pub trait Arch: Any + Debug + Send + Sync {
    /// Finishes arch-specific initialization that must be done after sections have been combined.
    fn post_init(&mut self, _sections: &[Section], _symbols: &[Symbol]) {}

    /// Generate a list of instructions references (offset, size, opcode) from the given code.
    ///
    /// The opcode IDs are used to generate the initial diff. Implementations should do as little
    /// parsing as possible here: just enough to identify the base instruction opcode, size, and
    /// possible branch destination (for visual representation). As needed, instructions are parsed
    /// via `process_instruction` to compare their arguments.
    fn scan_instructions_internal(
        &self,
        address: u64,
        code: &[u8],
        section_index: usize,
        relocations: &[Relocation],
        diff_config: &DiffObjConfig,
    ) -> Result<Vec<InstructionRef>>;

    /// Format an instruction for display.
    ///
    /// Implementations should call the callback for each part of the instruction: usually the
    /// mnemonic and arguments, plus any separators and visual formatting.
    fn display_instruction(
        &self,
        resolved: ResolvedInstructionRef,
        diff_config: &DiffObjConfig,
        cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()>;

    /// Generate a list of fake relocations from the given code that represent pooled data accesses.
    fn generate_pooled_relocations(
        &self,
        _address: u64,
        _code: &[u8],
        _relocations: &[Relocation],
        _symbols: &[Symbol],
    ) -> Vec<Relocation> {
        Vec::new()
    }

    // Perform detailed data flow analysis
    fn data_flow_analysis(
        &self,
        _obj: &Object,
        _symbol: &Symbol,
        _code: &[u8],
        _relocations: &[Relocation],
    ) -> Option<Box<dyn FlowAnalysisResult>> {
        None
    }

    fn relocation_override(
        &self,
        _file: &object::File<'_>,
        _section: &object::Section,
        _address: u64,
        _relocation: &object::Relocation,
    ) -> Result<Option<RelocationOverride>> {
        Ok(None)
    }

    fn reloc_name(&self, _flags: RelocationFlags) -> Option<&'static str> { None }

    fn data_reloc_size(&self, flags: RelocationFlags) -> usize;

    fn symbol_address(&self, address: u64, _kind: SymbolKind) -> u64 { address }

    fn extra_symbol_flags(&self, _symbol: &object::Symbol) -> SymbolFlagSet {
        SymbolFlagSet::default()
    }

    fn guess_data_type(
        &self,
        _resolved: ResolvedInstructionRef,
        _bytes: &[u8],
    ) -> Option<DataType> {
        None
    }

    fn symbol_hover(&self, _obj: &Object, _symbol_index: usize) -> Vec<HoverItem> { Vec::new() }

    fn symbol_context(&self, _obj: &Object, _symbol_index: usize) -> Vec<ContextItem> { Vec::new() }

    fn instruction_hover(
        &self,
        _obj: &Object,
        _resolved: ResolvedInstructionRef,
    ) -> Vec<HoverItem> {
        Vec::new()
    }

    fn instruction_context(
        &self,
        _obj: &Object,
        _resolved: ResolvedInstructionRef,
    ) -> Vec<ContextItem> {
        Vec::new()
    }

    fn infer_function_size(
        &self,
        symbol: &Symbol,
        _section: &Section,
        next_address: u64,
    ) -> Result<u64> {
        Ok(next_address.saturating_sub(symbol.address))
    }
}

pub fn new_arch(object: &object::File, diff_side: DiffSide) -> Result<Box<dyn Arch>> {
    use object::Object as _;
    // Avoid unused warnings on non-mips builds
    let _ = diff_side;

    Ok(match object.architecture() {
        #[cfg(feature = "ppc")]
        object::Architecture::PowerPc | object::Architecture::PowerPc64 => {
            Box::new(ppc::ArchPpc::new(object)?)
        }
        #[cfg(feature = "mips")]
        object::Architecture::Mips => Box::new(mips::ArchMips::new(object, diff_side)?),
        #[cfg(feature = "x86")]
        object::Architecture::I386 | object::Architecture::X86_64 => {
            Box::new(x86::ArchX86::new(object)?)
        }
        #[cfg(feature = "arm")]
        object::Architecture::Arm => Box::new(arm::ArchArm::new(object)?),
        #[cfg(feature = "arm64")]
        object::Architecture::Aarch64 => Box::new(arm64::ArchArm64::new(object)?),
        #[cfg(feature = "superh")]
        object::Architecture::SuperH => Box::new(superh::ArchSuperH::new(object)?),
        arch => bail!("Unsupported architecture: {arch:?}"),
    })
}

#[derive(Debug, Default)]
pub struct ArchDummy {}

impl ArchDummy {
    pub fn new() -> Box<Self> { Box::new(Self {}) }
}

impl Arch for ArchDummy {
    fn scan_instructions_internal(
        &self,
        _address: u64,
        _code: &[u8],
        _section_index: usize,
        _relocations: &[Relocation],
        _diff_config: &DiffObjConfig,
    ) -> Result<Vec<InstructionRef>> {
        Ok(Vec::new())
    }

    fn display_instruction(
        &self,
        _resolved: ResolvedInstructionRef,
        _diff_config: &DiffObjConfig,
        _cb: &mut dyn FnMut(InstructionPart) -> Result<()>,
    ) -> Result<()> {
        Ok(())
    }

    fn data_reloc_size(&self, _flags: RelocationFlags) -> usize { 0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelocationOverrideTarget {
    Keep,
    Skip,
    Symbol(object::SymbolIndex),
    Section(object::SectionIndex),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelocationOverride {
    pub target: RelocationOverrideTarget,
    pub addend: i64,
}

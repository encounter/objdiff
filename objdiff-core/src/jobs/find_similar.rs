use std::{sync::mpsc::Receiver, task::Waker};

use anyhow::Result;
use typed_path::Utf8PlatformPathBuf;

use crate::{
    build::{BuildConfig, run_make},
    diff::{DiffObjConfig, DiffSide, display::InstructionPart, find_similar_code_symbols},
    jobs::{Job, JobContext, JobResult, JobState, start_job, update_status},
    obj::{InstructionArg, read},
};

pub struct FindSimilarConfig {
    /// Path to the source object file to search for.
    pub source_path: Utf8PlatformPathBuf,
    /// Mangled name of the source symbol.
    pub source_symbol_name: String,
    /// 0 = left/target column, 1 = right/base column.
    pub source_column: usize,
    /// All project objects to scan.
    pub objects: Vec<ScanObject>,
    pub diff_config: DiffObjConfig,
    pub build_config: BuildConfig,
    pub build_base: bool,
    pub build_target: bool,
}

pub struct ScanObject {
    pub name: String,
    pub target_path: Option<Utf8PlatformPathBuf>,
    pub base_path: Option<Utf8PlatformPathBuf>,
}

#[derive(Debug, Clone)]
pub struct SimilarFunctionMatch {
    pub symbol_name: String,
    pub demangled_name: Option<String>,
    pub match_percent: f32,
    /// Human-readable identifier: "ObjectName (target)" or "ObjectName (base)".
    pub object_name: String,
}

pub struct FindSimilarResult {
    pub source_symbol_name: String,
    pub source_column: usize,
    pub matches: Vec<SimilarFunctionMatch>,
}

fn run_find_similar(
    context: &JobContext,
    cancel: Receiver<()>,
    config: FindSimilarConfig,
) -> Result<Box<FindSimilarResult>> {
    let diff_side = if config.source_column == 0 { DiffSide::Target } else { DiffSide::Base };
    let source_obj = read::read(config.source_path.as_ref(), &config.diff_config, diff_side)?;
    let source_symbol_idx =
        source_obj.symbol_by_name(&config.source_symbol_name).ok_or_else(|| {
            anyhow::anyhow!("Source symbol '{}' not found", config.source_symbol_name)
        })?;

    // Print the instructions of the source symbol to the console.
    'print: {
        let symbol = &source_obj.symbols[source_symbol_idx];
        let Some(section_index) = symbol.section else { break 'print };
        let section = &source_obj.sections[section_index];
        let Some(data) = section.data_range(symbol.address, symbol.size as usize) else {
            break 'print;
        };
        let Ok(instructions) = source_obj.arch.scan_instructions(
            crate::obj::ResolvedSymbol {
                obj: &source_obj,
                symbol_index: source_symbol_idx,
                symbol,
                section_index,
                section,
                data,
            },
            &config.diff_config,
        ) else {
            break 'print;
        };
        log::info!(
            "find_similar: source symbol '{}' — {} instructions",
            config.source_symbol_name,
            instructions.len()
        );
        for ins_ref in &instructions {
            let Some(resolved) = source_obj.resolve_instruction_ref(source_symbol_idx, *ins_ref)
            else {
                continue;
            };
            let mut text = format!("{:#010x}  ", ins_ref.address);
            let _ =
                source_obj.arch.display_instruction(resolved, &config.diff_config, &mut |part| {
                    match part {
                        InstructionPart::Basic(s) | InstructionPart::Opcode(s, _) => {
                            text.push_str(&s)
                        }
                        InstructionPart::Arg(InstructionArg::Value(v)) => {
                            text.push_str(&v.to_string())
                        }
                        InstructionPart::Arg(InstructionArg::BranchDest(addr)) => {
                            text.push_str(&format!("{addr:#x}"))
                        }
                        InstructionPart::Arg(InstructionArg::Reloc) => {
                            if let Some(reloc) = resolved.relocation {
                                let sym = &source_obj.symbols[reloc.relocation.target_symbol];
                                text.push_str(sym.demangled_name.as_deref().unwrap_or(&sym.name));
                                if reloc.relocation.addend != 0 {
                                    text.push_str(&format!("+{:#x}", reloc.relocation.addend));
                                }
                            } else {
                                text.push_str("<reloc>");
                            }
                        }
                        InstructionPart::Separator => text.push_str(", "),
                    }
                    Ok(())
                });
            log::info!("{text}");
        }
    }

    let total = config.objects.len() as u32;
    let mut all_matches = Vec::new();

    for (idx, scan_obj) in config.objects.iter().enumerate() {
        update_status(context, format!("Scanning {}", scan_obj.name), idx as u32, total, &cancel)?;

        let project_dir = config.build_config.project_dir.as_deref();

        for side in [DiffSide::Target, DiffSide::Base] {
            let (path, should_build) = match side {
                DiffSide::Target => (scan_obj.target_path.as_ref(), config.build_target),
                DiffSide::Base => (scan_obj.base_path.as_ref(), config.build_base),
            };
            let Some(path) = path else { continue };

            if should_build
                && let Some(project_dir) = project_dir
                && let Ok(rel_path) = path.strip_prefix(project_dir)
            {
                run_make(&config.build_config, rel_path.with_unix_encoding().as_ref());
            }

            let Ok(obj) = read::read(path.as_ref(), &config.diff_config, side) else { continue };
            let similar = find_similar_code_symbols(
                &source_obj,
                source_symbol_idx,
                &obj,
                &config.diff_config,
            );
            let side_label = if side == DiffSide::Target { "target" } else { "base" };
            for sim in similar {
                let symbol = &obj.symbols[sim.symbol_idx];
                all_matches.push(SimilarFunctionMatch {
                    symbol_name: symbol.name.clone(),
                    demangled_name: symbol.demangled_name.clone(),
                    match_percent: sim.match_percent,
                    object_name: format!("{} ({})", scan_obj.name, side_label),
                });
            }
        }
    }

    all_matches.sort_by(|a, b| {
        b.match_percent.partial_cmp(&a.match_percent).unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(Box::new(FindSimilarResult {
        source_symbol_name: config.source_symbol_name,
        source_column: config.source_column,
        matches: all_matches,
    }))
}

pub fn start_find_similar(waker: Waker, config: FindSimilarConfig) -> JobState {
    start_job(waker, "Find similar functions", Job::FindSimilar, move |context, cancel| {
        run_find_similar(&context, cancel, config).map(|r| JobResult::FindSimilar(Some(r)))
    })
}

use std::{
    collections::HashSet,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{bail, Context, Result};
use argp::FromArgs;
use objdiff_core::{
    config::ProjectObject,
    diff, obj,
    obj::{ObjSectionKind, ObjSymbolFlags},
};
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};

#[derive(FromArgs, PartialEq, Debug)]
/// Generate a report from a project.
#[argp(subcommand, name = "report")]
pub struct Args {
    #[argp(option, short = 'p')]
    /// Project directory
    project: Option<PathBuf>,
    #[argp(option, short = 'o')]
    /// Output JSON file
    output: Option<PathBuf>,
    #[argp(switch, short = 'd')]
    /// Deduplicate global and weak symbols
    deduplicate: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct Report {
    fuzzy_match_percent: f32,
    total_size: u64,
    matched_size: u64,
    matched_size_percent: f32,
    total_functions: u32,
    matched_functions: u32,
    matched_functions_percent: f32,
    units: Vec<ReportUnit>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct ReportUnit {
    name: String,
    match_percent: f32,
    total_size: u64,
    matched_size: u64,
    total_functions: u32,
    matched_functions: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    complete: Option<bool>,
    functions: Vec<ReportFunction>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct ReportFunction {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    demangled_name: Option<String>,
    size: u64,
    match_percent: f32,
}

pub fn run(args: Args) -> Result<()> {
    let project_dir = args.project.as_deref().unwrap_or_else(|| Path::new("."));
    log::info!("Loading project {}", project_dir.display());

    let config = objdiff_core::config::try_project_config(project_dir);
    let Some((Ok(mut project), _)) = config else {
        bail!("No project configuration found");
    };
    log::info!(
        "Generating report for {} units (using {} threads)",
        project.objects.len(),
        if args.deduplicate { 1 } else { rayon::current_num_threads() }
    );

    let start = Instant::now();
    let mut report = Report::default();
    let mut existing_functions: HashSet<String> = HashSet::new();
    if args.deduplicate {
        // If deduplicating, we need to run single-threaded
        for object in &mut project.objects {
            if let Some(unit) = report_object(
                object,
                project_dir,
                project.target_dir.as_deref(),
                project.base_dir.as_deref(),
                Some(&mut existing_functions),
            )? {
                report.units.push(unit);
            }
        }
    } else {
        let units = project
            .objects
            .par_iter_mut()
            .map(|object| {
                report_object(
                    object,
                    project_dir,
                    project.target_dir.as_deref(),
                    project.base_dir.as_deref(),
                    None,
                )
            })
            .collect::<Result<Vec<Option<ReportUnit>>>>()?;
        report.units = units.into_iter().flatten().collect();
    }
    for unit in &report.units {
        report.fuzzy_match_percent += unit.match_percent * unit.total_size as f32;
        report.total_size += unit.total_size;
        report.matched_size += unit.matched_size;
        report.total_functions += unit.total_functions;
        report.matched_functions += unit.matched_functions;
    }
    if report.total_size == 0 {
        report.fuzzy_match_percent = 100.0;
    } else {
        report.fuzzy_match_percent /= report.total_size as f32;
    }
    report.matched_size_percent = if report.total_size == 0 {
        100.0
    } else {
        report.matched_size as f32 / report.total_size as f32 * 100.0
    };
    report.matched_functions_percent = if report.total_functions == 0 {
        100.0
    } else {
        report.matched_functions as f32 / report.total_functions as f32 * 100.0
    };
    let duration = start.elapsed();
    log::info!("Report generated in {}.{:03}s", duration.as_secs(), duration.subsec_millis());
    if let Some(output) = &args.output {
        log::info!("Writing to {}", output.display());
        let mut output = BufWriter::new(
            File::create(output)
                .with_context(|| format!("Failed to create file {}", output.display()))?,
        );
        serde_json::to_writer_pretty(&mut output, &report)?;
        output.flush()?;
    } else {
        serde_json::to_writer_pretty(std::io::stdout(), &report)?;
    }
    Ok(())
}

fn report_object(
    object: &mut ProjectObject,
    project_dir: &Path,
    target_dir: Option<&Path>,
    base_dir: Option<&Path>,
    mut existing_functions: Option<&mut HashSet<String>>,
) -> Result<Option<ReportUnit>> {
    object.resolve_paths(project_dir, target_dir, base_dir);
    match (&object.target_path, &object.base_path) {
        (None, Some(_)) if object.complete != Some(true) => {
            log::warn!("Skipping object without target: {}", object.name());
            return Ok(None);
        }
        (None, None) => {
            log::warn!("Skipping object without target or base: {}", object.name());
            return Ok(None);
        }
        _ => {}
    }
    // println!("Checking {}", object.name());
    let mut target = object
        .target_path
        .as_ref()
        .map(|p| obj::elf::read(p).with_context(|| format!("Failed to open {}", p.display())))
        .transpose()?;
    let mut base = object
        .base_path
        .as_ref()
        .map(|p| obj::elf::read(p).with_context(|| format!("Failed to open {}", p.display())))
        .transpose()?;
    let config = diff::DiffObjConfig { relax_reloc_diffs: true };
    diff::diff_objs(&config, target.as_mut(), base.as_mut())?;
    let mut unit = ReportUnit { name: object.name().to_string(), ..Default::default() };
    let obj = target.as_ref().or(base.as_ref()).unwrap();
    for section in &obj.sections {
        if section.kind != ObjSectionKind::Code {
            continue;
        }
        for symbol in &section.symbols {
            if symbol.size == 0 {
                continue;
            }
            if let Some(existing_functions) = &mut existing_functions {
                if (symbol.flags.0.contains(ObjSymbolFlags::Global)
                    || symbol.flags.0.contains(ObjSymbolFlags::Weak))
                    && !existing_functions.insert(symbol.name.clone())
                {
                    continue;
                }
            }
            let match_percent = symbol.match_percent.unwrap_or_else(|| {
                // Support cases where we don't have a target object,
                // assume complete means 100% match
                if object.complete == Some(true) {
                    100.0
                } else {
                    0.0
                }
            });
            unit.match_percent += match_percent * symbol.size as f32;
            unit.total_size += symbol.size;
            if match_percent == 100.0 {
                unit.matched_size += symbol.size;
            }
            unit.functions.push(ReportFunction {
                name: symbol.name.clone(),
                demangled_name: symbol.demangled_name.clone(),
                size: symbol.size,
                match_percent,
            });
            if match_percent == 100.0 {
                unit.matched_functions += 1;
            }
            unit.total_functions += 1;
        }
    }
    if unit.total_size == 0 {
        unit.match_percent = 100.0;
    } else {
        unit.match_percent /= unit.total_size as f32;
    }
    Ok(Some(unit))
}

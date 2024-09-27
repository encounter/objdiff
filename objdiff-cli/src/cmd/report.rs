use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::{bail, Context, Result};
use argp::FromArgs;
use objdiff_core::{
    bindings::report::{
        ChangeItem, ChangeItemInfo, ChangeUnit, Changes, ChangesInput, Measures, Report,
        ReportCategory, ReportItem, ReportItemMetadata, ReportUnit, ReportUnitMetadata,
        REPORT_VERSION,
    },
    config::ProjectObject,
    diff, obj,
    obj::{ObjSectionKind, ObjSymbolFlags},
};
use prost::Message;
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};
use tracing::{info, warn};

use crate::util::output::{write_output, OutputFormat};

#[derive(FromArgs, PartialEq, Debug)]
/// Generate a progress report for a project.
#[argp(subcommand, name = "report")]
pub struct Args {
    #[argp(subcommand)]
    command: SubCommand,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argp(subcommand)]
pub enum SubCommand {
    Generate(GenerateArgs),
    Changes(ChangesArgs),
}

#[derive(FromArgs, PartialEq, Debug)]
/// Generate a progress report for a project.
#[argp(subcommand, name = "generate")]
pub struct GenerateArgs {
    #[argp(option, short = 'p')]
    /// Project directory
    project: Option<PathBuf>,
    #[argp(option, short = 'o')]
    /// Output file
    output: Option<PathBuf>,
    #[argp(switch, short = 'd')]
    /// Deduplicate global and weak symbols (runs single-threaded)
    deduplicate: bool,
    #[argp(option, short = 'f')]
    /// Output format (json, json-pretty, proto) (default: json)
    format: Option<String>,
}

#[derive(FromArgs, PartialEq, Debug)]
/// List any changes from a previous report.
#[argp(subcommand, name = "changes")]
pub struct ChangesArgs {
    #[argp(positional)]
    /// Previous report file
    previous: PathBuf,
    #[argp(positional)]
    /// Current report file
    current: PathBuf,
    #[argp(option, short = 'o')]
    /// Output file
    output: Option<PathBuf>,
    #[argp(option, short = 'f')]
    /// Output format (json, json-pretty, proto) (default: json)
    format: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Generate(args) => generate(args),
        SubCommand::Changes(args) => changes(args),
    }
}

fn generate(args: GenerateArgs) -> Result<()> {
    let output_format = OutputFormat::from_option(args.format.as_deref())?;
    let project_dir = args.project.as_deref().unwrap_or_else(|| Path::new("."));
    info!("Loading project {}", project_dir.display());

    let mut project = match objdiff_core::config::try_project_config(project_dir) {
        Some((Ok(config), _)) => config,
        Some((Err(err), _)) => bail!("Failed to load project configuration: {}", err),
        None => bail!("No project configuration found"),
    };
    info!(
        "Generating report for {} units (using {} threads)",
        project.objects.len(),
        if args.deduplicate { 1 } else { rayon::current_num_threads() }
    );

    let start = Instant::now();
    let mut units = vec![];
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
                units.push(unit);
            }
        }
    } else {
        let vec = project
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
        units = vec.into_iter().flatten().collect();
    }
    let measures = units.iter().flat_map(|u| u.measures.into_iter()).collect();
    let mut categories = Vec::new();
    for category in &project.progress_categories {
        categories.push(ReportCategory {
            id: category.id.clone(),
            name: category.name.clone(),
            measures: Some(Default::default()),
        });
    }
    let mut report =
        Report { measures: Some(measures), units, version: REPORT_VERSION, categories };
    report.calculate_progress_categories();
    let duration = start.elapsed();
    info!("Report generated in {}.{:03}s", duration.as_secs(), duration.subsec_millis());
    write_output(&report, args.output.as_deref(), output_format)?;
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
        (None, Some(_)) if !object.complete().unwrap_or(false) => {
            warn!("Skipping object without target: {}", object.name());
            return Ok(None);
        }
        (None, None) => {
            warn!("Skipping object without target or base: {}", object.name());
            return Ok(None);
        }
        _ => {}
    }
    let config = diff::DiffObjConfig { relax_reloc_diffs: true, ..Default::default() };
    let target = object
        .target_path
        .as_ref()
        .map(|p| {
            obj::read::read(p, &config).with_context(|| format!("Failed to open {}", p.display()))
        })
        .transpose()?;
    let base = object
        .base_path
        .as_ref()
        .map(|p| {
            obj::read::read(p, &config).with_context(|| format!("Failed to open {}", p.display()))
        })
        .transpose()?;
    let result = diff::diff_objs(&config, target.as_ref(), base.as_ref(), None)?;

    let metadata = ReportUnitMetadata {
        complete: object.complete(),
        module_name: target
            .as_ref()
            .and_then(|o| o.split_meta.as_ref())
            .and_then(|m| m.module_name.clone()),
        module_id: target.as_ref().and_then(|o| o.split_meta.as_ref()).and_then(|m| m.module_id),
        source_path: object.metadata.as_ref().and_then(|m| m.source_path.clone()),
        progress_categories: object
            .metadata
            .as_ref()
            .and_then(|m| m.progress_categories.clone())
            .unwrap_or_default(),
        auto_generated: object.metadata.as_ref().and_then(|m| m.auto_generated),
    };
    let mut measures = Measures::default();
    let mut sections = vec![];
    let mut functions = vec![];

    let obj = target.as_ref().or(base.as_ref()).unwrap();
    let obj_diff = result.left.as_ref().or(result.right.as_ref()).unwrap();
    for (section, section_diff) in obj.sections.iter().zip(&obj_diff.sections) {
        let section_match_percent = section_diff.match_percent.unwrap_or_else(|| {
            // Support cases where we don't have a target object,
            // assume complete means 100% match
            if object.complete().unwrap_or(false) {
                100.0
            } else {
                0.0
            }
        });
        sections.push(ReportItem {
            name: section.name.clone(),
            fuzzy_match_percent: section_match_percent,
            size: section.size,
            metadata: Some(ReportItemMetadata {
                demangled_name: None,
                virtual_address: section.virtual_address,
            }),
        });

        match section.kind {
            ObjSectionKind::Data | ObjSectionKind::Bss => {
                measures.total_data += section.size;
                if section_match_percent == 100.0 {
                    measures.matched_data += section.size;
                }
                continue;
            }
            ObjSectionKind::Code => (),
        }

        for (symbol, symbol_diff) in section.symbols.iter().zip(&section_diff.symbols) {
            if symbol.size == 0 || symbol.flags.0.contains(ObjSymbolFlags::Hidden) {
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
            let match_percent = symbol_diff.match_percent.unwrap_or_else(|| {
                // Support cases where we don't have a target object,
                // assume complete means 100% match
                if object.complete().unwrap_or(false) {
                    100.0
                } else {
                    0.0
                }
            });
            measures.fuzzy_match_percent += match_percent * symbol.size as f32;
            measures.total_code += symbol.size;
            if match_percent == 100.0 {
                measures.matched_code += symbol.size;
            }
            functions.push(ReportItem {
                name: symbol.name.clone(),
                size: symbol.size,
                fuzzy_match_percent: match_percent,
                metadata: Some(ReportItemMetadata {
                    demangled_name: symbol.demangled_name.clone(),
                    virtual_address: symbol.virtual_address,
                }),
            });
            if match_percent == 100.0 {
                measures.matched_functions += 1;
            }
            measures.total_functions += 1;
        }
    }
    if metadata.complete.unwrap_or(false) {
        measures.complete_code = measures.total_code;
        measures.complete_data = measures.total_data;
    }
    measures.calc_fuzzy_match_percent();
    measures.calc_matched_percent();
    Ok(Some(ReportUnit {
        name: object.name().to_string(),
        measures: Some(measures),
        sections,
        functions,
        metadata: Some(metadata),
    }))
}

fn changes(args: ChangesArgs) -> Result<()> {
    let output_format = OutputFormat::from_option(args.format.as_deref())?;
    let (previous, current) = if args.previous == Path::new("-") && args.current == Path::new("-") {
        // Special case for comparing two reports from stdin
        let mut data = vec![];
        std::io::stdin().read_to_end(&mut data)?;
        let input = ChangesInput::decode(data.as_slice())?;
        (input.from.unwrap(), input.to.unwrap())
    } else {
        let previous = read_report(&args.previous)?;
        let current = read_report(&args.current)?;
        (previous, current)
    };
    let mut changes = Changes { from: previous.measures, to: current.measures, units: vec![] };
    for prev_unit in &previous.units {
        let curr_unit = current.units.iter().find(|u| u.name == prev_unit.name);
        let sections = process_items(prev_unit, curr_unit, |u| &u.sections);
        let functions = process_items(prev_unit, curr_unit, |u| &u.functions);

        let prev_measures = prev_unit.measures;
        let curr_measures = curr_unit.and_then(|u| u.measures);
        if !functions.is_empty() || prev_measures != curr_measures {
            changes.units.push(ChangeUnit {
                name: prev_unit.name.clone(),
                from: prev_measures,
                to: curr_measures,
                sections,
                functions,
                metadata: curr_unit
                    .as_ref()
                    .and_then(|u| u.metadata.clone())
                    .or_else(|| prev_unit.metadata.clone()),
            });
        }
    }
    for curr_unit in &current.units {
        if !previous.units.iter().any(|u| u.name == curr_unit.name) {
            changes.units.push(ChangeUnit {
                name: curr_unit.name.clone(),
                from: None,
                to: curr_unit.measures,
                sections: process_new_items(&curr_unit.sections),
                functions: process_new_items(&curr_unit.functions),
                metadata: curr_unit.metadata.clone(),
            });
        }
    }
    write_output(&changes, args.output.as_deref(), output_format)?;
    Ok(())
}

fn process_items<F: Fn(&ReportUnit) -> &Vec<ReportItem>>(
    prev_unit: &ReportUnit,
    curr_unit: Option<&ReportUnit>,
    getter: F,
) -> Vec<ChangeItem> {
    let prev_items = getter(prev_unit);
    let mut items = vec![];
    if let Some(curr_unit) = curr_unit {
        let curr_items = getter(curr_unit);
        for prev_func in prev_items {
            let prev_func_info = ChangeItemInfo::from(prev_func);
            let curr_func = curr_items.iter().find(|f| f.name == prev_func.name);
            let curr_func_info = curr_func.map(ChangeItemInfo::from);
            if let Some(curr_func_info) = curr_func_info {
                if prev_func_info != curr_func_info {
                    items.push(ChangeItem {
                        name: prev_func.name.clone(),
                        from: Some(prev_func_info),
                        to: Some(curr_func_info),
                        metadata: curr_func.as_ref().unwrap().metadata.clone(),
                    });
                }
            } else {
                items.push(ChangeItem {
                    name: prev_func.name.clone(),
                    from: Some(prev_func_info),
                    to: None,
                    metadata: prev_func.metadata.clone(),
                });
            }
        }
        for curr_func in curr_items {
            if !prev_items.iter().any(|f| f.name == curr_func.name) {
                items.push(ChangeItem {
                    name: curr_func.name.clone(),
                    from: None,
                    to: Some(ChangeItemInfo::from(curr_func)),
                    metadata: curr_func.metadata.clone(),
                });
            }
        }
    } else {
        for prev_func in prev_items {
            items.push(ChangeItem {
                name: prev_func.name.clone(),
                from: Some(ChangeItemInfo::from(prev_func)),
                to: None,
                metadata: prev_func.metadata.clone(),
            });
        }
    }
    items
}

fn process_new_items(items: &[ReportItem]) -> Vec<ChangeItem> {
    items
        .iter()
        .map(|item| ChangeItem {
            name: item.name.clone(),
            from: None,
            to: Some(ChangeItemInfo::from(item)),
            metadata: item.metadata.clone(),
        })
        .collect()
}

fn read_report(path: &Path) -> Result<Report> {
    if path == Path::new("-") {
        let mut data = vec![];
        std::io::stdin().read_to_end(&mut data)?;
        return Report::parse(&data).with_context(|| "Failed to load report from stdin");
    }
    let file = File::open(path).with_context(|| format!("Failed to open {}", path.display()))?;
    let mmap = unsafe { memmap2::Mmap::map(&file) }
        .with_context(|| format!("Failed to map {}", path.display()))?;
    Report::parse(mmap.as_ref())
        .with_context(|| format!("Failed to load report {}", path.display()))
}

use std::{collections::HashSet, fs::File, io::Read, time::Instant};

use anyhow::{Context, Result, bail};
use argp::FromArgs;
use objdiff_core::{
    bindings::report::{
        ChangeItem, ChangeItemInfo, ChangeUnit, Changes, ChangesInput, Measures, REPORT_VERSION,
        Report, ReportCategory, ReportItem, ReportItemMetadata, ReportUnit, ReportUnitMetadata,
    },
    config::path::platform_path,
    diff, obj,
    obj::{SectionKind, SymbolFlag},
};
use prost::Message;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use tracing::{info, warn};
use typed_path::{Utf8PlatformPath, Utf8PlatformPathBuf};

use crate::{
    cmd::{apply_config_args, diff::ObjectConfig},
    util::output::{OutputFormat, write_output},
};

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
    #[argp(option, short = 'p', from_str_fn(platform_path))]
    /// Project directory
    project: Option<Utf8PlatformPathBuf>,
    #[argp(option, short = 'o', from_str_fn(platform_path))]
    /// Output file
    output: Option<Utf8PlatformPathBuf>,
    #[argp(switch, short = 'd')]
    /// Deduplicate global and weak symbols (runs single-threaded)
    deduplicate: bool,
    #[argp(option, short = 'f')]
    /// Output format (json, json-pretty, proto) (default: json)
    format: Option<String>,
    #[argp(option, short = 'c')]
    /// Configuration property (key=value)
    config: Vec<String>,
}

#[derive(FromArgs, PartialEq, Debug)]
/// List any changes from a previous report.
#[argp(subcommand, name = "changes")]
pub struct ChangesArgs {
    #[argp(positional, from_str_fn(platform_path))]
    /// Previous report file
    previous: Utf8PlatformPathBuf,
    #[argp(positional, from_str_fn(platform_path))]
    /// Current report file
    current: Utf8PlatformPathBuf,
    #[argp(option, short = 'o', from_str_fn(platform_path))]
    /// Output file
    output: Option<Utf8PlatformPathBuf>,
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
    let mut diff_config = diff::DiffObjConfig {
        function_reloc_diffs: diff::FunctionRelocDiffs::None,
        ..Default::default()
    };
    apply_config_args(&mut diff_config, &args.config)?;

    let output_format = OutputFormat::from_option(args.format.as_deref())?;
    let project_dir = args.project.as_deref().unwrap_or_else(|| Utf8PlatformPath::new("."));
    info!("Loading project {}", project_dir);

    let project = match objdiff_core::config::try_project_config(project_dir.as_ref()) {
        Some((Ok(config), _)) => config,
        Some((Err(err), _)) => bail!("Failed to load project configuration: {}", err),
        None => bail!("No project configuration found"),
    };
    info!(
        "Generating report for {} units (using {} threads)",
        project.units().len(),
        if args.deduplicate { 1 } else { rayon::current_num_threads() }
    );

    let target_obj_dir =
        project.target_dir.as_ref().map(|p| project_dir.join(p.with_platform_encoding()));
    let base_obj_dir =
        project.base_dir.as_ref().map(|p| project_dir.join(p.with_platform_encoding()));
    let objects = project
        .units
        .iter()
        .flatten()
        .map(|o| {
            ObjectConfig::new(o, project_dir, target_obj_dir.as_deref(), base_obj_dir.as_deref())
        })
        .collect::<Vec<_>>();

    let start = Instant::now();
    let mut units = vec![];
    let mut existing_functions: HashSet<String> = HashSet::new();
    if args.deduplicate {
        // If deduplicating, we need to run single-threaded
        for object in &objects {
            if let Some(unit) = report_object(object, &diff_config, Some(&mut existing_functions))?
            {
                units.push(unit);
            }
        }
    } else {
        let vec = objects
            .par_iter()
            .map(|object| report_object(object, &diff_config, None))
            .collect::<Result<Vec<Option<ReportUnit>>>>()?;
        units = vec.into_iter().flatten().collect();
    }
    let measures = units.iter().flat_map(|u| u.measures.into_iter()).collect();
    let mut categories = Vec::new();
    for category in project.progress_categories() {
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
    object: &ObjectConfig,
    diff_config: &diff::DiffObjConfig,
    mut existing_functions: Option<&mut HashSet<String>>,
) -> Result<Option<ReportUnit>> {
    match (&object.target_path, &object.base_path) {
        (None, Some(_)) if !object.complete.unwrap_or(false) => {
            warn!("Skipping object without target: {}", object.name);
            return Ok(None);
        }
        (None, None) => {
            warn!("Skipping object without target or base: {}", object.name);
            return Ok(None);
        }
        _ => {}
    }
    let mapping_config = diff::MappingConfig::default();
    let target = object
        .target_path
        .as_ref()
        .map(|p| {
            obj::read::read(p.as_ref(), diff_config)
                .with_context(|| format!("Failed to open {}", p))
        })
        .transpose()?;
    let base = object
        .base_path
        .as_ref()
        .map(|p| {
            obj::read::read(p.as_ref(), diff_config)
                .with_context(|| format!("Failed to open {}", p))
        })
        .transpose()?;
    let result =
        diff::diff_objs(target.as_ref(), base.as_ref(), None, diff_config, &mapping_config)?;

    let metadata = ReportUnitMetadata {
        complete: object.metadata.complete,
        module_name: target
            .as_ref()
            .and_then(|o| o.split_meta.as_ref())
            .and_then(|m| m.module_name.clone()),
        module_id: target.as_ref().and_then(|o| o.split_meta.as_ref()).and_then(|m| m.module_id),
        source_path: object.metadata.source_path.as_ref().map(|p| p.to_string()),
        progress_categories: object.metadata.progress_categories.clone().unwrap_or_default(),
        auto_generated: object.metadata.auto_generated,
    };
    let mut measures = Measures { total_units: 1, ..Default::default() };
    let mut sections = vec![];
    let mut functions = vec![];

    let obj = target.as_ref().or(base.as_ref()).unwrap();
    let obj_diff = result.left.as_ref().or(result.right.as_ref()).unwrap();
    for ((section_idx, section), section_diff) in
        obj.sections.iter().enumerate().zip(&obj_diff.sections)
    {
        if section.kind == SectionKind::Unknown {
            continue;
        }
        let section_match_percent = section_diff.match_percent.unwrap_or_else(|| {
            // Support cases where we don't have a target object,
            // assume complete means 100% match
            if object.complete.unwrap_or(false) { 100.0 } else { 0.0 }
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
            SectionKind::Data | SectionKind::Bss => {
                measures.total_data += section.size;
                if section_match_percent == 100.0 {
                    measures.matched_data += section.size;
                }
                continue;
            }
            _ => {}
        }

        for (symbol, symbol_diff) in obj.symbols.iter().zip(&obj_diff.symbols) {
            if symbol.section != Some(section_idx)
                || symbol.size == 0
                || symbol.flags.contains(SymbolFlag::Hidden)
                || symbol.flags.contains(SymbolFlag::Ignored)
            {
                continue;
            }
            if let Some(existing_functions) = &mut existing_functions {
                if (symbol.flags.contains(SymbolFlag::Global)
                    || symbol.flags.contains(SymbolFlag::Weak))
                    && !existing_functions.insert(symbol.name.clone())
                {
                    continue;
                }
            }
            let match_percent = symbol_diff.match_percent.unwrap_or_else(|| {
                // Support cases where we don't have a target object,
                // assume complete means 100% match
                if object.complete.unwrap_or(false) { 100.0 } else { 0.0 }
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
        measures.complete_units = 1;
    }
    measures.calc_fuzzy_match_percent();
    measures.calc_matched_percent();
    Ok(Some(ReportUnit {
        name: object.name.clone(),
        measures: Some(measures),
        sections,
        functions,
        metadata: Some(metadata),
    }))
}

fn changes(args: ChangesArgs) -> Result<()> {
    let output_format = OutputFormat::from_option(args.format.as_deref())?;
    let (previous, current) = if args.previous == "-" && args.current == "-" {
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

fn read_report(path: &Utf8PlatformPath) -> Result<Report> {
    if path == Utf8PlatformPath::new("-") {
        let mut data = vec![];
        std::io::stdin().read_to_end(&mut data)?;
        return Report::parse(&data).with_context(|| "Failed to load report from stdin");
    }
    let file = File::open(path).with_context(|| format!("Failed to open {}", path))?;
    let mmap =
        unsafe { memmap2::Mmap::map(&file) }.with_context(|| format!("Failed to map {}", path))?;
    Report::parse(mmap.as_ref()).with_context(|| format!("Failed to load report {}", path))
}

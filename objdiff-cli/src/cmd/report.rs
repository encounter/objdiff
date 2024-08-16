use std::{
    collections::HashSet,
    fs::File,
    io::{BufWriter, Read, Write},
    ops::DerefMut,
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
use prost::Message;
use rayon::iter::{IntoParallelRefMutIterator, ParallelIterator};
use tracing::{info, warn};

use crate::util::report::{
    ChangeInfo, ChangeItem, ChangeItemInfo, ChangeUnit, Changes, ChangesInput, Report, ReportItem,
    ReportUnit,
};

#[derive(FromArgs, PartialEq, Debug)]
/// Commands for processing NVIDIA Shield TV alf files.
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
/// Generate a report from a project.
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
    /// Output format (json or proto, default json)
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
    /// Output format (json or proto, default json)
    format: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Generate(args) => generate(args),
        SubCommand::Changes(args) => changes(args),
    }
}

enum OutputFormat {
    Json,
    Proto,
}

impl OutputFormat {
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "json" => Ok(Self::Json),
            "binpb" | "proto" | "protobuf" => Ok(Self::Proto),
            _ => bail!("Invalid output format: {}", s),
        }
    }
}

fn generate(args: GenerateArgs) -> Result<()> {
    let output_format = if let Some(format) = &args.format {
        OutputFormat::from_str(format)?
    } else {
        OutputFormat::Json
    };

    let project_dir = args.project.as_deref().unwrap_or_else(|| Path::new("."));
    info!("Loading project {}", project_dir.display());

    let config = objdiff_core::config::try_project_config(project_dir);
    let Some((Ok(mut project), _)) = config else {
        bail!("No project configuration found");
    };
    info!(
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
        report.fuzzy_match_percent += unit.fuzzy_match_percent * unit.total_code as f32;
        report.total_code += unit.total_code;
        report.matched_code += unit.matched_code;
        report.total_data += unit.total_data;
        report.matched_data += unit.matched_data;
        report.total_functions += unit.total_functions;
        report.matched_functions += unit.matched_functions;
    }
    if report.total_code == 0 {
        report.fuzzy_match_percent = 100.0;
    } else {
        report.fuzzy_match_percent /= report.total_code as f32;
    }

    report.matched_code_percent = if report.total_code == 0 {
        100.0
    } else {
        report.matched_code as f32 / report.total_code as f32 * 100.0
    };
    report.matched_data_percent = if report.total_data == 0 {
        100.0
    } else {
        report.matched_data as f32 / report.total_data as f32 * 100.0
    };
    report.matched_functions_percent = if report.total_functions == 0 {
        100.0
    } else {
        report.matched_functions as f32 / report.total_functions as f32 * 100.0
    };
    let duration = start.elapsed();
    info!("Report generated in {}.{:03}s", duration.as_secs(), duration.subsec_millis());
    write_output(&report, args.output.as_deref(), output_format)?;
    Ok(())
}

fn write_output<T>(input: &T, output: Option<&Path>, format: OutputFormat) -> Result<()>
where T: serde::Serialize + prost::Message {
    if let Some(output) = output {
        info!("Writing to {}", output.display());
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(output)
            .with_context(|| format!("Failed to create file {}", output.display()))?;
        match format {
            OutputFormat::Json => {
                let mut output = BufWriter::new(file);
                serde_json::to_writer_pretty(&mut output, input)
                    .context("Failed to write output file")?;
                output.flush().context("Failed to flush output file")?;
            }
            OutputFormat::Proto => {
                file.set_len(input.encoded_len() as u64)?;
                let map =
                    unsafe { memmap2::Mmap::map(&file) }.context("Failed to map output file")?;
                let mut output = map.make_mut().context("Failed to remap output file")?;
                input.encode(&mut output.deref_mut()).context("Failed to encode output")?;
            }
        }
    } else {
        match format {
            OutputFormat::Json => {
                serde_json::to_writer_pretty(std::io::stdout(), input)?;
            }
            OutputFormat::Proto => {
                std::io::stdout().write_all(&input.encode_to_vec())?;
            }
        }
    };
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
    let mut unit = ReportUnit {
        name: object.name().to_string(),
        complete: object.complete,
        module_name: target
            .as_ref()
            .and_then(|o| o.split_meta.as_ref())
            .and_then(|m| m.module_name.clone()),
        module_id: target.as_ref().and_then(|o| o.split_meta.as_ref()).and_then(|m| m.module_id),
        ..Default::default()
    };
    let obj = target.as_ref().or(base.as_ref()).unwrap();

    let obj_diff = result.left.as_ref().or(result.right.as_ref()).unwrap();
    for (section, section_diff) in obj.sections.iter().zip(&obj_diff.sections) {
        let section_match_percent = section_diff.match_percent.unwrap_or_else(|| {
            // Support cases where we don't have a target object,
            // assume complete means 100% match
            if object.complete == Some(true) {
                100.0
            } else {
                0.0
            }
        });
        unit.sections.push(ReportItem {
            name: section.name.clone(),
            demangled_name: None,
            fuzzy_match_percent: section_match_percent,
            size: section.size,
            address: section.virtual_address,
        });

        match section.kind {
            ObjSectionKind::Data | ObjSectionKind::Bss => {
                unit.total_data += section.size;
                if section_match_percent == 100.0 {
                    unit.matched_data += section.size;
                }
                continue;
            }
            ObjSectionKind::Code => (),
        }

        for (symbol, symbol_diff) in section.symbols.iter().zip(&section_diff.symbols) {
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
            let match_percent = symbol_diff.match_percent.unwrap_or_else(|| {
                // Support cases where we don't have a target object,
                // assume complete means 100% match
                if object.complete == Some(true) {
                    100.0
                } else {
                    0.0
                }
            });
            unit.fuzzy_match_percent += match_percent * symbol.size as f32;
            unit.total_code += symbol.size;
            if match_percent == 100.0 {
                unit.matched_code += symbol.size;
            }
            unit.functions.push(ReportItem {
                name: symbol.name.clone(),
                demangled_name: symbol.demangled_name.clone(),
                size: symbol.size,
                fuzzy_match_percent: match_percent,
                address: symbol.virtual_address,
            });
            if match_percent == 100.0 {
                unit.matched_functions += 1;
            }
            unit.total_functions += 1;
        }
    }
    if unit.total_code == 0 {
        unit.fuzzy_match_percent = 100.0;
    } else {
        unit.fuzzy_match_percent /= unit.total_code as f32;
    }
    Ok(Some(unit))
}

impl From<&Report> for ChangeInfo {
    fn from(report: &Report) -> Self {
        Self {
            fuzzy_match_percent: report.fuzzy_match_percent,
            total_code: report.total_code,
            matched_code: report.matched_code,
            matched_code_percent: report.matched_code_percent,
            total_data: report.total_data,
            matched_data: report.matched_data,
            matched_data_percent: report.matched_data_percent,
            total_functions: report.total_functions,
            matched_functions: report.matched_functions,
            matched_functions_percent: report.matched_functions_percent,
        }
    }
}

impl From<&ReportUnit> for ChangeInfo {
    fn from(value: &ReportUnit) -> Self {
        Self {
            fuzzy_match_percent: value.fuzzy_match_percent,
            total_code: value.total_code,
            matched_code: value.matched_code,
            matched_code_percent: if value.total_code == 0 {
                100.0
            } else {
                value.matched_code as f32 / value.total_code as f32 * 100.0
            },
            total_data: value.total_data,
            matched_data: value.matched_data,
            matched_data_percent: if value.total_data == 0 {
                100.0
            } else {
                value.matched_data as f32 / value.total_data as f32 * 100.0
            },
            total_functions: value.total_functions,
            matched_functions: value.matched_functions,
            matched_functions_percent: if value.total_functions == 0 {
                100.0
            } else {
                value.matched_functions as f32 / value.total_functions as f32 * 100.0
            },
        }
    }
}

impl From<&ReportItem> for ChangeItemInfo {
    fn from(value: &ReportItem) -> Self {
        Self { fuzzy_match_percent: value.fuzzy_match_percent, size: value.size }
    }
}

fn changes(args: ChangesArgs) -> Result<()> {
    let output_format = if let Some(format) = &args.format {
        OutputFormat::from_str(format)?
    } else {
        OutputFormat::Json
    };

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
    let mut changes = Changes {
        from: Some(ChangeInfo::from(&previous)),
        to: Some(ChangeInfo::from(&current)),
        units: vec![],
    };
    for prev_unit in &previous.units {
        let curr_unit = current.units.iter().find(|u| u.name == prev_unit.name);
        let sections = process_items(prev_unit, curr_unit, |u| &u.sections);
        let functions = process_items(prev_unit, curr_unit, |u| &u.functions);

        let prev_unit_info = ChangeInfo::from(prev_unit);
        let curr_unit_info = curr_unit.map(ChangeInfo::from);
        if !functions.is_empty() || !matches!(&curr_unit_info, Some(v) if v == &prev_unit_info) {
            changes.units.push(ChangeUnit {
                name: prev_unit.name.clone(),
                from: Some(prev_unit_info),
                to: curr_unit_info,
                sections,
                functions,
            });
        }
    }
    for curr_unit in &current.units {
        if !previous.units.iter().any(|u| u.name == curr_unit.name) {
            changes.units.push(ChangeUnit {
                name: curr_unit.name.clone(),
                from: None,
                to: Some(ChangeInfo::from(curr_unit)),
                sections: process_new_items(&curr_unit.sections),
                functions: process_new_items(&curr_unit.functions),
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
                    });
                }
            } else {
                items.push(ChangeItem {
                    name: prev_func.name.clone(),
                    from: Some(prev_func_info),
                    to: None,
                });
            }
        }
        for curr_func in curr_items {
            if !prev_items.iter().any(|f| f.name == curr_func.name) {
                items.push(ChangeItem {
                    name: curr_func.name.clone(),
                    from: None,
                    to: Some(ChangeItemInfo::from(curr_func)),
                });
            }
        }
    } else {
        for prev_func in prev_items {
            items.push(ChangeItem {
                name: prev_func.name.clone(),
                from: Some(ChangeItemInfo::from(prev_func)),
                to: None,
            });
        }
    }
    items
}

fn process_new_items(items: &[ReportItem]) -> Vec<ChangeItem> {
    items
        .iter()
        .map(|f| ChangeItem { name: f.name.clone(), from: None, to: Some(ChangeItemInfo::from(f)) })
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

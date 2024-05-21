use std::{
    collections::HashSet,
    fs::File,
    io::{BufReader, BufWriter, Write},
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
use tracing::{info, warn};

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
    /// Output JSON file
    output: Option<PathBuf>,
    #[argp(switch, short = 'd')]
    /// Deduplicate global and weak symbols (runs single-threaded)
    deduplicate: bool,
}

#[derive(FromArgs, PartialEq, Debug)]
/// List any changes from a previous report.
#[argp(subcommand, name = "changes")]
pub struct ChangesArgs {
    #[argp(positional)]
    /// Previous report JSON file
    previous: PathBuf,
    #[argp(positional)]
    /// Current report JSON file
    current: PathBuf,
    #[argp(option, short = 'o')]
    /// Output JSON file
    output: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Report {
    fuzzy_match_percent: f32,
    total_code: u64,
    matched_code: u64,
    matched_code_percent: f32,
    total_data: u64,
    matched_data: u64,
    matched_data_percent: f32,
    total_functions: u32,
    matched_functions: u32,
    matched_functions_percent: f32,
    units: Vec<ReportUnit>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ReportUnit {
    name: String,
    fuzzy_match_percent: f32,
    total_code: u64,
    matched_code: u64,
    total_data: u64,
    matched_data: u64,
    total_functions: u32,
    matched_functions: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    module_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    module_id: Option<u32>,
    sections: Vec<ReportItem>,
    functions: Vec<ReportItem>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ReportItem {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    demangled_name: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_hex",
        deserialize_with = "deserialize_hex"
    )]
    address: Option<u64>,
    size: u64,
    fuzzy_match_percent: f32,
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        SubCommand::Generate(args) => generate(args),
        SubCommand::Changes(args) => changes(args),
    }
}

fn generate(args: GenerateArgs) -> Result<()> {
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
    if let Some(output) = &args.output {
        info!("Writing to {}", output.display());
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
            warn!("Skipping object without target: {}", object.name());
            return Ok(None);
        }
        (None, None) => {
            warn!("Skipping object without target or base: {}", object.name());
            return Ok(None);
        }
        _ => {}
    }
    let target = object
        .target_path
        .as_ref()
        .map(|p| obj::read::read(p).with_context(|| format!("Failed to open {}", p.display())))
        .transpose()?;
    let base = object
        .base_path
        .as_ref()
        .map(|p| obj::read::read(p).with_context(|| format!("Failed to open {}", p.display())))
        .transpose()?;
    let config = diff::DiffObjConfig { relax_reloc_diffs: true, ..Default::default() };
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

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Changes {
    from: ChangeInfo,
    to: ChangeInfo,
    units: Vec<ChangeUnit>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct ChangeInfo {
    fuzzy_match_percent: f32,
    total_code: u64,
    matched_code: u64,
    matched_code_percent: f32,
    total_data: u64,
    matched_data: u64,
    matched_data_percent: f32,
    total_functions: u32,
    matched_functions: u32,
    matched_functions_percent: f32,
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

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ChangeUnit {
    name: String,
    from: Option<ChangeInfo>,
    to: Option<ChangeInfo>,
    sections: Vec<ChangeItem>,
    functions: Vec<ChangeItem>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct ChangeItem {
    name: String,
    from: Option<ChangeItemInfo>,
    to: Option<ChangeItemInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct ChangeItemInfo {
    fuzzy_match_percent: f32,
    size: u64,
}

impl From<&ReportItem> for ChangeItemInfo {
    fn from(value: &ReportItem) -> Self {
        Self { fuzzy_match_percent: value.fuzzy_match_percent, size: value.size }
    }
}

fn changes(args: ChangesArgs) -> Result<()> {
    let previous = read_report(&args.previous)?;
    let current = read_report(&args.current)?;
    let mut changes = Changes {
        from: ChangeInfo::from(&previous),
        to: ChangeInfo::from(&current),
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
    if let Some(output) = &args.output {
        info!("Writing to {}", output.display());
        let mut output = BufWriter::new(
            File::create(output)
                .with_context(|| format!("Failed to create file {}", output.display()))?,
        );
        serde_json::to_writer_pretty(&mut output, &changes)?;
        output.flush()?;
    } else {
        serde_json::to_writer_pretty(std::io::stdout(), &changes)?;
    }
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
    serde_json::from_reader(BufReader::new(
        File::open(path).with_context(|| format!("Failed to open {}", path.display()))?,
    ))
    .with_context(|| format!("Failed to read report {}", path.display()))
}

fn serialize_hex<S>(x: &Option<u64>, s: S) -> Result<S::Ok, S::Error>
where S: serde::Serializer {
    if let Some(x) = x {
        s.serialize_str(&format!("{:#x}", x))
    } else {
        s.serialize_none()
    }
}

fn deserialize_hex<'de, D>(d: D) -> Result<Option<u64>, D::Error>
where D: serde::Deserializer<'de> {
    use serde::Deserialize;
    let s = String::deserialize(d)?;
    if s.is_empty() {
        Ok(None)
    } else if !s.starts_with("0x") {
        Err(serde::de::Error::custom("expected hex string"))
    } else {
        u64::from_str_radix(&s[2..], 16).map(Some).map_err(serde::de::Error::custom)
    }
}

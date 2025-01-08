use std::{
    fs,
    io::stdout,
    mem,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    task::{Wake, Waker},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use argp::FromArgs;
use crossterm::{
    event,
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
    },
};
use objdiff_core::{
    bindings::diff::DiffResult,
    build::{
        watcher::{create_watcher, Watcher},
        BuildConfig,
    },
    config::{build_globset, ProjectConfig, ProjectObject},
    diff,
    diff::{
        ConfigEnum, ConfigPropertyId, ConfigPropertyKind, DiffObjConfig, MappingConfig, ObjDiff,
    },
    jobs::{
        objdiff::{start_build, ObjDiffConfig},
        Job, JobQueue, JobResult,
    },
    obj,
    obj::ObjInfo,
};
use ratatui::prelude::*;

use crate::{
    util::{
        output::{write_output, OutputFormat},
        term::crossterm_panic_handler,
    },
    views::{function_diff::FunctionDiffUi, EventControlFlow, EventResult, UiView},
};

#[derive(FromArgs, PartialEq, Debug)]
/// Diff two object files. (Interactive or one-shot mode)
#[argp(subcommand, name = "diff")]
pub struct Args {
    #[argp(option, short = '1')]
    /// Target object file
    target: Option<PathBuf>,
    #[argp(option, short = '2')]
    /// Base object file
    base: Option<PathBuf>,
    #[argp(option, short = 'p')]
    /// Project directory
    project: Option<PathBuf>,
    #[argp(option, short = 'u')]
    /// Unit name within project
    unit: Option<String>,
    #[argp(option, short = 'o')]
    /// Output file (one-shot mode) ("-" for stdout)
    output: Option<PathBuf>,
    #[argp(option)]
    /// Output format (json, json-pretty, proto) (default: json)
    format: Option<String>,
    #[argp(positional)]
    /// Function symbol to diff
    symbol: Option<String>,
    #[argp(option, short = 'c')]
    /// Configuration property (key=value)
    config: Vec<String>,
    #[argp(option, short = 'm')]
    /// Symbol mapping (target=base)
    mapping: Vec<String>,
    #[argp(option)]
    /// Left symbol name for selection
    selecting_left: Option<String>,
    #[argp(option)]
    /// Right symbol name for selection
    selecting_right: Option<String>,
}

pub fn run(args: Args) -> Result<()> {
    let (target_path, base_path, project_config) = match (
        &args.target,
        &args.base,
        &args.project,
        &args.unit,
    ) {
        (Some(_), Some(_), None, None)
        | (Some(_), None, None, None)
        | (None, Some(_), None, None) => (args.target.clone(), args.base.clone(), None),
        (None, None, p, u) => {
            let project = match p {
                Some(project) => project.clone(),
                _ => std::env::current_dir().context("Failed to get the current directory")?,
            };
            let Some((project_config, project_config_info)) =
                objdiff_core::config::try_project_config(&project)
            else {
                bail!("Project config not found in {}", &project.display())
            };
            let mut project_config = project_config.with_context(|| {
                format!("Reading project config {}", project_config_info.path.display())
            })?;
            let object = {
                let resolve_paths = |o: &mut ProjectObject| {
                    o.resolve_paths(
                        &project,
                        project_config.target_dir.as_deref(),
                        project_config.base_dir.as_deref(),
                    )
                };
                if let Some(u) = u {
                    let unit_path =
                        PathBuf::from_str(u).ok().and_then(|p| fs::canonicalize(p).ok());

                    let Some(object) = project_config
                        .units
                        .as_deref_mut()
                        .unwrap_or_default()
                        .iter_mut()
                        .find_map(|obj| {
                            if obj.name.as_deref() == Some(u) {
                                resolve_paths(obj);
                                return Some(obj);
                            }

                            let up = unit_path.as_deref()?;

                            resolve_paths(obj);

                            if [&obj.base_path, &obj.target_path]
                                .into_iter()
                                .filter_map(|p| p.as_ref().and_then(|p| p.canonicalize().ok()))
                                .any(|p| p == up)
                            {
                                return Some(obj);
                            }

                            None
                        })
                    else {
                        bail!("Unit not found: {}", u)
                    };

                    object
                } else if let Some(symbol_name) = &args.symbol {
                    let mut idx = None;
                    let mut count = 0usize;
                    for (i, obj) in project_config
                        .units
                        .as_deref_mut()
                        .unwrap_or_default()
                        .iter_mut()
                        .enumerate()
                    {
                        resolve_paths(obj);

                        if obj
                            .target_path
                            .as_deref()
                            .map(|o| obj::read::has_function(o, symbol_name))
                            .transpose()?
                            .unwrap_or(false)
                        {
                            idx = Some(i);
                            count += 1;
                            if count > 1 {
                                break;
                            }
                        }
                    }
                    match (count, idx) {
                        (0, None) => bail!("Symbol not found: {}", symbol_name),
                        (1, Some(i)) => &mut project_config.units_mut()[i],
                        (2.., Some(_)) => bail!(
                            "Multiple instances of {} were found, try specifying a unit",
                            symbol_name
                        ),
                        _ => unreachable!(),
                    }
                } else {
                    bail!("Must specify one of: symbol, project and unit, target and base objects")
                }
            };
            let target_path = object.target_path.clone();
            let base_path = object.base_path.clone();
            (target_path, base_path, Some(project_config))
        }
        _ => bail!("Either target and base or project and unit must be specified"),
    };

    if let Some(output) = &args.output {
        run_oneshot(&args, output, target_path.as_deref(), base_path.as_deref())
    } else {
        run_interactive(args, target_path, base_path, project_config)
    }
}

fn build_config_from_args(args: &Args) -> Result<(DiffObjConfig, MappingConfig)> {
    let mut diff_config = DiffObjConfig::default();
    for config in &args.config {
        let (key, value) = config.split_once('=').context("--config expects \"key=value\"")?;
        let property_id = ConfigPropertyId::from_str(key)
            .map_err(|()| anyhow!("Invalid configuration property: {}", key))?;
        diff_config.set_property_value_str(property_id, value).map_err(|()| {
            let mut options = String::new();
            match property_id.kind() {
                ConfigPropertyKind::Boolean => {
                    options = "true, false".to_string();
                }
                ConfigPropertyKind::Choice(variants) => {
                    for (i, variant) in variants.iter().enumerate() {
                        if i > 0 {
                            options.push_str(", ");
                        }
                        options.push_str(variant.value);
                    }
                }
            }
            anyhow!("Invalid value for {}. Expected one of: {}", property_id.name(), options)
        })?;
    }
    let mut mapping_config = MappingConfig {
        mappings: Default::default(),
        selecting_left: args.selecting_left.clone(),
        selecting_right: args.selecting_right.clone(),
    };
    for mapping in &args.mapping {
        let (target, base) =
            mapping.split_once('=').context("--mapping expects \"target=base\"")?;
        mapping_config.mappings.insert(target.to_string(), base.to_string());
    }
    Ok((diff_config, mapping_config))
}

fn run_oneshot(
    args: &Args,
    output: &Path,
    target_path: Option<&Path>,
    base_path: Option<&Path>,
) -> Result<()> {
    let output_format = OutputFormat::from_option(args.format.as_deref())?;
    let (diff_config, mapping_config) = build_config_from_args(args)?;
    let target = target_path
        .map(|p| {
            obj::read::read(p, &diff_config).with_context(|| format!("Loading {}", p.display()))
        })
        .transpose()?;
    let base = base_path
        .map(|p| {
            obj::read::read(p, &diff_config).with_context(|| format!("Loading {}", p.display()))
        })
        .transpose()?;
    let result =
        diff::diff_objs(&diff_config, &mapping_config, target.as_ref(), base.as_ref(), None)?;
    let left = target.as_ref().and_then(|o| result.left.as_ref().map(|d| (o, d)));
    let right = base.as_ref().and_then(|o| result.right.as_ref().map(|d| (o, d)));
    write_output(&DiffResult::new(left, right), Some(output), output_format)?;
    Ok(())
}

pub struct AppState {
    pub jobs: JobQueue,
    pub waker: Arc<TermWaker>,
    pub project_dir: Option<PathBuf>,
    pub project_config: Option<ProjectConfig>,
    pub target_path: Option<PathBuf>,
    pub base_path: Option<PathBuf>,
    pub left_obj: Option<(ObjInfo, ObjDiff)>,
    pub right_obj: Option<(ObjInfo, ObjDiff)>,
    pub prev_obj: Option<(ObjInfo, ObjDiff)>,
    pub reload_time: Option<time::OffsetDateTime>,
    pub time_format: Vec<time::format_description::FormatItem<'static>>,
    pub watcher: Option<Watcher>,
    pub modified: Arc<AtomicBool>,
    pub diff_obj_config: DiffObjConfig,
    pub mapping_config: MappingConfig,
}

fn create_objdiff_config(state: &AppState) -> ObjDiffConfig {
    ObjDiffConfig {
        build_config: BuildConfig {
            project_dir: state.project_dir.clone(),
            custom_make: state
                .project_config
                .as_ref()
                .and_then(|c| c.custom_make.as_ref())
                .cloned(),
            custom_args: state
                .project_config
                .as_ref()
                .and_then(|c| c.custom_args.as_ref())
                .cloned(),
            selected_wsl_distro: None,
        },
        build_base: state.project_config.as_ref().is_some_and(|p| p.build_base.unwrap_or(true)),
        build_target: state
            .project_config
            .as_ref()
            .is_some_and(|p| p.build_target.unwrap_or(false)),
        target_path: state.target_path.clone(),
        base_path: state.base_path.clone(),
        diff_obj_config: state.diff_obj_config.clone(),
        mapping_config: state.mapping_config.clone(),
    }
}

impl AppState {
    fn reload(&mut self) -> Result<()> {
        let config = create_objdiff_config(self);
        self.jobs.push_once(Job::ObjDiff, || start_build(Waker::from(self.waker.clone()), config));
        Ok(())
    }

    fn check_jobs(&mut self) -> Result<bool> {
        let mut redraw = false;
        self.jobs.collect_results();
        for result in mem::take(&mut self.jobs.results) {
            match result {
                JobResult::None => unreachable!("Unexpected JobResult::None"),
                JobResult::ObjDiff(result) => {
                    let result = result.unwrap();
                    self.left_obj = result.first_obj;
                    self.right_obj = result.second_obj;
                    self.reload_time = Some(result.time);
                    redraw = true;
                }
                JobResult::CheckUpdate(_) => todo!("CheckUpdate"),
                JobResult::Update(_) => todo!("Update"),
                JobResult::CreateScratch(_) => todo!("CreateScratch"),
            }
        }
        Ok(redraw)
    }
}

#[derive(Default)]
pub struct TermWaker(pub AtomicBool);

impl Wake for TermWaker {
    fn wake(self: Arc<Self>) { self.0.store(true, Ordering::Relaxed); }

    fn wake_by_ref(self: &Arc<Self>) { self.0.store(true, Ordering::Relaxed); }
}

fn run_interactive(
    args: Args,
    target_path: Option<PathBuf>,
    base_path: Option<PathBuf>,
    project_config: Option<ProjectConfig>,
) -> Result<()> {
    let Some(symbol_name) = &args.symbol else { bail!("Interactive mode requires a symbol name") };
    let time_format = time::format_description::parse_borrowed::<2>("[hour]:[minute]:[second]")
        .context("Failed to parse time format")?;
    let (diff_obj_config, mapping_config) = build_config_from_args(&args)?;
    let mut state = AppState {
        jobs: Default::default(),
        waker: Default::default(),
        project_dir: args.project.clone(),
        project_config,
        target_path,
        base_path,
        left_obj: None,
        right_obj: None,
        prev_obj: None,
        reload_time: None,
        time_format,
        watcher: None,
        modified: Default::default(),
        diff_obj_config,
        mapping_config,
    };
    if let (Some(project_dir), Some(project_config)) = (&state.project_dir, &state.project_config) {
        let watch_patterns = project_config.build_watch_patterns()?;
        state.watcher = Some(create_watcher(
            state.modified.clone(),
            project_dir,
            build_globset(&watch_patterns)?,
            Waker::from(state.waker.clone()),
        )?);
    }
    let mut view: Box<dyn UiView> =
        Box::new(FunctionDiffUi { symbol_name: symbol_name.clone(), ..Default::default() });
    state.reload()?;

    crossterm_panic_handler();
    enable_raw_mode()?;
    crossterm::queue!(
        stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        SetTitle(format!("{} - objdiff", symbol_name)),
    )?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut result = EventResult { redraw: true, ..Default::default() };
    'outer: loop {
        if result.redraw {
            terminal.draw(|f| loop {
                result.redraw = false;
                view.draw(&state, f, &mut result);
                result.click_xy = None;
                if !result.redraw {
                    break;
                }
                // Clear buffer on redraw
                f.buffer_mut().reset();
            })?;
        }
        loop {
            if event::poll(Duration::from_millis(100))? {
                match view.handle_event(&mut state, event::read()?) {
                    EventControlFlow::Break => break 'outer,
                    EventControlFlow::Continue(r) => result = r,
                    EventControlFlow::Reload => {
                        state.reload()?;
                        result.redraw = true;
                    }
                }
                break;
            } else if state.waker.0.swap(false, Ordering::Relaxed) {
                if state.modified.swap(false, Ordering::Relaxed) {
                    state.reload()?;
                }
                result.redraw = true;
                break;
            }
        }
        if state.check_jobs()? {
            result.redraw = true;
            view.reload(&state)?;
        }
    }

    // Reset terminal
    disable_raw_mode()?;
    crossterm::execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;
    Ok(())
}

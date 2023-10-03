use std::{
    default::Default,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
    time::Duration,
};

use globset::{Glob, GlobSet, GlobSetBuilder};
use notify::{RecursiveMode, Watcher};
use time::UtcOffset;

use crate::{
    app_config::{deserialize_config, AppConfigVersion},
    config::{
        build_globset, load_project_config, ProjectObject, ProjectObjectNode, CONFIG_FILENAMES,
    },
    jobs::{objdiff::start_build, Job, JobQueue, JobResult, JobStatus},
    views::{
        appearance::{appearance_window, Appearance},
        config::{config_ui, project_window, ConfigViewState, DEFAULT_WATCH_PATTERNS},
        data_diff::data_diff_ui,
        demangle::{demangle_window, DemangleViewState},
        function_diff::function_diff_ui,
        jobs::jobs_ui,
        symbol_diff::{symbol_diff_ui, DiffViewState, View},
    },
};

#[derive(Default)]
pub struct ViewState {
    pub jobs: JobQueue,
    pub config_state: ConfigViewState,
    pub demangle_state: DemangleViewState,
    pub diff_state: DiffViewState,
    pub show_appearance_config: bool,
    pub show_demangle: bool,
    pub show_project_config: bool,
}

/// The configuration for a single object file.
#[derive(Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ObjectConfig {
    pub name: String,
    pub target_path: Option<PathBuf>,
    pub base_path: Option<PathBuf>,
    pub reverse_fn_order: Option<bool>,
    pub complete: Option<bool>,
}

#[inline]
fn bool_true() -> bool { true }

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct AppConfig {
    // TODO: https://github.com/ron-rs/ron/pull/455
    // #[serde(flatten)]
    // pub version: AppConfigVersion,
    pub version: u32,
    pub custom_make: Option<String>,
    pub selected_wsl_distro: Option<String>,
    pub project_dir: Option<PathBuf>,
    pub target_obj_dir: Option<PathBuf>,
    pub base_obj_dir: Option<PathBuf>,
    pub selected_obj: Option<ObjectConfig>,
    pub build_target: bool,
    #[serde(default = "bool_true")]
    pub rebuild_on_changes: bool,
    pub auto_update_check: bool,
    pub watch_patterns: Vec<Glob>,
    pub recent_projects: Vec<PathBuf>,

    #[serde(skip)]
    pub objects: Vec<ProjectObject>,
    #[serde(skip)]
    pub object_nodes: Vec<ProjectObjectNode>,
    #[serde(skip)]
    pub watcher_change: bool,
    #[serde(skip)]
    pub config_change: bool,
    #[serde(skip)]
    pub obj_change: bool,
    #[serde(skip)]
    pub queue_build: bool,
    #[serde(skip)]
    pub project_config_loaded: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: AppConfigVersion::default().version,
            custom_make: None,
            selected_wsl_distro: None,
            project_dir: None,
            target_obj_dir: None,
            base_obj_dir: None,
            selected_obj: None,
            build_target: false,
            rebuild_on_changes: true,
            auto_update_check: true,
            watch_patterns: DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect(),
            recent_projects: vec![],
            objects: vec![],
            object_nodes: vec![],
            watcher_change: false,
            config_change: false,
            obj_change: false,
            queue_build: false,
            project_config_loaded: false,
        }
    }
}

impl AppConfig {
    pub fn set_project_dir(&mut self, path: PathBuf) {
        self.recent_projects.retain(|p| p != &path);
        if self.recent_projects.len() > 9 {
            self.recent_projects.truncate(9);
        }
        self.recent_projects.insert(0, path.clone());
        self.project_dir = Some(path);
        self.target_obj_dir = None;
        self.base_obj_dir = None;
        self.selected_obj = None;
        self.build_target = false;
        self.objects.clear();
        self.object_nodes.clear();
        self.watcher_change = true;
        self.config_change = true;
        self.obj_change = true;
        self.queue_build = false;
        self.project_config_loaded = false;
    }

    pub fn set_target_obj_dir(&mut self, path: PathBuf) {
        self.target_obj_dir = Some(path);
        self.selected_obj = None;
        self.obj_change = true;
        self.queue_build = false;
    }

    pub fn set_base_obj_dir(&mut self, path: PathBuf) {
        self.base_obj_dir = Some(path);
        self.selected_obj = None;
        self.obj_change = true;
        self.queue_build = false;
    }

    pub fn set_selected_obj(&mut self, object: ObjectConfig) {
        self.selected_obj = Some(object);
        self.obj_change = true;
        self.queue_build = false;
    }
}

pub type AppConfigRef = Arc<RwLock<AppConfig>>;

#[derive(Default)]
pub struct App {
    appearance: Appearance,
    view_state: ViewState,
    config: AppConfigRef,
    modified: Arc<AtomicBool>,
    config_modified: Arc<AtomicBool>,
    watcher: Option<notify::RecommendedWatcher>,
    relaunch_path: Rc<Mutex<Option<PathBuf>>>,
    should_relaunch: bool,
}

pub const APPEARANCE_KEY: &str = "appearance";
pub const CONFIG_KEY: &str = "app_config";

impl App {
    /// Called once before the first frame.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        utc_offset: UtcOffset,
        relaunch_path: Rc<Mutex<Option<PathBuf>>>,
    ) -> Self {
        // This is also where you can customized the look at feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        let mut app = Self::default();
        if let Some(storage) = cc.storage {
            if let Some(appearance) = eframe::get_value::<Appearance>(storage, APPEARANCE_KEY) {
                app.appearance = appearance;
            }
            if let Some(mut config) = deserialize_config(storage) {
                if config.project_dir.is_some() {
                    config.config_change = true;
                    config.watcher_change = true;
                    app.modified.store(true, Ordering::Relaxed);
                }
                app.view_state.config_state.queue_check_update = config.auto_update_check;
                app.config = Arc::new(RwLock::new(config));
            }
        }
        app.appearance.utc_offset = utc_offset;
        app.relaunch_path = relaunch_path;
        app
    }

    fn pre_update(&mut self) {
        let ViewState { jobs, diff_state, config_state, .. } = &mut self.view_state;

        let mut results = vec![];
        for (job, result) in jobs.iter_finished() {
            match result {
                Ok(result) => {
                    log::info!("Job {} finished", job.id);
                    match result {
                        JobResult::None => {
                            if let Some(err) = &job.status.read().unwrap().error {
                                log::error!("{:?}", err);
                            }
                        }
                        JobResult::Update(state) => {
                            if let Ok(mut guard) = self.relaunch_path.lock() {
                                *guard = Some(state.exe_path);
                            }
                            self.should_relaunch = true;
                        }
                        _ => results.push(result),
                    }
                }
                Err(err) => {
                    let err = if let Some(msg) = err.downcast_ref::<&'static str>() {
                        anyhow::Error::msg(*msg)
                    } else if let Some(msg) = err.downcast_ref::<String>() {
                        anyhow::Error::msg(msg.clone())
                    } else {
                        anyhow::Error::msg("Thread panicked")
                    };
                    let result = job.status.write();
                    if let Ok(mut guard) = result {
                        guard.error = Some(err);
                    } else {
                        drop(result);
                        job.status = Arc::new(RwLock::new(JobStatus {
                            title: "Error".to_string(),
                            progress_percent: 0.0,
                            progress_items: None,
                            status: "".to_string(),
                            error: Some(err),
                        }));
                    }
                }
            }
        }
        jobs.results.append(&mut results);
        jobs.clear_finished();

        diff_state.pre_update(jobs, &self.config);
        config_state.pre_update(jobs);
        debug_assert!(jobs.results.is_empty());
    }

    fn post_update(&mut self) {
        let ViewState { jobs, diff_state, config_state, .. } = &mut self.view_state;
        config_state.post_update(jobs, &self.config);
        diff_state.post_update(jobs, &self.config);

        let Ok(mut config) = self.config.write() else {
            return;
        };
        let config = &mut *config;

        if self.config_modified.swap(false, Ordering::Relaxed) {
            config.config_change = true;
        }

        if config.config_change {
            config.config_change = false;
            match load_project_config(config) {
                Ok(()) => config_state.load_error = None,
                Err(e) => {
                    log::error!("Failed to load project config: {e}");
                    config_state.load_error = Some(format!("{e}"));
                }
            }
        }

        if config.watcher_change {
            drop(self.watcher.take());

            if let Some(project_dir) = &config.project_dir {
                if !config.watch_patterns.is_empty() {
                    match build_globset(&config.watch_patterns)
                        .map_err(anyhow::Error::new)
                        .and_then(|globset| {
                            create_watcher(
                                self.modified.clone(),
                                self.config_modified.clone(),
                                project_dir,
                                globset,
                            )
                            .map_err(anyhow::Error::new)
                        }) {
                        Ok(watcher) => self.watcher = Some(watcher),
                        Err(e) => log::error!("Failed to create watcher: {e}"),
                    }
                }
                config.watcher_change = false;
            }
        }

        if config.obj_change {
            *diff_state = Default::default();
            if config.selected_obj.is_some() {
                config.queue_build = true;
            }
            config.obj_change = false;
        }

        if self.modified.swap(false, Ordering::Relaxed) && config.rebuild_on_changes {
            config.queue_build = true;
        }

        // Don't clear `queue_build` if a build is running. A file may have been modified during
        // the build, so we'll start another build after the current one finishes.
        if config.queue_build && config.selected_obj.is_some() && !jobs.is_running(Job::ObjDiff) {
            jobs.push(start_build(self.config.clone()));
            config.queue_build = false;
        }
    }
}

impl eframe::App for App {
    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.should_relaunch {
            frame.close();
            return;
        }

        self.pre_update();

        let Self { config, appearance, view_state, .. } = self;
        ctx.set_style(appearance.apply(ctx.style().as_ref()));

        let ViewState {
            jobs,
            show_appearance_config,
            demangle_state,
            show_demangle,
            diff_state,
            config_state,
            show_project_config,
        } = view_state;

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    let recent_projects = if let Ok(guard) = config.read() {
                        guard.recent_projects.clone()
                    } else {
                        vec![]
                    };
                    if recent_projects.is_empty() {
                        ui.add_enabled(false, egui::Button::new("Recent Projects…"));
                    } else {
                        ui.menu_button("Recent Projects…", |ui| {
                            for path in recent_projects {
                                if ui.button(format!("{}", path.display())).clicked() {
                                    config.write().unwrap().set_project_dir(path);
                                    ui.close_menu();
                                }
                            }
                        });
                    }
                    if ui.button("Appearance…").clicked() {
                        *show_appearance_config = !*show_appearance_config;
                        ui.close_menu();
                    }
                    if ui.button("Quit").clicked() {
                        frame.close();
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui.button("Demangle…").clicked() {
                        *show_demangle = !*show_demangle;
                        ui.close_menu();
                    }
                });
            });
        });

        let build_success = matches!(&diff_state.build, Some(b) if b.first_status.success && b.second_status.success);
        if diff_state.current_view == View::FunctionDiff && build_success {
            egui::CentralPanel::default().show(ctx, |ui| {
                function_diff_ui(ui, diff_state, appearance);
            });
        } else if diff_state.current_view == View::DataDiff && build_success {
            egui::CentralPanel::default().show(ctx, |ui| {
                data_diff_ui(ui, diff_state, appearance);
            });
        } else {
            egui::SidePanel::left("side_panel").show(ctx, |ui| {
                egui::ScrollArea::both().show(ui, |ui| {
                    config_ui(ui, config, show_project_config, config_state, appearance);
                    jobs_ui(ui, jobs, appearance);
                });
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                symbol_diff_ui(ui, diff_state, appearance);
            });
        }

        project_window(ctx, config, show_project_config, config_state, appearance);
        appearance_window(ctx, show_appearance_config, appearance);
        demangle_window(ctx, show_demangle, demangle_state, appearance);

        self.post_update();

        // Windows + request_repaint_after breaks dialogs:
        // https://github.com/emilk/egui/issues/2003
        if cfg!(windows) || self.view_state.jobs.any_running() {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(config) = self.config.read() {
            eframe::set_value(storage, CONFIG_KEY, &*config);
        }
        eframe::set_value(storage, APPEARANCE_KEY, &self.appearance);
    }
}

fn create_watcher(
    modified: Arc<AtomicBool>,
    config_modified: Arc<AtomicBool>,
    project_dir: &Path,
    patterns: GlobSet,
) -> notify::Result<notify::RecommendedWatcher> {
    let mut config_patterns = GlobSetBuilder::new();
    for filename in CONFIG_FILENAMES {
        config_patterns.add(Glob::new(filename).unwrap());
    }
    let config_patterns = config_patterns.build().unwrap();

    let base_dir = project_dir.to_owned();
    let mut watcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                if matches!(
                    event.kind,
                    notify::EventKind::Modify(..)
                        | notify::EventKind::Create(..)
                        | notify::EventKind::Remove(..)
                ) {
                    for path in &event.paths {
                        let Ok(path) = path.strip_prefix(&base_dir) else {
                            continue;
                        };
                        if config_patterns.is_match(path) {
                            config_modified.store(true, Ordering::Relaxed);
                        } else if patterns.is_match(path) {
                            modified.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
            Err(e) => log::error!("watch error: {e:?}"),
        })?;
    watcher.watch(project_dir, RecursiveMode::Recursive)?;
    Ok(watcher)
}

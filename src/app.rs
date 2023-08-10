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
    config::{build_globset, load_project_config, ProjectUnit, ProjectUnitNode, CONFIG_FILENAMES},
    jobs::{
        check_update::start_check_update, objdiff::start_build, Job, JobQueue, JobResult, JobStatus,
    },
    views::{
        appearance::{appearance_window, Appearance},
        config::{config_ui, project_window, ConfigViewState},
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
    pub show_appearance_config: bool,
    pub demangle_state: DemangleViewState,
    pub show_demangle: bool,
    pub diff_state: DiffViewState,
    pub config_state: ConfigViewState,
    pub show_project_config: bool,
}

#[derive(Default, Clone, serde::Deserialize, serde::Serialize)]
pub struct AppConfig {
    pub custom_make: Option<String>,
    pub selected_wsl_distro: Option<String>,
    pub project_dir: Option<PathBuf>,
    pub target_obj_dir: Option<PathBuf>,
    pub base_obj_dir: Option<PathBuf>,
    pub obj_path: Option<String>,
    pub build_target: bool,
    pub watcher_enabled: bool,
    pub auto_update_check: bool,
    pub watch_patterns: Vec<Glob>,

    #[serde(skip)]
    pub units: Vec<ProjectUnit>,
    #[serde(skip)]
    pub unit_nodes: Vec<ProjectUnitNode>,
    #[serde(skip)]
    pub watcher_change: bool,
    #[serde(skip)]
    pub config_change: bool,
}

#[derive(Default)]
pub struct App {
    appearance: Appearance,
    view_state: ViewState,
    config: Arc<RwLock<AppConfig>>,
    modified: Arc<AtomicBool>,
    config_modified: Arc<AtomicBool>,
    watcher: Option<notify::RecommendedWatcher>,
    relaunch_path: Rc<Mutex<Option<PathBuf>>>,
    should_relaunch: bool,
}

const APPEARANCE_KEY: &str = "appearance";
const CONFIG_KEY: &str = "app_config";

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
            if let Some(mut config) = eframe::get_value::<AppConfig>(storage, CONFIG_KEY) {
                if config.project_dir.is_some() {
                    config.config_change = true;
                    config.watcher_change = true;
                    app.modified.store(true, Ordering::Relaxed);
                }
                app.view_state.config_state.queue_update_check = config.auto_update_check;
                app.config = Arc::new(RwLock::new(config));
            }
        }
        app.appearance.utc_offset = utc_offset;
        app.relaunch_path = relaunch_path;
        app
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
                    if ui.button("Appearance…").clicked() {
                        *show_appearance_config = !*show_appearance_config;
                    }
                    if ui.button("Quit").clicked() {
                        frame.close();
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui.button("Demangle…").clicked() {
                        *show_demangle = !*show_demangle;
                    }
                });
            });
        });

        if diff_state.current_view == View::FunctionDiff
            && matches!(&diff_state.build, Some(b) if b.first_status.success && b.second_status.success)
        {
            egui::CentralPanel::default().show(ctx, |ui| {
                if function_diff_ui(ui, jobs, diff_state, appearance) {
                    jobs.push(start_build(config.clone()));
                }
            });
        } else if diff_state.current_view == View::DataDiff
            && matches!(&diff_state.build, Some(b) if b.first_status.success && b.second_status.success)
        {
            egui::CentralPanel::default().show(ctx, |ui| {
                if data_diff_ui(ui, jobs, diff_state, appearance) {
                    jobs.push(start_build(config.clone()));
                }
            });
        } else {
            egui::SidePanel::left("side_panel").show(ctx, |ui| {
                config_ui(ui, config, jobs, show_project_config, config_state, appearance);
                jobs_ui(ui, jobs, appearance);
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                symbol_diff_ui(ui, diff_state, appearance);
            });
        }

        project_window(ctx, config, show_project_config, config_state, appearance);
        appearance_window(ctx, show_appearance_config, appearance);
        demangle_window(ctx, show_demangle, demangle_state, appearance);

        // Windows + request_repaint_after breaks dialogs:
        // https://github.com/emilk/egui/issues/2003
        if cfg!(windows) || jobs.any_running() {
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

    fn post_rendering(&mut self, _window_size_px: [u32; 2], _frame: &eframe::Frame) {
        let ViewState { jobs, diff_state, config_state, .. } = &mut self.view_state;

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
                        JobResult::ObjDiff(state) => {
                            diff_state.build = Some(state);
                        }
                        JobResult::CheckUpdate(state) => {
                            config_state.check_update = Some(state);
                        }
                        JobResult::Update(state) => {
                            if let Ok(mut guard) = self.relaunch_path.lock() {
                                *guard = Some(state.exe_path);
                            }
                            self.should_relaunch = true;
                        }
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
        jobs.clear_finished();

        if let Ok(mut config) = self.config.write() {
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

            if config.obj_path.is_some()
                && self.modified.swap(false, Ordering::Relaxed)
                && !jobs.is_running(Job::ObjDiff)
            {
                jobs.push(start_build(self.config.clone()));
            }

            if config_state.queue_update_check {
                jobs.push(start_check_update());
                config_state.queue_update_check = false;
            }
        }
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
        config_patterns.add(Glob::new(&format!("**/{filename}")).unwrap());
    }
    let config_patterns = config_patterns.build().unwrap();

    let mut watcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                if matches!(event.kind, notify::EventKind::Modify(..)) {
                    for path in &event.paths {
                        if config_patterns.is_match(path) {
                            config_modified.store(true, Ordering::Relaxed);
                        }
                        if patterns.is_match(path) {
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

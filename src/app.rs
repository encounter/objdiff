use std::{
    default::Default,
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::Duration,
};

use eframe::Frame;
use egui::Widget;
use notify::{RecursiveMode, Watcher};
use time::{OffsetDateTime, UtcOffset};

use crate::{
    jobs::{
        objdiff::{queue_build, BuildStatus, ObjDiffResult},
        Job, JobResult, JobState,
    },
    views::{
        config::config_ui, function_diff::function_diff_ui, jobs::jobs_ui,
        symbol_diff::symbol_diff_ui,
    },
};

#[derive(Default, Eq, PartialEq)]
pub enum View {
    #[default]
    SymbolDiff,
    FunctionDiff,
}

#[derive(Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum DiffKind {
    #[default]
    SplitObj,
    WholeBinary,
}

#[derive(Default, Clone)]
pub struct DiffConfig {
    // TODO
    // pub stripped_symbols: Vec<String>,
    // pub mapped_symbols: HashMap<String, String>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ViewState {
    #[serde(skip)]
    pub jobs: Vec<JobState>,
    #[serde(skip)]
    pub build: Option<Box<ObjDiffResult>>,
    #[serde(skip)]
    pub highlighted_symbol: Option<String>,
    #[serde(skip)]
    pub selected_symbol: Option<String>,
    #[serde(skip)]
    pub current_view: View,
    #[serde(skip)]
    pub show_config: bool,
    #[serde(skip)]
    pub diff_config: DiffConfig,
    #[serde(skip)]
    pub search: String,
    #[serde(skip)]
    pub utc_offset: UtcOffset,
    // Config
    pub diff_kind: DiffKind,
    pub reverse_fn_order: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            jobs: vec![],
            build: None,
            highlighted_symbol: None,
            selected_symbol: None,
            current_view: Default::default(),
            show_config: false,
            diff_config: Default::default(),
            search: Default::default(),
            utc_offset: UtcOffset::UTC,
            diff_kind: Default::default(),
            reverse_fn_order: false,
        }
    }
}

#[derive(Default, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub custom_make: String,
    // WSL2 settings
    #[serde(skip)]
    pub available_wsl_distros: Option<Vec<String>>,
    pub selected_wsl_distro: Option<String>,
    // Split obj
    pub project_dir: Option<PathBuf>,
    pub target_obj_dir: Option<PathBuf>,
    pub base_obj_dir: Option<PathBuf>,
    pub obj_path: Option<String>,
    pub build_target: bool,
    // Whole binary
    pub left_obj: Option<PathBuf>,
    pub right_obj: Option<PathBuf>,
    #[serde(skip)]
    pub project_dir_change: bool,
}

/// We derive Deserialize/Serialize so we can persist app state on shutdown.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct App {
    view_state: ViewState,
    #[serde(skip)]
    config: Arc<RwLock<AppConfig>>,
    #[serde(skip)]
    modified: Arc<AtomicBool>,
    #[serde(skip)]
    watcher: Option<notify::RecommendedWatcher>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            view_state: ViewState::default(),
            config: Arc::new(Default::default()),
            modified: Arc::new(Default::default()),
            watcher: None,
        }
    }
}

const CONFIG_KEY: &str = "app_config";

impl App {
    /// Called once before the first frame.
    pub fn new(cc: &eframe::CreationContext<'_>, utc_offset: UtcOffset) -> Self {
        // This is also where you can customized the look at feel of egui using
        // `cc.egui_ctx.set_visuals` and `cc.egui_ctx.set_fonts`.

        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        if let Some(storage) = cc.storage {
            let mut app: App = eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
            let mut config: AppConfig = eframe::get_value(storage, CONFIG_KEY).unwrap_or_default();
            if config.project_dir.is_some() {
                config.project_dir_change = true;
            }
            app.config = Arc::new(RwLock::new(config));
            app.view_state.utc_offset = utc_offset;
            app
        } else {
            Self::default()
        }
    }
}

impl eframe::App for App {
    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    fn update(&mut self, ctx: &egui::Context, frame: &mut Frame) {
        let Self { config, view_state, .. } = self;

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Quit").clicked() {
                        frame.close();
                    }
                    if ui.button("Show config").clicked() {
                        view_state.show_config = !view_state.show_config;
                    }
                });
            });
        });

        if view_state.current_view == View::FunctionDiff
            && matches!(&view_state.build, Some(b) if b.first_status.success && b.second_status.success)
        {
            // egui::SidePanel::left("side_panel").show(ctx, |ui| {
            //     if ui.button("Back").clicked() {
            //         view_state.current_view = View::SymbolDiff;
            //     }
            //     ui.separator();
            //     jobs_ui(ui, view_state);
            // });

            egui::CentralPanel::default().show(ctx, |ui| {
                if function_diff_ui(ui, view_state) {
                    view_state
                        .jobs
                        .push(queue_build(config.clone(), view_state.diff_config.clone()));
                }
            });
        } else {
            egui::SidePanel::left("side_panel").show(ctx, |ui| {
                config_ui(ui, config, view_state);
                jobs_ui(ui, view_state);
            });

            egui::CentralPanel::default().show(ctx, |ui| {
                symbol_diff_ui(ui, view_state);
            });
        }

        egui::Window::new("Config").open(&mut view_state.show_config).show(ctx, |ui| {
            ui.label("Diff type:");

            if egui::RadioButton::new(
                view_state.diff_kind == DiffKind::SplitObj,
                "Split object diff",
            )
            .ui(ui)
            .on_hover_text("Compare individual object files")
            .clicked()
            {
                view_state.diff_kind = DiffKind::SplitObj;
            }

            if egui::RadioButton::new(
                view_state.diff_kind == DiffKind::WholeBinary,
                "Whole binary diff",
            )
            .ui(ui)
            .on_hover_text("Compare two full binaries")
            .clicked()
            {
                view_state.diff_kind = DiffKind::WholeBinary;
            }

            ui.separator();
        });

        // Windows + request_repaint_after breaks dialogs:
        // https://github.com/emilk/egui/issues/2003
        if cfg!(windows)
            || view_state.jobs.iter().any(|job| {
                if let Some(handle) = &job.handle {
                    return !handle.is_finished();
                }
                false
            })
        {
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
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    fn post_rendering(&mut self, _window_size_px: [u32; 2], _frame: &Frame) {
        for job in &mut self.view_state.jobs {
            if let Some(handle) = &job.handle {
                if !handle.is_finished() {
                    continue;
                }
                match job.handle.take().unwrap().join() {
                    Ok(result) => {
                        log::info!("Job {} finished", job.id);
                        match result {
                            JobResult::None => {
                                if let Some(err) = &job.status.read().unwrap().error {
                                    log::error!("{:?}", err);
                                }
                            }
                            JobResult::ObjDiff(state) => {
                                self.view_state.build = Some(state);
                            }
                            JobResult::BinDiff(state) => {
                                self.view_state.build = Some(Box::new(ObjDiffResult {
                                    first_status: BuildStatus {
                                        success: true,
                                        log: "".to_string(),
                                    },
                                    second_status: BuildStatus {
                                        success: true,
                                        log: "".to_string(),
                                    },
                                    first_obj: Some(state.first_obj),
                                    second_obj: Some(state.second_obj),
                                    time: OffsetDateTime::now_utc(),
                                }));
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to join job handle: {:?}", e);
                    }
                }
            }
        }
        if self.view_state.jobs.iter().any(|v| v.should_remove) {
            let mut i = 0;
            while i < self.view_state.jobs.len() {
                let job = &self.view_state.jobs[i];
                if job.should_remove
                    && job.handle.is_none()
                    && job.status.read().unwrap().error.is_none()
                {
                    self.view_state.jobs.remove(i);
                } else {
                    i += 1;
                }
            }
        }

        if let Ok(mut config) = self.config.write() {
            if config.project_dir_change {
                drop(self.watcher.take());
                if let Some(project_dir) = &config.project_dir {
                    match create_watcher(self.modified.clone(), project_dir) {
                        Ok(watcher) => self.watcher = Some(watcher),
                        Err(e) => eprintln!("Failed to create watcher: {}", e),
                    }
                    config.project_dir_change = false;
                    self.modified.store(true, Ordering::Relaxed);
                }
            }

            if config.obj_path.is_some() && self.modified.load(Ordering::Relaxed) {
                if !self
                    .view_state
                    .jobs
                    .iter()
                    .any(|j| j.job_type == Job::ObjDiff && j.handle.is_some())
                {
                    self.view_state.jobs.push(queue_build(
                        self.config.clone(),
                        self.view_state.diff_config.clone(),
                    ));
                }
                self.modified.store(false, Ordering::Relaxed);
            }
        }
    }
}

fn create_watcher(
    modified: Arc<AtomicBool>,
    project_dir: &Path,
) -> notify::Result<notify::RecommendedWatcher> {
    let mut watcher =
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
            Ok(event) => {
                if matches!(event.kind, notify::EventKind::Modify(..)) {
                    let watch_extensions = &[
                        Some(OsStr::new("c")),
                        Some(OsStr::new("cp")),
                        Some(OsStr::new("cpp")),
                        Some(OsStr::new("h")),
                        Some(OsStr::new("hpp")),
                    ];
                    if event.paths.iter().any(|p| watch_extensions.contains(&p.extension())) {
                        modified.store(true, Ordering::Relaxed);
                    }
                }
            }
            Err(e) => println!("watch error: {:?}", e),
        })?;
    watcher.watch(project_dir, RecursiveMode::Recursive)?;
    Ok(watcher)
}

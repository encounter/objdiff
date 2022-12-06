use std::{
    default::Default,
    ffi::OsStr,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
    time::Duration,
};

use egui::{Color32, FontFamily, FontId, TextStyle};
use notify::{RecursiveMode, Watcher};
use time::{OffsetDateTime, UtcOffset};

use crate::{
    jobs::{
        check_update::{queue_check_update, CheckUpdateResult},
        objdiff::{queue_build, BuildStatus, ObjDiffResult},
        Job, JobResult, JobState, JobStatus,
    },
    views::{
        config::config_ui, data_diff::data_diff_ui, function_diff::function_diff_ui, jobs::jobs_ui,
        symbol_diff::symbol_diff_ui,
    },
};

#[allow(clippy::enum_variant_names)]
#[derive(Default, Eq, PartialEq)]
pub enum View {
    #[default]
    SymbolDiff,
    FunctionDiff,
    DataDiff,
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

const DEFAULT_COLOR_ROTATION: [Color32; 9] = [
    Color32::from_rgb(255, 0, 255),
    Color32::from_rgb(0, 255, 255),
    Color32::from_rgb(0, 128, 0),
    Color32::from_rgb(255, 0, 0),
    Color32::from_rgb(255, 255, 0),
    Color32::from_rgb(255, 192, 203),
    Color32::from_rgb(0, 0, 255),
    Color32::from_rgb(0, 255, 0),
    Color32::from_rgb(128, 128, 128),
];

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ViewConfig {
    pub ui_font: FontId,
    pub code_font: FontId,
    pub diff_colors: Vec<Color32>,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            ui_font: FontId { size: 14.0, family: FontFamily::Proportional },
            code_font: FontId { size: 14.0, family: FontFamily::Monospace },
            diff_colors: DEFAULT_COLOR_ROTATION.to_vec(),
        }
    }
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
    pub show_demangle: bool,
    #[serde(skip)]
    pub demangle_text: String,
    #[serde(skip)]
    pub diff_config: DiffConfig,
    #[serde(skip)]
    pub search: String,
    #[serde(skip)]
    pub utc_offset: UtcOffset,
    #[serde(skip)]
    pub check_update: Option<Box<CheckUpdateResult>>,
    // Config
    pub diff_kind: DiffKind,
    pub reverse_fn_order: bool,
    pub view_config: ViewConfig,
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
            show_demangle: false,
            demangle_text: String::new(),
            diff_config: Default::default(),
            search: Default::default(),
            utc_offset: UtcOffset::UTC,
            check_update: None,
            diff_kind: Default::default(),
            reverse_fn_order: false,
            view_config: Default::default(),
        }
    }
}

#[derive(Default, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct AppConfig {
    pub custom_make: Option<String>,
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
    #[serde(skip)]
    pub queue_update_check: bool,
    pub auto_update_check: bool,
}

#[derive(Default, Clone, serde::Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub custom_make: Option<String>,
    pub project_dir: Option<PathBuf>,
    pub target_obj_dir: Option<PathBuf>,
    pub base_obj_dir: Option<PathBuf>,
    pub build_target: bool,
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
    #[serde(skip)]
    relaunch_path: Rc<Mutex<Option<PathBuf>>>,
    #[serde(skip)]
    should_relaunch: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            view_state: ViewState::default(),
            config: Arc::new(Default::default()),
            modified: Arc::new(Default::default()),
            watcher: None,
            relaunch_path: Default::default(),
            should_relaunch: false,
        }
    }
}

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
        if let Some(storage) = cc.storage {
            let mut app: App = eframe::get_value(storage, eframe::APP_KEY).unwrap_or_default();
            let mut config: AppConfig = eframe::get_value(storage, CONFIG_KEY).unwrap_or_default();
            if config.project_dir.is_some() {
                config.project_dir_change = true;
            }
            config.queue_update_check = config.auto_update_check;
            app.config = Arc::new(RwLock::new(config));
            app.view_state.utc_offset = utc_offset;
            app.relaunch_path = relaunch_path;
            app
        } else {
            let mut app = Self::default();
            app.view_state.utc_offset = utc_offset;
            app.relaunch_path = relaunch_path;
            app
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

        let Self { config, view_state, .. } = self;

        {
            let config = &view_state.view_config;
            let mut style = (*ctx.style()).clone();
            style.text_styles.insert(TextStyle::Body, FontId {
                size: (config.ui_font.size * 0.75).floor(),
                family: config.ui_font.family.clone(),
            });
            style.text_styles.insert(TextStyle::Body, config.ui_font.clone());
            style.text_styles.insert(TextStyle::Button, config.ui_font.clone());
            style.text_styles.insert(TextStyle::Heading, FontId {
                size: (config.ui_font.size * 1.5).floor(),
                family: config.ui_font.family.clone(),
            });
            style.text_styles.insert(TextStyle::Monospace, config.code_font.clone());
            ctx.set_style(style);
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Show config").clicked() {
                        view_state.show_config = !view_state.show_config;
                    }
                    if ui.button("Quit").clicked() {
                        frame.close();
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui.button("Demangle").clicked() {
                        view_state.show_demangle = !view_state.show_demangle;
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
        } else if view_state.current_view == View::DataDiff
            && matches!(&view_state.build, Some(b) if b.first_status.success && b.second_status.success)
        {
            egui::CentralPanel::default().show(ctx, |ui| {
                if data_diff_ui(ui, view_state) {
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
            ui.label("UI font:");
            egui::introspection::font_id_ui(ui, &mut view_state.view_config.ui_font);
            ui.separator();
            ui.label("Code font:");
            egui::introspection::font_id_ui(ui, &mut view_state.view_config.code_font);
            ui.separator();
            ui.label("Diff colors:");
            if ui.button("Reset").clicked() {
                view_state.view_config.diff_colors = DEFAULT_COLOR_ROTATION.to_vec();
            }
            let mut remove_at: Option<usize> = None;
            let num_colors = view_state.view_config.diff_colors.len();
            for (idx, color) in view_state.view_config.diff_colors.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.color_edit_button_srgba(color);
                    if num_colors > 1 && ui.small_button("-").clicked() {
                        remove_at = Some(idx);
                    }
                });
            }
            if let Some(idx) = remove_at {
                view_state.view_config.diff_colors.remove(idx);
            }
            if ui.small_button("+").clicked() {
                view_state.view_config.diff_colors.push(Color32::BLACK);
            }
        });

        egui::Window::new("Demangle").open(&mut view_state.show_demangle).show(ctx, |ui| {
            ui.text_edit_singleline(&mut view_state.demangle_text);
            ui.add_space(10.0);
            if let Some(demangled) =
                cwdemangle::demangle(&view_state.demangle_text, &Default::default())
            {
                ui.scope(|ui| {
                    ui.style_mut().override_text_style = Some(TextStyle::Monospace);
                    ui.colored_label(Color32::LIGHT_BLUE, &demangled);
                });
                if ui.button("Copy").clicked() {
                    ui.output().copied_text = demangled;
                }
            } else {
                ui.scope(|ui| {
                    ui.style_mut().override_text_style = Some(TextStyle::Monospace);
                    ui.colored_label(Color32::LIGHT_RED, "[invalid]");
                });
            }
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

    fn post_rendering(&mut self, _window_size_px: [u32; 2], _frame: &eframe::Frame) {
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
                            JobResult::CheckUpdate(state) => {
                                self.view_state.check_update = Some(state);
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
                        Err(e) => eprintln!("Failed to create watcher: {e}"),
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

            if config.queue_update_check {
                self.view_state.jobs.push(queue_check_update());
                config.queue_update_check = false;
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
                        Some(OsStr::new("s")),
                    ];
                    if event.paths.iter().any(|p| watch_extensions.contains(&p.extension())) {
                        modified.store(true, Ordering::Relaxed);
                    }
                }
            }
            Err(e) => println!("watch error: {e:?}"),
        })?;
    watcher.watch(project_dir, RecursiveMode::Recursive)?;
    Ok(watcher)
}

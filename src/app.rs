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

use egui::{Color32, FontFamily, FontId, TextStyle};
use globset::{Glob, GlobSet, GlobSetBuilder};
use notify::{RecursiveMode, Watcher};
use time::{OffsetDateTime, UtcOffset};

use crate::{
    config::{build_globset, load_project_config, ProjectUnit, ProjectUnitNode, CONFIG_FILENAMES},
    jobs::{
        check_update::{start_check_update, CheckUpdateResult},
        objdiff::{start_build, BuildStatus, ObjDiffResult},
        Job, JobQueue, JobResult, JobStatus,
    },
    views::{
        appearance::{appearance_window, DEFAULT_COLOR_ROTATION},
        config::{config_ui, project_window},
        data_diff::data_diff_ui,
        demangle::demangle_window,
        function_diff::function_diff_ui,
        jobs::jobs_ui,
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

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ViewConfig {
    pub ui_font: FontId,
    pub code_font: FontId,
    pub diff_colors: Vec<Color32>,
    pub reverse_fn_order: bool,
    pub theme: eframe::Theme,
    #[serde(skip)]
    pub text_color: Color32, // GRAY
    #[serde(skip)]
    pub emphasized_text_color: Color32, // LIGHT_GRAY
    #[serde(skip)]
    pub deemphasized_text_color: Color32, // DARK_GRAY
    #[serde(skip)]
    pub highlight_color: Color32, // WHITE
    #[serde(skip)]
    pub replace_color: Color32, // LIGHT_BLUE
    #[serde(skip)]
    pub insert_color: Color32, // GREEN
    #[serde(skip)]
    pub delete_color: Color32, // RED
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            ui_font: FontId { size: 12.0, family: FontFamily::Proportional },
            code_font: FontId { size: 14.0, family: FontFamily::Monospace },
            diff_colors: DEFAULT_COLOR_ROTATION.to_vec(),
            reverse_fn_order: false,
            theme: eframe::Theme::Dark,
            text_color: Color32::GRAY,
            emphasized_text_color: Color32::LIGHT_GRAY,
            deemphasized_text_color: Color32::DARK_GRAY,
            highlight_color: Color32::WHITE,
            replace_color: Color32::LIGHT_BLUE,
            insert_color: Color32::GREEN,
            delete_color: Color32::from_rgb(200, 40, 41),
        }
    }
}

pub struct SymbolReference {
    pub symbol_name: String,
    pub section_name: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ViewState {
    #[serde(skip)]
    pub jobs: JobQueue,
    #[serde(skip)]
    pub build: Option<Box<ObjDiffResult>>,
    #[serde(skip)]
    pub highlighted_symbol: Option<String>,
    #[serde(skip)]
    pub selected_symbol: Option<SymbolReference>,
    #[serde(skip)]
    pub current_view: View,
    #[serde(skip)]
    pub show_view_config: bool,
    #[serde(skip)]
    pub show_project_config: bool,
    #[serde(skip)]
    pub show_demangle: bool,
    #[serde(skip)]
    pub demangle_text: String,
    #[serde(skip)]
    pub watch_pattern_text: String,
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
    pub view_config: ViewConfig,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            jobs: Default::default(),
            build: None,
            highlighted_symbol: None,
            selected_symbol: None,
            current_view: Default::default(),
            show_view_config: false,
            show_project_config: false,
            show_demangle: false,
            demangle_text: String::new(),
            watch_pattern_text: String::new(),
            diff_config: Default::default(),
            search: Default::default(),
            utc_offset: UtcOffset::UTC,
            check_update: None,
            diff_kind: Default::default(),
            view_config: Default::default(),
        }
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
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
    pub watcher_change: bool,
    pub watcher_enabled: bool,
    #[serde(skip)]
    pub queue_update_check: bool,
    pub auto_update_check: bool,
    // Project config
    #[serde(skip)]
    pub config_change: bool,
    #[serde(skip)]
    pub watch_patterns: Vec<Glob>,
    #[serde(skip)]
    pub load_error: Option<String>,
    #[serde(skip)]
    pub units: Vec<ProjectUnit>,
    #[serde(skip)]
    pub unit_nodes: Vec<ProjectUnitNode>,
    #[serde(skip)]
    pub config_window_open: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            custom_make: None,
            available_wsl_distros: None,
            selected_wsl_distro: None,
            project_dir: None,
            target_obj_dir: None,
            base_obj_dir: None,
            obj_path: None,
            build_target: false,
            left_obj: None,
            right_obj: None,
            config_change: false,
            watcher_change: false,
            watcher_enabled: true,
            queue_update_check: false,
            auto_update_check: false,
            watch_patterns: vec![],
            load_error: None,
            units: vec![],
            unit_nodes: vec![],
            config_window_open: false,
        }
    }
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
    config_modified: Arc<AtomicBool>,
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
            config_modified: Arc::new(Default::default()),
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
                config.config_change = true;
                config.watcher_change = true;
                app.modified.store(true, Ordering::Relaxed);
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
            let config = &mut view_state.view_config;
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
            match config.theme {
                eframe::Theme::Dark => {
                    style.visuals = egui::Visuals::dark();
                    config.text_color = Color32::GRAY;
                    config.emphasized_text_color = Color32::LIGHT_GRAY;
                    config.deemphasized_text_color = Color32::DARK_GRAY;
                    config.highlight_color = Color32::WHITE;
                    config.replace_color = Color32::LIGHT_BLUE;
                    config.insert_color = Color32::GREEN;
                    config.delete_color = Color32::from_rgb(200, 40, 41);
                }
                eframe::Theme::Light => {
                    style.visuals = egui::Visuals::light();
                    config.text_color = Color32::GRAY;
                    config.emphasized_text_color = Color32::DARK_GRAY;
                    config.deemphasized_text_color = Color32::LIGHT_GRAY;
                    config.highlight_color = Color32::BLACK;
                    config.replace_color = Color32::DARK_BLUE;
                    config.insert_color = Color32::DARK_GREEN;
                    config.delete_color = Color32::from_rgb(200, 40, 41);
                }
            }
            ctx.set_style(style);
        }

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Appearance…").clicked() {
                        view_state.show_view_config = !view_state.show_view_config;
                    }
                    if ui.button("Quit").clicked() {
                        frame.close();
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui.button("Demangle…").clicked() {
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
                        .push(start_build(config.clone(), view_state.diff_config.clone()));
                }
            });
        } else if view_state.current_view == View::DataDiff
            && matches!(&view_state.build, Some(b) if b.first_status.success && b.second_status.success)
        {
            egui::CentralPanel::default().show(ctx, |ui| {
                if data_diff_ui(ui, view_state) {
                    view_state
                        .jobs
                        .push(start_build(config.clone(), view_state.diff_config.clone()));
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

        project_window(ctx, config, view_state);
        appearance_window(ctx, view_state);
        demangle_window(ctx, view_state);

        // Windows + request_repaint_after breaks dialogs:
        // https://github.com/emilk/egui/issues/2003
        if cfg!(windows) || view_state.jobs.any_running() {
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
        for (job, result) in self.view_state.jobs.iter_finished() {
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
                            self.view_state.build = Some(state);
                        }
                        JobResult::BinDiff(state) => {
                            self.view_state.build = Some(Box::new(ObjDiffResult {
                                first_status: BuildStatus { success: true, log: "".to_string() },
                                second_status: BuildStatus { success: true, log: "".to_string() },
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
        self.view_state.jobs.clear_finished();

        if let Ok(mut config) = self.config.write() {
            let config = &mut *config;

            if self.config_modified.swap(false, Ordering::Relaxed) {
                config.config_change = true;
            }

            if config.config_change {
                config.config_change = false;
                if let Err(e) = load_project_config(config) {
                    log::error!("Failed to load project config: {e}");
                    config.load_error = Some(format!("{e}"));
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
                && !self.view_state.jobs.is_running(Job::ObjDiff)
            {
                self.view_state
                    .jobs
                    .push(start_build(self.config.clone(), self.view_state.diff_config.clone()));
            }

            if config.queue_update_check {
                self.view_state.jobs.push(start_check_update());
                config.queue_update_check = false;
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

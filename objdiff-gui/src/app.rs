use std::{
    default::Default,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, RwLock,
    },
    time::Instant,
};

use filetime::FileTime;
use globset::{Glob, GlobSet};
use notify::{RecursiveMode, Watcher};
use objdiff_core::{
    config::{
        build_globset, ProjectConfigInfo, ProjectObject, ScratchConfig, DEFAULT_WATCH_PATTERNS,
    },
    diff::DiffObjConfig,
};
use time::UtcOffset;

use crate::{
    app_config::{deserialize_config, AppConfigVersion},
    config::{load_project_config, ProjectObjectNode},
    jobs::{
        objdiff::{start_build, ObjDiffConfig},
        Job, JobQueue, JobResult, JobStatus,
    },
    views::{
        appearance::{appearance_window, Appearance},
        config::{
            arch_config_window, config_ui, project_window, ConfigViewState, CONFIG_DISABLED_TEXT,
        },
        data_diff::data_diff_ui,
        debug::debug_window,
        demangle::{demangle_window, DemangleViewState},
        extab_diff::extab_diff_ui,
        frame_history::FrameHistory,
        function_diff::function_diff_ui,
        graphics::{graphics_window, GraphicsConfig, GraphicsViewState},
        jobs::{jobs_menu_ui, jobs_window},
        rlwinm::{rlwinm_decode_window, RlwinmDecodeViewState},
        symbol_diff::{symbol_diff_ui, DiffViewState, View},
    },
};

pub struct ViewState {
    pub jobs: JobQueue,
    pub config_state: ConfigViewState,
    pub demangle_state: DemangleViewState,
    pub rlwinm_decode_state: RlwinmDecodeViewState,
    pub diff_state: DiffViewState,
    pub graphics_state: GraphicsViewState,
    pub frame_history: FrameHistory,
    pub show_appearance_config: bool,
    pub show_demangle: bool,
    pub show_rlwinm_decode: bool,
    pub show_project_config: bool,
    pub show_arch_config: bool,
    pub show_debug: bool,
    pub show_graphics: bool,
    pub show_jobs: bool,
    pub show_side_panel: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            jobs: Default::default(),
            config_state: Default::default(),
            demangle_state: Default::default(),
            rlwinm_decode_state: Default::default(),
            diff_state: Default::default(),
            graphics_state: Default::default(),
            frame_history: Default::default(),
            show_appearance_config: false,
            show_demangle: false,
            show_rlwinm_decode: false,
            show_project_config: false,
            show_arch_config: false,
            show_debug: false,
            show_graphics: false,
            show_jobs: false,
            show_side_panel: true,
        }
    }
}

/// The configuration for a single object file.
#[derive(Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ObjectConfig {
    pub name: String,
    pub target_path: Option<PathBuf>,
    pub base_path: Option<PathBuf>,
    pub reverse_fn_order: Option<bool>,
    pub complete: Option<bool>,
    pub scratch: Option<ScratchConfig>,
    pub source_path: Option<String>,
}

#[inline]
fn bool_true() -> bool { true }

#[inline]
fn default_watch_patterns() -> Vec<Glob> {
    DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect()
}

pub struct AppState {
    pub config: AppConfig,
    pub objects: Vec<ProjectObject>,
    pub object_nodes: Vec<ProjectObjectNode>,
    pub watcher_change: bool,
    pub config_change: bool,
    pub obj_change: bool,
    pub queue_build: bool,
    pub queue_reload: bool,
    pub project_config_info: Option<ProjectConfigInfo>,
    pub last_mod_check: Instant,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            config: Default::default(),
            objects: vec![],
            object_nodes: vec![],
            watcher_change: false,
            config_change: false,
            obj_change: false,
            queue_build: false,
            queue_reload: false,
            project_config_info: None,
            last_mod_check: Instant::now(),
        }
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct AppConfig {
    // TODO: https://github.com/ron-rs/ron/pull/455
    // #[serde(flatten)]
    // pub version: AppConfigVersion,
    pub version: u32,
    #[serde(default)]
    pub custom_make: Option<String>,
    #[serde(default)]
    pub custom_args: Option<Vec<String>>,
    #[serde(default)]
    pub selected_wsl_distro: Option<String>,
    #[serde(default)]
    pub project_dir: Option<PathBuf>,
    #[serde(default)]
    pub target_obj_dir: Option<PathBuf>,
    #[serde(default)]
    pub base_obj_dir: Option<PathBuf>,
    #[serde(default)]
    pub selected_obj: Option<ObjectConfig>,
    #[serde(default = "bool_true")]
    pub build_base: bool,
    #[serde(default)]
    pub build_target: bool,
    #[serde(default = "bool_true")]
    pub rebuild_on_changes: bool,
    #[serde(default)]
    pub auto_update_check: bool,
    #[serde(default = "default_watch_patterns")]
    pub watch_patterns: Vec<Glob>,
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
    #[serde(default)]
    pub diff_obj_config: DiffObjConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: AppConfigVersion::default().version,
            custom_make: None,
            custom_args: None,
            selected_wsl_distro: None,
            project_dir: None,
            target_obj_dir: None,
            base_obj_dir: None,
            selected_obj: None,
            build_base: true,
            build_target: false,
            rebuild_on_changes: true,
            auto_update_check: true,
            watch_patterns: DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect(),
            recent_projects: vec![],
            diff_obj_config: Default::default(),
        }
    }
}

impl AppState {
    pub fn set_project_dir(&mut self, path: PathBuf) {
        self.config.recent_projects.retain(|p| p != &path);
        if self.config.recent_projects.len() > 9 {
            self.config.recent_projects.truncate(9);
        }
        self.config.recent_projects.insert(0, path.clone());
        self.config.project_dir = Some(path);
        self.config.target_obj_dir = None;
        self.config.base_obj_dir = None;
        self.config.selected_obj = None;
        self.config.build_target = false;
        self.objects.clear();
        self.object_nodes.clear();
        self.watcher_change = true;
        self.config_change = true;
        self.obj_change = true;
        self.queue_build = false;
        self.project_config_info = None;
    }

    pub fn set_target_obj_dir(&mut self, path: PathBuf) {
        self.config.target_obj_dir = Some(path);
        self.config.selected_obj = None;
        self.obj_change = true;
        self.queue_build = false;
    }

    pub fn set_base_obj_dir(&mut self, path: PathBuf) {
        self.config.base_obj_dir = Some(path);
        self.config.selected_obj = None;
        self.obj_change = true;
        self.queue_build = false;
    }

    pub fn set_selected_obj(&mut self, object: ObjectConfig) {
        self.config.selected_obj = Some(object);
        self.obj_change = true;
        self.queue_build = false;
    }
}

pub type AppStateRef = Arc<RwLock<AppState>>;

#[derive(Default)]
pub struct App {
    appearance: Appearance,
    view_state: ViewState,
    state: AppStateRef,
    modified: Arc<AtomicBool>,
    watcher: Option<notify::RecommendedWatcher>,
    app_path: Option<PathBuf>,
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
        app_path: Option<PathBuf>,
        graphics_config: GraphicsConfig,
        graphics_config_path: Option<PathBuf>,
    ) -> Self {
        // Load previous app state (if any).
        // Note that you must enable the `persistence` feature for this to work.
        let mut app = Self::default();
        if let Some(storage) = cc.storage {
            if let Some(appearance) = eframe::get_value::<Appearance>(storage, APPEARANCE_KEY) {
                app.appearance = appearance;
            }
            if let Some(config) = deserialize_config(storage) {
                let mut state = AppState { config, ..Default::default() };
                if state.config.project_dir.is_some() {
                    state.config_change = true;
                    state.watcher_change = true;
                }
                if state.config.selected_obj.is_some() {
                    state.queue_build = true;
                }
                state.config.diff_obj_config.symbol_overrides.clear();
                app.view_state.config_state.queue_check_update = state.config.auto_update_check;
                app.state = Arc::new(RwLock::new(state));
            }
        }
        app.appearance.init_fonts(&cc.egui_ctx);
        app.appearance.utc_offset = utc_offset;
        app.app_path = app_path;
        app.relaunch_path = relaunch_path;
        #[cfg(feature = "wgpu")]
        if let Some(wgpu_render_state) = &cc.wgpu_render_state {
            use eframe::egui_wgpu::wgpu::Backend;
            let info = wgpu_render_state.adapter.get_info();
            app.view_state.graphics_state.active_backend = match info.backend {
                Backend::Empty => "Unknown",
                Backend::Vulkan => "Vulkan",
                Backend::Metal => "Metal",
                Backend::Dx12 => "DirectX 12",
                Backend::Gl => "OpenGL",
                Backend::BrowserWebGpu => "WebGPU",
            }
            .to_string();
            app.view_state.graphics_state.active_device.clone_from(&info.name);
        }
        #[cfg(feature = "glow")]
        if let Some(gl) = &cc.gl {
            use eframe::glow::HasContext;
            app.view_state.graphics_state.active_backend = "OpenGL (Fallback)".to_string();
            app.view_state.graphics_state.active_device =
                unsafe { gl.get_parameter_string(0x1F01) }; // GL_RENDERER
        }
        app.view_state.graphics_state.graphics_config = graphics_config;
        app.view_state.graphics_state.graphics_config_path = graphics_config_path;
        app
    }

    fn pre_update(&mut self, ctx: &egui::Context) {
        self.appearance.pre_update(ctx);

        let ViewState { jobs, diff_state, config_state, .. } = &mut self.view_state;

        let mut results = vec![];
        for (job, result) in jobs.iter_finished() {
            match result {
                Ok(result) => {
                    log::info!("Job {} finished", job.id);
                    match result {
                        JobResult::None => {
                            if let Some(err) = &job.context.status.read().unwrap().error {
                                log::error!("{:?}", err);
                            }
                        }
                        JobResult::Update(state) => {
                            if let Ok(mut guard) = self.relaunch_path.lock() {
                                *guard = Some(state.exe_path);
                                self.should_relaunch = true;
                            }
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
                    let result = job.context.status.write();
                    if let Ok(mut guard) = result {
                        guard.error = Some(err);
                    } else {
                        drop(result);
                        job.context.status = Arc::new(RwLock::new(JobStatus {
                            title: "Error".to_string(),
                            progress_percent: 0.0,
                            progress_items: None,
                            status: String::new(),
                            error: Some(err),
                        }));
                    }
                }
            }
        }
        jobs.results.append(&mut results);
        jobs.clear_finished();

        diff_state.pre_update(jobs, &self.state);
        config_state.pre_update(jobs, &self.state);
        debug_assert!(jobs.results.is_empty());
    }

    fn post_update(&mut self, ctx: &egui::Context) {
        self.appearance.post_update(ctx);

        let ViewState { jobs, diff_state, config_state, graphics_state, .. } = &mut self.view_state;
        config_state.post_update(ctx, jobs, &self.state);
        diff_state.post_update(ctx, jobs, &self.state);

        let Ok(mut state) = self.state.write() else {
            return;
        };
        let state = &mut *state;

        if let Some(info) = &state.project_config_info {
            if file_modified(&info.path, info.timestamp) {
                state.config_change = true;
            }
        }

        if state.config_change {
            state.config_change = false;
            match load_project_config(state) {
                Ok(()) => config_state.load_error = None,
                Err(e) => {
                    log::error!("Failed to load project config: {e}");
                    config_state.load_error = Some(format!("{e}"));
                }
            }
        }

        if state.watcher_change {
            drop(self.watcher.take());

            if let Some(project_dir) = &state.config.project_dir {
                match build_globset(&state.config.watch_patterns)
                    .map_err(anyhow::Error::new)
                    .and_then(|globset| {
                        create_watcher(ctx.clone(), self.modified.clone(), project_dir, globset)
                            .map_err(anyhow::Error::new)
                    }) {
                    Ok(watcher) => self.watcher = Some(watcher),
                    Err(e) => log::error!("Failed to create watcher: {e}"),
                }
                state.watcher_change = false;
            }
        }

        if state.obj_change {
            *diff_state = Default::default();
            if state.config.selected_obj.is_some() {
                state.queue_build = true;
            }
            state.obj_change = false;
        }

        if self.modified.swap(false, Ordering::Relaxed) && state.config.rebuild_on_changes {
            state.queue_build = true;
        }

        if let Some(result) = &diff_state.build {
            if state.last_mod_check.elapsed().as_millis() >= 500 {
                state.last_mod_check = Instant::now();
                if let Some((obj, _)) = &result.first_obj {
                    if let (Some(path), Some(timestamp)) = (&obj.path, obj.timestamp) {
                        if file_modified(path, timestamp) {
                            state.queue_reload = true;
                        }
                    }
                }
                if let Some((obj, _)) = &result.second_obj {
                    if let (Some(path), Some(timestamp)) = (&obj.path, obj.timestamp) {
                        if file_modified(path, timestamp) {
                            state.queue_reload = true;
                        }
                    }
                }
            }
        }

        // Don't clear `queue_build` if a build is running. A file may have been modified during
        // the build, so we'll start another build after the current one finishes.
        if state.queue_build
            && state.config.selected_obj.is_some()
            && !jobs.is_running(Job::ObjDiff)
        {
            jobs.push(start_build(ctx, ObjDiffConfig::from_config(&state.config)));
            state.queue_build = false;
            state.queue_reload = false;
        } else if state.queue_reload && !jobs.is_running(Job::ObjDiff) {
            let mut diff_config = ObjDiffConfig::from_config(&state.config);
            // Don't build, just reload the current files
            diff_config.build_base = false;
            diff_config.build_target = false;
            jobs.push(start_build(ctx, diff_config));
            state.queue_reload = false;
        }

        if graphics_state.should_relaunch {
            if let Some(app_path) = &self.app_path {
                if let Ok(mut guard) = self.relaunch_path.lock() {
                    *guard = Some(app_path.clone());
                    self.should_relaunch = true;
                }
            }
        }
    }
}

impl eframe::App for App {
    /// Called each time the UI needs repainting, which may be many times per second.
    /// Put your widgets into a `SidePanel`, `TopPanel`, `CentralPanel`, `Window` or `Area`.
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if self.should_relaunch {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        self.pre_update(ctx);

        let Self { state, appearance, view_state, .. } = self;
        let ViewState {
            jobs,
            config_state,
            demangle_state,
            rlwinm_decode_state,
            diff_state,
            graphics_state,
            frame_history,
            show_appearance_config,
            show_demangle,
            show_rlwinm_decode,
            show_project_config,
            show_arch_config,
            show_debug,
            show_graphics,
            show_jobs,
            show_side_panel,
        } = view_state;

        frame_history.on_new_frame(ctx.input(|i| i.time), frame.info().cpu_usage);

        let side_panel_available = diff_state.current_view == View::SymbolDiff;

        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                if ui
                    .add_enabled(
                        side_panel_available,
                        egui::Button::new(if *show_side_panel { "⏴" } else { "⏵" }),
                    )
                    .on_hover_text("Toggle side panel")
                    .clicked()
                {
                    *show_side_panel = !*show_side_panel;
                }
                ui.separator();
                ui.menu_button("File", |ui| {
                    #[cfg(debug_assertions)]
                    if ui.button("Debug…").clicked() {
                        *show_debug = !*show_debug;
                        ui.close_menu();
                    }
                    if ui.button("Project…").clicked() {
                        *show_project_config = !*show_project_config;
                        ui.close_menu();
                    }
                    let recent_projects = if let Ok(guard) = state.read() {
                        guard.config.recent_projects.clone()
                    } else {
                        vec![]
                    };
                    if recent_projects.is_empty() {
                        ui.add_enabled(false, egui::Button::new("Recent projects…"));
                    } else {
                        ui.menu_button("Recent Projects…", |ui| {
                            if ui.button("Clear").clicked() {
                                state.write().unwrap().config.recent_projects.clear();
                            };
                            ui.separator();
                            for path in recent_projects {
                                if ui.button(format!("{}", path.display())).clicked() {
                                    state.write().unwrap().set_project_dir(path);
                                    ui.close_menu();
                                }
                            }
                        });
                    }
                    if ui.button("Appearance…").clicked() {
                        *show_appearance_config = !*show_appearance_config;
                        ui.close_menu();
                    }
                    if ui.button("Graphics…").clicked() {
                        *show_graphics = !*show_graphics;
                        ui.close_menu();
                    }
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Tools", |ui| {
                    if ui.button("Demangle…").clicked() {
                        *show_demangle = !*show_demangle;
                        ui.close_menu();
                    }
                    if ui.button("Rlwinm Decoder…").clicked() {
                        *show_rlwinm_decode = !*show_rlwinm_decode;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Diff Options", |ui| {
                    if ui.button("Arch Settings…").clicked() {
                        *show_arch_config = !*show_arch_config;
                        ui.close_menu();
                    }
                    let mut state = state.write().unwrap();
                    let response = ui
                        .checkbox(&mut state.config.rebuild_on_changes, "Rebuild on changes")
                        .on_hover_text("Automatically re-run the build & diff when files change.");
                    if response.changed() {
                        state.watcher_change = true;
                    };
                    ui.add_enabled(
                        !diff_state.symbol_state.disable_reverse_fn_order,
                        egui::Checkbox::new(
                            &mut diff_state.symbol_state.reverse_fn_order,
                            "Reverse function order (-inline deferred)",
                        ),
                    )
                    .on_disabled_hover_text(CONFIG_DISABLED_TEXT);
                    ui.checkbox(
                        &mut diff_state.symbol_state.show_hidden_symbols,
                        "Show hidden symbols",
                    );
                    if ui
                        .checkbox(
                            &mut state.config.diff_obj_config.relax_reloc_diffs,
                            "Relax relocation diffs",
                        )
                        .on_hover_text(
                            "Ignores differences in relocation targets. (Address, name, etc)",
                        )
                        .changed()
                    {
                        state.queue_reload = true;
                    }
                    if ui
                        .checkbox(
                            &mut state.config.diff_obj_config.space_between_args,
                            "Space between args",
                        )
                        .changed()
                    {
                        state.queue_reload = true;
                    }
                    if ui
                        .checkbox(
                            &mut state.config.diff_obj_config.combine_data_sections,
                            "Combine data sections",
                        )
                        .on_hover_text("Combines data sections with equal names.")
                        .changed()
                    {
                        state.queue_reload = true;
                    }
                });
                ui.separator();
                if jobs_menu_ui(ui, jobs, appearance) {
                    *show_jobs = !*show_jobs;
                }
            });
        });

        if side_panel_available {
            egui::SidePanel::left("side_panel").show_animated(ctx, *show_side_panel, |ui| {
                egui::ScrollArea::both().show(ui, |ui| {
                    config_ui(ui, state, show_project_config, config_state, appearance);
                });
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let build_success = matches!(&diff_state.build, Some(b) if b.first_status.success && b.second_status.success);
            if diff_state.current_view == View::FunctionDiff && build_success {
                function_diff_ui(ui, diff_state, appearance);
            } else if diff_state.current_view == View::DataDiff && build_success {
                data_diff_ui(ui, diff_state, appearance);
            } else if diff_state.current_view == View::ExtabDiff && build_success {
                extab_diff_ui(ui, diff_state, appearance);
            } else {
                symbol_diff_ui(ui, diff_state, appearance);
            }
        });

        project_window(ctx, state, show_project_config, config_state, appearance);
        appearance_window(ctx, show_appearance_config, appearance);
        demangle_window(ctx, show_demangle, demangle_state, appearance);
        rlwinm_decode_window(ctx, show_rlwinm_decode, rlwinm_decode_state, appearance);
        arch_config_window(ctx, state, show_arch_config, appearance);
        debug_window(ctx, show_debug, frame_history, appearance);
        graphics_window(ctx, show_graphics, frame_history, graphics_state, appearance);
        jobs_window(ctx, show_jobs, jobs, appearance);

        self.post_update(ctx);
    }

    /// Called by the frame work to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(state) = self.state.read() {
            eframe::set_value(storage, CONFIG_KEY, &state.config);
        }
        eframe::set_value(storage, APPEARANCE_KEY, &self.appearance);
    }
}

fn create_watcher(
    ctx: egui::Context,
    modified: Arc<AtomicBool>,
    project_dir: &Path,
    patterns: GlobSet,
) -> notify::Result<notify::RecommendedWatcher> {
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
                        if patterns.is_match(path) {
                            log::info!("File modified: {}", path.display());
                            modified.store(true, Ordering::Relaxed);
                            ctx.request_repaint();
                        }
                    }
                }
            }
            Err(e) => log::error!("watch error: {e:?}"),
        })?;
    watcher.watch(project_dir, RecursiveMode::Recursive)?;
    Ok(watcher)
}

#[inline]
fn file_modified(path: &Path, last_ts: FileTime) -> bool {
    if let Ok(metadata) = fs::metadata(path) {
        FileTime::from_last_modification_time(&metadata) != last_ts
    } else {
        false
    }
}

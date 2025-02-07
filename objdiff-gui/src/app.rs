use std::{
    collections::BTreeMap,
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
use globset::Glob;
use objdiff_core::{
    build::watcher::{create_watcher, Watcher},
    config::{
        build_globset, default_watch_patterns, path::platform_path_serde_option,
        save_project_config, ProjectConfig, ProjectConfigInfo, ProjectObject, ScratchConfig,
        DEFAULT_WATCH_PATTERNS,
    },
    diff::DiffObjConfig,
    jobs::{Job, JobQueue, JobResult},
};
use time::UtcOffset;
use typed_path::{Utf8PlatformPath, Utf8PlatformPathBuf};

use crate::{
    app_config::{deserialize_config, AppConfigVersion},
    config::{load_project_config, ProjectObjectNode},
    jobs::{create_objdiff_config, egui_waker, start_build},
    views::{
        appearance::{appearance_window, Appearance},
        config::{
            arch_config_window, config_ui, general_config_ui, project_window, ConfigViewState,
            CONFIG_DISABLED_TEXT,
        },
        debug::debug_window,
        demangle::{demangle_window, DemangleViewState},
        diff::diff_view_ui,
        frame_history::FrameHistory,
        graphics::{graphics_window, GraphicsConfig, GraphicsViewState},
        jobs::{jobs_menu_ui, jobs_window},
        rlwinm::{rlwinm_decode_window, RlwinmDecodeViewState},
        symbol_diff::{DiffViewAction, DiffViewNavigation, DiffViewState, View},
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
#[derive(Default, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ObjectConfig {
    pub name: String,
    #[serde(default, with = "platform_path_serde_option")]
    pub target_path: Option<Utf8PlatformPathBuf>,
    #[serde(default, with = "platform_path_serde_option")]
    pub base_path: Option<Utf8PlatformPathBuf>,
    pub reverse_fn_order: Option<bool>,
    pub complete: Option<bool>,
    #[serde(default)]
    pub hidden: bool,
    pub scratch: Option<ScratchConfig>,
    #[serde(default, with = "platform_path_serde_option")]
    pub source_path: Option<Utf8PlatformPathBuf>,
    #[serde(default)]
    pub symbol_mappings: BTreeMap<String, String>,
}

impl ObjectConfig {
    pub fn new(
        object: &ProjectObject,
        project_dir: &Utf8PlatformPath,
        target_obj_dir: Option<&Utf8PlatformPath>,
        base_obj_dir: Option<&Utf8PlatformPath>,
    ) -> Self {
        let target_path = if let (Some(target_obj_dir), Some(path), None) =
            (target_obj_dir, &object.path, &object.target_path)
        {
            Some(target_obj_dir.join(path.with_platform_encoding()))
        } else if let Some(path) = &object.target_path {
            Some(project_dir.join(path.with_platform_encoding()))
        } else {
            None
        };
        let base_path = if let (Some(base_obj_dir), Some(path), None) =
            (base_obj_dir, &object.path, &object.base_path)
        {
            Some(base_obj_dir.join(path.with_platform_encoding()))
        } else if let Some(path) = &object.base_path {
            Some(project_dir.join(path.with_platform_encoding()))
        } else {
            None
        };
        let source_path =
            object.source_path().map(|s| project_dir.join(s.with_platform_encoding()));
        Self {
            name: object.name().to_string(),
            target_path,
            base_path,
            reverse_fn_order: object.reverse_fn_order(),
            complete: object.complete(),
            hidden: object.hidden(),
            scratch: object.scratch.clone(),
            source_path,
            symbol_mappings: object.symbol_mappings.clone().unwrap_or_default(),
        }
    }
}

#[inline]
fn bool_true() -> bool { true }

pub struct AppState {
    pub config: AppConfig,
    pub objects: Vec<ObjectConfig>,
    pub object_nodes: Vec<ProjectObjectNode>,
    pub watcher_change: bool,
    pub config_change: bool,
    pub obj_change: bool,
    pub queue_build: bool,
    pub queue_reload: bool,
    pub current_project_config: Option<ProjectConfig>,
    pub project_config_info: Option<ProjectConfigInfo>,
    pub last_mod_check: Instant,
    /// The right object symbol name that we're selecting a left symbol for
    pub selecting_left: Option<String>,
    /// The left object symbol name that we're selecting a right symbol for
    pub selecting_right: Option<String>,
    pub config_error: Option<String>,
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
            current_project_config: None,
            project_config_info: None,
            last_mod_check: Instant::now(),
            selecting_left: None,
            selecting_right: None,
            config_error: None,
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
    #[serde(default, with = "platform_path_serde_option")]
    pub project_dir: Option<Utf8PlatformPathBuf>,
    #[serde(default, with = "platform_path_serde_option")]
    pub target_obj_dir: Option<Utf8PlatformPathBuf>,
    #[serde(default, with = "platform_path_serde_option")]
    pub base_obj_dir: Option<Utf8PlatformPathBuf>,
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
    pub recent_projects: Vec<String>,
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
    pub fn set_project_dir(&mut self, path: Utf8PlatformPathBuf) {
        self.config.recent_projects.retain(|p| p != &path);
        if self.config.recent_projects.len() > 9 {
            self.config.recent_projects.truncate(9);
        }
        self.config.recent_projects.insert(0, path.to_string());
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
        self.current_project_config = None;
        self.project_config_info = None;
        self.selecting_left = None;
        self.selecting_right = None;
    }

    pub fn set_target_obj_dir(&mut self, path: Utf8PlatformPathBuf) {
        self.config.target_obj_dir = Some(path);
        self.config.selected_obj = None;
        self.obj_change = true;
        self.queue_build = false;
        self.selecting_left = None;
        self.selecting_right = None;
    }

    pub fn set_base_obj_dir(&mut self, path: Utf8PlatformPathBuf) {
        self.config.base_obj_dir = Some(path);
        self.config.selected_obj = None;
        self.obj_change = true;
        self.queue_build = false;
        self.selecting_left = None;
        self.selecting_right = None;
    }

    pub fn set_selected_obj(&mut self, config: ObjectConfig) {
        let mut unit_changed = true;
        if let Some(existing) = self.config.selected_obj.as_ref() {
            if existing == &config {
                // Don't reload the object if there were no changes
                return;
            }
            if existing.name == config.name {
                unit_changed = false;
            }
        }
        self.config.selected_obj = Some(config);
        if unit_changed {
            self.obj_change = true;
            self.queue_build = false;
            self.selecting_left = None;
            self.selecting_right = None;
        } else {
            self.queue_build = true;
        }
    }

    pub fn clear_selected_obj(&mut self) {
        self.config.selected_obj = None;
        self.obj_change = true;
        self.queue_build = false;
        self.selecting_left = None;
        self.selecting_right = None;
    }

    pub fn set_selecting_left(&mut self, right: &str) {
        let Some(object) = self.config.selected_obj.as_mut() else {
            return;
        };
        object.symbol_mappings.retain(|_, r| r != right);
        self.selecting_left = Some(right.to_string());
        self.queue_reload = true;
        self.save_config();
    }

    pub fn set_selecting_right(&mut self, left: &str) {
        let Some(object) = self.config.selected_obj.as_mut() else {
            return;
        };
        object.symbol_mappings.retain(|l, _| l != left);
        self.selecting_right = Some(left.to_string());
        self.queue_reload = true;
        self.save_config();
    }

    pub fn set_symbol_mapping(&mut self, left: String, right: String) {
        let Some(object) = self.config.selected_obj.as_mut() else {
            log::warn!("No selected object");
            return;
        };
        self.selecting_left = None;
        self.selecting_right = None;
        object.symbol_mappings.retain(|l, r| l != &left && r != &right);
        if left != right {
            object.symbol_mappings.insert(left.clone(), right.clone());
        }
        self.queue_reload = true;
        self.save_config();
    }

    pub fn clear_selection(&mut self) {
        self.selecting_left = None;
        self.selecting_right = None;
        self.queue_reload = true;
    }

    pub fn clear_mappings(&mut self) {
        self.selecting_left = None;
        self.selecting_right = None;
        if let Some(object) = self.config.selected_obj.as_mut() {
            object.symbol_mappings.clear();
        }
        self.queue_reload = true;
        self.save_config();
    }

    pub fn is_selecting_symbol(&self) -> bool {
        self.selecting_left.is_some() || self.selecting_right.is_some()
    }

    pub fn save_config(&mut self) {
        let (Some(config), Some(info)) =
            (self.current_project_config.as_mut(), self.project_config_info.as_mut())
        else {
            return;
        };
        // Update the project config with the current state
        if let Some(object) = self.config.selected_obj.as_ref() {
            if let Some(existing) = config.units.as_mut().and_then(|v| {
                v.iter_mut().find(|u| u.name.as_ref().is_some_and(|n| n == &object.name))
            }) {
                existing.symbol_mappings = if object.symbol_mappings.is_empty() {
                    None
                } else {
                    Some(object.symbol_mappings.clone())
                };
            }
            if let Some(existing) = self.objects.iter_mut().find(|u| u.name == object.name) {
                existing.symbol_mappings = object.symbol_mappings.clone();
            }
        }
        // Save the updated project config
        match save_project_config(config, info) {
            Ok(new_info) => *info = new_info,
            Err(e) => {
                log::error!("Failed to save project config: {e}");
                self.config_error = Some(format!("Failed to save project config: {e}"));
            }
        }
    }
}

pub type AppStateRef = Arc<RwLock<AppState>>;

#[derive(Default)]
pub struct App {
    appearance: Appearance,
    view_state: ViewState,
    state: AppStateRef,
    modified: Arc<AtomicBool>,
    watcher: Option<Watcher>,
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

        jobs.collect_results();
        jobs.results.retain(|result| match result {
            JobResult::Update(state) => {
                if let Ok(mut guard) = self.relaunch_path.lock() {
                    *guard = Some(state.exe_path.clone());
                    self.should_relaunch = true;
                }
                false
            }
            _ => true,
        });
        diff_state.pre_update(jobs, &self.state);
        config_state.pre_update(jobs, &self.state);
        debug_assert!(jobs.results.is_empty());
    }

    fn post_update(&mut self, ctx: &egui::Context, action: Option<DiffViewAction>) {
        if action.is_some() {
            ctx.request_repaint();
        }

        self.appearance.post_update(ctx);

        let ViewState { jobs, diff_state, config_state, graphics_state, .. } = &mut self.view_state;
        config_state.post_update(ctx, jobs, &self.state);
        diff_state.post_update(action, ctx, jobs, &self.state);

        let Ok(mut state) = self.state.write() else {
            return;
        };
        let state = &mut *state;

        let mut mod_check = false;
        if state.last_mod_check.elapsed().as_millis() >= 500 {
            state.last_mod_check = Instant::now();
            mod_check = true;
        }

        if mod_check {
            if let Some(info) = &state.project_config_info {
                if let Some(last_ts) = info.timestamp {
                    if file_modified(&info.path, last_ts) {
                        state.config_change = true;
                    }
                }
            }
        }

        if state.config_change {
            state.config_change = false;
            match load_project_config(state) {
                Ok(()) => state.config_error = None,
                Err(e) => {
                    log::error!("Failed to load project config: {e}");
                    state.config_error = Some(format!("{e}"));
                }
            }
        }

        if state.watcher_change {
            drop(self.watcher.take());

            if let Some(project_dir) = &state.config.project_dir {
                match build_globset(&state.config.watch_patterns)
                    .map_err(anyhow::Error::new)
                    .and_then(|globset| {
                        create_watcher(
                            self.modified.clone(),
                            project_dir.as_ref(),
                            globset,
                            egui_waker(ctx),
                        )
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
            if mod_check {
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
            start_build(ctx, jobs, create_objdiff_config(state));
            state.queue_build = false;
            state.queue_reload = false;
        } else if state.queue_reload && !jobs.is_running(Job::ObjDiff) {
            let mut diff_config = create_objdiff_config(state);
            // Don't build, just reload the current files
            diff_config.build_base = false;
            diff_config.build_target = false;
            start_build(ctx, jobs, diff_config);
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
                                if ui.button(&path).clicked() {
                                    state
                                        .write()
                                        .unwrap()
                                        .set_project_dir(Utf8PlatformPathBuf::from(path));
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
                    ui.separator();
                    general_config_ui(ui, &mut state);
                    ui.separator();
                    if ui.button("Clear custom symbol mappings").clicked() {
                        state.clear_mappings();
                        diff_state.post_build_nav = Some(DiffViewNavigation::symbol_diff());
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

        let mut action = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            action = diff_view_ui(ui, diff_state, appearance);
        });

        project_window(ctx, state, show_project_config, config_state, appearance);
        appearance_window(ctx, show_appearance_config, appearance);
        demangle_window(ctx, show_demangle, demangle_state, appearance);
        rlwinm_decode_window(ctx, show_rlwinm_decode, rlwinm_decode_state, appearance);
        arch_config_window(ctx, state, show_arch_config, appearance);
        debug_window(ctx, show_debug, frame_history, appearance);
        graphics_window(ctx, show_graphics, frame_history, graphics_state, appearance);
        jobs_window(ctx, show_jobs, jobs, appearance);

        self.post_update(ctx, action);
    }

    /// Called by the framework to save state before shutdown.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(state) = self.state.read() {
            eframe::set_value(storage, CONFIG_KEY, &state.config);
        }
        eframe::set_value(storage, APPEARANCE_KEY, &self.appearance);
    }
}

#[inline]
fn file_modified<P: AsRef<Path>>(path: P, last_ts: FileTime) -> bool {
    if let Ok(metadata) = fs::metadata(path.as_ref()) {
        FileTime::from_last_modification_time(&metadata) != last_ts
    } else {
        false
    }
}

#[cfg(all(windows, feature = "wsl"))]
use std::string::FromUtf16Error;
use std::{mem::take, path::MAIN_SEPARATOR};

#[cfg(all(windows, feature = "wsl"))]
use anyhow::{Context, Result};
use egui::{
    output::OpenUrl, text::LayoutJob, CollapsingHeader, FontFamily, FontId, RichText,
    SelectableLabel, TextFormat, Widget,
};
use globset::Glob;
use objdiff_core::{
    config::{path::check_path_buf, DEFAULT_WATCH_PATTERNS},
    diff::{
        ConfigEnum, ConfigEnumVariantInfo, ConfigPropertyId, ConfigPropertyKind,
        ConfigPropertyValue, CONFIG_GROUPS,
    },
    jobs::{check_update::CheckUpdateResult, Job, JobQueue, JobResult},
};
use typed_path::Utf8PlatformPathBuf;

use crate::{
    app::{AppConfig, AppState, AppStateRef, ObjectConfig},
    config::ProjectObjectNode,
    hotkeys,
    jobs::{start_check_update, start_update},
    update::RELEASE_URL,
    views::{
        appearance::Appearance,
        file::{FileDialogResult, FileDialogState},
    },
};

#[derive(Default)]
pub struct ConfigViewState {
    pub check_update: Option<Box<CheckUpdateResult>>,
    pub check_update_running: bool,
    pub queue_check_update: bool,
    pub update_running: bool,
    pub queue_update: Option<String>,
    pub build_running: bool,
    pub queue_build: bool,
    pub watch_pattern_text: String,
    pub object_search: String,
    pub filter_diffable: bool,
    pub filter_incomplete: bool,
    pub show_hidden: bool,
    #[cfg(all(windows, feature = "wsl"))]
    pub available_wsl_distros: Option<Vec<String>>,
    pub file_dialog_state: FileDialogState,
}

impl ConfigViewState {
    pub fn pre_update(&mut self, jobs: &mut JobQueue, state: &AppStateRef) {
        jobs.results.retain_mut(|result| {
            if let JobResult::CheckUpdate(result) = result {
                self.check_update = take(result);
                false
            } else {
                true
            }
        });
        self.build_running = jobs.is_running(Job::ObjDiff);
        self.check_update_running = jobs.is_running(Job::CheckUpdate);
        self.update_running = jobs.is_running(Job::Update);

        // Check async file dialog results
        match self.file_dialog_state.poll() {
            FileDialogResult::None => {}
            FileDialogResult::ProjectDir(path) => {
                let mut guard = state.write().unwrap();
                guard.set_project_dir(path.to_path_buf());
            }
            FileDialogResult::TargetDir(path) => {
                let mut guard = state.write().unwrap();
                guard.set_target_obj_dir(path.to_path_buf());
            }
            FileDialogResult::BaseDir(path) => {
                let mut guard = state.write().unwrap();
                guard.set_base_obj_dir(path.to_path_buf());
            }
            FileDialogResult::Object(path) => {
                let mut guard = state.write().unwrap();
                if let (Some(base_dir), Some(target_dir)) =
                    (&guard.config.base_obj_dir, &guard.config.target_obj_dir)
                {
                    if let Ok(obj_path) = path.strip_prefix(base_dir) {
                        let target_path = target_dir.join(obj_path);
                        guard.set_selected_obj(ObjectConfig {
                            name: obj_path.to_string(),
                            target_path: Some(target_path),
                            base_path: Some(path),
                            ..Default::default()
                        });
                    } else if let Ok(obj_path) = path.strip_prefix(target_dir) {
                        let base_path = base_dir.join(obj_path);
                        guard.set_selected_obj(ObjectConfig {
                            name: obj_path.to_string(),
                            target_path: Some(path),
                            base_path: Some(base_path),
                            ..Default::default()
                        });
                    }
                }
            }
        }
    }

    pub fn post_update(&mut self, ctx: &egui::Context, jobs: &mut JobQueue, state: &AppStateRef) {
        if self.queue_build {
            self.queue_build = false;
            if let Ok(mut state) = state.write() {
                state.queue_build = true;
            }
        }

        if self.queue_check_update {
            self.queue_check_update = false;
            start_check_update(ctx, jobs);
        }

        if let Some(bin_name) = self.queue_update.take() {
            start_update(ctx, jobs, bin_name);
        }
    }
}

#[cfg(all(windows, feature = "wsl"))]
fn process_utf16(bytes: &[u8]) -> Result<String, FromUtf16Error> {
    let u16_bytes: Vec<u16> = bytes
        .chunks_exact(2)
        .filter_map(|c| Some(u16::from_ne_bytes(c.try_into().ok()?)))
        .collect();
    String::from_utf16(&u16_bytes)
}

#[cfg(all(windows, feature = "wsl"))]
fn wsl_cmd(args: &[&str]) -> Result<String> {
    use std::{os::windows::process::CommandExt, process::Command};
    let output = Command::new("wsl")
        .args(args)
        .creation_flags(winapi::um::winbase::CREATE_NO_WINDOW)
        .output()
        .context("Failed to execute wsl")?;
    process_utf16(&output.stdout).context("Failed to process stdout")
}

#[cfg(all(windows, feature = "wsl"))]
fn fetch_wsl2_distros() -> Vec<String> {
    wsl_cmd(&["-l", "-q"])
        .map(|stdout| {
            stdout
                .split('\n')
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

pub fn config_ui(
    ui: &mut egui::Ui,
    state: &AppStateRef,
    show_config_window: &mut bool,
    config_state: &mut ConfigViewState,
    appearance: &Appearance,
) {
    let mut state_guard = state.write().unwrap();
    let AppState {
        config: AppConfig { target_obj_dir, base_obj_dir, selected_obj, auto_update_check, .. },
        objects,
        object_nodes,
        ..
    } = &mut *state_guard;

    ui.heading("Updates");
    ui.checkbox(auto_update_check, "Check for updates on startup");
    if ui.add_enabled(!config_state.check_update_running, egui::Button::new("Check now")).clicked()
    {
        config_state.queue_check_update = true;
    }
    ui.label(format!("Current version: {}", env!("CARGO_PKG_VERSION")));
    if let Some(result) = &config_state.check_update {
        ui.label(format!("Latest version: {}", result.latest_release.version));
        if result.update_available {
            ui.colored_label(appearance.insert_color, "Update available");
            ui.horizontal(|ui| {
                if let Some(bin_name) = &result.found_binary {
                    if ui
                        .add_enabled(!config_state.update_running, egui::Button::new("Automatic"))
                        .on_hover_text_at_pointer(
                            "Automatically download and replace the current build",
                        )
                        .clicked()
                    {
                        config_state.queue_update = Some(bin_name.clone());
                    }
                }
                if ui
                    .button("Manual")
                    .on_hover_text_at_pointer("Open a link to the latest release on GitHub")
                    .clicked()
                {
                    ui.ctx().open_url(OpenUrl { url: RELEASE_URL.to_string(), new_tab: true });
                }
            });
        }
    }
    ui.separator();

    ui.horizontal(|ui| {
        ui.heading("Project");
        if ui.button(RichText::new("Settings")).clicked() {
            *show_config_window = true;
        }
    });

    let selected_index = selected_obj
        .as_ref()
        .and_then(|selected_obj| objects.iter().position(|obj| obj.name == selected_obj.name));
    let mut new_selected_index = selected_index;
    if objects.is_empty() {
        if let (Some(_base_dir), Some(target_dir)) = (base_obj_dir, target_obj_dir) {
            if ui.button("Select object").clicked() {
                config_state.file_dialog_state.queue(
                    || {
                        Box::pin(
                            rfd::AsyncFileDialog::new()
                                .set_directory(target_dir)
                                .add_filter("Object file", &["o", "elf", "obj"])
                                .pick_file(),
                        )
                    },
                    FileDialogResult::Object,
                );
            }
            if let Some(obj) = selected_obj {
                ui.label(
                    RichText::new(&obj.name)
                        .color(appearance.replace_color)
                        .family(FontFamily::Monospace),
                );
            }
        } else {
            ui.colored_label(appearance.delete_color, "Missing project settings");
        }
    } else {
        let had_search = !config_state.object_search.is_empty();
        let response =
            egui::TextEdit::singleline(&mut config_state.object_search).hint_text("Filter").ui(ui);
        if hotkeys::consume_object_filter_shortcut(ui.ctx()) {
            response.request_focus();
        }

        let mut root_open = None;
        let mut node_open = NodeOpen::Default;
        ui.horizontal(|ui| {
            if ui.small_button("⏶").on_hover_text_at_pointer("Collapse all").clicked() {
                root_open = Some(false);
                node_open = NodeOpen::Close;
            }
            if ui.small_button("⏷").on_hover_text_at_pointer("Expand all").clicked() {
                root_open = Some(true);
                node_open = NodeOpen::Open;
            }
            if ui
                .add_enabled(selected_obj.is_some(), egui::Button::new("⌖").small())
                .on_hover_text_at_pointer("Current object")
                .clicked()
            {
                root_open = Some(true);
                node_open = NodeOpen::Object;
            }
            let mut filters_text = RichText::new("Filter ⏷");
            if config_state.filter_diffable
                || config_state.filter_incomplete
                || config_state.show_hidden
            {
                filters_text = filters_text.color(appearance.replace_color);
            }
            egui::menu::menu_button(ui, filters_text, |ui| {
                ui.checkbox(&mut config_state.filter_diffable, "Diffable")
                    .on_hover_text_at_pointer("Only show objects with a source file");
                ui.checkbox(&mut config_state.filter_incomplete, "Incomplete")
                    .on_hover_text_at_pointer("Only show objects not marked complete");
                ui.checkbox(&mut config_state.show_hidden, "Hidden")
                    .on_hover_text_at_pointer("Show hidden (auto-generated) objects");
            });
        });
        if config_state.object_search.is_empty() {
            if had_search {
                root_open = Some(true);
                node_open = NodeOpen::Object;
            }
        } else if !had_search {
            root_open = Some(true);
            node_open = NodeOpen::Open;
        }

        CollapsingHeader::new(RichText::new("🗀 Objects").font(FontId {
            size: appearance.ui_font.size,
            family: appearance.code_font.family.clone(),
        }))
        .open(root_open)
        .default_open(true)
        .show(ui, |ui| {
            let search = config_state.object_search.to_ascii_lowercase();
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            for node in object_nodes.iter().filter_map(|node| {
                filter_node(
                    objects,
                    node,
                    &search,
                    config_state.filter_diffable,
                    config_state.filter_incomplete,
                    config_state.show_hidden,
                )
            }) {
                display_node(ui, &mut new_selected_index, objects, &node, appearance, node_open);
            }
        });
    }
    if new_selected_index != selected_index {
        if let Some(idx) = new_selected_index {
            // Will set obj_changed, which will trigger a rebuild
            let config = objects[idx].clone();
            state_guard.set_selected_obj(config);
        }
    }
    if state_guard.config.selected_obj.is_some()
        && ui.add_enabled(!config_state.build_running, egui::Button::new("Build")).clicked()
    {
        config_state.queue_build = true;
    }
}

fn display_unit(
    ui: &mut egui::Ui,
    selected_obj: &mut Option<usize>,
    name: &str,
    units: &[ObjectConfig],
    index: usize,
    appearance: &Appearance,
) {
    let object = &units[index];
    let selected = *selected_obj == Some(index);
    let color = if selected {
        appearance.emphasized_text_color
    } else if let Some(complete) = object.complete {
        if complete {
            appearance.insert_color
        } else {
            appearance.delete_color
        }
    } else {
        appearance.text_color
    };
    let response = SelectableLabel::new(
        selected,
        RichText::new(name)
            .font(FontId {
                size: appearance.ui_font.size,
                family: appearance.code_font.family.clone(),
            })
            .color(color),
    )
    .ui(ui);
    if object.source_path.is_some() {
        response.context_menu(|ui| object_context_ui(ui, object));
    }
    if response.clicked() {
        *selected_obj = Some(index);
    }
}

fn object_context_ui(ui: &mut egui::Ui, object: &ObjectConfig) {
    if let Some(source_path) = &object.source_path {
        if ui
            .button("Open source file")
            .on_hover_text("Open the source file in the default editor")
            .clicked()
        {
            log::info!("Opening file {}", source_path);
            if let Err(e) = open::that_detached(source_path.as_str()) {
                log::error!("Failed to open source file: {e}");
            }
            ui.close_menu();
        }
    }
}

#[derive(Default, Copy, Clone, PartialEq, Eq, Debug)]
enum NodeOpen {
    #[default]
    Default,
    Open,
    Close,
    Object,
}

fn display_node(
    ui: &mut egui::Ui,
    selected_obj: &mut Option<usize>,
    units: &[ObjectConfig],
    node: &ProjectObjectNode,
    appearance: &Appearance,
    node_open: NodeOpen,
) {
    match node {
        ProjectObjectNode::Unit(name, idx) => {
            display_unit(ui, selected_obj, name, units, *idx, appearance);
        }
        ProjectObjectNode::Dir(name, children) => {
            let contains_obj = selected_obj.map(|idx| contains_node(node, idx));
            let open = match node_open {
                NodeOpen::Default => None,
                NodeOpen::Open => Some(true),
                NodeOpen::Close => Some(false),
                NodeOpen::Object => contains_obj,
            };
            let color = if contains_obj == Some(true) {
                appearance.replace_color
            } else {
                appearance.text_color
            };
            CollapsingHeader::new(
                RichText::new(name)
                    .font(FontId {
                        size: appearance.ui_font.size,
                        family: appearance.code_font.family.clone(),
                    })
                    .color(color),
            )
            .open(open)
            .show(ui, |ui| {
                for node in children {
                    display_node(ui, selected_obj, units, node, appearance, node_open);
                }
            });
        }
    }
}

fn contains_node(node: &ProjectObjectNode, selected_obj: usize) -> bool {
    match node {
        ProjectObjectNode::Unit(_, idx) => *idx == selected_obj,
        ProjectObjectNode::Dir(_, children) => {
            children.iter().any(|node| contains_node(node, selected_obj))
        }
    }
}

fn filter_node(
    units: &[ObjectConfig],
    node: &ProjectObjectNode,
    search: &str,
    filter_diffable: bool,
    filter_incomplete: bool,
    show_hidden: bool,
) -> Option<ProjectObjectNode> {
    match node {
        ProjectObjectNode::Unit(name, idx) => {
            let unit = &units[*idx];
            if (search.is_empty() || name.to_ascii_lowercase().contains(search))
                && (!filter_diffable || (unit.base_path.is_some() && unit.target_path.is_some()))
                && (!filter_incomplete || matches!(unit.complete, None | Some(false)))
                && (show_hidden || !unit.hidden)
            {
                Some(node.clone())
            } else {
                None
            }
        }
        ProjectObjectNode::Dir(name, children) => {
            let new_children = children
                .iter()
                .filter_map(|child| {
                    filter_node(
                        units,
                        child,
                        search,
                        filter_diffable,
                        filter_incomplete,
                        show_hidden,
                    )
                })
                .collect::<Vec<_>>();
            if !new_children.is_empty() {
                Some(ProjectObjectNode::Dir(name.clone(), new_children))
            } else {
                None
            }
        }
    }
}

const HELP_ICON: &str = "ℹ";

fn subheading(ui: &mut egui::Ui, text: &str, appearance: &Appearance) {
    ui.label(
        RichText::new(text).size(appearance.ui_font.size).color(appearance.emphasized_text_color),
    );
}

fn format_path(path: &Option<Utf8PlatformPathBuf>, appearance: &Appearance) -> RichText {
    let mut color = appearance.replace_color;
    let text = if let Some(dir) = path {
        if let Some(rel) = dirs::home_dir()
            .and_then(|home| check_path_buf(home).ok())
            .and_then(|home| dir.strip_prefix(&home).ok())
        {
            format!("~{}{}", MAIN_SEPARATOR, rel)
        } else {
            dir.to_string()
        }
    } else {
        color = appearance.delete_color;
        "[none]".to_string()
    };
    RichText::new(text).color(color).family(FontFamily::Monospace)
}

pub const CONFIG_DISABLED_TEXT: &str =
    "Option disabled because it's set by the project configuration file.";

fn pick_folder_ui(
    ui: &mut egui::Ui,
    dir: &Option<Utf8PlatformPathBuf>,
    label: &str,
    tooltip: impl FnOnce(&mut egui::Ui),
    appearance: &Appearance,
    enabled: bool,
) -> egui::Response {
    let response = ui.horizontal(|ui| {
        subheading(ui, label, appearance);
        ui.link(HELP_ICON).on_hover_ui(tooltip);
        ui.add_enabled(enabled, egui::Button::new("Select"))
            .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
    });
    ui.label(format_path(dir, appearance));
    response.inner
}

pub fn project_window(
    ctx: &egui::Context,
    state: &AppStateRef,
    show: &mut bool,
    config_state: &mut ConfigViewState,
    appearance: &Appearance,
) {
    let mut state_guard = state.write().unwrap();

    egui::Window::new("Project").open(show).show(ctx, |ui| {
        split_obj_config_ui(ui, &mut state_guard, config_state, appearance);
    });

    if let Some(error) = &state_guard.config_error {
        let mut open = true;
        egui::Window::new("Error").open(&mut open).show(ctx, |ui| {
            ui.label("Failed to load project config:");
            ui.colored_label(appearance.delete_color, error);
        });
        if !open {
            state_guard.config_error = None;
        }
    }
}

fn split_obj_config_ui(
    ui: &mut egui::Ui,
    state: &mut AppState,
    config_state: &mut ConfigViewState,
    appearance: &Appearance,
) {
    let text_format = TextFormat::simple(appearance.ui_font.clone(), appearance.text_color);
    let code_format = TextFormat::simple(
        FontId { size: appearance.ui_font.size, family: appearance.code_font.family.clone() },
        appearance.emphasized_text_color,
    );

    let response = pick_folder_ui(
        ui,
        &state.config.project_dir,
        "Project directory",
        |ui| {
            let mut job = LayoutJob::default();
            job.append("The root project directory.\n\n", 0.0, text_format.clone());
            job.append(
                "If a configuration file exists, it will be loaded automatically.",
                0.0,
                text_format.clone(),
            );
            ui.label(job);
        },
        appearance,
        true,
    );
    if response.clicked() {
        config_state.file_dialog_state.queue(
            || Box::pin(rfd::AsyncFileDialog::new().pick_folder()),
            FileDialogResult::ProjectDir,
        );
    }
    ui.separator();

    ui.horizontal(|ui| {
        subheading(ui, "Build program", appearance);
        ui.link(HELP_ICON).on_hover_ui(|ui| {
            let mut job = LayoutJob::default();
            job.append("By default, objdiff will build with ", 0.0, text_format.clone());
            job.append("make", 0.0, code_format.clone());
            job.append(
                ".\nIf the project uses a different build system (e.g. ",
                0.0,
                text_format.clone(),
            );
            job.append("ninja", 0.0, code_format.clone());
            job.append(
                "), specify it here.\nThe program must be in your ",
                0.0,
                text_format.clone(),
            );
            job.append("PATH", 0.0, code_format.clone());
            job.append(".", 0.0, text_format.clone());
            ui.label(job);
        });
    });
    let mut custom_make_str = state.config.custom_make.clone().unwrap_or_default();
    if ui
        .add_enabled(
            state.project_config_info.is_none(),
            egui::TextEdit::singleline(&mut custom_make_str).hint_text("make"),
        )
        .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
        .changed()
    {
        if custom_make_str.is_empty() {
            state.config.custom_make = None;
        } else {
            state.config.custom_make = Some(custom_make_str);
        }
    }
    #[cfg(all(windows, feature = "wsl"))]
    {
        if config_state.available_wsl_distros.is_none() {
            config_state.available_wsl_distros = Some(fetch_wsl2_distros());
        }
        egui::ComboBox::from_label("Run in WSL2")
            .selected_text(
                state.config.selected_wsl_distro.as_ref().unwrap_or(&"Disabled".to_string()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.config.selected_wsl_distro, None, "Disabled");
                for distro in config_state.available_wsl_distros.as_ref().unwrap() {
                    ui.selectable_value(
                        &mut state.config.selected_wsl_distro,
                        Some(distro.clone()),
                        distro,
                    );
                }
            });
    }
    ui.separator();

    if let Some(project_dir) = state.config.project_dir.clone() {
        let response = pick_folder_ui(
            ui,
            &state.config.target_obj_dir,
            "Target build directory",
            |ui| {
                let mut job = LayoutJob::default();
                job.append(
                    "This contains the \"target\" or \"expected\" objects, which are the intended result of the match.\n\n",
                    0.0,
                    text_format.clone(),
                );
                job.append(
                    "These are usually created by the project's build system or assembled.",
                    0.0,
                    text_format.clone(),
                );
                ui.label(job);
            },
            appearance,
            state.project_config_info.is_none(),
        );
        if response.clicked() {
            config_state.file_dialog_state.queue(
                || Box::pin(rfd::AsyncFileDialog::new().set_directory(&project_dir).pick_folder()),
                FileDialogResult::TargetDir,
            );
        }
        ui.add_enabled(
            state.project_config_info.is_none(),
            egui::Checkbox::new(&mut state.config.build_target, "Build target objects"),
        )
        .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
        .on_hover_ui(|ui| {
            let mut job = LayoutJob::default();
            job.append(
                "Tells the build system to produce the target object.\n",
                0.0,
                text_format.clone(),
            );
            job.append("For example, this would call ", 0.0, text_format.clone());
            job.append("make path/to/target.o", 0.0, code_format.clone());
            job.append(".\n\n", 0.0, text_format.clone());
            job.append(
                "This is useful if the target objects are not already built\n",
                0.0,
                text_format.clone(),
            );
            job.append(
                "or if they can change based on project configuration,\n",
                0.0,
                text_format.clone(),
            );
            job.append(
                "but requires that the build system is configured correctly.",
                0.0,
                text_format.clone(),
            );
            ui.label(job);
        });
        ui.separator();

        let response = pick_folder_ui(
            ui,
            &state.config.base_obj_dir,
            "Base build directory",
            |ui| {
                let mut job = LayoutJob::default();
                job.append(
                    "This contains the objects built from your decompiled code.",
                    0.0,
                    text_format.clone(),
                );
                ui.label(job);
            },
            appearance,
            state.project_config_info.is_none(),
        );
        if response.clicked() {
            config_state.file_dialog_state.queue(
                || Box::pin(rfd::AsyncFileDialog::new().set_directory(&project_dir).pick_folder()),
                FileDialogResult::BaseDir,
            );
        }
        ui.add_enabled(
            state.project_config_info.is_none(),
            egui::Checkbox::new(&mut state.config.build_base, "Build base objects"),
        )
        .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
        .on_hover_ui(|ui| {
            let mut job = LayoutJob::default();
            job.append(
                "Tells the build system to produce the base object.\n",
                0.0,
                text_format.clone(),
            );
            job.append("For example, this would call ", 0.0, text_format.clone());
            job.append("make path/to/base.o", 0.0, code_format.clone());
            job.append(".\n\n", 0.0, text_format.clone());
            job.append(
                "This can be disabled if you're running the build system\n",
                0.0,
                text_format.clone(),
            );
            job.append(
                "externally, and just want objdiff to reload the files\n",
                0.0,
                text_format.clone(),
            );
            job.append("when they change.", 0.0, text_format.clone());
            ui.label(job);
        });
        ui.separator();
    }

    subheading(ui, "Watch settings", appearance);
    let response =
        ui.checkbox(&mut state.config.rebuild_on_changes, "Rebuild on changes").on_hover_ui(|ui| {
            let mut job = LayoutJob::default();
            job.append(
                "Automatically re-run the build & diff when files change.",
                0.0,
                text_format.clone(),
            );
            ui.label(job);
        });
    if response.changed() {
        state.watcher_change = true;
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new("File patterns").color(appearance.text_color));
        if ui
            .add_enabled(state.project_config_info.is_none(), egui::Button::new("Reset"))
            .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
            .clicked()
        {
            state.config.watch_patterns =
                DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect();
            state.watcher_change = true;
        }
    });
    let mut remove_at: Option<usize> = None;
    for (idx, glob) in state.config.watch_patterns.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}", glob))
                    .color(appearance.text_color)
                    .family(FontFamily::Monospace),
            );
            if ui
                .add_enabled(state.project_config_info.is_none(), egui::Button::new("-").small())
                .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
                .clicked()
            {
                remove_at = Some(idx);
            }
        });
    }
    if let Some(idx) = remove_at {
        state.config.watch_patterns.remove(idx);
        state.watcher_change = true;
    }
    ui.horizontal(|ui| {
        ui.add_enabled(
            state.project_config_info.is_none(),
            egui::TextEdit::singleline(&mut config_state.watch_pattern_text).desired_width(100.0),
        )
        .on_disabled_hover_text(CONFIG_DISABLED_TEXT);
        if ui
            .add_enabled(state.project_config_info.is_none(), egui::Button::new("+").small())
            .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
            .clicked()
        {
            if let Ok(glob) = Glob::new(&config_state.watch_pattern_text) {
                state.config.watch_patterns.push(glob);
                state.watcher_change = true;
                config_state.watch_pattern_text.clear();
            }
        }
    });
}

pub fn arch_config_window(
    ctx: &egui::Context,
    state: &AppStateRef,
    show: &mut bool,
    appearance: &Appearance,
) {
    let mut state_guard = state.write().unwrap();
    egui::Window::new("Arch Settings").open(show).show(ctx, |ui| {
        arch_config_ui(ui, &mut state_guard, appearance);
    });
}

fn config_property_ui(
    ui: &mut egui::Ui,
    state: &mut AppState,
    property_id: ConfigPropertyId,
) -> bool {
    let mut changed = false;
    let current_value = state.config.diff_obj_config.get_property_value(property_id);
    match (property_id.kind(), current_value) {
        (ConfigPropertyKind::Boolean, ConfigPropertyValue::Boolean(mut checked)) => {
            let mut response = ui.checkbox(&mut checked, property_id.name());
            if let Some(description) = property_id.description() {
                response = response.on_hover_text(description);
            }
            if response.changed() {
                state
                    .config
                    .diff_obj_config
                    .set_property_value(property_id, ConfigPropertyValue::Boolean(checked))
                    .expect("Failed to set property value");
                changed = true;
            }
        }
        (ConfigPropertyKind::Choice(variants), ConfigPropertyValue::Choice(selected)) => {
            fn variant_name(variant: &ConfigEnumVariantInfo) -> String {
                if variant.is_default {
                    format!("{} (default)", variant.name)
                } else {
                    variant.name.to_string()
                }
            }
            let selected_variant = variants
                .iter()
                .find(|v| v.value == selected)
                .or_else(|| variants.iter().find(|v| v.is_default))
                .expect("Invalid choice variant");
            let response = egui::ComboBox::new(property_id.name(), property_id.name())
                .selected_text(variant_name(selected_variant))
                .show_ui(ui, |ui| {
                    for variant in variants {
                        let mut response =
                            ui.selectable_label(selected == variant.value, variant_name(variant));
                        if let Some(description) = variant.description {
                            response = response.on_hover_text(description);
                        }
                        if response.clicked() {
                            state
                                .config
                                .diff_obj_config
                                .set_property_value(
                                    property_id,
                                    ConfigPropertyValue::Choice(variant.value),
                                )
                                .expect("Failed to set property value");
                            changed = true;
                        }
                    }
                })
                .response;
            if let Some(description) = property_id.description() {
                response.on_hover_text(description);
            }
        }
        _ => panic!("Incompatible property kind and value"),
    }
    changed
}

fn arch_config_ui(ui: &mut egui::Ui, state: &mut AppState, _appearance: &Appearance) {
    let mut first = true;
    let mut changed = false;
    for group in CONFIG_GROUPS {
        if group.id == "general" {
            continue;
        }
        if first {
            first = false;
        } else {
            ui.separator();
        }
        ui.heading(group.name);
        for property_id in group.properties.iter().cloned() {
            changed |= config_property_ui(ui, state, property_id);
        }
    }
    if changed {
        state.queue_reload = true;
    }
}

pub fn general_config_ui(ui: &mut egui::Ui, state: &mut AppState) {
    let mut changed = false;
    let group = CONFIG_GROUPS.iter().find(|group| group.id == "general").unwrap();
    for property_id in group.properties.iter().cloned() {
        changed |= config_property_ui(ui, state, property_id);
    }
    if changed {
        state.queue_reload = true;
    }
}

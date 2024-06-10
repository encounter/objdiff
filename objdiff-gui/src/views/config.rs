#[cfg(all(windows, feature = "wsl"))]
use std::string::FromUtf16Error;
use std::{
    borrow::Cow,
    mem::take,
    path::{PathBuf, MAIN_SEPARATOR},
};

#[cfg(all(windows, feature = "wsl"))]
use anyhow::{Context, Result};
use const_format::formatcp;
use egui::{
    output::OpenUrl, text::LayoutJob, CollapsingHeader, FontFamily, FontId, RichText,
    SelectableLabel, TextFormat, Widget,
};
use globset::Glob;
use objdiff_core::{
    config::{ProjectObject, DEFAULT_WATCH_PATTERNS},
    diff::{ArmArchVersion, MipsAbi, MipsInstrCategory, X86Formatter},
};
use self_update::cargo_crate_version;
use strum::{EnumMessage, VariantArray};

use crate::{
    app::{AppConfig, AppConfigRef, ObjectConfig},
    config::ProjectObjectNode,
    jobs::{
        check_update::{start_check_update, CheckUpdateResult},
        update::start_update,
        Job, JobQueue, JobResult,
    },
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
    pub load_error: Option<String>,
    pub object_search: String,
    pub filter_diffable: bool,
    pub filter_incomplete: bool,
    #[cfg(all(windows, feature = "wsl"))]
    pub available_wsl_distros: Option<Vec<String>>,
    pub file_dialog_state: FileDialogState,
}

impl ConfigViewState {
    pub fn pre_update(&mut self, jobs: &mut JobQueue, config: &AppConfigRef) {
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
                let mut guard = config.write().unwrap();
                guard.set_project_dir(path.to_path_buf());
            }
            FileDialogResult::TargetDir(path) => {
                let mut guard = config.write().unwrap();
                guard.set_target_obj_dir(path.to_path_buf());
            }
            FileDialogResult::BaseDir(path) => {
                let mut guard = config.write().unwrap();
                guard.set_base_obj_dir(path.to_path_buf());
            }
            FileDialogResult::Object(path) => {
                let mut guard = config.write().unwrap();
                if let (Some(base_dir), Some(target_dir)) =
                    (&guard.base_obj_dir, &guard.target_obj_dir)
                {
                    if let Ok(obj_path) = path.strip_prefix(base_dir) {
                        let target_path = target_dir.join(obj_path);
                        guard.set_selected_obj(ObjectConfig {
                            name: obj_path.display().to_string(),
                            target_path: Some(target_path),
                            base_path: Some(path),
                            reverse_fn_order: None,
                            complete: None,
                            scratch: None,
                        });
                    } else if let Ok(obj_path) = path.strip_prefix(target_dir) {
                        let base_path = base_dir.join(obj_path);
                        guard.set_selected_obj(ObjectConfig {
                            name: obj_path.display().to_string(),
                            target_path: Some(path),
                            base_path: Some(base_path),
                            reverse_fn_order: None,
                            complete: None,
                            scratch: None,
                        });
                    }
                }
            }
        }
    }

    pub fn post_update(&mut self, ctx: &egui::Context, jobs: &mut JobQueue, config: &AppConfigRef) {
        if self.queue_build {
            self.queue_build = false;
            if let Ok(mut config) = config.write() {
                config.queue_build = true;
            }
        }

        if self.queue_check_update {
            self.queue_check_update = false;
            jobs.push_once(Job::CheckUpdate, || start_check_update(ctx));
        }

        if let Some(bin_name) = self.queue_update.take() {
            jobs.push_once(Job::Update, || start_update(ctx, bin_name));
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
    config: &AppConfigRef,
    show_config_window: &mut bool,
    state: &mut ConfigViewState,
    appearance: &Appearance,
) {
    let mut config_guard = config.write().unwrap();
    let AppConfig {
        target_obj_dir,
        base_obj_dir,
        selected_obj,
        auto_update_check,
        objects,
        object_nodes,
        ..
    } = &mut *config_guard;

    ui.heading("Updates");
    ui.checkbox(auto_update_check, "Check for updates on startup");
    if ui.add_enabled(!state.check_update_running, egui::Button::new("Check now")).clicked() {
        state.queue_check_update = true;
    }
    ui.label(format!("Current version: {}", cargo_crate_version!())).on_hover_ui_at_pointer(|ui| {
        ui.label(formatcp!("Git branch: {}", env!("VERGEN_GIT_BRANCH")));
        ui.label(formatcp!("Git commit: {}", env!("VERGEN_GIT_SHA")));
        ui.label(formatcp!("Build target: {}", env!("VERGEN_CARGO_TARGET_TRIPLE")));
        ui.label(formatcp!("Debug: {}", env!("VERGEN_CARGO_DEBUG")));
    });
    if let Some(result) = &state.check_update {
        ui.label(format!("Latest version: {}", result.latest_release.version));
        if result.update_available {
            ui.colored_label(appearance.insert_color, "Update available");
            ui.horizontal(|ui| {
                if let Some(bin_name) = &result.found_binary {
                    if ui
                        .add_enabled(!state.update_running, egui::Button::new("Automatic"))
                        .on_hover_text_at_pointer(
                            "Automatically download and replace the current build",
                        )
                        .clicked()
                    {
                        state.queue_update = Some(bin_name.clone());
                    }
                }
                if ui
                    .button("Manual")
                    .on_hover_text_at_pointer("Open a link to the latest release on GitHub")
                    .clicked()
                {
                    ui.output_mut(|output| {
                        output.open_url =
                            Some(OpenUrl { url: RELEASE_URL.to_string(), new_tab: true })
                    });
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

    let mut new_selected_obj = selected_obj.clone();
    if objects.is_empty() {
        if let (Some(_base_dir), Some(target_dir)) = (base_obj_dir, target_obj_dir) {
            if ui.button("Select object").clicked() {
                state.file_dialog_state.queue(
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
        let had_search = !state.object_search.is_empty();
        egui::TextEdit::singleline(&mut state.object_search).hint_text("Filter").ui(ui);

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
            if ui
                .selectable_label(state.filter_diffable, "Diffable")
                .on_hover_text_at_pointer("Only show objects with a source file")
                .clicked()
            {
                state.filter_diffable = !state.filter_diffable;
            }
            if ui
                .selectable_label(state.filter_incomplete, "Incomplete")
                .on_hover_text_at_pointer("Only show objects not marked complete")
                .clicked()
            {
                state.filter_incomplete = !state.filter_incomplete;
            }
        });
        if state.object_search.is_empty() {
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
            let mut nodes = Cow::Borrowed(object_nodes);
            if !state.object_search.is_empty() || state.filter_diffable || state.filter_incomplete {
                let search = state.object_search.to_ascii_lowercase();
                nodes = Cow::Owned(
                    object_nodes
                        .iter()
                        .filter_map(|node| {
                            filter_node(
                                node,
                                &search,
                                state.filter_diffable,
                                state.filter_incomplete,
                            )
                        })
                        .collect(),
                );
            }

            ui.style_mut().wrap = Some(false);
            for node in nodes.iter() {
                display_node(ui, &mut new_selected_obj, node, appearance, node_open);
            }
        });
    }
    if new_selected_obj != *selected_obj {
        if let Some(obj) = new_selected_obj {
            // Will set obj_changed, which will trigger a rebuild
            config_guard.set_selected_obj(obj);
        }
    }
    if config_guard.selected_obj.is_some()
        && ui.add_enabled(!state.build_running, egui::Button::new("Build")).clicked()
    {
        state.queue_build = true;
    }

    ui.separator();
}

fn display_object(
    ui: &mut egui::Ui,
    selected_obj: &mut Option<ObjectConfig>,
    name: &str,
    object: &ProjectObject,
    appearance: &Appearance,
) {
    let object_name = object.name();
    let selected = matches!(selected_obj, Some(obj) if obj.name == object_name);
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
    let clicked = SelectableLabel::new(
        selected,
        RichText::new(name)
            .font(FontId {
                size: appearance.ui_font.size,
                family: appearance.code_font.family.clone(),
            })
            .color(color),
    )
    .ui(ui)
    .clicked();
    // Always recreate ObjectConfig if selected, in case the project config changed.
    // ObjectConfig is compared using equality, so this won't unnecessarily trigger a rebuild.
    if selected || clicked {
        *selected_obj = Some(ObjectConfig {
            name: object_name.to_string(),
            target_path: object.target_path.clone(),
            base_path: object.base_path.clone(),
            reverse_fn_order: object.reverse_fn_order,
            complete: object.complete,
            scratch: object.scratch.clone(),
        });
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
    selected_obj: &mut Option<ObjectConfig>,
    node: &ProjectObjectNode,
    appearance: &Appearance,
    node_open: NodeOpen,
) {
    match node {
        ProjectObjectNode::File(name, object) => {
            display_object(ui, selected_obj, name, object, appearance);
        }
        ProjectObjectNode::Dir(name, children) => {
            let contains_obj = selected_obj.as_ref().map(|path| contains_node(node, path));
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
                    display_node(ui, selected_obj, node, appearance, node_open);
                }
            });
        }
    }
}

fn contains_node(node: &ProjectObjectNode, selected_obj: &ObjectConfig) -> bool {
    match node {
        ProjectObjectNode::File(_, object) => object.name() == selected_obj.name,
        ProjectObjectNode::Dir(_, children) => {
            children.iter().any(|node| contains_node(node, selected_obj))
        }
    }
}

fn filter_node(
    node: &ProjectObjectNode,
    search: &str,
    filter_diffable: bool,
    filter_incomplete: bool,
) -> Option<ProjectObjectNode> {
    match node {
        ProjectObjectNode::File(name, object) => {
            if (search.is_empty() || name.to_ascii_lowercase().contains(search))
                && (!filter_diffable
                    || (object.base_path.is_some() && object.target_path.is_some()))
                && (!filter_incomplete || matches!(object.complete, None | Some(false)))
            {
                Some(node.clone())
            } else {
                None
            }
        }
        ProjectObjectNode::Dir(name, children) => {
            if (search.is_empty() || name.to_ascii_lowercase().contains(search))
                && !filter_diffable
                && !filter_incomplete
            {
                return Some(node.clone());
            }
            let new_children = children
                .iter()
                .filter_map(|child| filter_node(child, search, filter_diffable, filter_incomplete))
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

fn format_path(path: &Option<PathBuf>, appearance: &Appearance) -> RichText {
    let mut color = appearance.replace_color;
    let text = if let Some(dir) = path {
        if let Some(rel) = dirs::home_dir().and_then(|home| dir.strip_prefix(&home).ok()) {
            format!("~{}{}", MAIN_SEPARATOR, rel.display())
        } else {
            format!("{}", dir.display())
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
    dir: &Option<PathBuf>,
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
    config: &AppConfigRef,
    show: &mut bool,
    state: &mut ConfigViewState,
    appearance: &Appearance,
) {
    let mut config_guard = config.write().unwrap();

    egui::Window::new("Project").open(show).show(ctx, |ui| {
        split_obj_config_ui(ui, &mut config_guard, state, appearance);
    });

    if let Some(error) = &state.load_error {
        let mut open = true;
        egui::Window::new("Error").open(&mut open).show(ctx, |ui| {
            ui.label("Failed to load project config:");
            ui.colored_label(appearance.delete_color, error);
        });
        if !open {
            state.load_error = None;
        }
    }
}

fn split_obj_config_ui(
    ui: &mut egui::Ui,
    config: &mut AppConfig,
    state: &mut ConfigViewState,
    appearance: &Appearance,
) {
    let text_format = TextFormat::simple(appearance.ui_font.clone(), appearance.text_color);
    let code_format = TextFormat::simple(
        FontId { size: appearance.ui_font.size, family: appearance.code_font.family.clone() },
        appearance.emphasized_text_color,
    );

    let response = pick_folder_ui(
        ui,
        &config.project_dir,
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
        state.file_dialog_state.queue(
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
    let mut custom_make_str = config.custom_make.clone().unwrap_or_default();
    if ui
        .add_enabled(
            config.project_config_info.is_none(),
            egui::TextEdit::singleline(&mut custom_make_str).hint_text("make"),
        )
        .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
        .changed()
    {
        if custom_make_str.is_empty() {
            config.custom_make = None;
        } else {
            config.custom_make = Some(custom_make_str);
        }
    }
    #[cfg(all(windows, feature = "wsl"))]
    {
        if state.available_wsl_distros.is_none() {
            state.available_wsl_distros = Some(fetch_wsl2_distros());
        }
        egui::ComboBox::from_label("Run in WSL2")
            .selected_text(config.selected_wsl_distro.as_ref().unwrap_or(&"Disabled".to_string()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut config.selected_wsl_distro, None, "Disabled");
                for distro in state.available_wsl_distros.as_ref().unwrap() {
                    ui.selectable_value(
                        &mut config.selected_wsl_distro,
                        Some(distro.clone()),
                        distro,
                    );
                }
            });
    }
    ui.separator();

    if let Some(project_dir) = config.project_dir.clone() {
        let response = pick_folder_ui(
            ui,
            &config.target_obj_dir,
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
            config.project_config_info.is_none(),
        );
        if response.clicked() {
            state.file_dialog_state.queue(
                || Box::pin(rfd::AsyncFileDialog::new().set_directory(&project_dir).pick_folder()),
                FileDialogResult::TargetDir,
            );
        }
        ui.add_enabled(
            config.project_config_info.is_none(),
            egui::Checkbox::new(&mut config.build_target, "Build target objects"),
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
            &config.base_obj_dir,
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
            config.project_config_info.is_none(),
        );
        if response.clicked() {
            state.file_dialog_state.queue(
                || Box::pin(rfd::AsyncFileDialog::new().set_directory(&project_dir).pick_folder()),
                FileDialogResult::BaseDir,
            );
        }
        ui.add_enabled(
            config.project_config_info.is_none(),
            egui::Checkbox::new(&mut config.build_base, "Build base objects"),
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
        ui.checkbox(&mut config.rebuild_on_changes, "Rebuild on changes").on_hover_ui(|ui| {
            let mut job = LayoutJob::default();
            job.append(
                "Automatically re-run the build & diff when files change.",
                0.0,
                text_format.clone(),
            );
            ui.label(job);
        });
    if response.changed() {
        config.watcher_change = true;
    };

    ui.horizontal(|ui| {
        ui.label(RichText::new("File patterns").color(appearance.text_color));
        if ui
            .add_enabled(config.project_config_info.is_none(), egui::Button::new("Reset"))
            .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
            .clicked()
        {
            config.watch_patterns =
                DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect();
            config.watcher_change = true;
        }
    });
    let mut remove_at: Option<usize> = None;
    for (idx, glob) in config.watch_patterns.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{}", glob))
                    .color(appearance.text_color)
                    .family(FontFamily::Monospace),
            );
            if ui
                .add_enabled(config.project_config_info.is_none(), egui::Button::new("-").small())
                .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
                .clicked()
            {
                remove_at = Some(idx);
            }
        });
    }
    if let Some(idx) = remove_at {
        config.watch_patterns.remove(idx);
        config.watcher_change = true;
    }
    ui.horizontal(|ui| {
        ui.add_enabled(
            config.project_config_info.is_none(),
            egui::TextEdit::singleline(&mut state.watch_pattern_text).desired_width(100.0),
        )
        .on_disabled_hover_text(CONFIG_DISABLED_TEXT);
        if ui
            .add_enabled(config.project_config_info.is_none(), egui::Button::new("+").small())
            .on_disabled_hover_text(CONFIG_DISABLED_TEXT)
            .clicked()
        {
            if let Ok(glob) = Glob::new(&state.watch_pattern_text) {
                config.watch_patterns.push(glob);
                config.watcher_change = true;
                state.watch_pattern_text.clear();
            }
        }
    });
}

pub fn arch_config_window(
    ctx: &egui::Context,
    config: &AppConfigRef,
    show: &mut bool,
    appearance: &Appearance,
) {
    let mut config_guard = config.write().unwrap();
    egui::Window::new("Arch Settings").open(show).show(ctx, |ui| {
        arch_config_ui(ui, &mut config_guard, appearance);
    });
}

fn arch_config_ui(ui: &mut egui::Ui, config: &mut AppConfig, _appearance: &Appearance) {
    ui.heading("x86");
    egui::ComboBox::new("x86_formatter", "Format")
        .selected_text(config.diff_obj_config.x86_formatter.get_message().unwrap())
        .show_ui(ui, |ui| {
            for &formatter in X86Formatter::VARIANTS {
                if ui
                    .selectable_label(
                        config.diff_obj_config.x86_formatter == formatter,
                        formatter.get_message().unwrap(),
                    )
                    .clicked()
                {
                    config.diff_obj_config.x86_formatter = formatter;
                    config.queue_reload = true;
                }
            }
        });
    ui.separator();
    ui.heading("MIPS");
    egui::ComboBox::new("mips_abi", "ABI")
        .selected_text(config.diff_obj_config.mips_abi.get_message().unwrap())
        .show_ui(ui, |ui| {
            for &abi in MipsAbi::VARIANTS {
                if ui
                    .selectable_label(
                        config.diff_obj_config.mips_abi == abi,
                        abi.get_message().unwrap(),
                    )
                    .clicked()
                {
                    config.diff_obj_config.mips_abi = abi;
                    config.queue_reload = true;
                }
            }
        });
    egui::ComboBox::new("mips_instr_category", "Instruction Category")
        .selected_text(config.diff_obj_config.mips_instr_category.get_message().unwrap())
        .show_ui(ui, |ui| {
            for &category in MipsInstrCategory::VARIANTS {
                if ui
                    .selectable_label(
                        config.diff_obj_config.mips_instr_category == category,
                        category.get_message().unwrap(),
                    )
                    .clicked()
                {
                    config.diff_obj_config.mips_instr_category = category;
                    config.queue_reload = true;
                }
            }
        });
    ui.separator();
    ui.heading("ARM");
    egui::ComboBox::new("arm_arch_version", "Architecture Version")
        .selected_text(config.diff_obj_config.arm_arch_version.get_message().unwrap())
        .show_ui(ui, |ui| {
            for &version in ArmArchVersion::VARIANTS {
                if ui
                    .selectable_label(
                        config.diff_obj_config.arm_arch_version == version,
                        version.get_message().unwrap(),
                    )
                    .clicked()
                {
                    config.diff_obj_config.arm_arch_version = version;
                    config.queue_reload = true;
                }
            }
        });
}

#[cfg(windows)]
use std::string::FromUtf16Error;
use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};

#[cfg(windows)]
use anyhow::{Context, Result};
use const_format::formatcp;
use egui::{
    output::OpenUrl, text::LayoutJob, CollapsingHeader, FontFamily, FontId, RichText,
    SelectableLabel, TextFormat, Widget,
};
use globset::Glob;
use self_update::cargo_crate_version;

use crate::{
    app::{AppConfig, DiffKind, ViewConfig, ViewState},
    config::{ProjectUnit, ProjectUnitNode},
    jobs::{bindiff::queue_bindiff, objdiff::queue_build, update::queue_update},
    update::RELEASE_URL,
};

const DEFAULT_WATCH_PATTERNS: &[&str] = &[
    "*.c", "*.cp", "*.cpp", "*.cxx", "*.h", "*.hp", "*.hpp", "*.hxx", "*.s", "*.S", "*.asm",
    "*.inc", "*.py", "*.yml", "*.txt", "*.json",
];

#[cfg(windows)]
fn process_utf16(bytes: &[u8]) -> Result<String, FromUtf16Error> {
    let u16_bytes: Vec<u16> = bytes
        .chunks_exact(2)
        .filter_map(|c| Some(u16::from_ne_bytes(c.try_into().ok()?)))
        .collect();
    String::from_utf16(&u16_bytes)
}

#[cfg(windows)]
fn wsl_cmd(args: &[&str]) -> Result<String> {
    use std::{os::windows::process::CommandExt, process::Command};
    let output = Command::new("wsl")
        .args(args)
        .creation_flags(winapi::um::winbase::CREATE_NO_WINDOW)
        .output()
        .context("Failed to execute wsl")?;
    process_utf16(&output.stdout).context("Failed to process stdout")
}

#[cfg(windows)]
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

pub fn config_ui(ui: &mut egui::Ui, config: &Arc<RwLock<AppConfig>>, view_state: &mut ViewState) {
    let mut config_guard = config.write().unwrap();
    let AppConfig {
        available_wsl_distros,
        selected_wsl_distro,
        target_obj_dir,
        base_obj_dir,
        obj_path,
        left_obj,
        right_obj,
        queue_update_check,
        auto_update_check,
        units,
        unit_nodes,
        ..
    } = &mut *config_guard;

    ui.heading("Updates");
    ui.checkbox(auto_update_check, "Check for updates on startup");
    if ui.button("Check now").clicked() {
        *queue_update_check = true;
    }
    ui.label(format!("Current version: {}", cargo_crate_version!())).on_hover_ui_at_pointer(|ui| {
        ui.label(formatcp!("Git branch: {}", env!("VERGEN_GIT_BRANCH")));
        ui.label(formatcp!("Git commit: {}", env!("VERGEN_GIT_SHA")));
        ui.label(formatcp!("Build target: {}", env!("VERGEN_CARGO_TARGET_TRIPLE")));
        ui.label(formatcp!("Debug: {}", env!("VERGEN_CARGO_DEBUG")));
    });
    if let Some(state) = &view_state.check_update {
        ui.label(format!("Latest version: {}", state.latest_release.version));
        if state.update_available {
            ui.colored_label(view_state.view_config.insert_color, "Update available");
            ui.horizontal(|ui| {
                if state.found_binary
                    && ui
                        .button("Automatic")
                        .on_hover_text_at_pointer(
                            "Automatically download and replace the current build",
                        )
                        .clicked()
                {
                    view_state.jobs.push(queue_update());
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

    #[cfg(windows)]
    {
        ui.heading("Build");
        if available_wsl_distros.is_none() {
            *available_wsl_distros = Some(fetch_wsl2_distros());
        }
        egui::ComboBox::from_label("Run in WSL2")
            .selected_text(selected_wsl_distro.as_ref().unwrap_or(&"None".to_string()))
            .show_ui(ui, |ui| {
                ui.selectable_value(selected_wsl_distro, None, "None");
                for distro in available_wsl_distros.as_ref().unwrap() {
                    ui.selectable_value(selected_wsl_distro, Some(distro.clone()), distro);
                }
            });
        ui.separator();
    }
    #[cfg(not(windows))]
    {
        let _ = available_wsl_distros;
        let _ = selected_wsl_distro;
    }

    ui.horizontal(|ui| {
        ui.heading("Project");
        if ui.button(RichText::new("Settings")).clicked() {
            view_state.show_project_config = true;
        }
    });

    if view_state.diff_kind == DiffKind::SplitObj {
        if let (Some(base_dir), Some(target_dir)) = (base_obj_dir, target_obj_dir) {
            let mut new_build_obj = obj_path.clone();
            if units.is_empty() {
                if ui.button("Select obj").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_directory(&target_dir)
                        .add_filter("Object file", &["o", "elf"])
                        .pick_file()
                    {
                        if let Ok(obj_path) = path.strip_prefix(&base_dir) {
                            new_build_obj = Some(obj_path.display().to_string());
                        } else if let Ok(obj_path) = path.strip_prefix(&target_dir) {
                            new_build_obj = Some(obj_path.display().to_string());
                        }
                    }
                }
                if let Some(obj) = obj_path {
                    ui.label(&*obj);
                }
            } else {
                CollapsingHeader::new(RichText::new("Objects").font(FontId {
                    size: view_state.view_config.ui_font.size,
                    family: view_state.view_config.code_font.family.clone(),
                }))
                .default_open(true)
                .show(ui, |ui| {
                    for node in unit_nodes {
                        display_node(ui, &mut new_build_obj, node, &view_state.view_config);
                    }
                });
            }

            let mut build = false;
            if new_build_obj != *obj_path {
                *obj_path = new_build_obj;
                // TODO apply reverse_fn_order
                build = true;
            }
            if obj_path.is_some() && ui.button("Build").clicked() {
                build = true;
            }
            if build {
                view_state.jobs.push(queue_build(config.clone(), view_state.diff_config.clone()));
            }
        }
    } else if view_state.diff_kind == DiffKind::WholeBinary {
        if ui.button("Select left obj").clicked() {
            if let Some(path) =
                rfd::FileDialog::new().add_filter("Object file", &["o", "elf"]).pick_file()
            {
                *left_obj = Some(path);
            }
        }
        if let Some(obj) = left_obj {
            ui.label(obj.to_string_lossy());
        }

        if ui.button("Select right obj").clicked() {
            if let Some(path) =
                rfd::FileDialog::new().add_filter("Object file", &["o", "elf"]).pick_file()
            {
                *right_obj = Some(path);
            }
        }
        if let Some(obj) = right_obj {
            ui.label(obj.to_string_lossy());
        }

        if let (Some(_), Some(_)) = (left_obj, right_obj) {
            if ui.button("Build").clicked() {
                view_state.jobs.push(queue_bindiff(config.clone()));
            }
        }
    }

    // ui.checkbox(&mut view_state.view_config.reverse_fn_order, "Reverse function order (deferred)");
    ui.separator();
}

fn display_unit(
    ui: &mut egui::Ui,
    obj_path: &mut Option<String>,
    name: &str,
    unit: &ProjectUnit,
    view_config: &ViewConfig,
) {
    let path_string = unit.path.to_string_lossy().to_string();
    let selected = matches!(obj_path, Some(path) if path == &path_string);
    if SelectableLabel::new(
        selected,
        RichText::new(name).font(FontId {
            size: view_config.ui_font.size,
            family: view_config.code_font.family.clone(),
        }),
    )
    .ui(ui)
    .clicked()
    {
        *obj_path = Some(path_string);
    }
}

fn display_node(
    ui: &mut egui::Ui,
    obj_path: &mut Option<String>,
    node: &ProjectUnitNode,
    view_config: &ViewConfig,
) {
    match node {
        ProjectUnitNode::File(name, unit) => {
            display_unit(ui, obj_path, name, unit, view_config);
        }
        ProjectUnitNode::Dir(name, children) => {
            CollapsingHeader::new(RichText::new(name).font(FontId {
                size: view_config.ui_font.size,
                family: view_config.code_font.family.clone(),
            }))
            .default_open(false)
            .show(ui, |ui| {
                for node in children {
                    display_node(ui, obj_path, node, view_config);
                }
            });
        }
    }
}

const HELP_ICON: &str = "ℹ";

fn subheading(ui: &mut egui::Ui, text: &str, view_config: &ViewConfig) {
    ui.label(
        RichText::new(text).size(view_config.ui_font.size).color(view_config.emphasized_text_color),
    );
}

pub fn project_window(
    ctx: &egui::Context,
    config: &Arc<RwLock<AppConfig>>,
    view_state: &mut ViewState,
) {
    let mut config_guard = config.write().unwrap();
    let AppConfig {
        custom_make,
        project_dir,
        target_obj_dir,
        base_obj_dir,
        obj_path,
        build_target,
        config_change,
        watcher_change,
        watcher_enabled,
        watch_patterns,
        load_error,
        ..
    } = &mut *config_guard;

    egui::Window::new("Project").open(&mut view_state.show_project_config).show(ctx, |ui| {
        let text_format = TextFormat::simple(
            view_state.view_config.ui_font.clone(),
            view_state.view_config.text_color,
        );
        let code_format = TextFormat::simple(
            FontId {
                size: view_state.view_config.ui_font.size,
                family: view_state.view_config.code_font.family.clone(),
            },
            view_state.view_config.emphasized_text_color,
        );

        fn pick_folder_ui(
            ui: &mut egui::Ui,
            dir: &mut Option<PathBuf>,
            label: &str,
            tooltip: impl FnOnce(&mut egui::Ui),
            clicked: impl FnOnce(&mut Option<PathBuf>),
            view_config: &ViewConfig,
        ) {
            ui.horizontal(|ui| {
                subheading(ui, label, view_config);
                ui.link(HELP_ICON).on_hover_ui(tooltip);
                if ui.button("Select").clicked() {
                    clicked(dir);
                }
            });
            if let Some(dir) = dir {
                if let Some(home) = dirs::home_dir() {
                    if let Ok(rel) = dir.strip_prefix(&home) {
                        ui.label(RichText::new(format!("~/{}", rel.display())).color(view_config.replace_color).family(FontFamily::Monospace));
                        return;
                    }
                }
                ui.label(RichText::new(format!("{}", dir.display())).color(view_config.replace_color).family(FontFamily::Monospace));
            } else {
                ui.label(RichText::new("[none]").color(view_config.delete_color).family(FontFamily::Monospace));
            }
        }

        if view_state.diff_kind == DiffKind::SplitObj {
            pick_folder_ui(
                ui,
                project_dir,
                "Project directory",
                |ui| {
                    let mut job = LayoutJob::default();
                    job.append(
                        "The root project directory.\n\n",
                        0.0,
                        text_format.clone()
                    );
                    job.append(
                        "If a configuration file exists, it will be loaded automatically.",
                        0.0,
                        text_format.clone(),
                    );
                    ui.label(job);
                },
                |project_dir| {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        *project_dir = Some(path);
                        *config_change = true;
                        *watcher_change = true;
                        *target_obj_dir = None;
                        *base_obj_dir = None;
                        *obj_path = None;
                    }
                },
                &view_state.view_config,
            );
            ui.separator();

            ui.horizontal(|ui| {
                subheading(ui, "Custom make program", &view_state.view_config);
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
            let mut custom_make_str = custom_make.clone().unwrap_or_default();
            if ui.text_edit_singleline(&mut custom_make_str).changed() {
                if custom_make_str.is_empty() {
                    *custom_make = None;
                } else {
                    *custom_make = Some(custom_make_str);
                }
            }
            ui.separator();

            if let Some(project_dir) = project_dir {
                pick_folder_ui(
                    ui,
                    target_obj_dir,
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
                    |target_obj_dir| {
                        if let Some(path) =
                            rfd::FileDialog::new().set_directory(&project_dir).pick_folder()
                        {
                            *target_obj_dir = Some(path);
                            *obj_path = None;
                        }
                    },
                    &view_state.view_config,
                );
                ui.checkbox(build_target, "Build target objects").on_hover_ui(|ui| {
                    let mut job = LayoutJob::default();
                    job.append("Tells the build system to produce the target object.\n", 0.0, text_format.clone());
                    job.append("For example, this would call ", 0.0, text_format.clone());
                    job.append("make path/to/target.o", 0.0, code_format.clone());
                    job.append(".\n\n", 0.0, text_format.clone());
                    job.append("This is useful if the target objects are not already built\n", 0.0, text_format.clone());
                    job.append("or if they can change based on project configuration,\n", 0.0, text_format.clone());
                    job.append("but requires that the build system is configured correctly.", 0.0, text_format.clone());
                    ui.label(job);
                });
                ui.separator();

                pick_folder_ui(
                    ui,
                    base_obj_dir,
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
                    |base_obj_dir| {
                        if let Some(path) =
                            rfd::FileDialog::new().set_directory(&project_dir).pick_folder()
                        {
                            *base_obj_dir = Some(path);
                            *obj_path = None;
                        }
                    },
                    &view_state.view_config,
                );
                ui.separator();
            }

            subheading(ui, "Watch settings", &view_state.view_config);
            let response = ui.checkbox(watcher_enabled, "Rebuild on changes").on_hover_ui(|ui| {
                let mut job = LayoutJob::default();
                job.append("Automatically re-run the build & diff when files change.", 0.0, text_format.clone());
                ui.label(job);
            });
            if response.changed() {
                *watcher_change = true;
            };

            ui.horizontal(|ui| {
                ui.label(RichText::new("File Patterns").color(view_state.view_config.text_color));
                if ui.button("Reset").clicked() {
                    *watch_patterns = DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect();
                    *watcher_change = true;
                }
            });
            let mut remove_at: Option<usize> = None;
            for (idx, glob) in watch_patterns.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{}", glob))
                        .color(view_state.view_config.text_color)
                        .family(FontFamily::Monospace));
                    if ui.small_button("-").clicked() {
                        remove_at = Some(idx);
                    }
                });
            }
            if let Some(idx) = remove_at {
                watch_patterns.remove(idx);
                *watcher_change = true;
            }
            ui.horizontal(|ui| {
               egui::TextEdit::singleline(&mut view_state.watch_pattern_text)
                   .desired_width(100.0)
                   .show(ui);
                if ui.small_button("+").clicked() {
                    if let Ok(glob) = Glob::new(&view_state.watch_pattern_text) {
                        watch_patterns.push(glob);
                        *watcher_change = true;
                        view_state.watch_pattern_text.clear();
                    }
                }
            });
        }
    });

    if let Some(error) = &load_error {
        let mut open = true;
        egui::Window::new("Error").open(&mut open).show(ctx, |ui| {
            ui.label("Failed to load project config:");
            ui.colored_label(view_state.view_config.delete_color, error);
        });
        if !open {
            *load_error = None;
        }
    }
}

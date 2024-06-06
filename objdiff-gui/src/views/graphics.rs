use std::{
    fs::File,
    path::{Path, PathBuf},
};

use anyhow::Result;
use egui::{text::LayoutJob, Context, FontId, RichText, TextFormat, TextStyle, Window};
use serde::{Deserialize, Serialize};
use strum::{EnumIter, EnumMessage, IntoEnumIterator};

use crate::views::{appearance::Appearance, frame_history::FrameHistory};

#[derive(Default)]
pub struct GraphicsViewState {
    pub active_backend: String,
    pub active_device: String,
    pub graphics_config: GraphicsConfig,
    pub graphics_config_path: Option<PathBuf>,
    pub should_relaunch: bool,
}

#[derive(
    Copy, Clone, Debug, Default, PartialEq, Eq, EnumIter, EnumMessage, Serialize, Deserialize,
)]
pub enum GraphicsBackend {
    #[default]
    #[strum(message = "Auto")]
    Auto,
    #[strum(message = "Vulkan")]
    Vulkan,
    #[strum(message = "Metal")]
    Metal,
    #[strum(message = "DirectX 12")]
    Dx12,
    #[strum(message = "OpenGL")]
    OpenGL,
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct GraphicsConfig {
    #[serde(default)]
    pub desired_backend: GraphicsBackend,
}

pub fn load_graphics_config(path: &Path) -> Result<Option<GraphicsConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let file = File::open(path)?;
    let config: GraphicsConfig = ron::de::from_reader(file)?;
    Ok(Some(config))
}

pub fn save_graphics_config(path: &Path, config: &GraphicsConfig) -> Result<()> {
    let file = File::create(path)?;
    ron::ser::to_writer(file, config)?;
    Ok(())
}

impl GraphicsBackend {
    pub fn is_supported(&self) -> bool {
        match self {
            GraphicsBackend::Auto => true,
            GraphicsBackend::Vulkan => {
                cfg!(all(feature = "wgpu", any(target_os = "windows", target_os = "linux")))
            }
            GraphicsBackend::Metal => cfg!(all(feature = "wgpu", target_os = "macos")),
            GraphicsBackend::Dx12 => cfg!(all(feature = "wgpu", target_os = "windows")),
            GraphicsBackend::OpenGL => true,
        }
    }
}

pub fn graphics_window(
    ctx: &Context,
    show: &mut bool,
    frame_history: &mut FrameHistory,
    state: &mut GraphicsViewState,
    appearance: &Appearance,
) {
    Window::new("Graphics").open(show).show(ctx, |ui| {
        ui.label("Graphics backend:");
        ui.label(
            RichText::new(&state.active_backend)
                .color(appearance.emphasized_text_color)
                .text_style(TextStyle::Monospace),
        );
        ui.label("Graphics device:");
        ui.label(
            RichText::new(&state.active_device)
                .color(appearance.emphasized_text_color)
                .text_style(TextStyle::Monospace),
        );
        ui.label(format!("FPS: {:.1}", frame_history.fps()));

        ui.separator();
        let mut job = LayoutJob::default();
        job.append(
            "WARNING: ",
            0.0,
            TextFormat::simple(appearance.ui_font.clone(), appearance.delete_color),
        );
        job.append(
            "Changing the graphics backend may cause the application\nto no longer start or display correctly. Use with caution!",
            0.0,
            TextFormat::simple(appearance.ui_font.clone(), appearance.emphasized_text_color),
        );
        if let Some(config_path) = &state.graphics_config_path {
            job.append(
                "\n\nDelete the following file to reset:\n",
                0.0,
                TextFormat::simple(appearance.ui_font.clone(), appearance.emphasized_text_color),
            );
            job.append(
                config_path.to_string_lossy().as_ref(),
                0.0,
                TextFormat::simple(
                    FontId {
                        family: appearance.code_font.family.clone(),
                        size: appearance.ui_font.size,
                    },
                    appearance.emphasized_text_color,
                ),
            );
        }
        job.append(
            "\n\nChanging the graphics backend will restart the application.",
            0.0,
            TextFormat::simple(appearance.ui_font.clone(), appearance.replace_color),
        );
        ui.label(job);

        ui.add_enabled_ui(state.graphics_config_path.is_some(), |ui| {
            ui.horizontal(|ui| {
                ui.label("Desired backend:");
                for backend in GraphicsBackend::iter().filter(GraphicsBackend::is_supported) {
                    let selected = state.graphics_config.desired_backend == backend;
                    if ui.selectable_label(selected, backend.get_message().unwrap()).clicked() {
                        let prev_backend = state.graphics_config.desired_backend;
                        state.graphics_config.desired_backend = backend;
                        match save_graphics_config(
                            state.graphics_config_path.as_ref().unwrap(),
                            &state.graphics_config,
                        ) {
                            Ok(()) => {
                                state.should_relaunch = true;
                            }
                            Err(e) => {
                                log::error!("Failed to save graphics config: {:?}", e);
                                state.graphics_config.desired_backend = prev_backend;
                            }
                        }
                    }
                }
            });
        });
    });
}

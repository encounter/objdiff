use egui::{Color32, FontId, TextFormat, text::LayoutJob};

pub(crate) mod appearance;
pub(crate) mod column_layout;
pub(crate) mod config;
pub(crate) mod data_diff;
pub(crate) mod debug;
pub(crate) mod demangle;
pub(crate) mod diff;
pub(crate) mod extab_diff;
pub(crate) mod file;
pub(crate) mod frame_history;
pub(crate) mod function_diff;
pub(crate) mod graphics;
pub(crate) mod jobs;
pub(crate) mod rlwinm;
pub(crate) mod symbol_diff;

#[inline]
fn write_text(str: &str, color: Color32, job: &mut LayoutJob, font_id: FontId) {
    job.append(str, 0.0, TextFormat::simple(font_id, color));
}

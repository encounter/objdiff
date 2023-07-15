use egui::{text::LayoutJob, Color32, FontId, TextFormat};

pub(crate) mod config;
pub(crate) mod data_diff;
pub(crate) mod function_diff;
pub(crate) mod jobs;
pub(crate) mod symbol_diff;

fn write_text(str: &str, color: Color32, job: &mut LayoutJob, font_id: FontId) {
    job.append(str, 0.0, TextFormat::simple(font_id, color));
}

use egui::{text::LayoutJob, Color32, FontFamily, FontId, TextFormat};

pub(crate) mod config;
pub(crate) mod function_diff;
pub(crate) mod jobs;
pub(crate) mod symbol_diff;

const FONT_SIZE: f32 = 14.0;
const FONT_ID: FontId = FontId::new(FONT_SIZE, FontFamily::Monospace);

const COLOR_RED: Color32 = Color32::from_rgb(200, 40, 41);

fn write_text(str: &str, color: Color32, job: &mut LayoutJob) {
    job.append(str, 0.0, TextFormat { font_id: FONT_ID, color, ..Default::default() });
}

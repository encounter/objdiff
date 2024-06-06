use std::sync::Arc;

use egui::{text::LayoutJob, Color32, FontFamily, FontId, TextFormat, TextStyle, Widget};
use time::UtcOffset;

use crate::fonts::load_font_if_needed;

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Appearance {
    pub ui_font: FontId,
    pub code_font: FontId,
    pub diff_colors: Vec<Color32>,
    pub theme: eframe::Theme,

    // Applied by theme
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

    // Global
    #[serde(skip)]
    pub utc_offset: UtcOffset,
    #[serde(skip)]
    pub fonts: FontState,
    #[serde(skip)]
    pub next_ui_font: Option<FontId>,
    #[serde(skip)]
    pub next_code_font: Option<FontId>,
}

pub struct FontState {
    definitions: egui::FontDefinitions,
    source: font_kit::source::SystemSource,
    family_names: Vec<String>,
    // loaded_families: HashMap<String, LoadedFontFamily>,
}

const DEFAULT_UI_FONT: FontId = FontId { size: 12.0, family: FontFamily::Proportional };
const DEFAULT_CODE_FONT: FontId = FontId { size: 14.0, family: FontFamily::Monospace };

impl Default for Appearance {
    fn default() -> Self {
        Self {
            ui_font: DEFAULT_UI_FONT,
            code_font: DEFAULT_CODE_FONT,
            diff_colors: DEFAULT_COLOR_ROTATION.to_vec(),
            theme: eframe::Theme::Dark,
            text_color: Color32::GRAY,
            emphasized_text_color: Color32::LIGHT_GRAY,
            deemphasized_text_color: Color32::DARK_GRAY,
            highlight_color: Color32::WHITE,
            replace_color: Color32::LIGHT_BLUE,
            insert_color: Color32::GREEN,
            delete_color: Color32::from_rgb(200, 40, 41),
            utc_offset: UtcOffset::UTC,
            fonts: FontState::default(),
            next_ui_font: None,
            next_code_font: None,
        }
    }
}

impl Default for FontState {
    fn default() -> Self {
        Self {
            definitions: Default::default(),
            source: font_kit::source::SystemSource::new(),
            family_names: Default::default(),
            // loaded_families: Default::default(),
        }
    }
}

impl Appearance {
    pub fn pre_update(&mut self, ctx: &egui::Context) {
        let mut style = ctx.style().as_ref().clone();
        style.text_styles.insert(TextStyle::Body, FontId {
            size: (self.ui_font.size * 0.75).floor(),
            family: self.ui_font.family.clone(),
        });
        style.text_styles.insert(TextStyle::Body, self.ui_font.clone());
        style.text_styles.insert(TextStyle::Button, self.ui_font.clone());
        style.text_styles.insert(TextStyle::Heading, FontId {
            size: (self.ui_font.size * 1.5).floor(),
            family: self.ui_font.family.clone(),
        });
        style.text_styles.insert(TextStyle::Monospace, self.code_font.clone());
        match self.theme {
            eframe::Theme::Dark => {
                style.visuals = egui::Visuals::dark();
                self.text_color = Color32::GRAY;
                self.emphasized_text_color = Color32::LIGHT_GRAY;
                self.deemphasized_text_color = Color32::DARK_GRAY;
                self.highlight_color = Color32::WHITE;
                self.replace_color = Color32::LIGHT_BLUE;
                self.insert_color = Color32::GREEN;
                self.delete_color = Color32::from_rgb(200, 40, 41);
            }
            eframe::Theme::Light => {
                style.visuals = egui::Visuals::light();
                self.text_color = Color32::GRAY;
                self.emphasized_text_color = Color32::DARK_GRAY;
                self.deemphasized_text_color = Color32::LIGHT_GRAY;
                self.highlight_color = Color32::BLACK;
                self.replace_color = Color32::DARK_BLUE;
                self.insert_color = Color32::DARK_GREEN;
                self.delete_color = Color32::from_rgb(200, 40, 41);
            }
        }
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = 10.0;
        ctx.set_style(style);
    }

    pub fn post_update(&mut self, ctx: &egui::Context) {
        // Load fonts for next frame
        if let Some(next_ui_font) = self.next_ui_font.take() {
            match load_font_if_needed(
                ctx,
                &self.fonts.source,
                &next_ui_font,
                DEFAULT_UI_FONT.family,
                &mut self.fonts.definitions,
            ) {
                Ok(()) => self.ui_font = next_ui_font,
                Err(e) => {
                    log::error!("Failed to load font: {}", e)
                }
            }
        }
        if let Some(next_code_font) = self.next_code_font.take() {
            match load_font_if_needed(
                ctx,
                &self.fonts.source,
                &next_code_font,
                DEFAULT_CODE_FONT.family,
                &mut self.fonts.definitions,
            ) {
                Ok(()) => self.code_font = next_code_font,
                Err(e) => {
                    log::error!("Failed to load font: {}", e)
                }
            }
        }
    }

    pub fn init_fonts(&mut self, ctx: &egui::Context) {
        self.fonts.family_names = self.fonts.source.all_families().unwrap_or_default();
        match load_font_if_needed(
            ctx,
            &self.fonts.source,
            &self.ui_font,
            DEFAULT_UI_FONT.family,
            &mut self.fonts.definitions,
        ) {
            Ok(_) => {}
            Err(e) => {
                log::error!("Failed to load font: {}", e);
                // Revert to default
                self.ui_font = DEFAULT_UI_FONT;
            }
        }
        match load_font_if_needed(
            ctx,
            &self.fonts.source,
            &self.code_font,
            DEFAULT_CODE_FONT.family,
            &mut self.fonts.definitions,
        ) {
            Ok(_) => {}
            Err(e) => {
                log::error!("Failed to load font: {}", e);
                // Revert to default
                self.code_font = DEFAULT_CODE_FONT;
            }
        }
    }

    pub fn code_text_format(&self, base_color: Color32, highlight: bool) -> TextFormat {
        TextFormat {
            font_id: self.code_font.clone(),
            color: if highlight { self.emphasized_text_color } else { base_color },
            background: if highlight { self.deemphasized_text_color } else { Color32::TRANSPARENT },
            ..Default::default()
        }
    }
}

pub const DEFAULT_COLOR_ROTATION: [Color32; 9] = [
    Color32::from_rgb(255, 0, 255),
    Color32::from_rgb(0, 255, 255),
    Color32::from_rgb(0, 128, 0),
    Color32::from_rgb(255, 0, 0),
    Color32::from_rgb(255, 255, 0),
    Color32::from_rgb(255, 192, 203),
    Color32::from_rgb(0, 0, 255),
    Color32::from_rgb(0, 255, 0),
    Color32::from_rgb(213, 138, 138),
];

fn font_id_ui(
    ui: &mut egui::Ui,
    label: &str,
    mut font_id: FontId,
    default: FontId,
    appearance: &Appearance,
) -> Option<FontId> {
    ui.push_id(label, |ui| {
        let font_size = font_id.size;
        let label_job = LayoutJob::simple(
            font_id.family.to_string(),
            font_id.clone(),
            appearance.text_color,
            0.0,
        );
        let mut changed = ui
            .horizontal(|ui| {
                ui.label(label);
                let mut changed = egui::Slider::new(&mut font_id.size, 4.0..=40.0)
                    .max_decimals(1)
                    .ui(ui)
                    .changed();
                if ui.button("Reset").clicked() {
                    font_id = default;
                    changed = true;
                }
                changed
            })
            .inner;
        let family = &mut font_id.family;
        changed |= egui::ComboBox::from_label("Font family")
            .selected_text(label_job)
            .width(font_size * 20.0)
            .show_ui(ui, |ui| {
                let mut result = false;
                result |= ui
                    .selectable_value(family, FontFamily::Proportional, "Proportional (built-in)")
                    .changed();
                result |= ui
                    .selectable_value(family, FontFamily::Monospace, "Monospace (built-in)")
                    .changed();
                for family_name in &appearance.fonts.family_names {
                    result |= ui
                        .selectable_value(
                            family,
                            FontFamily::Name(Arc::from(family_name.as_str())),
                            family_name,
                        )
                        .changed();
                }
                result
            })
            .inner
            .unwrap_or(false);
        changed.then_some(font_id)
    })
    .inner
}

pub fn appearance_window(ctx: &egui::Context, show: &mut bool, appearance: &mut Appearance) {
    egui::Window::new("Appearance").open(show).show(ctx, |ui| {
        egui::ComboBox::from_label("Theme")
            .selected_text(format!("{:?}", appearance.theme))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut appearance.theme, eframe::Theme::Dark, "Dark");
                ui.selectable_value(&mut appearance.theme, eframe::Theme::Light, "Light");
            });
        ui.separator();
        appearance.next_ui_font =
            font_id_ui(ui, "UI font:", appearance.ui_font.clone(), DEFAULT_UI_FONT, appearance);
        ui.separator();
        appearance.next_code_font = font_id_ui(
            ui,
            "Code font:",
            appearance.code_font.clone(),
            DEFAULT_CODE_FONT,
            appearance,
        );
        ui.separator();
        ui.label("Diff colors:");
        if ui.button("Reset").clicked() {
            appearance.diff_colors = DEFAULT_COLOR_ROTATION.to_vec();
        }
        let mut remove_at: Option<usize> = None;
        let num_colors = appearance.diff_colors.len();
        for (idx, color) in appearance.diff_colors.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.color_edit_button_srgba(color);
                if num_colors > 1 && ui.small_button("-").clicked() {
                    remove_at = Some(idx);
                }
            });
        }
        if let Some(idx) = remove_at {
            appearance.diff_colors.remove(idx);
        }
        if ui.small_button("+").clicked() {
            appearance.diff_colors.push(Color32::BLACK);
        }
    });
}

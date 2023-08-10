use egui::{Color32, FontFamily, FontId, TextStyle};
use time::UtcOffset;

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct Appearance {
    pub ui_font: FontId,
    pub code_font: FontId,
    pub diff_colors: Vec<Color32>,
    pub reverse_fn_order: bool,
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
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            ui_font: FontId { size: 12.0, family: FontFamily::Proportional },
            code_font: FontId { size: 14.0, family: FontFamily::Monospace },
            diff_colors: DEFAULT_COLOR_ROTATION.to_vec(),
            reverse_fn_order: false,
            theme: eframe::Theme::Dark,
            text_color: Color32::GRAY,
            emphasized_text_color: Color32::LIGHT_GRAY,
            deemphasized_text_color: Color32::DARK_GRAY,
            highlight_color: Color32::WHITE,
            replace_color: Color32::LIGHT_BLUE,
            insert_color: Color32::GREEN,
            delete_color: Color32::from_rgb(200, 40, 41),
            utc_offset: UtcOffset::UTC,
        }
    }
}

impl Appearance {
    pub fn apply(&mut self, style: &egui::Style) -> egui::Style {
        let mut style = style.clone();
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
        style
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

pub fn appearance_window(ctx: &egui::Context, show: &mut bool, appearance: &mut Appearance) {
    egui::Window::new("Appearance").open(show).show(ctx, |ui| {
        egui::ComboBox::from_label("Theme")
            .selected_text(format!("{:?}", appearance.theme))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut appearance.theme, eframe::Theme::Dark, "Dark");
                ui.selectable_value(&mut appearance.theme, eframe::Theme::Light, "Light");
            });
        ui.label("UI font:");
        egui::introspection::font_id_ui(ui, &mut appearance.ui_font);
        ui.separator();
        ui.label("Code font:");
        egui::introspection::font_id_ui(ui, &mut appearance.code_font);
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

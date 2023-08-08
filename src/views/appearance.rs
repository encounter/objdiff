use egui::Color32;

use crate::app::ViewState;

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

pub fn appearance_window(ctx: &egui::Context, view_state: &mut ViewState) {
    egui::Window::new("Appearance").open(&mut view_state.show_view_config).show(ctx, |ui| {
        egui::ComboBox::from_label("Theme")
            .selected_text(format!("{:?}", view_state.view_config.theme))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut view_state.view_config.theme, eframe::Theme::Dark, "Dark");
                ui.selectable_value(
                    &mut view_state.view_config.theme,
                    eframe::Theme::Light,
                    "Light",
                );
            });
        ui.label("UI font:");
        egui::introspection::font_id_ui(ui, &mut view_state.view_config.ui_font);
        ui.separator();
        ui.label("Code font:");
        egui::introspection::font_id_ui(ui, &mut view_state.view_config.code_font);
        ui.separator();
        ui.label("Diff colors:");
        if ui.button("Reset").clicked() {
            view_state.view_config.diff_colors = DEFAULT_COLOR_ROTATION.to_vec();
        }
        let mut remove_at: Option<usize> = None;
        let num_colors = view_state.view_config.diff_colors.len();
        for (idx, color) in view_state.view_config.diff_colors.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.color_edit_button_srgba(color);
                if num_colors > 1 && ui.small_button("-").clicked() {
                    remove_at = Some(idx);
                }
            });
        }
        if let Some(idx) = remove_at {
            view_state.view_config.diff_colors.remove(idx);
        }
        if ui.small_button("+").clicked() {
            view_state.view_config.diff_colors.push(Color32::BLACK);
        }
    });
}

use egui::TextStyle;

use crate::views::appearance::Appearance;

#[derive(Default)]
pub struct RlwinmDecodeViewState {
    pub text: String,
}

pub fn rlwinm_decode_window(
    ctx: &egui::Context,
    show: &mut bool,
    state: &mut RlwinmDecodeViewState,
    appearance: &Appearance,
) {
    egui::Window::new("Rlwinm Decoder").open(show).show(ctx, |ui| {
        ui.text_edit_singleline(&mut state.text);
        ui.add_space(10.0);
        if let Some(demangled) = rlwinmdec::decode(&state.text) {
            ui.scope(|ui| {
                ui.style_mut().override_text_style = Some(TextStyle::Monospace);
                ui.colored_label(appearance.replace_color, demangled.trim());
            });
            if ui.button("Copy").clicked() {
                ui.output_mut(|output| output.copied_text = demangled);
            }
        } else {
            ui.scope(|ui| {
                ui.style_mut().override_text_style = Some(TextStyle::Monospace);
                ui.colored_label(appearance.replace_color, "[invalid]");
            });
        }
    });
}

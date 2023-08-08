use egui::TextStyle;

use crate::app::ViewState;

pub fn demangle_window(ctx: &egui::Context, view_state: &mut ViewState) {
    egui::Window::new("Demangle").open(&mut view_state.show_demangle).show(ctx, |ui| {
        ui.text_edit_singleline(&mut view_state.demangle_text);
        ui.add_space(10.0);
        if let Some(demangled) =
            cwdemangle::demangle(&view_state.demangle_text, &Default::default())
        {
            ui.scope(|ui| {
                ui.style_mut().override_text_style = Some(TextStyle::Monospace);
                ui.colored_label(view_state.view_config.replace_color, &demangled);
            });
            if ui.button("Copy").clicked() {
                ui.output_mut(|output| output.copied_text = demangled);
            }
        } else {
            ui.scope(|ui| {
                ui.style_mut().override_text_style = Some(TextStyle::Monospace);
                ui.colored_label(view_state.view_config.replace_color, "[invalid]");
            });
        }
    });
}

use egui::TextStyle;
use objdiff_core::diff::Demangler;

use crate::views::appearance::Appearance;

#[derive(Default)]
pub struct DemangleViewState {
    pub text: String,
}

pub fn demangle_window(
    ctx: &egui::Context,
    show: &mut bool,
    state: &mut DemangleViewState,
    appearance: &Appearance,
    demangler: Demangler,
) {
    egui::Window::new("Demangle").open(show).show(ctx, |ui| {
        ui.text_edit_singleline(&mut state.text);
        ui.add_space(10.0);
        if let Some(demangled) = demangler.demangle(&state.text) {
            ui.scope(|ui| {
                ui.style_mut().override_text_style = Some(TextStyle::Monospace);
                ui.colored_label(appearance.replace_color, &demangled);
            });
            if ui.button("Copy").clicked() {
                ctx.copy_text(demangled);
            }
        } else {
            ui.scope(|ui| {
                ui.style_mut().override_text_style = Some(TextStyle::Monospace);
                ui.colored_label(appearance.replace_color, "[invalid]");
            });
        }
    });
}

use crate::views::{appearance::Appearance, frame_history::FrameHistory};

pub fn debug_window(
    ctx: &egui::Context,
    show: &mut bool,
    frame_history: &mut FrameHistory,
    appearance: &Appearance,
) {
    egui::Window::new("Debug").open(show).show(ctx, |ui| {
        debug_ui(ui, frame_history, appearance);
    });
}

fn debug_ui(ui: &mut egui::Ui, frame_history: &mut FrameHistory, _appearance: &Appearance) {
    if ui.button("Clear memory").clicked() {
        ui.memory_mut(|m| *m = Default::default());
    }
    ui.label(format!("Repainting the UI each frame. FPS: {:.1}", frame_history.fps()));
    frame_history.ui(ui);
}

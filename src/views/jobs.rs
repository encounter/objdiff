use egui::{Color32, ProgressBar, Widget};

use crate::app::ViewState;

pub fn jobs_ui(ui: &mut egui::Ui, view_state: &ViewState) {
    ui.label("Jobs");

    for job in &view_state.jobs {
        if let Ok(status) = job.status.read() {
            ui.group(|ui| {
                ui.label(&status.title);
                let mut bar = ProgressBar::new(status.progress_percent);
                if let Some(items) = &status.progress_items {
                    bar = bar.text(format!("{} / {}", items[0], items[1]));
                }
                bar.ui(ui);
                const STATUS_LENGTH: usize = 80;
                if let Some(err) = &status.error {
                    let err_string = err.to_string();
                    ui.colored_label(
                        Color32::from_rgb(255, 0, 0),
                        if err_string.len() > STATUS_LENGTH - 10 {
                            format!("Error: {}...", &err_string[0..STATUS_LENGTH - 10])
                        } else {
                            format!("Error: {:width$}", err_string, width = STATUS_LENGTH - 7)
                        },
                    );
                } else {
                    ui.label(if status.status.len() > STATUS_LENGTH - 3 {
                        format!("{}...", &status.status[0..STATUS_LENGTH - 3])
                    } else {
                        format!("{:width$}", &status.status, width = STATUS_LENGTH)
                    });
                }
            });
        }
    }
}

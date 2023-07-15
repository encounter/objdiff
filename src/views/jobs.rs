use egui::{ProgressBar, Widget};

use crate::app::ViewState;

pub fn jobs_ui(ui: &mut egui::Ui, view_state: &mut ViewState) {
    ui.label("Jobs");

    let mut remove_job: Option<usize> = None;
    for (idx, job) in view_state.jobs.iter_mut().enumerate() {
        let Ok(status) = job.status.read() else {
            continue;
        };
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.label(&status.title);
                if ui.small_button("✖").clicked() {
                    if job.handle.is_some() {
                        job.should_remove = true;
                        if let Err(e) = job.cancel.send(()) {
                            log::error!("Failed to cancel job: {e:?}");
                        }
                    } else {
                        remove_job = Some(idx);
                    }
                }
            });
            let mut bar = ProgressBar::new(status.progress_percent);
            if let Some(items) = &status.progress_items {
                bar = bar.text(format!("{} / {}", items[0], items[1]));
            }
            bar.ui(ui);
            const STATUS_LENGTH: usize = 80;
            if let Some(err) = &status.error {
                let err_string = err.to_string();
                ui.colored_label(
                    view_state.view_config.delete_color,
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

    if let Some(idx) = remove_job {
        view_state.jobs.remove(idx);
    }
}

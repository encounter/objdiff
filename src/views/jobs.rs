use egui::{ProgressBar, RichText, Widget};

use crate::{jobs::JobQueue, views::appearance::Appearance};

pub fn jobs_ui(ui: &mut egui::Ui, jobs: &mut JobQueue, appearance: &Appearance) {
    ui.label("Jobs");

    let mut remove_job: Option<usize> = None;
    for job in jobs.iter_mut() {
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
                        remove_job = Some(job.id);
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
                let err_string = format!("{:#}", err);
                ui.colored_label(
                    appearance.delete_color,
                    if err_string.len() > STATUS_LENGTH - 10 {
                        format!("Error: {}…", &err_string[0..STATUS_LENGTH - 10])
                    } else {
                        format!("Error: {:width$}", err_string, width = STATUS_LENGTH - 7)
                    },
                )
                .on_hover_text_at_pointer(RichText::new(err_string).color(appearance.delete_color));
            } else {
                ui.label(if status.status.len() > STATUS_LENGTH - 3 {
                    format!("{}…", &status.status[0..STATUS_LENGTH - 3])
                } else {
                    format!("{:width$}", &status.status, width = STATUS_LENGTH)
                })
                .on_hover_text_at_pointer(&status.status);
            }
        });
    }

    if let Some(idx) = remove_job {
        jobs.remove(idx);
    }
}

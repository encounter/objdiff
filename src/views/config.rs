use std::sync::{Arc, RwLock};

use crate::{
    app::{AppConfig, ViewState},
    jobs::build::queue_build,
};

pub fn config_ui(ui: &mut egui::Ui, config: &Arc<RwLock<AppConfig>>, view_state: &mut ViewState) {
    let mut config_guard = config.write().unwrap();
    let AppConfig { project_dir, project_dir_change, build_asm_dir, build_src_dir, build_obj } =
        &mut *config_guard;

    if ui.button("Select project dir").clicked() {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            *project_dir = Some(path);
            *project_dir_change = true;
            *build_asm_dir = None;
            *build_src_dir = None;
            *build_obj = None;
        }
    }
    if let Some(dir) = project_dir {
        ui.label(dir.to_string_lossy());
    }

    ui.separator();

    if let Some(project_dir) = project_dir {
        if ui.button("Select asm build dir").clicked() {
            if let Some(path) = rfd::FileDialog::new().set_directory(&project_dir).pick_folder() {
                *build_asm_dir = Some(path);
                *build_obj = None;
            }
        }
        if let Some(dir) = build_asm_dir {
            ui.label(dir.to_string_lossy());
        }

        ui.separator();

        if ui.button("Select src build dir").clicked() {
            if let Some(path) = rfd::FileDialog::new().set_directory(&project_dir).pick_folder() {
                *build_src_dir = Some(path);
                *build_obj = None;
            }
        }
        if let Some(dir) = build_src_dir {
            ui.label(dir.to_string_lossy());
        }

        ui.separator();
    }

    if let Some(build_src_dir) = build_src_dir {
        if ui.button("Select obj").clicked() {
            if let Some(path) = rfd::FileDialog::new()
                .set_directory(&build_src_dir)
                .add_filter("Object file", &["o"])
                .pick_file()
            {
                let mut new_build_obj: Option<String> = None;
                if let Ok(obj_path) = path.strip_prefix(&build_src_dir) {
                    new_build_obj = Some(obj_path.display().to_string());
                } else if let Some(build_asm_dir) = build_asm_dir {
                    if let Ok(obj_path) = path.strip_prefix(&build_asm_dir) {
                        new_build_obj = Some(obj_path.display().to_string());
                    }
                }
                if let Some(new_build_obj) = new_build_obj {
                    *build_obj = Some(new_build_obj.clone());
                    view_state.jobs.push(queue_build(new_build_obj, config.clone()));
                }
            }
        }
        if let Some(build_obj) = build_obj {
            ui.label(&*build_obj);
            if ui.button("Build").clicked() {
                view_state.jobs.push(queue_build(build_obj.clone(), config.clone()));
            }
        }

        ui.separator();
    }

    ui.checkbox(&mut view_state.reverse_fn_order, "Reverse function order (deferred)");
    ui.separator();
}

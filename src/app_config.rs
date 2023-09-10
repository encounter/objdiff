use std::path::PathBuf;

use eframe::Storage;
use globset::Glob;

use crate::app::{AppConfig, ObjectConfig, CONFIG_KEY};

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct AppConfigVersion {
    pub version: u32,
}

impl Default for AppConfigVersion {
    fn default() -> Self { Self { version: 1 } }
}

/// Deserialize the AppConfig from storage, handling upgrades from older versions.
pub fn deserialize_config(storage: &dyn Storage) -> Option<AppConfig> {
    let str = storage.get_string(CONFIG_KEY)?;
    match ron::from_str::<AppConfigVersion>(&str) {
        Ok(version) => match version.version {
            1 => from_str::<AppConfig>(&str),
            _ => {
                log::warn!("Unknown config version: {}", version.version);
                None
            }
        },
        Err(e) => {
            log::warn!("Failed to decode config version: {e}");
            // Try to decode as v0
            from_str::<AppConfigV0>(&str).map(|c| c.into_config())
        }
    }
}

fn from_str<T>(str: &str) -> Option<T>
where T: serde::de::DeserializeOwned {
    match ron::from_str(str) {
        Ok(config) => Some(config),
        Err(err) => {
            log::warn!("Failed to decode config: {err}");
            None
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct ObjectConfigV0 {
    pub name: String,
    pub target_path: PathBuf,
    pub base_path: PathBuf,
    pub reverse_fn_order: Option<bool>,
}

impl ObjectConfigV0 {
    fn into_config(self) -> ObjectConfig {
        ObjectConfig {
            name: self.name,
            target_path: Some(self.target_path),
            base_path: Some(self.base_path),
            reverse_fn_order: self.reverse_fn_order,
            complete: None,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct AppConfigV0 {
    pub custom_make: Option<String>,
    pub selected_wsl_distro: Option<String>,
    pub project_dir: Option<PathBuf>,
    pub target_obj_dir: Option<PathBuf>,
    pub base_obj_dir: Option<PathBuf>,
    pub selected_obj: Option<ObjectConfigV0>,
    pub build_target: bool,
    pub auto_update_check: bool,
    pub watch_patterns: Vec<Glob>,
}

impl AppConfigV0 {
    fn into_config(self) -> AppConfig {
        log::info!("Upgrading configuration from v0");
        AppConfig {
            custom_make: self.custom_make,
            selected_wsl_distro: self.selected_wsl_distro,
            project_dir: self.project_dir,
            target_obj_dir: self.target_obj_dir,
            base_obj_dir: self.base_obj_dir,
            selected_obj: self.selected_obj.map(|obj| obj.into_config()),
            build_target: self.build_target,
            auto_update_check: self.auto_update_check,
            watch_patterns: self.watch_patterns,
            ..Default::default()
        }
    }
}

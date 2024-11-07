use std::path::PathBuf;

use eframe::Storage;
use globset::Glob;
use objdiff_core::{
    config::ScratchConfig,
    diff::{ArmArchVersion, ArmR9Usage, DiffObjConfig, MipsAbi, MipsInstrCategory, X86Formatter},
};

use crate::app::{AppConfig, ObjectConfig, CONFIG_KEY};

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct AppConfigVersion {
    pub version: u32,
}

impl Default for AppConfigVersion {
    fn default() -> Self { Self { version: 2 } }
}

/// Deserialize the AppConfig from storage, handling upgrades from older versions.
pub fn deserialize_config(storage: &dyn Storage) -> Option<AppConfig> {
    let str = storage.get_string(CONFIG_KEY)?;
    match ron::from_str::<AppConfigVersion>(&str) {
        Ok(version) => match version.version {
            2 => from_str::<AppConfig>(&str),
            1 => from_str::<AppConfigV1>(&str).map(|c| c.into_config()),
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
pub struct ScratchConfigV1 {
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub compiler: Option<String>,
    #[serde(default)]
    pub c_flags: Option<String>,
    #[serde(default)]
    pub ctx_path: Option<PathBuf>,
    #[serde(default)]
    pub build_ctx: bool,
}

impl ScratchConfigV1 {
    fn into_config(self) -> ScratchConfig {
        ScratchConfig {
            platform: self.platform,
            compiler: self.compiler,
            c_flags: self.c_flags,
            ctx_path: self.ctx_path,
            build_ctx: self.build_ctx.then_some(true),
            preset_id: None,
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
pub struct ObjectConfigV1 {
    pub name: String,
    pub target_path: Option<PathBuf>,
    pub base_path: Option<PathBuf>,
    pub reverse_fn_order: Option<bool>,
    pub complete: Option<bool>,
    pub scratch: Option<ScratchConfigV1>,
    pub source_path: Option<String>,
}

impl ObjectConfigV1 {
    fn into_config(self) -> ObjectConfig {
        ObjectConfig {
            name: self.name,
            target_path: self.target_path,
            base_path: self.base_path,
            reverse_fn_order: self.reverse_fn_order,
            complete: self.complete,
            scratch: self.scratch.map(|scratch| scratch.into_config()),
            source_path: self.source_path,
            ..Default::default()
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct DiffObjConfigV1 {
    pub relax_reloc_diffs: bool,
    #[serde(default = "bool_true")]
    pub space_between_args: bool,
    pub combine_data_sections: bool,
    // x86
    pub x86_formatter: X86Formatter,
    // MIPS
    pub mips_abi: MipsAbi,
    pub mips_instr_category: MipsInstrCategory,
    // ARM
    pub arm_arch_version: ArmArchVersion,
    pub arm_unified_syntax: bool,
    pub arm_av_registers: bool,
    pub arm_r9_usage: ArmR9Usage,
    pub arm_sl_usage: bool,
    pub arm_fp_usage: bool,
    pub arm_ip_usage: bool,
}

impl Default for DiffObjConfigV1 {
    fn default() -> Self {
        Self {
            relax_reloc_diffs: false,
            space_between_args: true,
            combine_data_sections: false,
            x86_formatter: Default::default(),
            mips_abi: Default::default(),
            mips_instr_category: Default::default(),
            arm_arch_version: Default::default(),
            arm_unified_syntax: true,
            arm_av_registers: false,
            arm_r9_usage: Default::default(),
            arm_sl_usage: false,
            arm_fp_usage: false,
            arm_ip_usage: false,
        }
    }
}

impl DiffObjConfigV1 {
    fn into_config(self) -> DiffObjConfig {
        DiffObjConfig {
            relax_reloc_diffs: self.relax_reloc_diffs,
            space_between_args: self.space_between_args,
            combine_data_sections: self.combine_data_sections,
            x86_formatter: self.x86_formatter,
            mips_abi: self.mips_abi,
            mips_instr_category: self.mips_instr_category,
            arm_arch_version: self.arm_arch_version,
            arm_unified_syntax: self.arm_unified_syntax,
            arm_av_registers: self.arm_av_registers,
            arm_r9_usage: self.arm_r9_usage,
            arm_sl_usage: self.arm_sl_usage,
            arm_fp_usage: self.arm_fp_usage,
            arm_ip_usage: self.arm_ip_usage,
            ..Default::default()
        }
    }
}

#[inline]
fn bool_true() -> bool { true }

#[derive(serde::Deserialize, serde::Serialize)]
pub struct AppConfigV1 {
    pub version: u32,
    #[serde(default)]
    pub custom_make: Option<String>,
    #[serde(default)]
    pub custom_args: Option<Vec<String>>,
    #[serde(default)]
    pub selected_wsl_distro: Option<String>,
    #[serde(default)]
    pub project_dir: Option<PathBuf>,
    #[serde(default)]
    pub target_obj_dir: Option<PathBuf>,
    #[serde(default)]
    pub base_obj_dir: Option<PathBuf>,
    #[serde(default)]
    pub selected_obj: Option<ObjectConfigV1>,
    #[serde(default = "bool_true")]
    pub build_base: bool,
    #[serde(default)]
    pub build_target: bool,
    #[serde(default = "bool_true")]
    pub rebuild_on_changes: bool,
    #[serde(default)]
    pub auto_update_check: bool,
    #[serde(default)]
    pub watch_patterns: Vec<Glob>,
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
    #[serde(default)]
    pub diff_obj_config: DiffObjConfigV1,
}

impl AppConfigV1 {
    fn into_config(self) -> AppConfig {
        log::info!("Upgrading configuration from v1");
        AppConfig {
            custom_make: self.custom_make,
            custom_args: self.custom_args,
            selected_wsl_distro: self.selected_wsl_distro,
            project_dir: self.project_dir,
            target_obj_dir: self.target_obj_dir,
            base_obj_dir: self.base_obj_dir,
            selected_obj: self.selected_obj.map(|obj| obj.into_config()),
            build_base: self.build_base,
            build_target: self.build_target,
            rebuild_on_changes: self.rebuild_on_changes,
            auto_update_check: self.auto_update_check,
            watch_patterns: self.watch_patterns,
            recent_projects: self.recent_projects,
            diff_obj_config: self.diff_obj_config.into_config(),
            ..Default::default()
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
            ..Default::default()
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

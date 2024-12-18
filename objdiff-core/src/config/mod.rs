use std::{
    fs,
    fs::File,
    io::{BufReader, BufWriter, Read},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use filetime::FileTime;
use globset::{Glob, GlobSet, GlobSetBuilder};

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify_next::Tsify), tsify(from_wasm_abi))]
pub struct ProjectConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_make: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_base: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_target: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_patterns: Option<Vec<Glob>>,
    #[serde(default, alias = "objects", skip_serializing_if = "Option::is_none")]
    pub units: Option<Vec<ProjectObject>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_categories: Option<Vec<ProjectProgressCategory>>,
}

impl ProjectConfig {
    #[inline]
    pub fn units(&self) -> &[ProjectObject] { self.units.as_deref().unwrap_or_default() }

    #[inline]
    pub fn units_mut(&mut self) -> &mut Vec<ProjectObject> {
        self.units.get_or_insert_with(Vec::new)
    }

    #[inline]
    pub fn progress_categories(&self) -> &[ProjectProgressCategory] {
        self.progress_categories.as_deref().unwrap_or_default()
    }

    #[inline]
    pub fn progress_categories_mut(&mut self) -> &mut Vec<ProjectProgressCategory> {
        self.progress_categories.get_or_insert_with(Vec::new)
    }
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify_next::Tsify), tsify(from_wasm_abi))]
pub struct ProjectObject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[deprecated(note = "Use metadata.reverse_fn_order")]
    pub reverse_fn_order: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[deprecated(note = "Use metadata.complete")]
    pub complete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scratch: Option<ScratchConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<ProjectObjectMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol_mappings: Option<SymbolMappings>,
}

#[cfg(feature = "wasm")]
#[tsify_next::declare]
pub type SymbolMappings = std::collections::BTreeMap<String, String>;

#[cfg(not(feature = "wasm"))]
pub type SymbolMappings = bimap::BiBTreeMap<String, String>;

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify_next::Tsify), tsify(from_wasm_abi))]
pub struct ProjectObjectMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverse_fn_order: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_categories: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_generated: Option<bool>,
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "wasm", derive(tsify_next::Tsify), tsify(from_wasm_abi))]
pub struct ProjectProgressCategory {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

impl ProjectObject {
    pub fn name(&self) -> &str {
        if let Some(name) = &self.name {
            name
        } else if let Some(path) = &self.path {
            path.to_str().unwrap_or("[invalid path]")
        } else {
            "[unknown]"
        }
    }

    pub fn resolve_paths(
        &mut self,
        project_dir: &Path,
        target_obj_dir: Option<&Path>,
        base_obj_dir: Option<&Path>,
    ) {
        if let (Some(target_obj_dir), Some(path), None) =
            (target_obj_dir, &self.path, &self.target_path)
        {
            self.target_path = Some(target_obj_dir.join(path));
        } else if let Some(path) = &self.target_path {
            self.target_path = Some(project_dir.join(path));
        }
        if let (Some(base_obj_dir), Some(path), None) = (base_obj_dir, &self.path, &self.base_path)
        {
            self.base_path = Some(base_obj_dir.join(path));
        } else if let Some(path) = &self.base_path {
            self.base_path = Some(project_dir.join(path));
        }
    }

    pub fn complete(&self) -> Option<bool> {
        #[expect(deprecated)]
        self.metadata.as_ref().and_then(|m| m.complete).or(self.complete)
    }

    pub fn reverse_fn_order(&self) -> Option<bool> {
        #[expect(deprecated)]
        self.metadata.as_ref().and_then(|m| m.reverse_fn_order).or(self.reverse_fn_order)
    }

    pub fn hidden(&self) -> bool {
        self.metadata.as_ref().and_then(|m| m.auto_generated).unwrap_or(false)
    }

    pub fn source_path(&self) -> Option<&String> {
        self.metadata.as_ref().and_then(|m| m.source_path.as_ref())
    }
}

#[derive(Default, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "wasm", derive(tsify_next::Tsify), tsify(from_wasm_abi))]
pub struct ScratchConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compiler: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c_flags: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctx_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_ctx: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_id: Option<u32>,
}

pub const CONFIG_FILENAMES: [&str; 3] = ["objdiff.json", "objdiff.yml", "objdiff.yaml"];

pub const DEFAULT_WATCH_PATTERNS: &[&str] = &[
    "*.c", "*.cp", "*.cpp", "*.cxx", "*.h", "*.hp", "*.hpp", "*.hxx", "*.s", "*.S", "*.asm",
    "*.inc", "*.py", "*.yml", "*.txt", "*.json",
];

pub fn default_watch_patterns() -> Vec<Glob> {
    DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect()
}

#[derive(Clone, Eq, PartialEq)]
pub struct ProjectConfigInfo {
    pub path: PathBuf,
    pub timestamp: Option<FileTime>,
}

pub fn try_project_config(dir: &Path) -> Option<(Result<ProjectConfig>, ProjectConfigInfo)> {
    for filename in CONFIG_FILENAMES.iter() {
        let config_path = dir.join(filename);
        let Ok(file) = File::open(&config_path) else {
            continue;
        };
        let metadata = file.metadata();
        if let Ok(metadata) = metadata {
            if !metadata.is_file() {
                continue;
            }
            let ts = FileTime::from_last_modification_time(&metadata);
            let mut reader = BufReader::new(file);
            let mut result = match filename.contains("json") {
                true => read_json_config(&mut reader),
                false => read_yml_config(&mut reader),
            };
            if let Ok(config) = &result {
                // Validate min_version if present
                if let Err(e) = validate_min_version(config) {
                    result = Err(e);
                }
            }
            return Some((result, ProjectConfigInfo { path: config_path, timestamp: Some(ts) }));
        }
    }
    None
}

pub fn save_project_config(
    config: &ProjectConfig,
    info: &ProjectConfigInfo,
) -> Result<ProjectConfigInfo> {
    if let Some(last_ts) = info.timestamp {
        // Check if the file has changed since we last read it
        if let Ok(metadata) = fs::metadata(&info.path) {
            let ts = FileTime::from_last_modification_time(&metadata);
            if ts != last_ts {
                return Err(anyhow!("Config file has changed since last read"));
            }
        }
    }
    let mut writer =
        BufWriter::new(File::create(&info.path).context("Failed to create config file")?);
    let ext = info.path.extension().and_then(|ext| ext.to_str()).unwrap_or("json");
    match ext {
        "json" => serde_json::to_writer_pretty(&mut writer, config).context("Failed to write JSON"),
        "yml" | "yaml" => {
            serde_yaml::to_writer(&mut writer, config).context("Failed to write YAML")
        }
        _ => Err(anyhow!("Unknown config file extension: {ext}")),
    }?;
    let file = writer.into_inner().context("Failed to flush file")?;
    let metadata = file.metadata().context("Failed to get file metadata")?;
    let ts = FileTime::from_last_modification_time(&metadata);
    Ok(ProjectConfigInfo { path: info.path.clone(), timestamp: Some(ts) })
}

fn validate_min_version(config: &ProjectConfig) -> Result<()> {
    let Some(min_version) = &config.min_version else { return Ok(()) };
    let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .context("Failed to parse package version")?;
    let min_version = semver::Version::parse(min_version).context("Failed to parse min_version")?;
    if version >= min_version {
        Ok(())
    } else {
        Err(anyhow!("Project requires objdiff version {min_version} or higher"))
    }
}

fn read_yml_config<R: Read>(reader: &mut R) -> Result<ProjectConfig> {
    Ok(serde_yaml::from_reader(reader)?)
}

fn read_json_config<R: Read>(reader: &mut R) -> Result<ProjectConfig> {
    Ok(serde_json::from_reader(reader)?)
}

pub fn build_globset(vec: &[Glob]) -> std::result::Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for glob in vec {
        builder.add(glob.clone());
    }
    builder.build()
}

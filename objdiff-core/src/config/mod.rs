pub mod path;

use alloc::{
    borrow::Cow,
    collections::BTreeMap,
    string::{String, ToString},
    vec::Vec,
};

use anyhow::{Context, Result, anyhow};
use globset::{Glob, GlobSet, GlobSetBuilder};
use path::unix_path_serde_option;
use typed_path::Utf8UnixPathBuf;

#[derive(Default, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct ProjectConfig {
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub min_version: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub custom_make: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub custom_args: Option<Vec<String>>,
    #[cfg_attr(
        feature = "serde",
        serde(with = "unix_path_serde_option", skip_serializing_if = "Option::is_none")
    )]
    pub target_dir: Option<Utf8UnixPathBuf>,
    #[cfg_attr(
        feature = "serde",
        serde(with = "unix_path_serde_option", skip_serializing_if = "Option::is_none")
    )]
    pub base_dir: Option<Utf8UnixPathBuf>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub build_base: Option<bool>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub build_target: Option<bool>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub watch_patterns: Option<Vec<String>>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub ignore_patterns: Option<Vec<String>>,
    #[cfg_attr(
        feature = "serde",
        serde(alias = "objects", skip_serializing_if = "Option::is_none")
    )]
    pub units: Option<Vec<ProjectObject>>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub progress_categories: Option<Vec<ProjectProgressCategory>>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub options: Option<ProjectOptions>,
}

impl ProjectConfig {
    #[inline]
    pub fn units(&self) -> &[ProjectObject] { self.units.as_deref().unwrap_or_default() }

    #[inline]
    pub fn progress_categories(&self) -> &[ProjectProgressCategory] {
        self.progress_categories.as_deref().unwrap_or_default()
    }

    #[inline]
    pub fn progress_categories_mut(&mut self) -> &mut Vec<ProjectProgressCategory> {
        self.progress_categories.get_or_insert_with(Vec::new)
    }

    pub fn build_watch_patterns(&self) -> Result<Vec<Glob>, globset::Error> {
        Ok(if let Some(watch_patterns) = &self.watch_patterns {
            watch_patterns
                .iter()
                .map(|s| Glob::new(s))
                .collect::<Result<Vec<Glob>, globset::Error>>()?
        } else {
            default_watch_patterns()
        })
    }

    pub fn build_ignore_patterns(&self) -> Result<Vec<Glob>, globset::Error> {
        Ok(if let Some(ignore_patterns) = &self.ignore_patterns {
            ignore_patterns
                .iter()
                .map(|s| Glob::new(s))
                .collect::<Result<Vec<Glob>, globset::Error>>()?
        } else {
            default_ignore_patterns()
        })
    }
}

#[derive(Default, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct ProjectObject {
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub name: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(with = "unix_path_serde_option", skip_serializing_if = "Option::is_none")
    )]
    pub path: Option<Utf8UnixPathBuf>,
    #[cfg_attr(
        feature = "serde",
        serde(with = "unix_path_serde_option", skip_serializing_if = "Option::is_none")
    )]
    pub target_path: Option<Utf8UnixPathBuf>,
    #[cfg_attr(
        feature = "serde",
        serde(with = "unix_path_serde_option", skip_serializing_if = "Option::is_none")
    )]
    pub base_path: Option<Utf8UnixPathBuf>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    #[deprecated(note = "Use metadata.reverse_fn_order")]
    pub reverse_fn_order: Option<bool>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    #[deprecated(note = "Use metadata.complete")]
    pub complete: Option<bool>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub scratch: Option<ScratchConfig>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub metadata: Option<ProjectObjectMetadata>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub symbol_mappings: Option<BTreeMap<String, String>>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub options: Option<ProjectOptions>,
}

#[derive(Default, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct ProjectObjectMetadata {
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub complete: Option<bool>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub reverse_fn_order: Option<bool>,
    #[cfg_attr(
        feature = "serde",
        serde(with = "unix_path_serde_option", skip_serializing_if = "Option::is_none")
    )]
    pub source_path: Option<Utf8UnixPathBuf>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub progress_categories: Option<Vec<String>>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub auto_generated: Option<bool>,
}

#[derive(Default, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(default))]
pub struct ProjectProgressCategory {
    pub id: String,
    pub name: String,
}

pub type ProjectOptions = BTreeMap<String, ProjectOptionValue>;

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize), serde(untagged))]
pub enum ProjectOptionValue {
    Bool(bool),
    String(String),
}

impl ProjectObject {
    pub fn name(&self) -> &str {
        if let Some(name) = &self.name {
            name
        } else if let Some(path) = &self.path {
            path.as_str()
        } else {
            "[unknown]"
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

    pub fn source_path(&self) -> Option<&Utf8UnixPathBuf> {
        self.metadata.as_ref().and_then(|m| m.source_path.as_ref())
    }

    pub fn progress_categories(&self) -> &[String] {
        self.metadata.as_ref().and_then(|m| m.progress_categories.as_deref()).unwrap_or_default()
    }

    pub fn auto_generated(&self) -> Option<bool> {
        self.metadata.as_ref().and_then(|m| m.auto_generated)
    }

    pub fn options(&self) -> Option<&ProjectOptions> { self.options.as_ref() }
}

#[derive(Default, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize), serde(default))]
pub struct ScratchConfig {
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub platform: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub compiler: Option<String>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub c_flags: Option<String>,
    #[cfg_attr(
        feature = "serde",
        serde(with = "unix_path_serde_option", skip_serializing_if = "Option::is_none")
    )]
    pub ctx_path: Option<Utf8UnixPathBuf>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub build_ctx: Option<bool>,
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub preset_id: Option<u32>,
}

pub const CONFIG_FILENAMES: [&str; 3] = ["objdiff.json", "objdiff.yml", "objdiff.yaml"];

pub const DEFAULT_WATCH_PATTERNS: &[&str] = &[
    "*.c", "*.cc", "*.cp", "*.cpp", "*.cxx", "*.c++", "*.h", "*.hh", "*.hp", "*.hpp", "*.hxx",
    "*.h++", "*.pch", "*.pch++", "*.inc", "*.s", "*.S", "*.asm", "*.py", "*.yml", "*.txt",
    "*.json",
];

pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &["build/**/*"];

pub fn default_watch_patterns() -> Vec<Glob> {
    DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect()
}

pub fn default_ignore_patterns() -> Vec<Glob> {
    DEFAULT_IGNORE_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect()
}

#[cfg(feature = "std")]
#[derive(Clone, Eq, PartialEq)]
pub struct ProjectConfigInfo {
    pub path: std::path::PathBuf,
    pub timestamp: Option<filetime::FileTime>,
}

#[cfg(feature = "std")]
pub fn try_project_config(
    dir: &std::path::Path,
) -> Option<(Result<ProjectConfig>, ProjectConfigInfo)> {
    for filename in CONFIG_FILENAMES.iter() {
        let config_path = dir.join(filename);
        let Ok(file) = std::fs::File::open(&config_path) else {
            continue;
        };
        let metadata = file.metadata();
        if let Ok(metadata) = metadata {
            if !metadata.is_file() {
                continue;
            }
            let ts = filetime::FileTime::from_last_modification_time(&metadata);
            let mut reader = std::io::BufReader::new(file);
            let mut result = read_json_config(&mut reader);
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

#[cfg(feature = "std")]
pub fn save_project_config(
    config: &ProjectConfig,
    info: &ProjectConfigInfo,
) -> Result<ProjectConfigInfo> {
    if let Some(last_ts) = info.timestamp {
        // Check if the file has changed since we last read it
        if let Ok(metadata) = std::fs::metadata(&info.path) {
            let ts = filetime::FileTime::from_last_modification_time(&metadata);
            if ts != last_ts {
                return Err(anyhow!("Config file has changed since last read"));
            }
        }
    }
    let mut writer = std::io::BufWriter::new(
        std::fs::File::create(&info.path).context("Failed to create config file")?,
    );
    let ext = info.path.extension().and_then(|ext| ext.to_str()).unwrap_or("json");
    match ext {
        "json" => serde_json::to_writer_pretty(&mut writer, config).context("Failed to write JSON"),
        _ => Err(anyhow!("Unknown config file extension: {ext}")),
    }?;
    let file = writer.into_inner().context("Failed to flush file")?;
    let metadata = file.metadata().context("Failed to get file metadata")?;
    let ts = filetime::FileTime::from_last_modification_time(&metadata);
    Ok(ProjectConfigInfo { path: info.path.clone(), timestamp: Some(ts) })
}

fn validate_min_version(config: &ProjectConfig) -> Result<()> {
    let Some(min_version) = &config.min_version else { return Ok(()) };
    let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|e| anyhow::Error::msg(e.to_string()))
        .context("Failed to parse package version")?;
    let min_version = semver::Version::parse(min_version)
        .map_err(|e| anyhow::Error::msg(e.to_string()))
        .context("Failed to parse min_version")?;
    if version >= min_version {
        Ok(())
    } else {
        Err(anyhow!("Project requires objdiff version {min_version} or higher"))
    }
}

#[cfg(feature = "std")]
fn read_json_config<R: std::io::Read>(reader: &mut R) -> Result<ProjectConfig> {
    Ok(serde_json::from_reader(reader)?)
}

pub fn build_globset(vec: &[Glob]) -> Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for glob in vec {
        builder.add(glob.clone());
    }
    builder.build()
}

#[cfg(feature = "any-arch")]
pub fn apply_project_options(
    diff_config: &mut crate::diff::DiffObjConfig,
    options: &ProjectOptions,
) -> Result<()> {
    use core::str::FromStr;

    use crate::diff::{ConfigEnum, ConfigPropertyId, ConfigPropertyKind};

    let mut result = Ok(());
    for (key, value) in options.iter() {
        let property_id = ConfigPropertyId::from_str(key)
            .map_err(|()| anyhow!("Invalid configuration property: {key}"))?;
        let value = match value {
            ProjectOptionValue::Bool(value) => Cow::Borrowed(if *value { "true" } else { "false" }),
            ProjectOptionValue::String(value) => Cow::Borrowed(value.as_str()),
        };
        if diff_config.set_property_value_str(property_id, &value).is_err() {
            if result.is_err() {
                // Already returning an error, skip further errors
                continue;
            }
            let mut expected = String::new();
            match property_id.kind() {
                ConfigPropertyKind::Boolean => expected.push_str("true, false"),
                ConfigPropertyKind::Choice(variants) => {
                    for (idx, variant) in variants.iter().enumerate() {
                        if idx > 0 {
                            expected.push_str(", ");
                        }
                        expected.push_str(variant.value);
                    }
                }
            }
            result = Err(anyhow!(
                "Invalid value for {}. Expected one of: {}",
                property_id.name(),
                expected
            ));
        }
    }
    result
}

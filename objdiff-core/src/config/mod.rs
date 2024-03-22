use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use filetime::FileTime;
use globset::{Glob, GlobSet, GlobSetBuilder};

#[inline]
fn bool_true() -> bool { true }

#[derive(Default, Clone, serde::Deserialize)]
pub struct ProjectConfig {
    #[serde(default)]
    pub min_version: Option<String>,
    #[serde(default)]
    pub custom_make: Option<String>,
    #[serde(default)]
    pub target_dir: Option<PathBuf>,
    #[serde(default)]
    pub base_dir: Option<PathBuf>,
    #[serde(default = "bool_true")]
    pub build_base: bool,
    #[serde(default)]
    pub build_target: bool,
    #[serde(default)]
    pub watch_patterns: Option<Vec<Glob>>,
    #[serde(default, alias = "units")]
    pub objects: Vec<ProjectObject>,
}

#[derive(Default, Clone, serde::Deserialize)]
pub struct ProjectObject {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub target_path: Option<PathBuf>,
    #[serde(default)]
    pub base_path: Option<PathBuf>,
    #[serde(default)]
    pub reverse_fn_order: Option<bool>,
    #[serde(default)]
    pub complete: Option<bool>,
    #[serde(default)]
    pub scratch: Option<ScratchConfig>,
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
}

#[derive(Default, Clone, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ScratchConfig {
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

pub const CONFIG_FILENAMES: [&str; 3] = ["objdiff.json", "objdiff.yml", "objdiff.yaml"];

pub const DEFAULT_WATCH_PATTERNS: &[&str] = &[
    "*.c", "*.cp", "*.cpp", "*.cxx", "*.h", "*.hp", "*.hpp", "*.hxx", "*.s", "*.S", "*.asm",
    "*.inc", "*.py", "*.yml", "*.txt", "*.json",
];

#[derive(Clone, Eq, PartialEq)]
pub struct ProjectConfigInfo {
    pub path: PathBuf,
    pub timestamp: FileTime,
}

pub fn try_project_config(dir: &Path) -> Option<(Result<ProjectConfig>, ProjectConfigInfo)> {
    for filename in CONFIG_FILENAMES.iter() {
        let config_path = dir.join(filename);
        let Ok(mut file) = File::open(&config_path) else {
            continue;
        };
        let metadata = file.metadata();
        if let Ok(metadata) = metadata {
            if !metadata.is_file() {
                continue;
            }
            let ts = FileTime::from_last_modification_time(&metadata);
            let mut result = match filename.contains("json") {
                true => read_json_config(&mut file),
                false => read_yml_config(&mut file),
            };
            if let Ok(config) = &result {
                // Validate min_version if present
                if let Err(e) = validate_min_version(config) {
                    result = Err(e);
                }
            }
            return Some((result, ProjectConfigInfo { path: config_path, timestamp: ts }));
        }
    }
    None
}

fn validate_min_version(config: &ProjectConfig) -> Result<()> {
    let Some(min_version) = &config.min_version else { return Ok(()) };
    let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .context("Failed to parse package version")?;
    match semver::VersionReq::parse(&format!(">={min_version}")) {
        Ok(version_req) if version_req.matches(&version) => Ok(()),
        Ok(_) => Err(anyhow!("Project requires objdiff version {min_version} or higher")),
        Err(e) => Err(anyhow::Error::new(e).context("Failed to parse min_version")),
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

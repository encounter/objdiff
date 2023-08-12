use std::{
    fs::File,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::app::AppConfig;

#[derive(Default, Clone, serde::Deserialize)]
#[serde(default)]
pub struct ProjectConfig {
    pub custom_make: Option<String>,
    pub target_dir: Option<PathBuf>,
    pub base_dir: Option<PathBuf>,
    pub build_target: bool,
    pub watch_patterns: Vec<Glob>,
    #[serde(alias = "units")]
    pub objects: Vec<ProjectObject>,
}

#[derive(Default, Clone, serde::Deserialize)]
pub struct ProjectObject {
    pub name: Option<String>,
    pub path: PathBuf,
    pub reverse_fn_order: Option<bool>,
}

#[derive(Clone)]
pub enum ProjectObjectNode {
    File(String, ProjectObject),
    Dir(String, Vec<ProjectObjectNode>),
}

fn find_dir<'a>(
    name: &str,
    nodes: &'a mut Vec<ProjectObjectNode>,
) -> &'a mut Vec<ProjectObjectNode> {
    if let Some(index) = nodes
        .iter()
        .position(|node| matches!(node, ProjectObjectNode::Dir(dir_name, _) if dir_name == name))
    {
        if let ProjectObjectNode::Dir(_, children) = &mut nodes[index] {
            return children;
        }
    } else {
        nodes.push(ProjectObjectNode::Dir(name.to_string(), vec![]));
        if let Some(ProjectObjectNode::Dir(_, children)) = nodes.last_mut() {
            return children;
        }
    }
    unreachable!();
}

fn build_nodes(objects: &[ProjectObject]) -> Vec<ProjectObjectNode> {
    let mut nodes = vec![];
    for object in objects {
        let mut out_nodes = &mut nodes;
        let path = object.name.as_ref().map(Path::new).unwrap_or(&object.path);
        if let Some(parent) = path.parent() {
            for component in parent.components() {
                if let Component::Normal(name) = component {
                    let name = name.to_str().unwrap();
                    out_nodes = find_dir(name, out_nodes);
                }
            }
        }
        let filename = path.file_name().unwrap().to_str().unwrap().to_string();
        out_nodes.push(ProjectObjectNode::File(filename, object.clone()));
    }
    nodes
}

pub const CONFIG_FILENAMES: [&str; 3] = ["objdiff.yml", "objdiff.yaml", "objdiff.json"];

pub fn load_project_config(config: &mut AppConfig) -> Result<()> {
    let Some(project_dir) = &config.project_dir else {
        return Ok(());
    };
    if let Some(result) = try_project_config(project_dir) {
        let project_config = result?;
        config.custom_make = project_config.custom_make;
        config.target_obj_dir = project_config.target_dir.map(|p| project_dir.join(p));
        config.base_obj_dir = project_config.base_dir.map(|p| project_dir.join(p));
        config.build_target = project_config.build_target;
        config.watch_patterns = project_config.watch_patterns;
        config.watcher_change = true;
        config.objects = project_config.objects;
        config.object_nodes = build_nodes(&config.objects);
    }
    Ok(())
}

fn try_project_config(dir: &Path) -> Option<Result<ProjectConfig>> {
    for filename in CONFIG_FILENAMES.iter() {
        let config_path = dir.join(filename);
        if config_path.is_file() {
            return match filename.contains("json") {
                true => Some(read_json_config(&config_path)),
                false => Some(read_yml_config(&config_path)),
            };
        }
    }
    None
}

fn read_yml_config(config_path: &Path) -> Result<ProjectConfig> {
    let mut reader = File::open(config_path)
        .with_context(|| format!("Failed to open config file '{}'", config_path.display()))?;
    Ok(serde_yaml::from_reader(&mut reader)?)
}

fn read_json_config(config_path: &Path) -> Result<ProjectConfig> {
    let mut reader = File::open(config_path)
        .with_context(|| format!("Failed to open config file '{}'", config_path.display()))?;
    Ok(serde_json::from_reader(&mut reader)?)
}

pub fn build_globset(vec: &[Glob]) -> std::result::Result<GlobSet, globset::Error> {
    let mut builder = GlobSetBuilder::new();
    for glob in vec {
        builder.add(glob.clone());
    }
    builder.build()
}

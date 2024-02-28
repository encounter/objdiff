use std::path::{Component, Path};

use anyhow::{ensure, Result};
use globset::Glob;
use objdiff_core::config::{try_project_config, ProjectObject, DEFAULT_WATCH_PATTERNS};

use crate::app::AppConfig;

#[derive(Clone)]
pub enum ProjectObjectNode {
    File(String, Box<ProjectObject>),
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

fn build_nodes(
    objects: &[ProjectObject],
    project_dir: &Path,
    target_obj_dir: Option<&Path>,
    base_obj_dir: Option<&Path>,
) -> Vec<ProjectObjectNode> {
    let mut nodes = vec![];
    for object in objects {
        let mut out_nodes = &mut nodes;
        let path = if let Some(name) = &object.name {
            Path::new(name)
        } else if let Some(path) = &object.path {
            path
        } else {
            continue;
        };
        if let Some(parent) = path.parent() {
            for component in parent.components() {
                if let Component::Normal(name) = component {
                    let name = name.to_str().unwrap();
                    out_nodes = find_dir(name, out_nodes);
                }
            }
        }
        let mut object = Box::new(object.clone());
        object.resolve_paths(project_dir, target_obj_dir, base_obj_dir);
        let filename = path.file_name().unwrap().to_str().unwrap().to_string();
        out_nodes.push(ProjectObjectNode::File(filename, object));
    }
    nodes
}

pub fn load_project_config(config: &mut AppConfig) -> Result<()> {
    let Some(project_dir) = &config.project_dir else {
        return Ok(());
    };
    if let Some((result, info)) = try_project_config(project_dir) {
        let project_config = result?;
        if let Some(min_version) = &project_config.min_version {
            let version_str = env!("CARGO_PKG_VERSION");
            let version = semver::Version::parse(version_str).unwrap();
            let version_req = semver::VersionReq::parse(&format!(">={min_version}"))?;
            ensure!(
                version_req.matches(&version),
                "Project requires objdiff version {min_version} or higher"
            );
        }
        config.custom_make = project_config.custom_make;
        config.target_obj_dir = project_config.target_dir.map(|p| project_dir.join(p));
        config.base_obj_dir = project_config.base_dir.map(|p| project_dir.join(p));
        config.build_base = project_config.build_base;
        config.build_target = project_config.build_target;
        config.watch_patterns = project_config.watch_patterns.unwrap_or_else(|| {
            DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect()
        });
        config.watcher_change = true;
        config.objects = project_config.objects;
        config.object_nodes = build_nodes(
            &config.objects,
            project_dir,
            config.target_obj_dir.as_deref(),
            config.base_obj_dir.as_deref(),
        );
        config.project_config_info = Some(info);
    }
    Ok(())
}

use std::path::{Component, Path};

use anyhow::Result;
use globset::Glob;
use objdiff_core::config::{try_project_config, ProjectObject, DEFAULT_WATCH_PATTERNS};

use crate::app::AppState;

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

pub fn load_project_config(state: &mut AppState) -> Result<()> {
    let Some(project_dir) = &state.config.project_dir else {
        return Ok(());
    };
    if let Some((result, info)) = try_project_config(project_dir) {
        let project_config = result?;
        state.config.custom_make = project_config.custom_make;
        state.config.custom_args = project_config.custom_args;
        state.config.target_obj_dir = project_config.target_dir.map(|p| project_dir.join(p));
        state.config.base_obj_dir = project_config.base_dir.map(|p| project_dir.join(p));
        state.config.build_base = project_config.build_base;
        state.config.build_target = project_config.build_target;
        state.config.watch_patterns = project_config.watch_patterns.unwrap_or_else(|| {
            DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect()
        });
        state.watcher_change = true;
        state.objects = project_config.objects;
        state.object_nodes = build_nodes(
            &state.objects,
            project_dir,
            state.config.target_obj_dir.as_deref(),
            state.config.base_obj_dir.as_deref(),
        );
        state.project_config_info = Some(info);
    }
    Ok(())
}

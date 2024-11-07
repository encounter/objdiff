use std::path::{Component, Path};

use anyhow::Result;
use globset::Glob;
use objdiff_core::config::{try_project_config, ProjectObject, DEFAULT_WATCH_PATTERNS};

use crate::app::{AppState, ObjectConfig};

#[derive(Clone)]
pub enum ProjectObjectNode {
    Unit(String, usize),
    Dir(String, Vec<ProjectObjectNode>),
}

fn join_single_dir_entries(nodes: &mut Vec<ProjectObjectNode>) {
    for node in nodes {
        if let ProjectObjectNode::Dir(my_name, my_nodes) = node {
            join_single_dir_entries(my_nodes);
            // If this directory consists of a single sub-directory...
            if let [ProjectObjectNode::Dir(sub_name, sub_nodes)] = &mut my_nodes[..] {
                // ... join the two names with a path separator and eliminate the layer
                *my_name += "/";
                *my_name += sub_name;
                *my_nodes = std::mem::take(sub_nodes);
            }
        }
    }
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
    units: &mut [ProjectObject],
    project_dir: &Path,
    target_obj_dir: Option<&Path>,
    base_obj_dir: Option<&Path>,
) -> Vec<ProjectObjectNode> {
    let mut nodes = vec![];
    for (idx, unit) in units.iter_mut().enumerate() {
        unit.resolve_paths(project_dir, target_obj_dir, base_obj_dir);
        let mut out_nodes = &mut nodes;
        let path = if let Some(name) = &unit.name {
            Path::new(name)
        } else if let Some(path) = &unit.path {
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
        let filename = path.file_name().unwrap().to_str().unwrap().to_string();
        out_nodes.push(ProjectObjectNode::Unit(filename, idx));
    }
    // Within the top-level module directories, join paths. Leave the
    // top-level name intact though since it's the module name.
    for node in &mut nodes {
        if let ProjectObjectNode::Dir(_, sub_nodes) = node {
            join_single_dir_entries(sub_nodes);
        }
    }

    nodes
}

pub fn load_project_config(state: &mut AppState) -> Result<()> {
    let Some(project_dir) = &state.config.project_dir else {
        return Ok(());
    };
    if let Some((result, info)) = try_project_config(project_dir) {
        let project_config = result?;
        state.config.custom_make = project_config.custom_make.clone();
        state.config.custom_args = project_config.custom_args.clone();
        state.config.target_obj_dir =
            project_config.target_dir.as_deref().map(|p| project_dir.join(p));
        state.config.base_obj_dir = project_config.base_dir.as_deref().map(|p| project_dir.join(p));
        state.config.build_base = project_config.build_base.unwrap_or(true);
        state.config.build_target = project_config.build_target.unwrap_or(false);
        state.config.watch_patterns = project_config.watch_patterns.clone().unwrap_or_else(|| {
            DEFAULT_WATCH_PATTERNS.iter().map(|s| Glob::new(s).unwrap()).collect()
        });
        state.watcher_change = true;
        state.objects = project_config.units.clone().unwrap_or_default();
        state.object_nodes = build_nodes(
            &mut state.objects,
            project_dir,
            state.config.target_obj_dir.as_deref(),
            state.config.base_obj_dir.as_deref(),
        );
        state.current_project_config = Some(project_config);
        state.project_config_info = Some(info);

        // Reload selected object
        if let Some(selected_obj) = &state.config.selected_obj {
            if let Some(obj) = state.objects.iter().find(|o| o.name() == selected_obj.name) {
                let config = ObjectConfig::from(obj);
                state.set_selected_obj(config);
            } else {
                state.clear_selected_obj();
            }
        }
    }
    Ok(())
}

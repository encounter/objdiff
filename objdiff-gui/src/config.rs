use anyhow::Result;
use globset::Glob;
use objdiff_core::{
    config::{
        apply_project_options, default_ignore_patterns, default_watch_patterns, try_project_config,
    },
    diff::DiffObjConfig,
};
use typed_path::{Utf8UnixComponent, Utf8UnixPath};

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

fn build_nodes(units: &mut [ObjectConfig]) -> Vec<ProjectObjectNode> {
    let mut nodes = vec![];
    for (idx, unit) in units.iter_mut().enumerate() {
        let mut out_nodes = &mut nodes;
        let path = Utf8UnixPath::new(&unit.name);
        if let Some(parent) = path.parent() {
            for component in parent.components() {
                if let Utf8UnixComponent::Normal(name) = component {
                    out_nodes = find_dir(name, out_nodes);
                }
            }
        }
        let filename = path.file_name().unwrap().to_string();
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
    if let Some((result, info)) = try_project_config(project_dir.as_ref()) {
        let project_config = result?;
        state.config.custom_make = project_config.custom_make.clone();
        state.config.custom_args = project_config.custom_args.clone();
        state.config.target_obj_dir = project_config
            .target_dir
            .as_deref()
            .map(|p| project_dir.join(p.with_platform_encoding()));
        state.config.base_obj_dir = project_config
            .base_dir
            .as_deref()
            .map(|p| project_dir.join(p.with_platform_encoding()));
        state.config.build_base = project_config.build_base.unwrap_or(true);
        state.config.build_target = project_config.build_target.unwrap_or(false);
        if let Some(watch_patterns) = &project_config.watch_patterns {
            state.config.watch_patterns = watch_patterns
                .iter()
                .map(|s| Glob::new(s))
                .collect::<Result<Vec<Glob>, globset::Error>>()?;
        } else {
            state.config.watch_patterns = default_watch_patterns();
        }
        if let Some(ignore_patterns) = &project_config.ignore_patterns {
            state.config.ignore_patterns = ignore_patterns
                .iter()
                .map(|s| Glob::new(s))
                .collect::<Result<Vec<Glob>, globset::Error>>()?;
        } else {
            state.config.ignore_patterns = default_ignore_patterns();
        }
        state.watcher_change = true;
        state.objects = project_config
            .units
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|o| {
                ObjectConfig::new(
                    o,
                    project_dir,
                    state.config.target_obj_dir.as_deref(),
                    state.config.base_obj_dir.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        state.object_nodes = build_nodes(&mut state.objects);
        state.current_project_config = Some(project_config);
        state.project_config_info = Some(info);
        if let Some(options) =
            state.current_project_config.as_ref().and_then(|project| project.options.as_ref())
        {
            let mut diff_config = DiffObjConfig::default();
            if let Err(e) = apply_project_options(&mut diff_config, options) {
                log::error!("Failed to apply project config options: {e:#}");
                state.show_error_toast("Failed to apply project config options", &e);
            }
        }

        // Reload selected object
        if let Some(selected_obj) = &state.config.selected_obj {
            if let Some(obj) = state.objects.iter().find(|o| o.name == selected_obj.name) {
                state.set_selected_obj(obj.clone());
            } else {
                state.clear_selected_obj();
            }
        }
    }
    Ok(())
}

//! Project (`objdiff.json`) awareness: load a config, resolve a unit's
//! target/base object paths exactly like objdiff does, merge diff options,
//! and run the project's build command.

use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use objdiff_core::{
    build::{BuildConfig, BuildStatus},
    config::{
        ProjectConfig, ProjectConfigInfo, ProjectObject, apply_project_options, save_project_config,
    },
    diff::DiffObjConfig,
};
use typed_path::Utf8PlatformPathBuf;

/// A loaded project plus its directory (native encoding).
pub struct LoadedProject {
    pub dir: Utf8PlatformPathBuf,
    pub config: ProjectConfig,
    pub info: ProjectConfigInfo,
}

impl LoadedProject {
    pub fn load(dir: &str) -> Result<Self> {
        let native = PathBuf::from(dir);
        let (result, info) = objdiff_core::config::try_project_config(&native)
            .with_context(|| format!("No objdiff.json/yml found in {dir}"))?;
        let config = result.context("Failed to parse project config")?;
        Ok(Self { dir: Utf8PlatformPathBuf::from(dir), config, info })
    }

    fn object(&self, unit: &str) -> Result<&ProjectObject> {
        self.config
            .units()
            .iter()
            .find(|o| o.name() == unit)
            .with_context(|| format!("Unit `{unit}` not found in project"))
    }

    fn object_mut(&mut self, unit: &str) -> Result<&mut ProjectObject> {
        self.config
            .units
            .as_mut()
            .and_then(|units| units.iter_mut().find(|o| o.name() == unit))
            .with_context(|| format!("Unit `{unit}` not found in project"))
    }

    /// Resolve a unit's (target, base) object paths, mirroring objdiff's
    /// `ObjectConfig::new` path logic.
    pub fn resolve_paths(&self, unit: &str) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
        let obj = self.object(unit)?;
        let target_dir =
            self.config.target_dir.as_ref().map(|d| self.dir.join(d.with_platform_encoding()));
        let base_dir =
            self.config.base_dir.as_ref().map(|d| self.dir.join(d.with_platform_encoding()));

        let target = match (&target_dir, &obj.path, &obj.target_path) {
            (Some(td), Some(path), None) => Some(td.join(path.with_platform_encoding())),
            _ => obj.target_path.as_ref().map(|p| self.dir.join(p.with_platform_encoding())),
        };
        let base = match (&base_dir, &obj.path, &obj.base_path) {
            (Some(bd), Some(path), None) => Some(bd.join(path.with_platform_encoding())),
            _ => obj.base_path.as_ref().map(|p| self.dir.join(p.with_platform_encoding())),
        };
        Ok((target.map(to_std), base.map(to_std)))
    }

    /// A unit's manual symbol mappings (target name -> base name), if any.
    pub fn symbol_mappings(&self, unit: &str) -> Result<BTreeMap<String, String>> {
        Ok(self.object(unit)?.symbol_mappings.clone().unwrap_or_default())
    }

    /// Add, remove, or clear a unit's manual symbol mappings, persisting the
    /// change to the project config file.
    ///
    /// * `target` + `base`: map the target symbol to the base symbol.
    /// * `target` only: remove the mapping for that target symbol.
    /// * neither: clear all mappings for the unit.
    pub fn set_symbol_mapping(
        &mut self,
        unit: &str,
        target: Option<&str>,
        base: Option<&str>,
    ) -> Result<String> {
        let summary = {
            let object = self.object_mut(unit)?;
            let mappings = object.symbol_mappings.get_or_insert_with(BTreeMap::new);
            let summary = match (target, base) {
                (Some(target), Some(base)) => {
                    // Mirrors the GUI: a symbol may only participate in one
                    // mapping on each side, and identity mappings are implicit.
                    mappings.retain(|l, r| l != target && r != base);
                    if target == base {
                        format!("`{target}` maps to itself; no mapping stored for unit `{unit}`")
                    } else {
                        mappings.insert(target.to_string(), base.to_string());
                        format!("Mapped target `{target}` -> base `{base}` in unit `{unit}`")
                    }
                }
                (Some(target), None) => match mappings.remove(target) {
                    Some(base) => {
                        format!("Removed mapping `{target}` -> `{base}` from unit `{unit}`")
                    }
                    None => format!("No mapping for target `{target}` in unit `{unit}`"),
                },
                (None, Some(base)) => {
                    // Removing by base name, since either side identifies a mapping.
                    let before = mappings.len();
                    mappings.retain(|_, r| r != base);
                    match before - mappings.len() {
                        0 => format!("No mapping to base `{base}` in unit `{unit}`"),
                        _ => format!("Removed mapping to base `{base}` from unit `{unit}`"),
                    }
                }
                (None, None) => {
                    let count = mappings.len();
                    mappings.clear();
                    format!("Cleared {count} mapping(s) from unit `{unit}`")
                }
            };
            if mappings.is_empty() {
                object.symbol_mappings = None;
            }
            summary
        };
        self.save()?;
        Ok(summary)
    }

    /// Write the (possibly modified) project config back to disk.
    fn save(&mut self) -> Result<()> {
        self.info = save_project_config(&self.config, &self.info)
            .context("Failed to save project config")?;
        Ok(())
    }

    /// One-line-per-mapping listing for a unit.
    pub fn list_symbol_mappings(&self, unit: &str) -> Result<String> {
        use std::fmt::Write as _;
        let mappings = self.symbol_mappings(unit)?;
        let mut out = format!("{} mapping(s) in unit `{unit}` (target -> base):\n", mappings.len());
        for (target, base) in &mappings {
            let _ = writeln!(out, "  {target} -> {base}");
        }
        Ok(out)
    }

    /// Build the diff config for a unit: project options then unit options.
    pub fn apply_options(&self, unit: Option<&str>, config: &mut DiffObjConfig) -> Result<()> {
        if let Some(options) = &self.config.options {
            apply_project_options(config, options)?;
        }
        if let Some(unit) = unit
            && let Some(options) = &self.object(unit)?.options
        {
            apply_project_options(config, options)?;
        }
        Ok(())
    }

    /// Prepare a build invocation (config + relative unix object path) for a
    /// unit's base (or target) object. The caller runs `run_make` off-lock.
    pub fn build_invocation(
        &self,
        unit: &str,
        target: bool,
    ) -> Result<(BuildConfig, typed_path::Utf8UnixPathBuf)> {
        let (target_path, base_path) = self.resolve_paths(unit)?;
        let path = if target { target_path } else { base_path };
        let path = path.ok_or_else(|| {
            anyhow!("Unit `{unit}` has no {} path", if target { "target" } else { "base" })
        })?;
        let path_str = path.to_string_lossy().into_owned();
        let path_pp = Utf8PlatformPathBuf::from(path_str);
        let rel = path_pp.strip_prefix(&self.dir).map_err(|_| {
            anyhow!("Object path `{path_pp}` is not inside project dir `{}`", self.dir)
        })?;
        let rel_unix = rel.with_unix_encoding();
        let build_config = BuildConfig {
            project_dir: Some(self.dir.clone()),
            custom_make: self.config.custom_make.clone(),
            custom_args: self.config.custom_args.clone(),
            selected_wsl_distro: None,
        };
        Ok((build_config, rel_unix))
    }

    /// One-line-per-unit listing with resolved path presence.
    pub fn list_units(&self, filter: Option<&str>) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let units = self.config.units();
        let _ = writeln!(out, "{} unit(s):", units.len());
        for obj in units {
            let name = obj.name();
            if let Some(f) = filter
                && !name.contains(f)
            {
                continue;
            }
            let (t, b) = self.resolve_paths(name).unwrap_or((None, None));
            let _ = writeln!(
                out,
                "  {name}  [target: {}] [base: {}]",
                t.map(|p| p.display().to_string()).unwrap_or_else(|| "-".into()),
                b.map(|p| p.display().to_string()).unwrap_or_else(|| "-".into()),
            );
        }
        out
    }
}

fn to_std(p: Utf8PlatformPathBuf) -> PathBuf { PathBuf::from(p.as_str()) }

/// Ensure a build succeeded, otherwise surface stdout/stderr.
pub fn build_status_text(status: &BuildStatus) -> String {
    let mut out = format!(
        "$ {}\nexit: {}\n",
        status.cmdline,
        if status.success { "success" } else { "FAILED" }
    );
    if !status.stdout.is_empty() {
        out.push_str("--- stdout ---\n");
        out.push_str(&status.stdout);
        out.push('\n');
    }
    if !status.stderr.is_empty() {
        out.push_str("--- stderr ---\n");
        out.push_str(&status.stderr);
        out.push('\n');
    }
    out
}

/// Resolve final (target, base) std paths from either explicit paths or a unit.
pub fn resolve_inputs(
    project: Option<&LoadedProject>,
    unit: Option<&str>,
    target: Option<&str>,
    base: Option<&str>,
) -> Result<(PathBuf, PathBuf)> {
    if let Some(unit) = unit {
        let project = project.context("No project loaded; call open_project first")?;
        let (t, b) = project.resolve_paths(unit)?;
        let t = t.with_context(|| format!("Unit `{unit}` has no target path"))?;
        let b = b.with_context(|| format!("Unit `{unit}` has no base path"))?;
        Ok((t, b))
    } else {
        match (target, base) {
            (Some(t), Some(b)) => Ok((PathBuf::from(t), PathBuf::from(b))),
            _ => bail!("Provide either `unit` (with a loaded project) or both `target` and `base`"),
        }
    }
}

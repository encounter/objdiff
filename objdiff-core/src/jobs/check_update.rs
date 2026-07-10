use std::{sync::mpsc::Receiver, task::Waker};

use anyhow::{Context, Result};
use self_update::update::{Release, ReleaseUpdate};

use crate::jobs::{Job, JobContext, JobResult, JobState, start_job, update_status};

pub struct CheckUpdateConfig {
    pub build_updater: fn() -> Result<Box<dyn ReleaseUpdate>>,
    pub bin_names: Vec<String>,
}

pub struct CheckUpdateResult {
    pub update_available: bool,
    pub latest_release: Release,
    pub found_binary: Option<String>,
}

fn run_check_update(
    context: &JobContext,
    cancel: Receiver<()>,
    config: CheckUpdateConfig,
) -> Result<Box<CheckUpdateResult>> {
    update_status(context, "Fetching latest release".to_string(), 0, 1, &cancel)?;
    let updater = (config.build_updater)().context("Failed to create release updater")?;
    let releases = updater.get_latest_release()?;
    let update_available = releases.is_update_available()?;
    let latest_release = releases.latest().context("No release found")?.clone();
    // Find the binary name in the release assets
    let mut found_binary = None;
    for bin_name in &config.bin_names {
        if latest_release.assets().iter().any(|a| a.name() == bin_name) {
            found_binary = Some(bin_name.clone());
            break;
        }
    }

    update_status(context, "Complete".to_string(), 1, 1, &cancel)?;
    Ok(Box::new(CheckUpdateResult { update_available, latest_release, found_binary }))
}

pub fn start_check_update(waker: Waker, config: CheckUpdateConfig) -> JobState {
    start_job(waker, "Check for updates", Job::CheckUpdate, move |context, cancel| {
        run_check_update(&context, cancel, config)
            .map(|result| JobResult::CheckUpdate(Some(result)))
    })
}

use std::sync::mpsc::Receiver;

use anyhow::{Context, Result};
use self_update::{cargo_crate_version, update::Release};

use crate::{
    jobs::{start_job, update_status, Job, JobContext, JobResult, JobState},
    update::{build_updater, BIN_NAME},
};

pub struct CheckUpdateResult {
    pub update_available: bool,
    pub latest_release: Release,
    pub found_binary: bool,
}

fn run_check_update(context: &JobContext, cancel: Receiver<()>) -> Result<Box<CheckUpdateResult>> {
    update_status(context, "Fetching latest release".to_string(), 0, 1, &cancel)?;
    let updater = build_updater().context("Failed to create release updater")?;
    let latest_release = updater.get_latest_release()?;
    let update_available =
        self_update::version::bump_is_greater(cargo_crate_version!(), &latest_release.version)?;
    let found_binary = latest_release.assets.iter().any(|a| a.name == BIN_NAME);

    update_status(context, "Complete".to_string(), 1, 1, &cancel)?;
    Ok(Box::new(CheckUpdateResult { update_available, latest_release, found_binary }))
}

pub fn start_check_update(ctx: &egui::Context) -> JobState {
    start_job(ctx, "Check for updates", Job::CheckUpdate, move |context, cancel| {
        run_check_update(&context, cancel).map(|result| JobResult::CheckUpdate(Some(result)))
    })
}

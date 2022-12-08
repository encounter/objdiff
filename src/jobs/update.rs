use std::{
    env::{current_dir, current_exe},
    fs::File,
    path::PathBuf,
    sync::mpsc::Receiver,
};

use anyhow::{Context, Result};
use const_format::formatcp;

use crate::{
    jobs::{queue_job, update_status, Job, JobResult, JobState, Status},
    update::{build_updater, BIN_NAME},
};

pub struct UpdateResult {
    pub exe_path: PathBuf,
}

fn run_update(status: &Status, cancel: Receiver<()>) -> Result<Box<UpdateResult>> {
    update_status(status, "Fetching latest release".to_string(), 0, 3, &cancel)?;
    let updater = build_updater().context("Failed to create release updater")?;
    let latest_release = updater.get_latest_release()?;
    let asset = latest_release
        .assets
        .iter()
        .find(|a| a.name == BIN_NAME)
        .ok_or_else(|| anyhow::Error::msg(formatcp!("No release asset for {}", BIN_NAME)))?;

    update_status(status, "Downloading release".to_string(), 1, 3, &cancel)?;
    let tmp_dir = tempfile::Builder::new().prefix("update").tempdir_in(current_dir()?)?;
    let tmp_path = tmp_dir.path().join(&asset.name);
    let tmp_file = File::create(&tmp_path)?;
    self_update::Download::from_url(&asset.download_url)
        .set_header(reqwest::header::ACCEPT, "application/octet-stream".parse()?)
        .download_to(&tmp_file)?;

    update_status(status, "Extracting release".to_string(), 2, 3, &cancel)?;
    let tmp_file = tmp_dir.path().join("replacement_tmp");
    let target_file = current_exe()?;
    self_update::Move::from_source(&tmp_path)
        .replace_using_temp(&tmp_file)
        .to_dest(&target_file)?;
    #[cfg(unix)]
    {
        use std::{fs, os::unix::fs::PermissionsExt};
        let mut perms = fs::metadata(&target_file)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&target_file, perms)?;
    }

    update_status(status, "Complete".to_string(), 3, 3, &cancel)?;
    Ok(Box::from(UpdateResult { exe_path: target_file }))
}

pub fn queue_update() -> JobState {
    queue_job("Update app", Job::Update, move |status, cancel| {
        run_update(status, cancel).map(JobResult::Update)
    })
}

use std::{
    env::{current_dir, current_exe},
    fs::File,
    path::PathBuf,
    sync::mpsc::Receiver,
    task::Waker,
};

use anyhow::{Context, Result};
pub use self_update; // Re-export self_update crate
use self_update::update::ReleaseUpdate;

use crate::jobs::{start_job, update_status, Job, JobContext, JobResult, JobState};

pub struct UpdateConfig {
    pub build_updater: fn() -> Result<Box<dyn ReleaseUpdate>>,
    pub bin_name: String,
}

pub struct UpdateResult {
    pub exe_path: PathBuf,
}

fn run_update(
    status: &JobContext,
    cancel: Receiver<()>,
    config: UpdateConfig,
) -> Result<Box<UpdateResult>> {
    update_status(status, "Fetching latest release".to_string(), 0, 3, &cancel)?;
    let updater = (config.build_updater)().context("Failed to create release updater")?;
    let latest_release = updater.get_latest_release()?;
    let asset =
        latest_release.assets.iter().find(|a| a.name == config.bin_name).ok_or_else(|| {
            anyhow::Error::msg(format!("No release asset for {}", config.bin_name))
        })?;

    update_status(status, "Downloading release".to_string(), 1, 3, &cancel)?;
    let tmp_dir = tempfile::Builder::new().prefix("update").tempdir_in(current_dir()?)?;
    let tmp_path = tmp_dir.path().join(&asset.name);
    let tmp_file = File::create(&tmp_path)?;
    self_update::Download::from_url(&asset.download_url)
        .set_header(reqwest::header::ACCEPT, "application/octet-stream".parse()?)
        .download_to(tmp_file)?;

    update_status(status, "Extracting release".to_string(), 2, 3, &cancel)?;
    let tmp_file = tmp_dir.path().join("replacement_tmp");
    let target_file = current_exe()?;
    self_update::Move::from_source(&tmp_path)
        .replace_using_temp(&tmp_file)
        .to_dest(&target_file)?;
    #[cfg(unix)]
    {
        use std::{fs, os::unix::fs::PermissionsExt};
        fs::set_permissions(&target_file, fs::Permissions::from_mode(0o755))?;
    }
    tmp_dir.close()?;

    update_status(status, "Complete".to_string(), 3, 3, &cancel)?;
    Ok(Box::from(UpdateResult { exe_path: target_file }))
}

pub fn start_update(waker: Waker, config: UpdateConfig) -> JobState {
    start_job(waker, "Update app", Job::Update, move |context, cancel| {
        run_update(&context, cancel, config).map(JobResult::Update)
    })
}

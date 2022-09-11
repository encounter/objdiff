use std::sync::{mpsc::Receiver, Arc, RwLock};

use anyhow::{Error, Result};

use crate::{
    app::AppConfig,
    diff::diff_objs,
    elf,
    jobs::{queue_job, update_status, Job, JobResult, JobState, Status},
    obj::ObjInfo,
};

pub struct BinDiffResult {
    pub first_obj: ObjInfo,
    pub second_obj: ObjInfo,
}

fn run_build(
    status: &Status,
    cancel: Receiver<()>,
    config: Arc<RwLock<AppConfig>>,
) -> Result<Box<BinDiffResult>> {
    let config = config.read().map_err(|_| Error::msg("Failed to lock app config"))?.clone();
    let left_path = config.left_obj.as_ref().ok_or_else(|| Error::msg("Missing left obj path"))?;
    let right_path =
        config.right_obj.as_ref().ok_or_else(|| Error::msg("Missing right obj path"))?;

    update_status(status, "Loading left obj".to_string(), 0, 3, &cancel)?;
    let mut left_obj = elf::read(left_path)?;

    update_status(status, "Loading right obj".to_string(), 1, 3, &cancel)?;
    let mut right_obj = elf::read(right_path)?;

    update_status(status, "Performing diff".to_string(), 2, 3, &cancel)?;
    diff_objs(&mut left_obj, &mut right_obj)?;

    update_status(status, "Complete".to_string(), 3, 3, &cancel)?;
    Ok(Box::new(BinDiffResult { first_obj: left_obj, second_obj: right_obj }))
}

pub fn queue_bindiff(config: Arc<RwLock<AppConfig>>) -> JobState {
    queue_job(Job::BinDiff, move |status, cancel| {
        run_build(status, cancel, config).map(JobResult::BinDiff)
    })
}

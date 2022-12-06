use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, Sender, TryRecvError},
        Arc, RwLock,
    },
    thread::JoinHandle,
};

use anyhow::Result;

use crate::jobs::{
    bindiff::BinDiffResult, check_update::CheckUpdateResult, objdiff::ObjDiffResult,
    update::UpdateResult,
};

pub mod bindiff;
pub mod check_update;
pub mod objdiff;
pub mod update;

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum Job {
    ObjDiff,
    BinDiff,
    CheckUpdate,
    Update,
}
pub static JOB_ID: AtomicUsize = AtomicUsize::new(0);
pub struct JobState {
    pub id: usize,
    pub job_type: Job,
    pub handle: Option<JoinHandle<JobResult>>,
    pub status: Arc<RwLock<JobStatus>>,
    pub cancel: Sender<()>,
    pub should_remove: bool,
}
#[derive(Default)]
pub struct JobStatus {
    pub title: String,
    pub progress_percent: f32,
    pub progress_items: Option<[u32; 2]>,
    pub status: String,
    pub error: Option<anyhow::Error>,
}
pub enum JobResult {
    None,
    ObjDiff(Box<ObjDiffResult>),
    BinDiff(Box<BinDiffResult>),
    CheckUpdate(Box<CheckUpdateResult>),
    Update(Box<UpdateResult>),
}

fn should_cancel(rx: &Receiver<()>) -> bool {
    match rx.try_recv() {
        Ok(_) | Err(TryRecvError::Disconnected) => true,
        Err(_) => false,
    }
}

type Status = Arc<RwLock<JobStatus>>;

fn queue_job(
    title: &str,
    job_type: Job,
    run: impl FnOnce(&Status, Receiver<()>) -> Result<JobResult> + Send + 'static,
) -> JobState {
    let status = Arc::new(RwLock::new(JobStatus {
        title: title.to_string(),
        progress_percent: 0.0,
        progress_items: None,
        status: String::new(),
        error: None,
    }));
    let status_clone = status.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        return match run(&status, rx) {
            Ok(state) => state,
            Err(e) => {
                if let Ok(mut w) = status.write() {
                    w.error = Some(e);
                }
                JobResult::None
            }
        };
    });
    let id = JOB_ID.fetch_add(1, Ordering::Relaxed);
    log::info!("Started job {}", id);
    JobState {
        id,
        job_type,
        handle: Some(handle),
        status: status_clone,
        cancel: tx,
        should_remove: true,
    }
}

fn update_status(
    status: &Status,
    str: String,
    count: u32,
    total: u32,
    cancel: &Receiver<()>,
) -> Result<()> {
    let mut w = status.write().map_err(|_| anyhow::Error::msg("Failed to lock job status"))?;
    w.progress_items = Some([count, total]);
    w.progress_percent = count as f32 / total as f32;
    if should_cancel(cancel) {
        w.status = "Cancelled".to_string();
        return Err(anyhow::Error::msg("Cancelled"));
    } else {
        w.status = str;
    }
    Ok(())
}

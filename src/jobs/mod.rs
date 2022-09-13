use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, Sender, TryRecvError},
        Arc, RwLock,
    },
    thread::JoinHandle,
};

use anyhow::Result;

use crate::jobs::{bindiff::BinDiffResult, objdiff::ObjDiffResult};

pub mod bindiff;
pub mod objdiff;

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum Job {
    ObjDiff,
    BinDiff,
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
}

fn should_cancel(rx: &Receiver<()>) -> bool {
    match rx.try_recv() {
        Ok(_) | Err(TryRecvError::Disconnected) => true,
        Err(_) => false,
    }
}

type Status = Arc<RwLock<JobStatus>>;

fn queue_job(
    job_type: Job,
    run: impl FnOnce(&Status, Receiver<()>) -> Result<JobResult> + Send + 'static,
) -> JobState {
    let status = Arc::new(RwLock::new(JobStatus {
        title: String::new(),
        progress_percent: 0.0,
        progress_items: None,
        status: "".to_string(),
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

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc::{Receiver, Sender, TryRecvError},
        Arc, RwLock,
    },
    thread::JoinHandle,
};

use anyhow::Result;

use crate::jobs::{check_update::CheckUpdateResult, objdiff::ObjDiffResult, update::UpdateResult};

pub mod check_update;
pub mod objdiff;
pub mod update;

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub enum Job {
    ObjDiff,
    CheckUpdate,
    Update,
}
pub static JOB_ID: AtomicUsize = AtomicUsize::new(0);

#[derive(Default)]
pub struct JobQueue {
    pub jobs: Vec<JobState>,
}

impl JobQueue {
    /// Adds a job to the queue.
    pub fn push(&mut self, state: JobState) { self.jobs.push(state); }

    /// Returns whether a job of the given kind is running.
    pub fn is_running(&self, kind: Job) -> bool {
        self.jobs.iter().any(|j| j.kind == kind && j.handle.is_some())
    }

    /// Returns whether any job is running.
    pub fn any_running(&self) -> bool {
        self.jobs.iter().any(|job| {
            if let Some(handle) = &job.handle {
                return !handle.is_finished();
            }
            false
        })
    }

    /// Iterates over all jobs mutably.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut JobState> + '_ { self.jobs.iter_mut() }

    /// Iterates over all finished jobs, returning the job state and the result.
    pub fn iter_finished(
        &mut self,
    ) -> impl Iterator<Item = (&mut JobState, std::thread::Result<JobResult>)> + '_ {
        self.jobs.iter_mut().filter_map(|job| {
            if let Some(handle) = &job.handle {
                if !handle.is_finished() {
                    return None;
                }
                let result = job.handle.take().unwrap().join();
                return Some((job, result));
            }
            None
        })
    }

    /// Clears all finished jobs.
    pub fn clear_finished(&mut self) {
        self.jobs.retain(|job| {
            !(job.should_remove
                && job.handle.is_none()
                && job.status.read().unwrap().error.is_none())
        });
    }

    /// Removes a job from the queue given its ID.
    pub fn remove(&mut self, id: usize) { self.jobs.retain(|job| job.id != id); }
}

pub struct JobState {
    pub id: usize,
    pub kind: Job,
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

fn start_job(
    title: &str,
    kind: Job,
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
        kind,
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

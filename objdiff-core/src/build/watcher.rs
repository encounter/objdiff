use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::Waker,
    time::Duration,
};

use globset::GlobSet;
use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, new_debouncer_opt};

pub type Watcher = notify_debouncer_full::Debouncer<
    notify::RecommendedWatcher,
    notify_debouncer_full::RecommendedCache,
>;

pub struct WatcherState {
    pub config_path: Option<PathBuf>,
    pub left_obj_path: Option<PathBuf>,
    pub right_obj_path: Option<PathBuf>,
    pub patterns: GlobSet,
}

pub fn create_watcher(
    modified: Arc<AtomicBool>,
    project_dir: &Path,
    patterns: GlobSet,
    waker: Waker,
) -> notify::Result<Watcher> {
    let base_dir = fs::canonicalize(project_dir)?;
    let base_dir_clone = base_dir.clone();
    let timeout = Duration::from_millis(200);
    let config = notify::Config::default().with_poll_interval(Duration::from_secs(2));
    let mut debouncer = new_debouncer_opt(
        timeout,
        None,
        move |result: DebounceEventResult| match result {
            Ok(events) => {
                let mut any_match = false;
                for event in events.iter() {
                    if !matches!(
                        event.kind,
                        notify::EventKind::Modify(..)
                            | notify::EventKind::Create(..)
                            | notify::EventKind::Remove(..)
                    ) {
                        continue;
                    }
                    for path in &event.paths {
                        let Ok(path) = path.strip_prefix(&base_dir_clone) else {
                            continue;
                        };
                        if patterns.is_match(path) {
                            // log::info!("File modified: {}", path.display());
                            any_match = true;
                        }
                    }
                }
                if any_match {
                    modified.store(true, Ordering::Relaxed);
                    waker.wake_by_ref();
                }
            }
            Err(errors) => errors.iter().for_each(|e| log::error!("Watch error: {e:?}")),
        },
        notify_debouncer_full::RecommendedCache::new(),
        config,
    )?;
    debouncer.watch(base_dir, RecursiveMode::Recursive)?;
    Ok(debouncer)
}

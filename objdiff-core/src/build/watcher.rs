use std::{
    fs,
    path::{Component, Path, PathBuf, Prefix},
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

pub enum AnyDebouncer {
    NativeDebouncer(
        notify_debouncer_full::Debouncer<
            notify::RecommendedWatcher,
            notify_debouncer_full::RecommendedCache,
        >,
    ),
    NetworkFSDebouncer(
        notify_debouncer_full::Debouncer<notify::PollWatcher, notify_debouncer_full::NoCache>,
    ),
}

impl AnyDebouncer {
    pub fn watch(&mut self, path: impl AsRef<Path>, mode: RecursiveMode) -> notify::Result<()> {
        match self {
            Self::NativeDebouncer(d) => d.watch(path, mode),
            Self::NetworkFSDebouncer(d) => d.watch(path, mode),
        }
    }

    pub fn unwatch(&mut self, path: impl AsRef<Path>) -> notify::Result<()> {
        match self {
            Self::NativeDebouncer(d) => d.unwatch(path),
            Self::NetworkFSDebouncer(d) => d.unwatch(path),
        }
    }
}

pub struct WatcherState {
    pub config_path: Option<PathBuf>,
    pub left_obj_path: Option<PathBuf>,
    pub right_obj_path: Option<PathBuf>,
    pub patterns: GlobSet,
}

pub fn is_win_networkfs_path(path: &Path) -> bool {
    let mut components = path.components();
    let first_component = components.next().expect("Path should not be empty");
    match first_component {
        Component::Prefix(prefix) => match prefix.kind() {
            Prefix::VerbatimUNC(_, _) => true,
            _ => false,
        },
        _ => panic!("Path should have a prefix (use canonicalize)"),
    }
}

pub fn create_watcher(
    modified: Arc<AtomicBool>,
    project_dir: &Path,
    patterns: GlobSet,
    ignore_patterns: GlobSet,
    waker: Waker,
) -> notify::Result<AnyDebouncer> {
    let base_dir = fs::canonicalize(project_dir)?;
    let base_dir_clone = base_dir.clone();
    let timeout = Duration::from_millis(200);
    let config = notify::Config::default().with_poll_interval(Duration::from_secs(2));
    let handler = move |result: DebounceEventResult| match result {
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
                    if patterns.is_match(path) && !ignore_patterns.is_match(path) {
                        log::debug!("File modified: {}", path.display());
                        any_match = true;
                    }
                }
            }
            if any_match {
                modified.store(true, Ordering::Relaxed);
                waker.wake_by_ref();
            }
        }
        Err(errors) => errors.iter().for_each(|e| {
            let path_is_ignored = e.paths.iter().all(|absolute_path| {
                let Ok(path) = absolute_path.strip_prefix(&base_dir_clone) else {
                    return false;
                };
                ignore_patterns.is_match(path)
            });
            // Don't directly report watch errors for path that are ignored
            if path_is_ignored {
                log::debug!("Watch error (ignored path) : {e:?}");
                return;
            }
            log::error!("Watch error: {e:?}");
        }),
    };

    let mut debouncer;
    // On Windows (only), check if base_dir is inside a network fs, if so switch to polling watcher
    #[cfg(windows)]
    {
        if is_win_networkfs_path(base_dir.as_path()) {
            // Use more frequent polling to compensate the slow polls
            let poll_config = config.with_poll_interval(Duration::from_secs(1));
            log::debug!("Using NetworkFS debouncer");
            debouncer = AnyDebouncer::NetworkFSDebouncer(new_debouncer_opt(
                timeout,
                None,
                handler,
                notify_debouncer_full::NoCache::new(),
                poll_config,
            )?);
        } else {
            log::debug!("Using Native debouncer");
            debouncer = AnyDebouncer::NativeDebouncer(new_debouncer_opt(
                timeout,
                None,
                handler,
                notify_debouncer_full::RecommendedCache::new(),
                config,
            )?);
        }
    }
    #[cfg(not(windows))]
    {
        debouncer = AnyDebouncer::NativeDebouncer(new_debouncer_opt(
            timeout,
            None,
            handler,
            notify_debouncer_full::RecommendedCache::new(),
            config,
        )?);
    }

    debouncer.watch(base_dir, RecursiveMode::Recursive)?;
    Ok(debouncer)
}

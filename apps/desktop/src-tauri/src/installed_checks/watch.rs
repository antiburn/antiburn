use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub(super) struct DirectoryWatcher {
    watcher: RecommendedWatcher,
    directories: Arc<RwLock<Vec<PathBuf>>>,
    rebind: Arc<AtomicBool>,
    watched: BTreeSet<PathBuf>,
}

impl DirectoryWatcher {
    pub(super) fn new(on_change: impl Fn() + Send + 'static) -> notify::Result<Self> {
        let directories = Arc::new(RwLock::new(Vec::new()));
        let rebind = Arc::new(AtomicBool::new(false));
        let targets = Arc::clone(&directories);
        let changed = Arc::clone(&rebind);
        let watcher = RecommendedWatcher::new(
            move |result: notify::Result<Event>| {
                match result {
                    Ok(event) => {
                        let targets = targets.read().expect("installed watch targets");
                        if !is_relevant(&event, &targets) {
                            return;
                        }
                        if event.need_rescan()
                            || event.paths.is_empty()
                            || event
                                .paths
                                .iter()
                                .any(|path| targets.iter().any(|target| target.starts_with(path)))
                        {
                            changed.store(true, Ordering::Release);
                        }
                    }
                    Err(_) => {
                        ::tracing::error!(
                            event = "installed_check_watch_failed",
                            reason = "watch_event_failed"
                        );
                        changed.store(true, Ordering::Release);
                    }
                }
                on_change();
            },
            notify::Config::default(),
        )?;
        Ok(Self {
            watcher,
            directories,
            rebind,
            watched: BTreeSet::new(),
        })
    }

    pub(super) fn refresh(&mut self, directories: &[PathBuf]) -> bool {
        let desired = watch_paths(directories);
        let replaced = self.rebind.swap(false, Ordering::AcqRel);
        *self.directories.write().expect("installed watch targets") = directories.to_vec();
        if desired == self.watched && !replaced {
            return false;
        }
        let previous = self.watched.clone();
        for path in &self.watched {
            let _ = self.watcher.unwatch(path);
        }
        self.watched.clear();
        for path in desired {
            match self.watcher.watch(&path, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.watched.insert(path);
                }
                Err(_) => {
                    ::tracing::error!(
                        event = "installed_check_watch_failed",
                        reason = "watch_bind_failed"
                    )
                }
            }
        }
        replaced || previous != self.watched
    }
}

pub(super) fn watch_paths(directories: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for directory in directories {
        for path in directory.ancestors().take(3) {
            if path.is_dir() {
                paths.insert(path.to_path_buf());
            }
        }
        if let Some(base) = directory.ancestors().nth(2)
            && !base.is_dir()
            && let Some(parent) = base.ancestors().skip(1).find(|parent| parent.is_dir())
        {
            paths.insert(parent.to_path_buf());
        }
    }
    paths
}

pub(super) fn is_relevant(event: &Event, directories: &[PathBuf]) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    event.need_rescan()
        || event.paths.is_empty()
        || event.paths.iter().any(|path| {
            directories.iter().any(|directory| {
                directory.starts_with(path)
                    || (path.parent() == Some(directory.as_path()) && is_yaml(path))
            })
        })
}

fn is_yaml(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("yml" | "yaml")
    )
}

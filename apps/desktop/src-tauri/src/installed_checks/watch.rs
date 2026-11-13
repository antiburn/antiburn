use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

pub(super) struct DirectoryWatcher {
    watcher: Option<RecommendedWatcher>,
    on_change: Arc<dyn Fn() + Send + Sync>,
    directories: Arc<RwLock<Vec<PathBuf>>>,
    rebind: Arc<AtomicBool>,
    watched: BTreeSet<PathBuf>,
}

impl DirectoryWatcher {
    pub(super) fn new(on_change: impl Fn() + Send + Sync + 'static) -> notify::Result<Self> {
        let directories = Arc::new(RwLock::new(Vec::new()));
        let rebind = Arc::new(AtomicBool::new(false));
        let on_change: Arc<dyn Fn() + Send + Sync> = Arc::new(on_change);
        let watcher = Self::backend(&directories, &rebind, &on_change)?;
        Ok(Self {
            watcher: Some(watcher),
            on_change,
            directories,
            rebind,
            watched: BTreeSet::new(),
        })
    }

    fn backend(
        directories: &Arc<RwLock<Vec<PathBuf>>>,
        rebind: &Arc<AtomicBool>,
        on_change: &Arc<dyn Fn() + Send + Sync>,
    ) -> notify::Result<RecommendedWatcher> {
        let targets = Arc::clone(directories);
        let changed = Arc::clone(rebind);
        let on_change = Arc::clone(on_change);
        RecommendedWatcher::new(
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
        )
    }

    pub(super) fn refresh(
        &mut self,
        directories: &[PathBuf],
        admitted: impl Fn(&Path) -> bool,
    ) -> bool {
        let desired = watch_paths(directories, &admitted, Path::is_dir);
        let replaced = self.rebind.swap(false, Ordering::AcqRel);
        *self.directories.write().expect("installed watch targets") = directories.to_vec();
        if desired == self.watched && !replaced {
            return false;
        }
        let previous = std::mem::take(&mut self.watched);
        if !previous.is_empty() {
            // The macOS backend resolves paths during unwatch. Drop it without a path lookup.
            self.watcher = None;
        }
        if desired.is_empty() {
            return replaced || !previous.is_empty();
        }
        if self.watcher.is_none() {
            match Self::backend(&self.directories, &self.rebind, &self.on_change) {
                Ok(watcher) => self.watcher = Some(watcher),
                Err(_) => {
                    ::tracing::error!(
                        event = "installed_check_watch_failed",
                        reason = "watch_rebind_failed"
                    );
                    return !previous.is_empty();
                }
            }
        }
        let watcher = self.watcher.as_mut().expect("installed watch backend");
        for path in desired {
            if !admitted(&path) {
                continue;
            }
            match watcher.watch(&path, RecursiveMode::NonRecursive) {
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

pub(super) fn watch_paths(
    directories: &[PathBuf],
    admitted: impl Fn(&Path) -> bool,
    is_directory: impl Fn(&Path) -> bool,
) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for directory in directories {
        for (distance, path) in directory.ancestors().enumerate() {
            if !admitted(path) {
                break;
            }
            if is_directory(path) {
                paths.insert(path.to_path_buf());
                if distance >= 2 {
                    break;
                }
            }
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

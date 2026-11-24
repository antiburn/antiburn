//! Match recorded working directories to saved repository rules.

use crate::{
    cloud::repository_checkout::{CheckoutOrigin, CheckoutOriginCache, lookup_checkout_origin},
    store::cloud_capture::sync::{Job, Policy},
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const MAX_CACHED_PATHS: usize = 512;
// Let one lookup finish after this limit so every page can make progress.
// The checkout lookup limits that extra time to one second.
const PAGE_LOOKUP_BUDGET: Duration = Duration::from_millis(250);

/// The page must resume before it checks this path.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct LookupDeferred;

/// Reuse lookups for identical recorded paths within one discovery page.
/// Never reuse these results for a new capture or reservation retry.
#[derive(Default)]
pub(super) struct RepositoryLookups {
    origins: BTreeMap<PathBuf, Option<String>>,
    checkouts: CheckoutOriginCache,
    lookup_time: Duration,
}

impl RepositoryLookups {
    /// Read the nearest verified origin. A cached result needs no lookup time.
    /// Call only on the bounded discovery worker. A deferral makes no claim
    /// about this path. Keep it for the next page.
    pub(super) fn origin(&mut self, path: &Path) -> Result<Option<String>, LookupDeferred> {
        if let Some(origin) = self.origins.get(path) {
            return Ok(origin.clone());
        }
        if self.lookup_time >= PAGE_LOOKUP_BUDGET {
            return Err(LookupDeferred);
        }
        let started = Instant::now();
        let result = tauri::async_runtime::block_on(self.checkouts.lookup(path));
        self.lookup_time += started.elapsed();
        let origin = origin_for_path(result, path);
        if self.origins.len() < MAX_CACHED_PATHS {
            self.origins.insert(path.to_owned(), origin.clone());
        }
        Ok(origin)
    }
}

/// Check a repository capture against its exact recorded path and fresh Git
/// evidence. Pass the complete paths from `ProjectSelection::admit_for_routing`
/// and call only on the bounded capture worker before enqueue or recapture.
pub(super) fn recheck_repository_project(
    policy: &Policy,
    scope: &str,
    origin: &str,
    project: &str,
    recorded_paths: &[PathBuf],
) -> bool {
    let project = Path::new(project);
    policy.allows_repository(scope, origin)
        && recorded_paths
            .iter()
            .any(|path| path.as_os_str() == project.as_os_str())
        && verified_origin(project).as_deref() == Some(origin)
}

/// Check a saved capture's repository origin before transfer or new capture.
/// Call on the bounded capture worker. Exact folder jobs need no Git check.
pub(super) fn saved_job_origin_current(job: &Job) -> bool {
    job.repository_origin.as_deref().is_none_or(|origin| {
        verified_origin(Path::new(&job.project_path)).as_deref() == Some(origin)
    })
}

/// Use the desktop Tokio runtime from the bounded blocking worker. The async
/// lookup limits its own filesystem and Git work.
fn verified_origin(path: &Path) -> Option<String> {
    origin_for_path(
        tauri::async_runtime::block_on(lookup_checkout_origin(path)),
        path,
    )
}

fn origin_for_path(result: CheckoutOrigin, path: &Path) -> Option<String> {
    match result {
        CheckoutOrigin::Verified {
            checkout,
            origin: Some(origin),
        } if path.starts_with(&checkout) => Some(origin.as_label().to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_page_reuses_cache_but_defers_new_paths() {
        let mut lookups = RepositoryLookups {
            origins: BTreeMap::from([
                (
                    "/matched".into(),
                    Some("github.com/example/repository".into()),
                ),
                ("/unmatched".into(), None),
            ]),
            lookup_time: PAGE_LOOKUP_BUDGET,
            ..RepositoryLookups::default()
        };

        assert_eq!(
            lookups.origin(Path::new("/matched")),
            Ok(Some("github.com/example/repository".into()))
        );
        assert_eq!(lookups.origin(Path::new("/unmatched")), Ok(None));
        assert_eq!(lookups.origin(Path::new("/unread")), Err(LookupDeferred));
        assert_eq!(lookups.origins.len(), 2);
    }
}

//! Keep bounded Git progress for one listing page. Never use it for consent.

use super::{
    CACHE_BYTES, CheckoutOriginCache, Context, Cursor, Folder, ListingAuthority, ProjectOrigin,
    REQUEST_CONTEXT_ENTRIES, SourceSelection, classify_unverified_path, context_before,
    inspect_origin,
};
use crate::store::cloud_capture::sync::valid_project_path;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, time::Duration};
use tokio::{sync::Semaphore, time::Instant};

const PROGRESS_TTL: Duration = Duration::from_secs(20);
const PATH_LOOKUP_LIMIT: Duration = Duration::from_secs(1);
const CONTEXT_OVERHEAD_BYTES: usize = 128;
static FILESYSTEM_SLOTS: Semaphore = Semaphore::const_new(1);

pub(super) struct ListingProgress {
    key: [u8; 32],
    expires_at: Instant,
    contexts: BTreeMap<String, Context>,
    bytes: usize,
}

impl ListingProgress {
    /// Fence progress by authority, exact page, saved choices, and resolved roots.
    /// Count changes do not change source choices.
    pub(super) fn key(
        authority: &ListingAuthority,
        sync_generation: u64,
        after: Option<&Cursor>,
        folders: &[Folder],
        roots: &[SourceSelection],
    ) -> [u8; 32] {
        let choices: Vec<_> = folders
            .iter()
            .map(|folder| (folder.source_kind, folder.path.as_str(), folder.selected))
            .collect();
        let bytes = serde_json::to_vec(&(
            &authority.binding.account_id,
            &authority.binding.destination_id,
            authority.generation,
            sync_generation,
            after,
            choices,
            roots,
        ))
        .expect("listing scope serialization");
        Sha256::digest(bytes).into()
    }
}

pub(super) struct ListingLookups {
    progress: ListingProgress,
    checkouts: CheckoutOriginCache,
    start_before: Instant,
    advanced: bool,
    deferred: bool,
}

impl ListingLookups {
    /// Take progress only for the same page, without extending its expiration.
    pub(super) fn new(
        progress: Option<ListingProgress>,
        key: [u8; 32],
        start_before: Instant,
    ) -> Self {
        let now = Instant::now();
        Self {
            progress: progress
                .filter(|entry| entry.key == key && now < entry.expires_at)
                .unwrap_or_else(|| ListingProgress {
                    key,
                    expires_at: now + PROGRESS_TTL,
                    contexts: BTreeMap::new(),
                    bytes: 0,
                }),
            checkouts: CheckoutOriginCache::default(),
            start_before,
            advanced: false,
            deferred: false,
        }
    }

    /// Inspect native paths only. A deferred path has no cached result.
    pub(super) fn context(&mut self, environment: &str, path: &str) -> Context {
        if environment != "native" {
            return Context::unavailable(path, false);
        }
        self.inspect(path, inspect_context)
    }

    fn inspect(
        &mut self,
        path: &str,
        check: impl FnOnce(&mut CheckoutOriginCache, &str, Instant) -> Context,
    ) -> Context {
        if !valid_project_path(path) {
            return Context::unavailable(path, true);
        }
        if let Some(context) = self.progress.contexts.get(path) {
            return context.clone();
        }
        // Leave room for a terminal result even when the complete context will not fit.
        let terminal_bytes = path.len() * 2 + CONTEXT_OVERHEAD_BYTES;
        let remaining = CACHE_BYTES - self.progress.bytes;
        if !self.can_store(terminal_bytes) {
            return Context::unavailable(path, true);
        }
        let now = Instant::now();
        if now >= self.start_before {
            self.deferred = true;
            return Context::unavailable(path, true);
        }
        // The page limits when work can start. A started check has its own limit.
        let context = check(&mut self.checkouts, path, now + PATH_LOOKUP_LIMIT);
        let bytes = path.len()
            + context.main.len()
            + context.origin.as_ref().map_or(0, String::len)
            + context.folders.iter().map(String::len).sum::<usize>()
            + context.folders.capacity() * std::mem::size_of::<String>()
            + CONTEXT_OVERHEAD_BYTES;
        let (context, bytes) = if bytes <= remaining {
            (context, bytes)
        } else {
            (Context::unavailable(path, true), terminal_bytes)
        };
        self.progress.bytes += bytes;
        self.progress.contexts.insert(path.into(), context.clone());
        self.advanced = true;
        context
    }

    /// Note that the listing deadline stopped provider work that can require Git.
    pub(super) fn defer_provider_work(&mut self) {
        self.deferred |= self.can_store(CONTEXT_OVERHEAD_BYTES + 2 * "/".len());
    }

    fn can_store(&self, bytes: usize) -> bool {
        self.progress.contexts.len() < REQUEST_CONTEXT_ENTRIES
            && CACHE_BYTES - self.progress.bytes >= bytes
    }

    /// Keep results for a later request, but ask for a retry only when this
    /// request added a result.
    pub(super) fn finish(self) -> (Option<ListingProgress>, bool) {
        let retain = self.deferred
            && !self.progress.contexts.is_empty()
            && Instant::now() < self.progress.expires_at;
        (retain.then_some(self.progress), retain && self.advanced)
    }
}

fn inspect_context(checkouts: &mut CheckoutOriginCache, path: &str, deadline: Instant) -> Context {
    match inspect_origin(checkouts, path, deadline, || {
        filesystem_before(path, deadline, |path| {
            classify_unverified_path(&path, Some(Path::new(&path).canonicalize()))
        })
        .unwrap_or(ProjectOrigin::Incomplete)
    }) {
        ProjectOrigin::Repository(checkout, origin) => Context {
            main: checkout.clone(),
            folders: vec![checkout],
            origin: Some(origin),
            complete: true,
            inspection_incomplete: false,
        },
        ProjectOrigin::Plain => Context::plain(path),
        ProjectOrigin::Unavailable => Context::unavailable(path, false),
        ProjectOrigin::Incomplete => Context::unavailable(path, true),
        ProjectOrigin::ExactGit => filesystem_before(path, deadline, move |path| {
            context_before("native", &path, Some(deadline))
        })
        .unwrap_or_else(|| Context::unavailable(path, true)),
    }
}

/// Exact worktree checks use blocking filesystem APIs. Keep the permit until
/// they finish even if the caller reaches its deadline first.
fn filesystem_before<T: Send + 'static>(
    path: &str,
    deadline: Instant,
    check: impl FnOnce(String) -> T + Send + 'static,
) -> Option<T> {
    let path = path.to_owned();
    tauri::async_runtime::block_on(async {
        tokio::time::timeout_at(deadline, async {
            let permit = FILESYSTEM_SLOTS.acquire().await.ok()?;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                (Instant::now() < deadline).then(|| check(path))
            })
            .await
            .ok()?
        })
        .await
        .ok()?
    })
}

#[cfg(test)]
#[path = "tests/project_listing_tests.rs"]
mod tests;

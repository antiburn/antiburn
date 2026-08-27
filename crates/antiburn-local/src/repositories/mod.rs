//! Finding the repositories a developer actually works in, on this machine.
//!
//! Discovery answers one question: *which repositories belonging to `owner` are
//! cloned here, where, and can we read them?* It answers it two ways and merges
//! the results:
//!
//! 1. **Forward** — walk the directories developers keep clones in
//!    ([`scan`]), matching each repository by its git remote. This finds
//!    repositories with no agent sessions at all, and clones whose folder name
//!    differs from the repository name.
//! 2. **From sessions** — resolve the working directories of recent agent
//!    sessions to repository roots ([`sessions`]). This finds clones outside the
//!    scanned directories and yields the per-repository session count.
//!
//! Optionally, the caller supplies a list of repositories it already knows about
//! ([`RepositoryDescriptor`]) and gets a gap analysis: which are cloned, which
//! are blocked by the operating system, and which are missing.
//!
//! # What the engine does not decide
//!
//! Three things are the embedding application's, and reach the engine only as
//! seams:
//!
//! - **Consent** ([`ConsentGrants`]) — which OS-protected directories the user
//!   already allowed, and where that record is kept. The engine never raises a
//!   consent dialog on its own initiative and never persists a grant.
//! - **Progress** ([`ProgressSink`]) — the engine reports phases as data; the
//!   application owns wording and transport.
//! - **Sessions** ([`SessionCwdSource`]) — where recent session working
//!   directories come from. [`ExplorerCwdSource`] is the built-in answer.
//!
//! Nothing here knows about hosting providers, accounts, uploads, or selection
//! policy beyond the opt-out flag the caller sets on each descriptor.

mod access;
mod consent;
mod identity;
mod matching;
mod model;
pub mod platform;
mod progress;
mod scan;
mod sessions;

#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::path::Path;

pub use access::verify_dir_access;
pub use consent::{ConsentGrants, NoConsentGrants, partition_cwds_by_grants};
pub use identity::{
    normalize_for_prefix, normalize_remote_url, parse_repo_name_from_url, repo_root_identity,
    repo_root_identity_for_platform,
};
pub use matching::{
    dedup_deferred_by_cwd, filter_deferred_by_known, located_repos_for_owner, match_known_repos,
    merge_located_repos,
};
pub use model::{
    DeferredProtectedPath, DiscoveryMode, LocalRepoStatus, LocatedRepo, RepoAccessStatus,
    RepoDiscoveryResult, RepositoryDescriptor,
};
pub use platform::PlatformDiscovery;
pub use progress::{
    DiscoveryPhase, DiscoveryProgress, FnProgress, NoProgress, ProgressRepo, ProgressSink,
};
pub use scan::{
    MAX_PARENT_SCAN_ROOTS, SCAN_DEPTH, parent_scan_roots, persist_confirmed_parent_roots,
    scan_roots_for_repos, sibling_scan_roots,
};
pub use sessions::{
    AgentProgress, ExplorerCwdSource, SessionCwdSource, concurrency_from, default_concurrency,
    resolve_granted_repos,
};

/// One discovery run's inputs.
pub struct DiscoveryRequest<'a> {
    /// Remote owner (organization or user login) a clone's git remote must
    /// name for it to be reported. Compared case-insensitively.
    pub owner: &'a str,
    /// Repositories the caller already knows about. `Some` produces a
    /// [`DiscoveryMode::Full`] gap analysis; `None` reports only what is on
    /// disk ([`DiscoveryMode::SessionsOnly`]).
    pub known_repos: Option<Vec<RepositoryDescriptor>>,
    /// How far back to look for agent sessions, in seconds.
    pub since_secs: i64,
    /// Re-probe consent-protected directories to pick up grants made outside
    /// the application.
    ///
    /// **This can raise the native consent dialog**, so it must only ever be
    /// `true` in response to an explicit user action. Background runs pass
    /// `false`.
    pub probe_protected: bool,
    /// Directory holding the user's declared extra scan roots (see
    /// [`crate::paths::scan_roots`]). Roots confirmed to contain a repository
    /// are persisted back here.
    pub state_dir: &'a Path,
    /// Maximum concurrent repository probes. `None` uses
    /// [`default_concurrency`].
    pub concurrency: Option<usize>,
}

/// Discover the `owner`'s repositories on the local machine.
///
/// Runs the session pass, then the directory scan, merges the two by canonical
/// repository root, and matches the result against
/// [`known_repos`](DiscoveryRequest::known_repos) when one was supplied.
///
/// Deferred (consent-protected) paths are filtered before being returned: with
/// a known-repository list, only paths whose folder name matches a known
/// repository survive, so unrelated local projects are never reported. Scan
/// roots bypass that filter — a directory like `~/Documents/GitHub` never
/// correlates by folder name, yet it is exactly what consent must be asked for.
pub async fn discover_repositories(
    request: DiscoveryRequest<'_>,
    sessions: &dyn SessionCwdSource,
    consent: &dyn ConsentGrants,
    progress: &dyn ProgressSink,
) -> RepoDiscoveryResult {
    let DiscoveryRequest {
        owner,
        known_repos,
        since_secs,
        probe_protected,
        state_dir,
        concurrency,
    } = request;
    let concurrency = concurrency.unwrap_or_else(default_concurrency);

    let t0 = std::time::Instant::now();
    ::tracing::info!(
        event = "repo_discovery_start",
        owner,
        window_days = since_secs / 86400,
        has_known_repos = known_repos.is_some(),
    );

    let session_result = sessions::discover_repos_from_sessions(
        owner,
        since_secs,
        probe_protected,
        concurrency,
        sessions,
        consent,
        progress,
    )
    .await;

    ::tracing::info!(
        event = "repo_discovery_sessions_done",
        repos_found = session_result.repos.len(),
        deferred = session_result.deferred.len(),
        elapsed_ms = t0.elapsed().as_millis() as u64,
    );

    let (scan_repos, scan_deferred) = match crate::paths::home_dir() {
        Some(home) => {
            // Extra scan roots beyond the common directories: parents of
            // session-located repositories (so a sibling with no sessions,
            // cloned alongside an active repository, is found), plus the
            // directories the user declared.
            let mut extra_roots = scan::sibling_scan_roots(&session_result.repos, &home);
            extra_roots.extend(crate::paths::scan_roots::load_scan_roots(state_dir));
            // A session whose working directory is a *parent folder above the
            // repositories* (not itself a repository) resolves to nothing and is
            // dropped. Scan those parents so the child repositories beneath them
            // are discovered.
            let parent_roots = scan::parent_scan_roots(&session_result.unresolved_cwds, &home);
            extra_roots.extend(parent_roots.iter().cloned());
            let (scan_repos, scan_deferred) =
                scan::scan_roots_for_repos(&home, owner, probe_protected, &extra_roots, consent)
                    .await;
            // Persist parents that actually contained a repository so they keep
            // being scanned after the originating sessions age out of the
            // lookback window. Only confirmed parents are stored, to avoid
            // polluting the declared-root list with one-off directories.
            scan::persist_confirmed_parent_roots(state_dir, &parent_roots, &scan_repos).await;
            (scan_repos, scan_deferred)
        }
        None => (Vec::new(), Vec::new()),
    };

    ::tracing::info!(
        event = "repo_discovery_scan_done",
        scanned_repos = scan_repos.len(),
        scan_deferred = scan_deferred.len(),
        elapsed_ms = t0.elapsed().as_millis() as u64,
    );

    // Merge session + scan locations, deduped by canonical repository root.
    // Sessions carry the session count; scan-only repositories contribute `0`
    // (found on disk, no agent activity yet).
    let located = merge_located_repos(session_result.repos, scan_repos);

    // Capture the known repository names before `known_repos` is moved, so
    // protected directories that cannot be read can still be correlated. Empty
    // without a list, which drops every deferred path.
    let known_repo_names: HashSet<String> = known_repos
        .as_ref()
        .map(|repos| {
            repos
                .iter()
                .map(|repo| repo.name.to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    // Whether an authoritative list exists at all. This is distinct from
    // `known_repo_names` being empty: a caller with zero repositories still has
    // a list, and uncorrelated protected folders must be dropped, not shown.
    let has_known_list = known_repos.is_some();

    let mut result = match known_repos {
        Some(known) => match_known_repos(known, &located, owner),
        None => located_repos_for_owner(owner, &located),
    };

    // Session working directories under protected directories correlate to
    // known repositories by folder name, dropping uncorrelated ones. Scan roots
    // never correlate by folder name yet are exactly what consent must be asked
    // for, so they bypass the filter and are always kept.
    let mut deferred =
        filter_deferred_by_known(session_result.deferred, &known_repo_names, has_known_list);
    deferred.extend(scan_deferred);
    dedup_deferred_by_cwd(&mut deferred);
    result.deferred_protected = deferred;

    ::tracing::info!(
        event = "repo_discovery_complete",
        mode = ?result.discovery_mode,
        total_repos = result.repos.len(),
        accessible = result.repos.iter().filter(|r| r.status == RepoAccessStatus::Accessible).count(),
        permission_denied = result.repos.iter().filter(|r| r.status == RepoAccessStatus::PermissionDenied).count(),
        not_cloned = result.repos.iter().filter(|r| r.status == RepoAccessStatus::NotCloned).count(),
        deferred = result.deferred_protected.len(),
        elapsed_ms = t0.elapsed().as_millis() as u64,
    );

    result
}

/// Whether a path is under an OS-level access-control mechanism (for example
/// macOS TCC). Returns `false` on platforms without such controls.
pub fn is_access_protected(path: &Path) -> bool {
    platform::platform().is_access_protected(path)
}

/// If `path` is under a consent-protected directory, its human-readable name
/// (for example `"Documents"`). `None` when the path is not protected.
pub fn protected_dir_name(path: &Path) -> Option<String> {
    crate::paths::protected::protected_dir_name(path)
}

/// A deep link to the OS settings page where the user can grant folder access.
/// `None` on platforms without one.
pub fn permission_settings_url() -> Option<&'static str> {
    platform::platform().permission_settings_url()
}

/// A deep link to the OS blanket disk-access settings page (macOS Full Disk
/// Access). `None` on platforms without one.
pub fn full_disk_access_settings_url() -> Option<&'static str> {
    platform::platform().full_disk_access_settings_url()
}

/// Re-probe a path to check whether access has been granted.
///
/// Returns `true` if the path is now readable. On macOS this reads the
/// directory, which **can raise the consent dialog** — call it only in response
/// to an explicit user action.
pub async fn probe_path_access(path: &Path) -> bool {
    platform::platform().probe_path_access(path).await
}

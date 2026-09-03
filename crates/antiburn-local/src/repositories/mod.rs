//! Building blocks for finding the repositories a developer actually works
//! in, on this machine.
//!
//! This module does not run a discovery pipeline itself; it gives the
//! embedding application the parts to assemble one:
//!
//! - **Identity** ([`repo_root_identity`], [`normalize_remote_url`],
//!   [`parse_repo_name_from_url`], …) — recognizing that a worktree, a
//!   differently-cased path, or a re-cloned copy is the same repository.
//! - **Matching** ([`merge_located_repos`], [`match_known_repos`],
//!   [`located_repos_for_owner`], [`RepoDiscoveryResult`],
//!   [`DiscoveryMode`]) — deduping locations found by different passes and,
//!   optionally, reconciling them against a caller-supplied
//!   [`RepositoryDescriptor`] list into a gap analysis: which are cloned,
//!   which are blocked by the operating system, and which are missing.
//! - **Scan-root walk** ([`scan_roots_for_repos`], [`sibling_scan_roots`],
//!   [`parent_scan_roots`]) — walking the directories developers keep clones
//!   in, owner-scoped, matching each repository by its git remote.
//! - **Session-cwd resolution** ([`sessions`]) — bounded-concurrency
//!   primitives that resolve a working directory to a canonical repository
//!   root, for callers that already have their own source of recent session
//!   working directories (the desktop app resolves these from its session
//!   store; see `apps/desktop/src-tauri/src/repositories.rs`).
//! - **Consent** ([`ConsentGrants`], [`partition_cwds_by_grants`],
//!   [`verify_dir_access`], [`is_access_protected`], [`protected_dir_name`],
//!   the settings-URL helpers) — which OS-protected directories the user
//!   already allowed, and where that record is kept. The engine never raises
//!   a consent dialog on its own initiative and never persists a grant.
//!
//! [`ProgressSink`] lets a caller assembling its own pipeline report phases
//! as data; the application owns wording and transport.
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
pub use sessions::{concurrency_from, resolve_granted_repos};

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

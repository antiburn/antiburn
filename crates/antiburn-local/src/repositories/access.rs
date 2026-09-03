//! Deciding whether a directory is really readable, and reacting when it is not.
//!
//! `stat()` is not enough: macOS TCC allows `metadata()` on a path inside a
//! protected folder while blocking `read_dir()`, so a metadata-only check
//! reports a repository as readable that the caller cannot actually enumerate.
//! Everything here therefore tests with `read_dir()`, records the outcome
//! through [`ConsentGrants`], and drops a grant that a denial has proven stale.

use std::io::ErrorKind;
use std::path::Path;

use super::consent::ConsentGrants;

/// Whether a path exists, without raising a consent dialog.
///
/// `stat()`/`metadata()` is permitted on consent-protected paths (only
/// `read_dir()` prompts or blocks), so `NotFound` reliably means "absent" while
/// a permission error means "present but access-controlled". Used to avoid
/// reporting a non-existent protected directory (for example
/// `~/Documents/GitHub` on a machine that does not use it) as needing consent.
pub(super) async fn path_exists_without_prompting(path: &Path) -> bool {
    match tokio::fs::metadata(path).await {
        Ok(_) => true,
        Err(e) => e.kind() == ErrorKind::PermissionDenied,
    }
}

/// Verify a repository root is truly accessible by testing `read_dir()`.
///
/// A denial is reported to `consent` and drops the grant covering the path: the
/// grant is stale (revoked or reset externally), and dropping it is what makes
/// the next run defer the directory instead of trusting the record. This
/// observes a denial that already happened; it never probes an ungranted path,
/// so it cannot raise a consent dialog.
pub async fn verify_dir_access(consent: &dyn ConsentGrants, path: &Path) -> bool {
    let t0 = std::time::Instant::now();
    match tokio::fs::read_dir(path).await {
        Ok(_) => true,
        Err(e) => {
            if e.kind() == ErrorKind::PermissionDenied {
                // Same vocabulary the application uses for a probe it asked
                // for, so a pasted diagnostics blob reads as one scheme rather
                // than betraying which layer happened to observe each line.
                consent.record_probe(
                    &path.to_string_lossy(),
                    "denied",
                    t0.elapsed().as_millis() as u64,
                );
                ::tracing::debug!(
                    event = "repo_discovery_access_denied",
                    path = path.to_string_lossy().as_ref(),
                    "read_dir blocked (likely an OS consent control)",
                );
                consent.revoke_grant_covering(path).await;
            }
            false
        }
    }
}

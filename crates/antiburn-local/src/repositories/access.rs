// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Deciding whether a directory is really readable, and reacting when it is not.
//!
//! `stat()` is not enough: macOS TCC allows `metadata()` on a path inside a
//! protected folder while blocking `read_dir()`, so a metadata-only check
//! reports a repository as readable that the caller cannot actually enumerate.
//! Everything here therefore tests with `read_dir()`, records the outcome
//! through [`ConsentGrants`], and drops a grant that a denial has proven stale.

use std::collections::HashSet;
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
                consent.record_probe(
                    &path.to_string_lossy(),
                    "PermissionDenied",
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

/// Re-check a working directory that discovery admitted on a recorded grant but
/// that then failed to resolve as a repository.
///
/// git failing under a directory believed to be granted is the shape a revoked
/// or reset grant takes: git's own reads are blocked, so resolution fails before
/// anything else notices. [`verify_dir_access`] records the probe and drops the
/// covering grant only on an actual `PermissionDenied` — a `NotFound` from a
/// deleted directory, or a readable non-repository, change nothing. git has
/// already run under this path, so the decision is recorded by now and the read
/// is silent; this raises no dialog on a background run.
///
/// Returns whether the directory was checked and found accessible.
pub(super) async fn verify_cwd_admitted_on_grant(
    consent: &dyn ConsentGrants,
    cwd: &str,
    admitted_cwds: &HashSet<String>,
) -> bool {
    if !admitted_cwds.contains(cwd) {
        return false;
    }
    verify_dir_access(consent, Path::new(cwd)).await
}

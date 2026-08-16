// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The consent seam: which OS-protected directories the user already allowed.
//!
//! On macOS, reading `~/Documents`, `~/Desktop`, or `~/Downloads` without a
//! recorded decision raises a native consent dialog. Discovery must never
//! trigger one at a random background moment, so it asks the embedding
//! application which directories the user already granted and skips the rest,
//! reporting them as [`DeferredProtectedPath`](super::DeferredProtectedPath).
//!
//! The engine does not persist grants, and it does not decide where that record
//! lives — the application does, behind [`ConsentGrants`]. The engine only:
//!
//! - reads the current grant set before it walks anything,
//! - tells the application when a read under a supposedly-granted directory
//!   came back denied, so a stale grant can be dropped, and
//! - hands it every probe outcome for diagnostics.
//!
//! [`NoConsentGrants`] is the "nothing granted, nothing recorded" default,
//! which is the whole story on platforms without such controls.

use std::collections::HashSet;
use std::path::Path;

use async_trait::async_trait;

use super::model::DeferredProtectedPath;
use crate::paths::protected;

/// The embedding application's record of consent-protected directories the user
/// has granted, plus the hooks discovery uses to keep that record honest.
#[async_trait]
pub trait ConsentGrants: Send + Sync {
    /// Directory names, relative to the home directory, the user has already
    /// granted (for example `{"Documents", "Desktop"}`).
    ///
    /// Read once per stage and trusted as-is. Discovery never probes a
    /// directory to answer this question, because a probe with no recorded
    /// decision is exactly what raises the consent dialog.
    fn granted_dirs(&self) -> HashSet<String>;

    /// Probe the named protected directories for grants made outside the
    /// application (for example through system settings) and return those that
    /// are now readable.
    ///
    /// **This can raise the native consent dialog**, so discovery calls it only
    /// when the caller sets
    /// [`DiscoveryRequest::probe_protected`](super::DiscoveryRequest::probe_protected),
    /// which must itself be driven by an explicit user action. The default
    /// implementation probes nothing.
    async fn discover_external_grants(&self, dir_names: &HashSet<String>) -> HashSet<String> {
        let _ = dir_names;
        HashSet::new()
    }

    /// Drop the recorded grant covering `path` and return its directory name.
    ///
    /// Called only *after* a read under a supposedly-granted directory came
    /// back `PermissionDenied` — the grant was revoked or reset externally.
    /// This reacts to a denial that already happened and never reads anything
    /// itself, so it cannot raise a consent dialog. The default implementation
    /// records nothing and drops nothing.
    async fn revoke_grant_covering(&self, path: &Path) -> Option<String> {
        let _ = path;
        None
    }

    /// Record the outcome of a single directory-access probe, for diagnostics.
    ///
    /// `elapsed_ms` doubles as a dialog-shown heuristic: a denial in a few
    /// milliseconds means the operating system answered from a recorded
    /// decision, while hundreds of milliseconds means a dialog was shown and
    /// the user answered it. The default implementation records nothing.
    fn record_probe(&self, target: &str, outcome: &str, elapsed_ms: u64) {
        let _ = (target, outcome, elapsed_ms);
    }
}

/// A [`ConsentGrants`] with nothing granted and nothing recorded.
///
/// The complete implementation for platforms without consent-controlled
/// directories, and the right default for callers that would rather defer a
/// protected directory than manage grants.
pub struct NoConsentGrants;

impl ConsentGrants for NoConsentGrants {
    fn granted_dirs(&self) -> HashSet<String> {
        HashSet::new()
    }
}

/// Split working directories into those safe to resolve now and those held back
/// pending the user's consent.
///
/// Anything under a protected directory absent from `granted` lands in the
/// second list and must not be read, `stat`-ed for repository markers, or handed
/// to `git` — that is what raises the dialog. The deferred entries carry the
/// directory name responsible so the caller can ask for exactly that grant.
///
/// Pure and home-explicit: it touches no filesystem and resolves no home
/// directory, so it behaves identically in tests on every platform. On platforms
/// without consent controls nothing is ever deferred.
#[must_use]
pub fn partition_cwds_by_grants<I, S>(
    home: &Path,
    cwds: I,
    granted: &HashSet<String>,
) -> (Vec<String>, Vec<DeferredProtectedPath>)
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut admitted = Vec::new();
    let mut deferred = Vec::new();

    for cwd in cwds {
        let cwd = cwd.as_ref();
        match protected::protected_dir_name_in(home, Path::new(cwd)) {
            Some(protected_dir) if !granted.contains(&protected_dir) => {
                deferred.push(DeferredProtectedPath {
                    cwd: cwd.to_string(),
                    protected_dir,
                });
            }
            _ => admitted.push(cwd.to_string()),
        }
    }

    super::matching::dedup_deferred_by_cwd(&mut deferred);
    (admitted, deferred)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[cfg(target_os = "macos")]
    fn protected_cwds_defer_until_their_directory_is_granted() {
        let home = PathBuf::from("/Users/avery");
        let documents = home.join("Documents/GitHub/repo").display().to_string();
        let desktop = home.join("Desktop/scratch").display().to_string();
        let plain = home.join("dev/repo").display().to_string();

        let (admitted, deferred) = partition_cwds_by_grants(
            &home,
            [documents.clone(), desktop.clone(), plain.clone()],
            &HashSet::new(),
        );
        assert_eq!(admitted, vec![plain.clone()]);
        assert_eq!(
            deferred
                .iter()
                .map(|entry| entry.protected_dir.as_str())
                .collect::<Vec<_>>(),
            vec!["Documents", "Desktop"]
        );

        // Granting one directory admits its paths and holds the others back.
        let (admitted, deferred) = partition_cwds_by_grants(
            &home,
            [documents.clone(), desktop.clone(), plain.clone()],
            &HashSet::from(["Documents".to_string()]),
        );
        assert_eq!(admitted, vec![documents, plain]);
        assert_eq!(deferred.len(), 1);
        assert_eq!(deferred[0].cwd, desktop);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn repeated_cwds_defer_once() {
        let home = PathBuf::from("/Users/avery");
        let documents = home.join("Documents/GitHub/repo").display().to_string();

        let (_, deferred) =
            partition_cwds_by_grants(&home, [&documents, &documents], &HashSet::new());

        assert_eq!(deferred.len(), 1);
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn platforms_without_consent_controls_defer_nothing() {
        let home = PathBuf::from("/home/avery");
        let documents = home.join("Documents/repo").display().to_string();

        let (admitted, deferred) =
            partition_cwds_by_grants(&home, [documents.clone()], &HashSet::new());

        assert_eq!(admitted, vec![documents]);
        assert!(deferred.is_empty());
    }
}

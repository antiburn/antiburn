// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Directories the operating system guards behind explicit user consent.
//!
//! On macOS, reading `~/Documents`, `~/Desktop`, or `~/Downloads` without a
//! recorded decision raises a native consent dialog. Anything that probes the
//! filesystem in the background — directory walks, `git` invocations with a
//! working directory inside one of those folders — must therefore ask here
//! first and skip paths the user has not already granted.
//!
//! These helpers are pure: they compare paths against the platform's protected
//! list and never touch the filesystem. Persisting *which* directories the user
//! granted is the embedding application's job.

use std::path::Path;

/// Directory names, relative to the home directory, that the current platform
/// guards behind an explicit consent prompt. Empty on platforms without such
/// controls.
#[cfg(target_os = "macos")]
const PROTECTED_DIR_NAMES: &[&str] = &["Documents", "Desktop", "Downloads"];

/// Directory names, relative to the home directory, that the current platform
/// guards behind an explicit consent prompt. Empty on platforms without such
/// controls.
#[cfg(not(target_os = "macos"))]
const PROTECTED_DIR_NAMES: &[&str] = &[];

/// Directory names (relative to the home directory) that require explicit user
/// consent on this platform. Empty on platforms without such controls.
pub fn protected_dir_names() -> &'static [&'static str] {
    PROTECTED_DIR_NAMES
}

/// Whether `path` sits under a consent-protected directory.
///
/// Returns `false` on platforms without such controls, and `false` when the
/// home directory cannot be resolved.
pub fn is_access_protected(path: &Path) -> bool {
    if PROTECTED_DIR_NAMES.is_empty() {
        return false;
    }
    let Some(home) = super::home_dir() else {
        return false;
    };
    is_access_protected_in(&home, path)
}

/// [`is_access_protected`] against an explicit home directory.
pub fn is_access_protected_in(home: &Path, path: &Path) -> bool {
    PROTECTED_DIR_NAMES
        .iter()
        .any(|name| path.starts_with(home.join(name)))
}

/// If `path` sits under a consent-protected directory, return that directory's
/// human-readable name (for example `"Documents"`).
pub fn protected_dir_name(path: &Path) -> Option<String> {
    if PROTECTED_DIR_NAMES.is_empty() {
        return None;
    }
    let home = super::home_dir()?;
    protected_dir_name_in(&home, path)
}

/// [`protected_dir_name`] against an explicit home directory.
pub fn protected_dir_name_in(home: &Path, path: &Path) -> Option<String> {
    PROTECTED_DIR_NAMES
        .iter()
        .find(|name| path.starts_with(home.join(name)))
        .map(|name| (*name).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[cfg(target_os = "macos")]
    fn protected_dirs_cover_the_documented_macos_set() {
        let home = PathBuf::from("/Users/avery");

        assert!(is_access_protected_in(
            &home,
            &home.join("Documents").join("GitHub").join("repo")
        ));
        assert!(is_access_protected_in(
            &home,
            &home.join("Desktop").join("file")
        ));
        assert!(is_access_protected_in(
            &home,
            &home.join("Downloads").join("file")
        ));
        assert!(!is_access_protected_in(
            &home,
            &home.join("dev").join("repo")
        ));
        assert!(!is_access_protected_in(&home, Path::new("/tmp/repo")));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn protected_dir_name_reports_the_matching_directory() {
        let home = PathBuf::from("/Users/avery");

        assert_eq!(
            protected_dir_name_in(&home, &home.join("Documents").join("GitHub")),
            Some("Documents".to_string())
        );
        assert_eq!(
            protected_dir_name_in(&home, &home.join("dev").join("repo")),
            None
        );
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn platforms_without_consent_controls_never_report_protection() {
        let home = PathBuf::from("/home/avery");

        assert!(protected_dir_names().is_empty());
        assert!(!is_access_protected(&home.join("Documents")));
        assert!(!is_access_protected_in(&home, &home.join("Documents")));
        assert_eq!(protected_dir_name(&home.join("Documents")), None);
    }
}

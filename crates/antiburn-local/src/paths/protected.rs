//! Directories the operating system guards behind explicit user consent.
//!
//! Pure lexical decisions precede project filesystem access.
//! The path admission helpers additionally reject unverified symlink components.
//! Stored grants do not guarantee operating-system permission.

pub use super::project_access::{
    ProjectAccessDecision, git_project_access, project_access_decision, project_access_decision_in,
    project_path_access, project_path_access_in,
};
use std::collections::HashSet;
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
/// Missing home information defers access on platforms with consent controls.
pub fn is_access_protected(path: &Path) -> bool {
    !project_access_decision(path, &HashSet::new()).is_allowed()
}

/// [`is_access_protected`] against an explicit home directory.
pub fn is_access_protected_in(home: &Path, path: &Path) -> bool {
    !project_access_decision_in(Some(home), path, &HashSet::new()).is_allowed()
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
    match project_access_decision_in(Some(home), path, &HashSet::new()) {
        ProjectAccessDecision::DeferredProtected(name) => Some(name),
        _ => None,
    }
}

/// Return true when project access must wait for authorization or path verification.
#[must_use]
pub fn cwd_resolution_blocked(path: &Path, granted: &HashSet<String>) -> bool {
    !project_access_decision(path, granted).is_allowed()
}

/// [`cwd_resolution_blocked`] against an explicit home directory.
#[must_use]
pub fn cwd_resolution_blocked_in(home: &Path, path: &Path, granted: &HashSet<String>) -> bool {
    !project_access_decision_in(Some(home), path, granted).is_allowed()
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
    #[cfg(target_os = "macos")]
    fn resolution_is_blocked_only_inside_ungranted_protected_dirs() {
        let home = PathBuf::from("/Users/avery");
        let documents = home.join("Documents").join("GitHub").join("repo");
        let unprotected = home.join("dev").join("repo");

        let nothing_granted = HashSet::new();
        assert!(cwd_resolution_blocked_in(
            &home,
            &documents,
            &nothing_granted
        ));
        assert!(!cwd_resolution_blocked_in(
            &home,
            &unprotected,
            &nothing_granted
        ));

        let documents_granted = HashSet::from(["Documents".to_string()]);
        assert!(!cwd_resolution_blocked_in(
            &home,
            &documents,
            &documents_granted
        ));
        // A grant covers one directory, not every protected directory.
        assert!(cwd_resolution_blocked_in(
            &home,
            &home.join("Desktop").join("repo"),
            &documents_granted
        ));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn platforms_without_consent_controls_allow_local_resolution() {
        let home = synthetic_local_home();

        assert!(!cwd_resolution_blocked(
            &home.join("Documents"),
            &HashSet::new()
        ));
        assert!(!cwd_resolution_blocked_in(
            &home,
            &home.join("Documents"),
            &HashSet::new()
        ));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn platforms_without_consent_controls_report_no_protected_local_folder() {
        let home = synthetic_local_home();

        assert!(protected_dir_names().is_empty());
        assert!(!is_access_protected(&home.join("Documents")));
        assert!(!is_access_protected_in(&home, &home.join("Documents")));
        assert_eq!(protected_dir_name(&home.join("Documents")), None);
    }

    #[cfg(not(target_os = "macos"))]
    fn synthetic_local_home() -> PathBuf {
        let home = if cfg!(windows) {
            r"C:\Users\avery"
        } else {
            "/home/avery"
        };
        PathBuf::from(home)
    }
}

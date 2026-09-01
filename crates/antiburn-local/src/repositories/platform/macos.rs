use std::path::Path;

use async_trait::async_trait;

use crate::paths::protected;

use super::PlatformDiscovery;

const MACOS_CODE_DIRS: &[&str] = &[
    "dev",
    // Xcode clones into ~/Developer by default, and TCC does not guard it.
    "Developer",
    "Documents/GitHub",
    "src",
    "code",
    "Projects",
    "workspaces",
    "repos",
    "github",
    "work",
];

pub struct MacOsPlatform;

#[async_trait]
impl PlatformDiscovery for MacOsPlatform {
    fn common_code_dirs(&self) -> &'static [&'static str] {
        MACOS_CODE_DIRS
    }

    /// The protected-directory list and the path test are owned by
    /// [`crate::paths::protected`], which shares them with the engine's git
    /// probing so both sides of the consent guard cannot drift apart.
    fn is_access_protected(&self, path: &Path) -> bool {
        protected::is_access_protected(path)
    }

    fn protected_dir_names(&self) -> &'static [&'static str] {
        protected::protected_dir_names()
    }

    fn permission_settings_url(&self) -> Option<&'static str> {
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders")
    }

    fn full_disk_access_settings_url(&self) -> Option<&'static str> {
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
    }

    /// Override: use `read_dir()` instead of `metadata()` because macOS TCC
    /// allows `stat()`/`metadata()` on protected paths even without permission.
    /// Only `read_dir()` correctly triggers the consent dialog and reflects
    /// whether the user clicked "Allow" or "Don't Allow".
    async fn probe_path_access(&self, path: &Path) -> bool {
        tokio::fs::read_dir(path).await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::home_dir;
    use serial_test::serial;
    use std::path::PathBuf;

    #[test]
    fn common_dirs_not_empty() {
        assert!(!MacOsPlatform.common_code_dirs().is_empty());
    }

    /// A repeated directory shows twice in the setup list and breaks the keys
    /// the list is drawn from.
    #[test]
    fn common_dirs_have_no_duplicates() {
        let dirs = MacOsPlatform.common_code_dirs();
        let unique: std::collections::BTreeSet<&str> = dirs.iter().copied().collect();
        assert_eq!(unique.len(), dirs.len(), "duplicate entry in {dirs:?}");
    }

    #[test]
    #[serial]
    fn developer_dir_is_searched_and_unprotected() {
        assert!(MacOsPlatform.common_code_dirs().contains(&"Developer"));
        if let Some(home) = home_dir() {
            let xcode_clone = home.join("Developer").join("repo");
            assert!(!MacOsPlatform.is_access_protected(&xcode_clone));
        }
    }

    #[test]
    fn permission_url_is_some() {
        assert!(MacOsPlatform.permission_settings_url().is_some());
    }

    #[test]
    #[serial]
    fn protected_documents() {
        if let Some(home) = home_dir() {
            let docs = home.join("Documents").join("GitHub").join("repo");
            assert!(MacOsPlatform.is_access_protected(&docs));

            let dev = home.join("dev").join("repo");
            assert!(!MacOsPlatform.is_access_protected(&dev));
        }
    }

    #[test]
    #[serial]
    fn protected_desktop_and_downloads() {
        if let Some(home) = home_dir() {
            assert!(MacOsPlatform.is_access_protected(&home.join("Desktop").join("file")));
            assert!(MacOsPlatform.is_access_protected(&home.join("Downloads").join("file")));
        }
    }

    #[test]
    fn unprotected_path_is_not_protected() {
        assert!(!MacOsPlatform.is_access_protected(&PathBuf::from("/tmp/repo")));
    }
}

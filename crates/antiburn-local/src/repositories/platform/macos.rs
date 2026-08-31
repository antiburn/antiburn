use std::path::Path;

use async_trait::async_trait;

use crate::paths::protected;

use super::PlatformDiscovery;

const MACOS_CODE_DIRS: &[&str] = &[
    "dev",
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

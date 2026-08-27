use std::path::Path;

use async_trait::async_trait;

use super::PlatformDiscovery;

const WINDOWS_CODE_DIRS: &[&str] = &[
    "dev",
    "source/repos",
    "Documents/GitHub",
    "src",
    "code",
    "Projects",
    "workspaces",
    "repos",
    "github",
    "work",
];

pub struct WindowsPlatform;

#[async_trait]
impl PlatformDiscovery for WindowsPlatform {
    fn common_code_dirs(&self) -> &'static [&'static str] {
        WINDOWS_CODE_DIRS
    }

    fn is_access_protected(&self, _path: &Path) -> bool {
        false
    }

    fn permission_settings_url(&self) -> Option<&'static str> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn common_dirs_not_empty() {
        assert!(!WindowsPlatform.common_code_dirs().is_empty());
    }

    #[test]
    fn includes_visual_studio_default() {
        assert!(WindowsPlatform.common_code_dirs().contains(&"source/repos"));
    }

    #[test]
    fn no_access_protection() {
        assert!(
            !WindowsPlatform.is_access_protected(&PathBuf::from("C:\\Users\\avery\\Documents"))
        );
    }

    #[test]
    fn no_permission_url() {
        assert!(WindowsPlatform.permission_settings_url().is_none());
    }
}

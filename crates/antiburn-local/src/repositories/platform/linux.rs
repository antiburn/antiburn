// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use std::path::Path;

use async_trait::async_trait;

use super::PlatformDiscovery;

const LINUX_CODE_DIRS: &[&str] = &[
    "dev",
    "src",
    "code",
    "Projects",
    "projects",
    "workspaces",
    "repos",
    "github",
    "work",
];

pub struct LinuxPlatform;

#[async_trait]
impl PlatformDiscovery for LinuxPlatform {
    fn common_code_dirs(&self) -> &'static [&'static str] {
        LINUX_CODE_DIRS
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
        assert!(!LinuxPlatform.common_code_dirs().is_empty());
    }

    #[test]
    fn no_access_protection() {
        assert!(!LinuxPlatform.is_access_protected(&PathBuf::from("/home/avery/Documents")));
    }

    #[test]
    fn no_permission_url() {
        assert!(LinuxPlatform.permission_settings_url().is_none());
    }
}

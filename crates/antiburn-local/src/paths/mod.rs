// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Portable local path policy.
//!
//! This module owns the path decisions the engine makes on every supported
//! desktop platform: resolving the user's home directory, normalizing paths
//! for comparison, remembering the extra roots a user asked to scan, and
//! remembering the folders a user opted out of.
//!
//! The engine never decides *where* local state lives. Every persisted
//! surface here takes an explicit state directory (or file path) so the
//! embedding application owns that policy.
//!
//! # Modules
//!
//! - [`state_files`] — atomic JSON/byte persistence plus the timestamp format
//!   shared by the state files below.
//! - [`scan_roots`] — user-declared extra directories to scan for repositories.
//! - [`ignored_paths`] — folders the user opted out of, per scope.
//! - [`protected`] — directories the operating system guards behind an
//!   explicit user-consent prompt.

use std::path::PathBuf;

pub mod ignored_paths;
pub mod protected;
pub mod scan_roots;
pub mod state_files;

/// Resolve the user's home directory across supported desktop platforms.
///
/// Local state is kept under the user profile on every OS, so Windows falls
/// back to the usual profile environment variables instead of switching to
/// `%APPDATA%`.
pub fn home_dir() -> Option<PathBuf> {
    non_empty_env_path("HOME")
        .or_else(|| non_empty_env_path("USERPROFILE"))
        .or_else(|| {
            let drive = non_empty_env("HOMEDRIVE")?;
            let path = non_empty_env("HOMEPATH")?;
            Some(PathBuf::from(format!("{drive}{path}")))
        })
}

/// Read an environment variable as a path, treating blank values as unset.
pub fn non_empty_env_path(key: &str) -> Option<PathBuf> {
    non_empty_env(key).map(PathBuf::from)
}

/// Read an environment variable as a trimmed string, treating blank values as
/// unset.
pub fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct EnvGuard {
        key: &'static str,
        original: Option<String>,
    }

    impl EnvGuard {
        fn new(key: &'static str) -> Self {
            Self {
                key,
                original: std::env::var(key).ok(),
            }
        }

        fn set(&self, value: &str) {
            // SAFETY: every test using this guard is `#[serial]`, so no other
            // thread reads or writes the environment concurrently.
            unsafe { std::env::set_var(self.key, value) };
        }

        fn remove(&self) {
            // SAFETY: same `#[serial]` guarantee as `set`.
            unsafe { std::env::remove_var(self.key) };
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: same `#[serial]` guarantee as `set`.
            match &self.original {
                Some(value) => unsafe { std::env::set_var(self.key, value) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    #[serial]
    fn home_dir_prefers_home() {
        let home = EnvGuard::new("HOME");
        let userprofile = EnvGuard::new("USERPROFILE");
        home.set("/tmp/avery-home");
        userprofile.set("C:\\Users\\avery");

        assert_eq!(home_dir(), Some(PathBuf::from("/tmp/avery-home")));
    }

    #[test]
    #[serial]
    fn home_dir_uses_userprofile_when_home_absent() {
        let home = EnvGuard::new("HOME");
        let userprofile = EnvGuard::new("USERPROFILE");
        let homedrive = EnvGuard::new("HOMEDRIVE");
        let homepath = EnvGuard::new("HOMEPATH");
        home.remove();
        homedrive.remove();
        homepath.remove();
        userprofile.set("C:\\Users\\avery");

        assert_eq!(home_dir(), Some(PathBuf::from("C:\\Users\\avery")));
    }

    #[test]
    #[serial]
    fn home_dir_uses_drive_and_path_when_profile_absent() {
        let home = EnvGuard::new("HOME");
        let userprofile = EnvGuard::new("USERPROFILE");
        let homedrive = EnvGuard::new("HOMEDRIVE");
        let homepath = EnvGuard::new("HOMEPATH");
        home.remove();
        userprofile.remove();
        homedrive.set("C:");
        homepath.set("\\Users\\avery");

        assert_eq!(home_dir(), Some(PathBuf::from("C:\\Users\\avery")));
    }

    #[test]
    #[serial]
    fn non_empty_env_treats_blank_values_as_unset() {
        let guard = EnvGuard::new("ANTIBURN_LOCAL_TEST_ENV");
        guard.set("   ");
        assert_eq!(non_empty_env("ANTIBURN_LOCAL_TEST_ENV"), None);
        guard.set("  value  ");
        assert_eq!(
            non_empty_env("ANTIBURN_LOCAL_TEST_ENV"),
            Some("value".to_string())
        );
    }
}

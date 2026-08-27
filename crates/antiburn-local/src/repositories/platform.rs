//! Platform-specific behavior for repository discovery.
//!
//! Each target OS provides a [`PlatformDiscovery`] implementation via the
//! [`platform()`] factory function, selected at compile time. The trait covers
//! the two things that genuinely differ per platform: where developers keep
//! their clones, and how the operating system guards access to them.

use std::path::Path;

use async_trait::async_trait;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// Platform-specific behavior needed by repository discovery.
#[async_trait]
pub trait PlatformDiscovery: Send + Sync {
    /// Relative directory names (from the home directory) where developers
    /// commonly clone repositories.
    fn common_code_dirs(&self) -> &'static [&'static str];

    /// Whether the given path is under an OS-level access-control mechanism
    /// that requires explicit user consent (for example macOS TCC).
    fn is_access_protected(&self, path: &Path) -> bool;

    /// URL that opens the OS permission settings panel for granting file
    /// access.
    ///
    /// Returns `None` on platforms without such a deep link.
    fn permission_settings_url(&self) -> Option<&'static str>;

    /// Directory names (relative to the home directory) that are protected by
    /// OS-level access control. Returns an empty slice on platforms without
    /// such controls.
    fn protected_dir_names(&self) -> &'static [&'static str] {
        &[]
    }

    /// Re-probe a path to check whether access has been granted.
    ///
    /// The default implementation checks filesystem metadata.
    async fn probe_path_access(&self, path: &Path) -> bool {
        tokio::fs::metadata(path).await.is_ok()
    }

    /// URL that opens the OS settings panel for blanket disk access (macOS
    /// Full Disk Access). The escape hatch when per-folder permission is stuck.
    ///
    /// Returns `None` on platforms without such a panel.
    fn full_disk_access_settings_url(&self) -> Option<&'static str> {
        None
    }
}

/// Return the [`PlatformDiscovery`] implementation for the current OS.
#[cfg(target_os = "macos")]
pub fn platform() -> macos::MacOsPlatform {
    macos::MacOsPlatform
}

/// Return the [`PlatformDiscovery`] implementation for the current OS.
#[cfg(target_os = "linux")]
pub fn platform() -> linux::LinuxPlatform {
    linux::LinuxPlatform
}

/// Return the [`PlatformDiscovery`] implementation for the current OS.
#[cfg(target_os = "windows")]
pub fn platform() -> windows::WindowsPlatform {
    windows::WindowsPlatform
}

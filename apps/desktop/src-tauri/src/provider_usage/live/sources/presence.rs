//! The metadata-only boundary every source's `detect()` is built on.
//!
//! Detection answers "is this tool's login carrier here at all?" without
//! reading it. The three primitives below are the whole vocabulary: a path's
//! metadata, a Keychain item's attributes (macOS, never the secret), and a
//! binary on `PATH`. Which paths, which service name, and which binary are
//! the only things that differ between vendors, so those stay in each
//! source; the primitives and the test fake live here so the vendors cannot
//! drift apart.
//!
//! Nothing here opens a file for its contents, and nothing here passes `-w`
//! to `security`. A source that needs a vendor-specific check that is still
//! metadata-only — Antigravity's read-only SQLite key probe — keeps it in
//! its own module and passes it in beside the probe.

use std::fs;
use std::io;
use std::path::Path;

use super::claude_touch;
#[cfg(target_os = "macos")]
pub(super) use super::claude_touch::KeychainMetadata;

/// The metadata primitives a detector may use. Injectable so a test can
/// assert exactly which calls a detector made — and that none of them
/// touched a secret.
pub(super) trait PresenceProbe {
    /// `Ok(true)` when the path exists, an error when metadata could not be
    /// read. Callers use [`path_exists`] to fold `NotFound` into `Ok(false)`.
    fn path_exists(&self, path: &Path) -> Result<bool, io::Error>;

    /// Attributes of one generic-password item, without its secret.
    #[cfg(target_os = "macos")]
    fn keychain_metadata(&self, service: &str, account: Option<&str>) -> KeychainMetadata;

    /// Whether `binary` resolves on the reader's `PATH`.
    fn binary_present(&self, binary: &str) -> bool;
}

/// The production probe.
pub(super) struct SystemPresenceProbe {
    /// Whether to consult the Keychain at all. Tests rooted at a fixture
    /// path disable it so a detector under test never spawns `security`.
    #[cfg(target_os = "macos")]
    pub(super) try_keychain: bool,
}

impl PresenceProbe for SystemPresenceProbe {
    fn path_exists(&self, path: &Path) -> Result<bool, io::Error> {
        fs::metadata(path).map(|_| true)
    }

    #[cfg(target_os = "macos")]
    fn keychain_metadata(&self, service: &str, account: Option<&str>) -> KeychainMetadata {
        if self.try_keychain {
            claude_touch::keychain_metadata_for(service, account)
        } else {
            KeychainMetadata::Absent
        }
    }

    fn binary_present(&self, binary: &str) -> bool {
        claude_touch::binary_on_path(binary)
    }
}

/// `path_exists` with `NotFound` read as ordinary absence. Every other error
/// stays an error, so a permission problem is reported as "could not tell"
/// rather than "not here".
pub(super) fn path_exists(probe: &impl PresenceProbe, path: &Path) -> Result<bool, io::Error> {
    match probe.path_exists(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        result => result,
    }
}

/// A probe that answers from a table and records every call it received.
#[cfg(test)]
#[derive(Default)]
pub(super) struct RecordingPresence {
    /// Answers by path. A path not listed answers `Ok(false)`, unless
    /// `fallthrough` is set, in which case the real filesystem answers.
    pub(super) paths: std::collections::BTreeMap<std::path::PathBuf, Result<bool, io::ErrorKind>>,
    /// When set, unlisted paths are checked on the real filesystem, for
    /// tests that build fixtures in a temporary directory.
    pub(super) fallthrough: bool,
    #[cfg(target_os = "macos")]
    pub(super) keychain: Option<KeychainMetadata>,
    pub(super) binary: bool,
    pub(super) calls: std::cell::RefCell<Vec<String>>,
}

#[cfg(test)]
impl RecordingPresence {
    pub(super) fn record(&self, call: String) {
        self.calls.borrow_mut().push(call);
    }
}

#[cfg(test)]
impl PresenceProbe for RecordingPresence {
    fn path_exists(&self, path: &Path) -> Result<bool, io::Error> {
        self.record(format!("path_exists:{}", path.display()));
        match self.paths.get(path) {
            Some(result) => result.map_err(io::Error::from),
            None if self.fallthrough => fs::metadata(path).map(|_| true),
            None => Ok(false),
        }
    }

    #[cfg(target_os = "macos")]
    fn keychain_metadata(&self, _service: &str, _account: Option<&str>) -> KeychainMetadata {
        self.record("keychain_metadata".into());
        self.keychain.clone().unwrap_or(KeychainMetadata::Absent)
    }

    fn binary_present(&self, _binary: &str) -> bool {
        self.record("binary_present".into());
        self.binary
    }
}

//! Own the private spool used by ongoing Desktop capture.

use super::Folder;
use crate::store::cloud_capture::SourceSelection;
use antiburn_cloud_capture::{Limits, Spool};
use std::path::{Path, PathBuf};

/// Serialize access to the process-owned spool. Logout does not remove saved state.
pub struct Host {
    directory: PathBuf,
    spool: Option<Spool>,
    limits: Limits,
}

impl Host {
    /// Save the spool path without opening storage during UI setup.
    pub fn new(directory: &Path) -> Self {
        Self {
            directory: directory.join("cloud-capture-v1"),
            spool: None,
            limits: Limits::default(),
        }
    }

    /// Open one process-owned spool only inside admitted blocking work.
    pub(super) fn spool(&mut self) -> Result<&mut Spool, String> {
        if self.spool.is_none() {
            self.spool = Some(
                Spool::open(&self.directory, self.limits.clone())
                    .map_err(|_| "Desktop capture storage is unavailable. Saved bytes are kept.")?,
            );
        }
        self.spool
            .as_mut()
            .ok_or_else(|| "Desktop capture storage is unavailable.".into())
    }

    /// Apply verified service bounds without deleting, rewriting, or recompressing
    /// saved entries. Restore the old bounds if pending entries need them.
    pub(super) fn configure_limits(&mut self, limits: Limits) -> Result<(), String> {
        if self.limits.source_bytes == limits.source_bytes
            && self.limits.capture_bytes == limits.capture_bytes
        {
            return Ok(());
        }
        self.spool = None;
        match Spool::open(&self.directory, limits.clone()) {
            Ok(spool) => {
                self.spool = Some(spool);
                self.limits = limits;
                Ok(())
            }
            Err(_) => {
                self.spool = Some(Spool::open(&self.directory, self.limits.clone()).map_err(
                    |_| "Desktop capture storage is unavailable. Saved bytes are kept.",
                )?);
                Err("Saved captures need their prior limits. Drain or preserve them before new capture.".into())
            }
        }
    }
}

/// Sort consent-relevant roots without silently removing duplicate selections.
pub fn selected_roots(folders: &[Folder]) -> Vec<SourceSelection> {
    let mut roots = folders
        .iter()
        .filter(|folder| folder.selected)
        .map(|folder| SourceSelection {
            source_kind: folder.source_kind,
            path: folder.path.clone(),
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots
}

/// Admit bounded capture IDs, never paths or arbitrary renderer text.
pub fn valid_capture_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(all(test, unix))]
#[test]
fn spool_has_one_owner_until_the_host_closes() {
    let directory = tempfile::tempdir().unwrap();
    let directory = directory.path().canonicalize().unwrap();
    let mut first = Host::new(&directory);
    let mut second = Host::new(&directory);
    first.spool().unwrap();
    assert!(second.spool().is_err());
    drop(first);
    second.spool().unwrap();
}

//! Durable authentication markers. The controller mutex serializes operations;
//! the lifetime owner lock excludes other processes from the same vault namespace.
//! Synchronous file operations require admitted blocking work.

use super::auth::{Failure, random};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Durable auth lifecycle. Closed phases cannot restore credentials as active access.
#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    SignedOut,
    Pending,
    Active,
    ReauthRequired,
}

/// Keep remote uncertainty separate from local credential deletion.
///
/// `Revoke` retains proof until every stored proof has confirmed revocation.
/// Persist `DeleteCredential` before deleting proof so restart can finish safely.
/// `RemoteUnconfirmed` permits reconnect after explicit reset, but a later
/// connection's cleanup must not clear that earlier warning.
#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Cleanup {
    #[default]
    None,
    Revoke {
        prior_unconfirmed: bool,
    },
    DeleteCredential {
        prior_unconfirmed: bool,
    },
    RemoteUnconfirmed,
}

impl Cleanup {
    /// Block reconnect until retained proof is revoked and securely removed.
    pub fn blocks_connection(self) -> bool {
        matches!(self, Self::Revoke { .. } | Self::DeleteCredential { .. })
    }

    /// Preserve uncertainty about an older connection across later grant cleanup.
    pub fn prior_unconfirmed(self) -> bool {
        match self {
            Self::None => false,
            Self::Revoke { prior_unconfirmed } | Self::DeleteCredential { prior_unconfirmed } => {
                prior_unconfirmed
            }
            Self::RemoteUnconfirmed => true,
        }
    }

    /// Clear the current cleanup obligation without clearing an earlier remote warning.
    pub fn prior_warning_only(self) -> Self {
        if self.prior_unconfirmed() {
            Self::RemoteUnconfirmed
        } else {
            Self::None
        }
    }
}

/// Credential-free marker. A fresh epoch prevents old callbacks or vault records restoring access.
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub epoch: String,
    pub phase: Phase,
    pub cleanup: Cleanup,
}

/// Bind state files and lifetime process ownership to one desktop state directory.
/// The owner lock is not in-process serialization; the controller supplies that.
pub struct StateStore {
    directory: PathBuf,
    owner: Mutex<Option<Arc<File>>>,
}

impl StateStore {
    /// Save the directory in memory without opening files during UI setup.
    pub fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            owner: Mutex::new(None),
        }
    }

    /// Acquire lifetime process ownership once. Call only in serialized blocking
    /// work; returned guards do not exclude concurrent callers in this process.
    pub fn lock(&self) -> Result<Guard, Failure> {
        let mut owner = self.owner.lock().map_err(|_| Failure::Unavailable)?;
        if owner.is_none() {
            let mut options = private_options();
            options.read(true).write(true).create(true);
            let file = options
                .open(self.directory.join("cloud-auth-owner-v2.lock"))
                .map_err(|_| Failure::CredentialFailed)?;
            verify_file(&file)?;
            file.try_lock().map_err(|_| Failure::Unavailable)?;
            sync_directory(&self.directory)?;
            *owner = Some(Arc::new(file));
        }
        Ok(Guard {
            directory: self.directory.clone(),
            _owner: owner.as_ref().ok_or(Failure::Unavailable)?.clone(),
        })
    }
}

/// Keep process ownership alive while reading or atomically replacing state.
/// Callers still need controller serialization and the blocking-work permit.
pub struct Guard {
    directory: PathBuf,
    _owner: Arc<File>,
}

/// Apply private creation permissions and reject Unix symlink traversal.
fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}

/// Reject nonprivate, foreign-owned, linked, or nonregular state/lock files on Unix.
fn verify_file(file: &File) -> Result<(), Failure> {
    let metadata = file.metadata().map_err(|_| Failure::CredentialFailed)?;
    if !metadata.is_file() {
        return Err(Failure::CredentialFailed);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(Failure::CredentialFailed);
        }
    }
    Ok(())
}

/// Flush Unix directory entries after lock creation or atomic state replacement.
fn sync_directory(directory: &Path) -> Result<(), Failure> {
    #[cfg(unix)]
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| Failure::CredentialFailed)?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

impl Guard {
    /// Read bounded, credential-free state and reject invalid cleanup/phase combinations.
    pub fn read(&mut self) -> Result<Record, Failure> {
        let file = match private_options()
            .read(true)
            .open(self.directory.join("cloud-auth-state-v2"))
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Record::default());
            }
            Err(_) => return Err(Failure::CredentialFailed),
        };
        verify_file(&file)?;
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::CredentialFailed)?;
        if bytes.len() > 4096 {
            return Err(Failure::CredentialFailed);
        }
        let record: Record =
            serde_json::from_slice(&bytes).map_err(|_| Failure::CredentialFailed)?;
        if matches!(record.cleanup, Cleanup::DeleteCredential { .. })
            && record.phase != Phase::SignedOut
            || matches!(record.cleanup, Cleanup::Revoke { .. })
                && !matches!(record.phase, Phase::SignedOut | Phase::ReauthRequired)
        {
            return Err(Failure::CredentialFailed);
        }
        Ok(record)
    }

    /// Flush a private replacement before rename and directory sync. A failure
    /// must leave the engine closed; it cannot establish successful credential cleanup.
    pub fn write(&mut self, value: &Record) -> Result<(), Failure> {
        let bytes = serde_json::to_vec(value).map_err(|_| Failure::CredentialFailed)?;
        let temporary = self
            .directory
            .join(format!(".cloud-auth-state-{}.tmp", random()?));
        let result = (|| {
            let mut file = private_options()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| Failure::CredentialFailed)?;
            file.write_all(&bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| Failure::CredentialFailed)?;
            fs::rename(&temporary, self.directory.join("cloud-auth-state-v2"))
                .map_err(|_| Failure::CredentialFailed)?;
            sync_directory(&self.directory)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    /// Persist a fresh epoch for a lifecycle boundary before returning it to the engine.
    pub fn replace(&mut self, phase: Phase, cleanup: Cleanup) -> Result<Record, Failure> {
        let value = Record {
            epoch: random()?,
            phase,
            cleanup,
        };
        self.write(&value)?;
        Ok(value)
    }
}

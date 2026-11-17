use super::auth::{Failure, random};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    SignedOut,
    Pending,
    Active,
    ReauthRequired,
}

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
    pub fn blocks_connection(self) -> bool {
        matches!(self, Self::Revoke { .. } | Self::DeleteCredential { .. })
    }

    pub fn prior_unconfirmed(self) -> bool {
        match self {
            Self::None => false,
            Self::Revoke { prior_unconfirmed } | Self::DeleteCredential { prior_unconfirmed } => {
                prior_unconfirmed
            }
            Self::RemoteUnconfirmed => true,
        }
    }

    pub fn prior_warning_only(self) -> Self {
        if self.prior_unconfirmed() {
            Self::RemoteUnconfirmed
        } else {
            Self::None
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub epoch: String,
    pub phase: Phase,
    pub cleanup: Cleanup,
}

pub struct StateStore {
    directory: PathBuf,
    owner: Mutex<Option<Arc<File>>>,
}

impl StateStore {
    pub fn new(directory: &Path) -> Self {
        Self {
            directory: directory.to_owned(),
            owner: Mutex::new(None),
        }
    }

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

pub struct Guard {
    directory: PathBuf,
    _owner: Arc<File>,
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options
}

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

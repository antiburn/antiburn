use std::fs;
#[cfg(not(windows))]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::ConfigUnavailableReason;

const MAX_CONFIG_BYTES: u64 = 256 * 1024;

pub(super) struct CheckedFile {
    pub(super) bytes: Vec<u8>,
    #[cfg(not(windows))]
    pub(super) identity: FileIdentity,
    #[cfg(not(windows))]
    pub(super) permissions: fs::Permissions,
    #[cfg(unix)]
    pub(super) ownership: FileOwnership,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileOwnership {
    user: u32,
    group: u32,
}

pub(super) fn canonical_root(path: &Path) -> Result<PathBuf, ConfigUnavailableReason> {
    path.canonicalize()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)
}

pub(super) fn path_entry_exists(path: &Path) -> Result<bool, ConfigUnavailableReason> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            Err(ConfigUnavailableReason::PermissionDenied)
        }
        Err(_) => Err(ConfigUnavailableReason::UnsafePath),
    }
}

pub(super) fn read_checked(
    path: &Path,
    trusted_root: &Path,
) -> Result<CheckedFile, ConfigUnavailableReason> {
    reject_symlink_ancestors(path, trusted_root)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => ConfigUnavailableReason::MissingConfig,
        std::io::ErrorKind::PermissionDenied => ConfigUnavailableReason::PermissionDenied,
        _ => ConfigUnavailableReason::UnsafePath,
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ConfigUnavailableReason::SymlinkTarget);
    }
    if !metadata.is_file() {
        return Err(ConfigUnavailableReason::NonRegularFile);
    }
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err(ConfigUnavailableReason::FileTooLarge);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if !canonical.starts_with(trusted_root) {
        return Err(ConfigUnavailableReason::UnsafePath);
    }
    check_owner(&metadata, path, trusted_root)?;
    #[cfg(unix)]
    let bytes = read_final_target(path, &metadata)?;
    #[cfg(not(unix))]
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            ConfigUnavailableReason::PermissionDenied
        } else {
            ConfigUnavailableReason::UnsafePath
        }
    })?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(ConfigUnavailableReason::FileTooLarge);
    }
    let after = fs::symlink_metadata(path).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if after.file_type().is_symlink() || file_identity(&after) != file_identity(&metadata) {
        return Err(ConfigUnavailableReason::ChangedIdentity);
    }
    Ok(CheckedFile {
        bytes,
        #[cfg(not(windows))]
        identity: file_identity(&metadata),
        #[cfg(not(windows))]
        permissions: metadata.permissions(),
        #[cfg(unix)]
        ownership: file_ownership(&metadata),
    })
}

#[cfg(unix)]
fn read_final_target(
    path: &Path,
    initial: &fs::Metadata,
) -> Result<Vec<u8>, ConfigUnavailableReason> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(unix_libc::O_NOFOLLOW)
        .open(path)
        .map_err(map_read_error)?;
    let metadata = file.metadata().map_err(map_read_error)?;
    if !metadata.is_file()
        || file_identity(&metadata) != file_identity(initial)
        || metadata.permissions() != initial.permissions()
        || file_ownership(&metadata) != file_ownership(initial)
    {
        return Err(ConfigUnavailableReason::ChangedIdentity);
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(map_read_error)?;
    Ok(bytes)
}

#[cfg(unix)]
fn map_read_error(error: std::io::Error) -> ConfigUnavailableReason {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        ConfigUnavailableReason::PermissionDenied
    } else {
        ConfigUnavailableReason::UnsafePath
    }
}

fn reject_symlink_ancestors(
    path: &Path,
    trusted_root: &Path,
) -> Result<(), ConfigUnavailableReason> {
    let relative = path
        .strip_prefix(trusted_root)
        .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    let mut current = trusted_root.to_owned();
    let components = relative.components().collect::<Vec<_>>();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        current.push(component);
        let metadata = fs::symlink_metadata(&current).map_err(|error| match error.kind() {
            std::io::ErrorKind::PermissionDenied => ConfigUnavailableReason::PermissionDenied,
            _ => ConfigUnavailableReason::UnsafePath,
        })?;
        if metadata.file_type().is_symlink() {
            return Err(ConfigUnavailableReason::SymlinkTarget);
        }
        if !metadata.is_dir() {
            return Err(ConfigUnavailableReason::UnsafePath);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn check_owner(
    metadata: &fs::Metadata,
    path: &Path,
    root: &Path,
) -> Result<(), ConfigUnavailableReason> {
    use std::os::unix::fs::MetadataExt;
    let root = fs::metadata(root).map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    let parent = fs::metadata(path.parent().ok_or(ConfigUnavailableReason::UnsafePath)?)
        .map_err(|_| ConfigUnavailableReason::UnsafePath)?;
    if metadata.uid() != root.uid() || metadata.gid() != parent.gid() {
        Err(ConfigUnavailableReason::UnsupportedOwner)
    } else {
        Ok(())
    }
}

#[cfg(unix)]
pub(super) fn file_ownership(metadata: &fs::Metadata) -> FileOwnership {
    use std::os::unix::fs::MetadataExt;
    FileOwnership {
        user: metadata.uid(),
        group: metadata.gid(),
    }
}

#[cfg(not(unix))]
fn check_owner(_: &fs::Metadata, _: &Path, _: &Path) -> Result<(), ConfigUnavailableReason> {
    Ok(())
}

pub(super) fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        }
    }
    #[cfg(not(unix))]
    {
        FileIdentity {
            modified: metadata.modified().ok(),
        }
    }
}

#[cfg(not(windows))]
pub(super) fn create_temporary(
    path: &Path,
    permissions: &fs::Permissions,
) -> std::io::Result<fs::File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(permissions.mode())
        .open(path)?;
    file.set_permissions(permissions.clone())?;
    Ok(file)
}

#[cfg(not(windows))]
pub(super) fn map_write_error(error: std::io::Error) -> ConfigUnavailableReason {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        ConfigUnavailableReason::PermissionDenied
    } else {
        ConfigUnavailableReason::WriteFailed
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use super::{ConfigUnavailableReason, read_final_target};

    #[test]
    fn final_target_symlink_swap_is_rejected() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("config.json");
        let replacement = temporary.path().join("replacement.json");
        fs::write(&path, "original").unwrap();
        fs::write(&replacement, "replacement").unwrap();
        let initial = fs::symlink_metadata(&path).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&replacement, &path).unwrap();

        assert!(matches!(
            read_final_target(&path, &initial),
            Err(ConfigUnavailableReason::UnsafePath)
        ));
    }
}

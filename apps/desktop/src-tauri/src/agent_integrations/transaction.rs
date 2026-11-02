//! Private write-ahead receipts for narrow per-user configuration changes.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent_config::ConfigUnavailableReason;
use crate::agent_config::filesystem::{
    MAX_CONFIG_BYTES, check_regular_file, file_identity, read_bounded,
};

use super::document::{self, Delta};

pub type Result<T> = std::result::Result<T, &'static str>;
pub const MAX_BYTES: u64 = MAX_CONFIG_BYTES;

pub fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn checked_directory(path: &Path) -> Result<()> {
    if !path.is_absolute() {
        return Err("unsafe_path");
    }
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| "unsafe_path")?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("unsafe_path");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(path).map_err(|_| "unsafe_path")?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err("unsafe_owner_or_permissions");
        }
    }
    Ok(())
}

pub fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    checked_directory(path.parent().ok_or("unsafe_path")?)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("config_unreadable"),
    };
    check_regular_file(&metadata).map_err(|_| "unsafe_config")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || metadata.mode() & 0o022 != 0
        {
            return Err("unsafe_owner_or_permissions");
        }
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        if metadata.st_flags() != 0 {
            return Err("locked_or_managed_config");
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = options.open(path).map_err(|_| "config_unreadable")?;
    let opened = file.metadata().map_err(|_| "config_unreadable")?;
    if file_identity(&opened) != file_identity(&metadata) || !opened.is_file() {
        return Err("concurrent_config_change");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.uid() != unsafe { libc::geteuid() }
            || opened.nlink() != 1
            || opened.mode() & 0o022 != 0
        {
            return Err("concurrent_config_change");
        }
    }
    let bytes = read_bounded(&mut file).map_err(|reason| match reason {
        ConfigUnavailableReason::FileTooLarge => "config_too_large",
        _ => "config_unreadable",
    })?;
    Ok(Some(bytes))
}

fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path).map_err(|_| "installation_locked")
}

pub fn private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        checked_directory(path.parent().ok_or("unsafe_path")?)?;
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path).map_err(|_| "state_unavailable")?;
    }
    checked_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if fs::metadata(path).map_err(|_| "state_unavailable")?.mode() & 0o077 != 0 {
            return Err("state_not_private");
        }
    }
    Ok(())
}

pub struct Lock {
    _file: File,
}

impl Lock {
    pub fn acquire(state: &Path) -> Result<Self> {
        private_directory(state)?;
        let path = state.join("operation.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path).map_err(|_| "installation_locked")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = file.metadata().map_err(|_| "state_invalid")?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
            {
                return Err("state_not_private");
            }
        }
        file.try_lock().map_err(|_| "installation_locked")?;
        Ok(Self { _file: file })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Receipt {
    pub owner: String,
    pub target: PathBuf,
    pub original: Option<Vec<u8>>,
    pub installed: Vec<u8>,
    pub deltas: Vec<Delta>,
    pub artifact: String,
    pub removed: bool,
    pub published: bool,
    pub manager_source: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<Box<Receipt>>,
}

impl Receipt {
    pub fn prepare(
        target: PathBuf,
        original: Option<Vec<u8>>,
        installed: Vec<u8>,
        deltas: Vec<Delta>,
        artifact: String,
    ) -> Result<Self> {
        document::validate(&installed)?;
        if original
            .as_ref()
            .is_some_and(|b| b.len() as u64 > MAX_BYTES)
            || installed.len() as u64 > MAX_BYTES
        {
            return Err("config_too_large");
        }
        Ok(Self {
            owner: "antiburn-agent-integrations-v1".into(),
            target,
            original,
            installed,
            deltas,
            artifact,
            removed: false,
            published: false,
            manager_source: None,
            previous: None,
        })
    }

    pub fn rollback(&self, current: &[u8]) -> Result<Vec<u8>> {
        let mut output = current.to_vec();
        for delta in self.deltas.iter().rev() {
            output = document::remove(&output, delta)?;
        }
        Ok(output)
    }
}

pub fn save_receipt(path: &Path, receipt: &Receipt) -> Result<()> {
    if cfg!(windows) {
        return Err("platform_native_gate_pending");
    }
    let bytes = serde_json::to_vec(receipt).map_err(|_| "state_invalid")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("state_too_large");
    }
    let stage = path.with_extension(format!("{}.pending", nonce()?));
    let mut file = private_file(&stage)?;
    let result = (|| {
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "state_unavailable")?;
        fs::rename(&stage, path).map_err(|_| "state_unavailable")?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&stage);
    }
    result
}

pub fn load_receipt(path: &Path) -> Result<Option<Receipt>> {
    checked_directory(path.parent().ok_or("unsafe_path")?)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("state_unavailable"),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 2 * 1024 * 1024
    {
        return Err("state_invalid");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err("state_not_private");
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| "state_unavailable")?;
    let opened = file.metadata().map_err(|_| "state_invalid")?;
    if !opened.is_file() {
        return Err("state_invalid");
    }
    if file_identity(&opened) != file_identity(&metadata) {
        return Err("state_changed");
    }
    let mut bytes = Vec::new();
    file.take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "state_unavailable")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("state_too_large");
    }
    let receipt: Receipt = serde_json::from_slice(&bytes).map_err(|_| "state_invalid")?;
    if receipt.owner != "antiburn-agent-integrations-v1" {
        return Err("state_invalid");
    }
    Ok(Some(receipt))
}

fn sync_parent(path: &Path) -> Result<()> {
    File::open(path.parent().ok_or("unsafe_path")?)
        .and_then(|file| file.sync_all())
        .map_err(|_| "state_unavailable")
}

fn nonce() -> Result<String> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| "random_unavailable")?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Compare the original bytes immediately before atomic publication.
pub fn publish(path: &Path, expected: Option<&[u8]>, proposed: &[u8]) -> Result<()> {
    publish_inner(path, expected, proposed, || {}, |_| {})
}

fn publish_inner(
    path: &Path,
    expected: Option<&[u8]>,
    proposed: &[u8],
    before_commit: impl FnOnce(),
    mut after_exchange: impl FnMut(&Path),
) -> Result<()> {
    if proposed.len() as u64 > MAX_BYTES {
        return Err("config_too_large");
    }
    if read(path)?.as_deref() != expected {
        return Err("concurrent_config_change");
    }
    if cfg!(windows) {
        return Err("platform_native_gate_pending");
    }
    #[cfg(target_os = "macos")]
    let original_metadata = expected
        .map(|_| super::metadata::snapshot(path))
        .transpose()?;
    let name = format!(".antiburn-{}.tmp", nonce()?);
    let stage = path.parent().ok_or("unsafe_path")?.join(name);
    let mut preserve_stage = false;
    let result = (|| {
        if expected.is_some() {
            clone_original(path, &stage)?;
        } else {
            private_file(&stage)?;
        }
        #[cfg(target_os = "macos")]
        if let Some(original) = &original_metadata
            && !original.same_policy(&super::metadata::snapshot(&stage)?)
        {
            return Err("metadata_preserving_copy_unavailable");
        }
        let mut options = OpenOptions::new();
        options.write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(&stage).map_err(|_| "config_unwritable")?;
        let metadata = file.metadata().map_err(|_| "config_unwritable")?;
        if !metadata.is_file() {
            return Err("unsafe_config_file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1 {
                return Err("unsafe_config_file");
            }
        }
        file.set_len(0).map_err(|_| "config_unwritable")?;
        file.write_all(proposed)
            .and_then(|_| file.sync_all())
            .map_err(|_| "config_unwritable")?;
        #[cfg(target_os = "macos")]
        let proposed_metadata = super::metadata::snapshot(&stage)?;
        if read(path)?.as_deref() != expected {
            return Err("concurrent_config_change");
        }
        before_commit();
        if expected.is_none() {
            // A hard link publishes a new file without replacing a concurrent creation.
            fs::hard_link(&stage, path).map_err(|_| "concurrent_config_change")?;
            fs::remove_file(&stage).map_err(|_| "config_publish_failed")?;
        } else {
            swap_files(&stage, path)?;
            preserve_stage = true;
            after_exchange(&stage);
            // The exchange keeps the exact displaced file available for conflict detection.
            let displaced_matches = read(&stage).ok().flatten().as_deref() == expected;
            #[cfg(target_os = "macos")]
            let displaced_matches =
                displaced_matches && super::metadata::snapshot(&stage).ok() == original_metadata;
            if !displaced_matches {
                let can_restore = read(path)?.as_deref() == Some(proposed);
                #[cfg(target_os = "macos")]
                let can_restore = can_restore
                    && super::metadata::snapshot(path).ok().as_ref() == Some(&proposed_metadata);
                if can_restore {
                    swap_files(&stage, path)?;
                    after_exchange(&stage);
                    preserve_stage = read(&stage)?.as_deref() != Some(proposed);
                    #[cfg(target_os = "macos")]
                    {
                        preserve_stage |= super::metadata::snapshot(&stage).ok().as_ref()
                            != Some(&proposed_metadata);
                    }
                }
                return Err(if preserve_stage {
                    "concurrent_change_recovery_required"
                } else {
                    "concurrent_config_change"
                });
            }
            fs::remove_file(&stage).map_err(|_| "config_publish_failed")?;
            preserve_stage = false;
        }
        sync_parent(path)
    })();
    if result.is_err() && !preserve_stage {
        let _ = fs::remove_file(&stage);
    }
    result
}

#[cfg(target_os = "macos")]
fn swap_files(left: &Path, right: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let left = CString::new(left.as_os_str().as_bytes()).map_err(|_| "unsafe_path")?;
    let right = CString::new(right.as_os_str().as_bytes()).map_err(|_| "unsafe_path")?;
    // Both paths name sibling files on the same volume.
    if unsafe {
        libc::renameatx_np(
            libc::AT_FDCWD,
            left.as_ptr(),
            libc::AT_FDCWD,
            right.as_ptr(),
            libc::RENAME_SWAP,
        )
    } != 0
    {
        return Err("atomic_exchange_unavailable");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn swap_files(_: &Path, _: &Path) -> Result<()> {
    Err("platform_native_gate_pending")
}

#[cfg(target_os = "macos")]
fn clone_original(source: &Path, destination: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| "unsafe_path")?;
    let destination =
        CString::new(destination.as_os_str().as_bytes()).map_err(|_| "unsafe_path")?;
    // clonefile preserves the original ACL, permissions, and extended attributes.
    // CLONE_ACL and CLONE_NOFOLLOW come from the macOS sys/clonefile.h ABI.
    if unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), 0x0004 | 0x0001) } != 0 {
        return Err("metadata_preserving_copy_unavailable");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn clone_original(_: &Path, _: &Path) -> Result<()> {
    Err("platform_native_gate_pending")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_before_publish_refuses_concurrent_changes() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().canonicalize().unwrap().join("hooks.json");
        fs::write(&target, b"{}").unwrap();
        assert_eq!(
            publish(&target, Some(b"changed"), b"{\"hooks\":{}}"),
            Err("concurrent_config_change")
        );
        assert_eq!(fs::read(&target).unwrap(), b"{}");
    }

    #[test]
    fn locks_are_exclusive_and_released_on_drop() {
        let temp = tempfile::tempdir().unwrap();
        let state = temp.path().canonicalize().unwrap().join("private");
        let lock = Lock::acquire(&state).unwrap();
        assert!(Lock::acquire(&state).is_err());
        drop(lock);
        assert!(Lock::acquire(&state).is_ok());
    }

    fn receipt(target: PathBuf) -> Receipt {
        let (installed, delta) = document::append(
            b"{}",
            &["hooks".into(), "test".into()],
            serde_json::json!({"command":"owned"}),
        )
        .expect("Append the owned hook");
        Receipt::prepare(
            target,
            Some(b"{}".to_vec()),
            installed,
            vec![delta],
            "artifact".into(),
        )
        .expect("Prepare the receipt")
    }

    #[test]
    fn receipt_round_trip_retains_exact_backup_and_rollback() {
        let temp = tempfile::tempdir().expect("Create the receipt directory");
        let state = temp.path().canonicalize().expect("Resolve the directory");
        let receipt = receipt(state.join("hooks.json"));
        let path = state.join("codex.json");
        let bytes = serde_json::to_vec(&receipt).expect("Serialize the receipt");
        private_file(&path)
            .expect("Create the receipt fixture")
            .write_all(&bytes)
            .expect("Write the receipt fixture");
        let loaded = load_receipt(&path)
            .expect("Read the receipt")
            .expect("Find the receipt");
        assert_eq!(
            serde_json::to_vec(&loaded).expect("Serialize the loaded receipt"),
            bytes
        );
        assert_eq!(loaded.original, Some(b"{}".to_vec()));
        assert_eq!(
            loaded
                .rollback(&receipt.installed)
                .expect("Remove the owned hook"),
            b"{}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn durable_journal_retains_exact_backup_before_publication() {
        let temp = tempfile::tempdir().expect("Create the receipt directory");
        let root = temp.path().canonicalize().expect("Resolve the directory");
        let state = root.join("private");
        private_directory(&state).expect("Create private state");
        let path = state.join("codex.json");
        let mut receipt = receipt(root.join("hooks.json"));
        for published in [false, true] {
            receipt.published = published;
            save_receipt(&path, &receipt).expect("Save the durable receipt");
            let loaded = load_receipt(&path)
                .expect("Read the durable receipt")
                .expect("Find the durable receipt");
            assert_eq!(loaded.published, published);
            assert_eq!(loaded.original, Some(b"{}".to_vec()));
            assert_eq!(
                loaded
                    .rollback(&receipt.installed)
                    .expect("Remove the owned hook"),
                b"{}"
            );
        }
    }

    #[cfg(windows)]
    fn directory_entries(path: &Path) -> Vec<PathBuf> {
        let mut entries: Vec<_> = fs::read_dir(path)
            .expect("Read directory entries")
            .map(|entry| entry.expect("Read the entry").path())
            .collect();
        entries.sort();
        entries
    }

    #[cfg(windows)]
    #[test]
    fn receipt_publication_refuses_windows_without_changing_files() {
        for exists in [false, true] {
            let temp = tempfile::tempdir().expect("Create the receipt directory");
            let state = temp.path().canonicalize().expect("Resolve the directory");
            let path = state.join("codex.json");
            let receipt = receipt(state.join("hooks.json"));
            if exists {
                fs::write(&path, b"original receipt").expect("Write the original receipt");
            }
            let before = directory_entries(&state);
            assert_eq!(
                save_receipt(&path, &receipt),
                Err("platform_native_gate_pending")
            );
            if exists {
                assert_eq!(
                    fs::read(&path).expect("Read the original receipt"),
                    b"original receipt"
                );
            } else {
                assert!(!path.try_exists().expect("Check the absent receipt"));
            }
            assert_eq!(directory_entries(&state), before);
        }
    }

    #[cfg(windows)]
    #[test]
    fn config_publication_refuses_windows_without_changing_files() {
        for exists in [false, true] {
            let temp = tempfile::tempdir().expect("Create the config directory");
            let root = temp.path().canonicalize().expect("Resolve the directory");
            let path = root.join("hooks.json");
            if exists {
                fs::write(&path, b"{}").expect("Write the original config");
            }
            let before = directory_entries(&root);
            let expected = exists.then_some(b"{}".as_slice());
            assert_eq!(
                publish(&path, expected, b"{\"owned\":true}"),
                Err("platform_native_gate_pending")
            );
            if exists {
                assert_eq!(fs::read(&path).expect("Read the original config"), b"{}");
            } else {
                assert!(!path.try_exists().expect("Check the absent config"));
            }
            assert_eq!(directory_entries(&root), before);
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn exchange_detects_and_restores_a_write_in_the_final_compare_window() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("hooks.json");
        fs::write(&path, b"{}").unwrap();
        let result = publish_inner(
            &path,
            Some(b"{}"),
            b"{\"owned\":true}",
            || fs::write(&path, b"{\"vendor\":true}").unwrap(),
            |_| {},
        );
        assert_eq!(result, Err("concurrent_config_change"));
        assert_eq!(fs::read(&path).unwrap(), b"{\"vendor\":true}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn exchange_preserves_metadata_changes_in_the_final_compare_window() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        for change in ["permissions", "acl", "xattr"] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().canonicalize().unwrap().join("hooks.json");
            fs::write(&path, b"{}").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            let result = publish_inner(
                &path,
                Some(b"{}"),
                b"{\"owned\":true}",
                || match change {
                    "permissions" => {
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
                    }
                    "acl" => {
                        assert!(
                            Command::new("/bin/chmod")
                                .args(["+a", "everyone allow read"])
                                .arg(&path)
                                .status()
                                .unwrap()
                                .success()
                        );
                    }
                    _ => {
                        assert!(
                            Command::new("/usr/bin/xattr")
                                .args(["-w", "com.antiburn.test", "concurrent"])
                                .arg(&path)
                                .status()
                                .unwrap()
                                .success()
                        );
                    }
                },
                |_| {},
            );
            assert_eq!(result, Err("concurrent_config_change"), "{change}");
            assert_eq!(fs::read(&path).unwrap(), b"{}", "{change}");
            match change {
                "permissions" => assert_eq!(
                    fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o400
                ),
                "acl" => {
                    let output = Command::new("/bin/ls")
                        .arg("-le")
                        .arg(&path)
                        .output()
                        .unwrap();
                    assert!(
                        String::from_utf8(output.stdout)
                            .unwrap()
                            .contains("everyone allow read")
                    );
                }
                _ => {
                    let output = Command::new("/usr/bin/xattr")
                        .args(["-p", "com.antiburn.test"])
                        .arg(&path)
                        .output()
                        .unwrap();
                    assert!(output.status.success());
                    assert_eq!(output.stdout, b"concurrent\n");
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_and_hardlink_targets_are_refused() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let target = root.join("target");
        fs::write(&target, b"{}").unwrap();
        let link = root.join("symlink");
        symlink(&target, &link).unwrap();
        assert!(read(&link).is_err());
        fs::hard_link(&target, root.join("hardlink")).unwrap();
        assert!(read(&target).is_err());
        assert_eq!(
            crate::agent_config::filesystem::read_checked(&target, &root)
                .unwrap()
                .bytes,
            b"{}"
        );
    }

    #[test]
    fn readers_share_regular_file_and_size_checks() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let target = root.join("config.json");
        fs::write(&target, b"{}").unwrap();
        assert_eq!(
            read(&target).unwrap().unwrap(),
            crate::agent_config::filesystem::read_checked(&target, &root)
                .unwrap()
                .bytes
        );
        let file = fs::OpenOptions::new().write(true).open(&target).unwrap();
        file.set_len(MAX_BYTES).unwrap();
        assert_eq!(read(&target).unwrap().unwrap().len() as u64, MAX_BYTES);
        file.set_len(MAX_BYTES + 1).unwrap();
        assert_eq!(read(&target), Err("unsafe_config"));
        assert!(matches!(
            crate::agent_config::filesystem::read_checked(&target, &root),
            Err(ConfigUnavailableReason::FileTooLarge)
        ));
        let directory = root.join("directory");
        fs::create_dir(&directory).unwrap();
        assert_eq!(read(&directory), Err("unsafe_config"));
        assert!(matches!(
            crate::agent_config::filesystem::read_checked(&directory, &root),
            Err(ConfigUnavailableReason::NonRegularFile)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn integration_reader_keeps_its_stricter_permission_policy() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let target = root.join("config.json");
        fs::write(&target, b"{}").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o660)).unwrap();
        assert_eq!(read(&target), Err("unsafe_owner_or_permissions"));
        assert_eq!(
            crate::agent_config::filesystem::read_checked(&target, &root)
                .unwrap()
                .bytes,
            b"{}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn publication_retains_original_permissions_acl_and_attributes() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;

        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("hooks.json");
        fs::write(&path, b"{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(
            Command::new("/bin/chmod")
                .args(["+a", "everyone allow read"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("/usr/bin/xattr")
                .args(["-w", "com.antiburn.test", "original"])
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        publish(&path, Some(b"{}"), b"{\"owned\":true}").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{\"owned\":true}");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let output = Command::new("/bin/ls")
            .arg("-le")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains("everyone allow read")
        );
        let output = Command::new("/usr/bin/xattr")
            .args(["-p", "com.antiburn.test"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"original\n");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recovery_retains_metadata_edits_to_the_proposed_inode() {
        use std::os::unix::fs::PermissionsExt;

        for change_before_restore in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().canonicalize().unwrap();
            let path = root.join("hooks.json");
            fs::write(&path, b"{}").unwrap();
            let mut exchanges = 0;
            let result = publish_inner(
                &path,
                Some(b"{}"),
                b"{\"owned\":true}",
                || fs::write(&path, b"{\"vendor\":true}").unwrap(),
                |stage| {
                    exchanges += 1;
                    if change_before_restore && exchanges == 1 {
                        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
                    } else if !change_before_restore && exchanges == 2 {
                        fs::set_permissions(stage, fs::Permissions::from_mode(0o400)).unwrap();
                    }
                },
            );
            assert_eq!(result, Err("concurrent_change_recovery_required"));
            let retained = fs::read_dir(&root)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|entry| entry != &path)
                .unwrap();
            let (proposed, original) = if change_before_restore {
                (&path, &retained)
            } else {
                (&retained, &path)
            };
            assert_eq!(fs::read(proposed).unwrap(), b"{\"owned\":true}");
            assert_eq!(fs::read(original).unwrap(), b"{\"vendor\":true}");
            assert_eq!(
                fs::metadata(proposed).unwrap().permissions().mode() & 0o777,
                0o400
            );
        }
    }
}

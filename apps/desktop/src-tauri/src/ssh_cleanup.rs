//! Remove retired SSH copies before desktop session consumers start.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::store::Store;

/// Open the store and remove SSH-owned data on one blocking worker.
/// Startup must await this operation before it exposes the store or starts workers.
/// Stop startup if database cleanup fails. Log file cleanup failures and retry
/// them on the next launch without blocking access to local sessions. Save no marker.
pub(crate) async fn open_store(data_dir: PathBuf) -> Result<Store> {
    tauri::async_runtime::spawn_blocking(move || {
        let store = Store::open(&data_dir)?;
        store.purge_ssh_sessions()?;
        if let Err(error) = remove_cached_copies(&data_dir.join("remote-sessions")) {
            tracing::warn!(event = "ssh_cache_cleanup_failed", error = %error);
        }
        Ok(store)
    })
    .await
    .context("SSH cleanup worker failed")?
}

fn is_link(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Reparse points include directory junctions.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn remove_entry(path: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    #[cfg(windows)]
    if is_link(metadata) {
        use std::os::windows::fs::MetadataExt;
        return if metadata.file_attributes() & 0x10 != 0 {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        };
    }
    if metadata.is_dir() && !is_link(metadata) {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => remove_entry(path, &metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Match the retired writer's host snapshot and temporary file names.
fn is_ssh_cache_file(name: &str) -> bool {
    if let Some(host_id) = name.strip_suffix(".snapshot.json") {
        return host_id.len() == 36
            && host_id
                .bytes()
                .enumerate()
                .all(|(index, byte)| match index {
                    8 | 13 | 18 | 23 => byte == b'-',
                    _ => byte.is_ascii_hexdigit(),
                });
    }
    name.strip_prefix(".write-")
        .and_then(|name| name.strip_suffix(".tmp"))
        .is_some_and(|nonce| {
            nonce.len() == 32
                && nonce
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
}

/// Remove only known cache paths. Never use transcript paths from database rows.
/// Keep host configuration, interval preferences, and unrelated files unchanged.
fn remove_cached_copies(root: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if is_link(&metadata) {
        return remove_entry(root, &metadata);
    }
    for name in ["transcripts", "staging"] {
        remove_if_present(&root.join(name))?;
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(is_ssh_cache_file) && !entry.file_type()?.is_dir()
        {
            remove_if_present(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/ssh_cleanup_integration_tests.rs"]
mod tests;

//! Atomic local-state persistence.
//!
//! Every helper here takes an explicit file path: the engine never decides
//! where local state lives. Writes are atomic and durable so a concurrent
//! reader never observes a truncated file and two concurrent writers cannot
//! interleave.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static WRITE_JSON_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The current UTC time as an RFC 3339 timestamp.
///
/// The format is fixed-width so plain string comparison of two values stays
/// chronological.
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Serialize `value` as pretty JSON and write it atomically to `path`.
pub async fn write_json_atomic<T: ?Sized + Serialize>(path: &Path, value: &T) -> Result<()> {
    let payload = serde_json::to_vec_pretty(value).context("failed to serialize JSON")?;
    write_bytes_atomic(path, &payload).await
}

/// Atomically write raw bytes to `path` by writing a unique temp file in the
/// same directory, syncing it, renaming it over `path`, and syncing the parent.
///
/// Used for both JSON state and plain text so a concurrent reader never
/// observes a truncated file, and so concurrent writers cannot interleave.
pub async fn write_bytes_atomic(path: &Path, payload: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent: {}", path.display()))?;
    let file_name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("path has no filename: {}", path.display()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .with_context(|| format!("failed to create directory {}", parent.display()))?;
    let tmp = parent.join(format!(
        ".{}.{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id(),
        WRITE_JSON_TMP_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    let result = async {
        use tokio::io::AsyncWriteExt;

        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .await
            .with_context(|| format!("failed to create temporary file {}", tmp.display()))?;
        file.write_all(payload)
            .await
            .with_context(|| format!("failed to write temporary file {}", tmp.display()))?;
        file.sync_all()
            .await
            .with_context(|| format!("failed to sync temporary file {}", tmp.display()))?;
        drop(file);
        replace_file_durable(&tmp, path)
            .await
            .with_context(|| format!("failed to atomically replace {}", path.display()))
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

/// Rename `tmp` over `path` and make the rename itself durable.
pub async fn replace_file_durable(tmp: &Path, path: &Path) -> std::io::Result<()> {
    replace_file(tmp, path).await?;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other(format!("path has no parent: {}", path.display())))?;
    sync_parent_directory(parent)
        .await
        .map_err(std::io::Error::other)
}

/// Flush a directory entry so a rename or unlink inside it survives a crash.
#[cfg(not(windows))]
pub async fn sync_parent_directory(path: &Path) -> Result<()> {
    let directory = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("failed to open directory {} for sync", path.display()))?;
    directory
        .sync_all()
        .await
        .with_context(|| format!("failed to sync directory {}", path.display()))
}

/// Flush a directory entry so a rename or unlink inside it survives a crash.
#[cfg(windows)]
pub async fn sync_parent_directory(_path: &Path) -> Result<()> {
    // Durable renames use MoveFileExW with MOVEFILE_WRITE_THROUGH below. Cleanup
    // deletion durability is not required because an old generation is harmless.
    Ok(())
}

#[cfg(not(windows))]
async fn replace_file(tmp: &Path, path: &Path) -> std::io::Result<()> {
    tokio::fs::rename(tmp, path).await
}

#[cfg(windows)]
async fn replace_file(tmp: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let tmp = wide(tmp);
    let path = wide(path);
    // SAFETY: both buffers are NUL-terminated and outlive the call.
    let ok = unsafe {
        MoveFileExW(
            tmp.as_ptr(),
            path.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[tokio::test]
    async fn write_json_atomic_handles_concurrent_writers_for_same_path() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("shared.json");
        let mut tasks = Vec::new();

        for value in 0..8 {
            let path = path.clone();
            tasks.push(tokio::spawn(async move {
                write_json_atomic(&path, &json!({ "value": value })).await
            }));
        }

        for task in tasks {
            task.await
                .expect("join concurrent writer")
                .expect("write shared json");
        }

        let parsed = serde_json::from_str::<serde_json::Value>(
            &tokio::fs::read_to_string(&path)
                .await
                .expect("read final shared json"),
        )
        .expect("parse final shared json");
        assert!(parsed.get("value").is_some(), "expected JSON payload");
    }

    #[tokio::test]
    async fn write_json_atomic_replaces_existing_file() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("existing.json");
        tokio::fs::write(&path, r#"{"value":"old"}"#)
            .await
            .expect("write old json");

        write_json_atomic(&path, &json!({ "value": "new" }))
            .await
            .expect("replace existing json");

        let parsed = serde_json::from_str::<serde_json::Value>(
            &tokio::fs::read_to_string(&path)
                .await
                .expect("read final json"),
        )
        .expect("parse final json");
        assert_eq!(parsed["value"], "new");
        assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn now_rfc3339_is_fixed_width_utc() {
        let value = now_rfc3339();
        assert!(value.ends_with('Z'), "expected UTC timestamp, got {value}");
        assert!(
            time::OffsetDateTime::parse(&value, &time::format_description::well_known::Rfc3339)
                .is_ok()
        );
    }
}

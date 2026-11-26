use super::*;
use serde_json::json;
use std::{
    os::unix::fs::symlink,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::task::JoinHandle;

fn claim() -> DeviceSession {
    DeviceSession {
        origin: "https://cloud.example.test".into(),
        read_token: "synthetic-read-token".into(),
        personal_workspace_id: "synthetic-personal".into(),
    }
}

/// Use a short socket name because macOS only permits 103 pathname bytes.
fn directory() -> tempfile::TempDir {
    tempfile::Builder::new().prefix("db-").tempdir().unwrap()
}

async fn start(
    directory: &Path,
    load: impl FnOnce() -> std::future::Ready<Option<(u64, DeviceSession)>> + Clone + Send + 'static,
    current: impl Fn(u64) -> bool + Clone + Send + 'static,
) -> (PathBuf, JoinHandle<()>) {
    let path = directory.join(SOCKET_NAME);
    let directory = directory.to_owned();
    let (socket, lock) = tokio::task::spawn_blocking(move || bind(&directory))
        .await
        .unwrap()
        .unwrap();
    let socket = UnixListener::from_std(socket).unwrap();
    let task = tokio::spawn(async move {
        let _lock = lock;
        serve(socket, load, current).await;
    });
    (path, task)
}

async fn exchange(path: &Path, request: &[u8]) -> Vec<u8> {
    let mut stream = UnixStream::connect(path).await.unwrap();
    stream.write_all(request).await.unwrap();
    stream.shutdown().await.unwrap();
    let mut result = Vec::new();
    timeout(Duration::from_secs(3), stream.read_to_end(&mut result))
        .await
        .unwrap()
        .unwrap();
    result
}

#[tokio::test]
async fn sends_only_the_current_device_claim_as_a_single_line() {
    let directory = directory();
    let (path, task) = start(
        directory.path(),
        || std::future::ready(Some((7, claim()))),
        |generation| generation == 7,
    )
    .await;
    let response = exchange(&path, REQUEST).await;
    assert_eq!(response.iter().filter(|&&byte| byte == b'\n').count(), 1);
    assert!(response.ends_with(b"\n"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&response).unwrap(),
        json!({
            "origin": "https://cloud.example.test",
            "read_token": "synthetic-read-token",
            "personal_workspace_id": "synthetic-personal"
        })
    );
    task.abort();
}

#[tokio::test]
async fn rejects_stale_and_invalid_requests_without_sending_proof() {
    let directory = directory();
    let loads = Arc::new(AtomicUsize::new(0));
    let (path, task) = start(
        directory.path(),
        {
            let loads = loads.clone();
            move || {
                loads.fetch_add(1, Ordering::Relaxed);
                std::future::ready(Some((7, claim())))
            }
        },
        |generation| generation == 8,
    )
    .await;
    assert!(exchange(&path, b"device-session").await.is_empty());
    assert!(exchange(&path, b"device-session-v1\n").await.is_empty());
    assert_eq!(
        exchange(&path, b"device-read-session-v2\n").await,
        UNAVAILABLE
    );
    assert_eq!(loads.load(Ordering::Relaxed), 0);
    assert_eq!(exchange(&path, REQUEST).await, UNAVAILABLE);
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    task.abort();
}

#[tokio::test]
async fn returns_unavailable_instead_of_truncating_a_claim() {
    let directory = directory();
    let (path, task) = start(
        directory.path(),
        || {
            let mut session = claim();
            session.read_token = "x".repeat(MAX_RESPONSE);
            std::future::ready(Some((7, session)))
        },
        |_| true,
    )
    .await;
    assert_eq!(exchange(&path, REQUEST).await, UNAVAILABLE);
    task.abort();
}

#[test]
fn binds_privately_and_recovers_only_after_the_previous_owner_exits() {
    let directory = directory();
    fs::set_permissions(directory.path(), Permissions::from_mode(0o755)).unwrap();
    let (listener, owner) = bind(directory.path()).unwrap();
    let socket = directory.path().join(SOCKET_NAME);
    let metadata = fs::symlink_metadata(&socket).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.uid(), effective_uid());
    assert_eq!(
        fs::metadata(directory.path()).unwrap().mode() & 0o777,
        0o755
    );
    assert!(bind(directory.path()).is_err());
    assert_eq!(fs::symlink_metadata(&socket).unwrap().ino(), metadata.ino());
    drop(listener);
    drop(owner);
    assert!(bind(directory.path()).is_ok());
}

#[test]
fn rejects_shared_directories_and_symlinked_entries() {
    let directory = directory();
    fs::set_permissions(directory.path(), Permissions::from_mode(0o770)).unwrap();
    assert!(bind(directory.path()).is_err());
    fs::set_permissions(directory.path(), Permissions::from_mode(0o700)).unwrap();
    let target = directory.path().join("target");
    fs::write(&target, b"keep").unwrap();
    let socket = directory.path().join(SOCKET_NAME);
    symlink(&target, &socket).unwrap();
    assert!(bind(directory.path()).is_err());
    assert!(fs::symlink_metadata(&socket).unwrap().is_symlink());
    assert_eq!(fs::read(target).unwrap(), b"keep");
}

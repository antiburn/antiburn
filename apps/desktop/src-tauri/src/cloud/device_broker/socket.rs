//! Bound local socket I/O. The transport does not read files that contain Cloud
//! credentials and does not own account state.

use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions, Permissions},
    future::Future,
    io::{self, ErrorKind},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{SocketAddr, UnixListener as StdUnixListener},
    },
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    task::JoinSet,
    time::timeout,
};

const SOCKET_NAME: &str = "cloud-device.sock";
const OWNER_FILE: &str = ".cloud-device.lock";
const REQUEST: &[u8] = b"device-read-session-v1\n";
const UNAVAILABLE: &[u8] = b"{\"status\":\"unavailable\"}\n";
const MAX_RESPONSE: usize = 32 * 1024;
const MAX_CONNECTIONS: usize = 8;
const IO_DEADLINE: Duration = Duration::from_secs(2);
// Restore can exchange proof and verify the session before getting a read token.
const SESSION_DEADLINE: Duration = Duration::from_secs(50);

/// Private wire fields. The access token is read-only. Never log it or send it
/// to a renderer.
#[derive(Serialize)]
pub(super) struct DeviceSession {
    pub(super) origin: String,
    pub(super) read_token: String,
    pub(super) personal_workspace_id: String,
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no arguments and does not modify memory.
    unsafe { libc::geteuid() }
}

/// Bind in the app's own directory on a blocking worker. Keep the returned
/// owner file open for the listener's lifetime so a second process cannot
/// remove a live socket. An owned stale socket is removed on the next start.
pub(super) fn bind(directory: &Path) -> io::Result<(StdUnixListener, File)> {
    let path = directory.join(SOCKET_NAME);
    let address = SocketAddr::from_pathname(&path)?;
    let denied = || io::Error::from(ErrorKind::PermissionDenied);
    let uid = effective_uid();
    let app_directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(directory)?;
    let metadata = app_directory.metadata()?;
    if metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        return Err(denied());
    }
    let owner = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join(OWNER_FILE))?;
    let metadata = owner.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(denied());
    }
    owner.try_lock().map_err(io::Error::from)?;

    match fs::symlink_metadata(&path) {
        Ok(metadata)
            if metadata.file_type().is_socket()
                && metadata.uid() == uid
                && metadata.nlink() == 1 =>
        {
            fs::remove_file(&path)?;
        }
        Ok(_) => return Err(denied()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let listener = StdUnixListener::bind_addr(&address)?;
    fs::set_permissions(&path, Permissions::from_mode(0o600))?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(denied());
    }
    listener.set_nonblocking(true)?;
    Ok((listener, owner))
}

/// Admit only peers whose OS credential matches Desktop's effective user.
/// Limit the number of socket tasks independently of Cloud work admission.
pub(super) async fn serve<Load, Session, Current>(
    listener: UnixListener,
    load_session: Load,
    current: Current,
) where
    Load: FnOnce() -> Session + Clone + Send + 'static,
    Session: Future<Output = Option<(u64, DeviceSession)>> + Send + 'static,
    Current: Fn(u64) -> bool + Clone + Send + 'static,
{
    let uid = effective_uid();
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            _ = connections.join_next(), if !connections.is_empty() => {}
            connection = listener.accept(), if connections.len() < MAX_CONNECTIONS => {
                let Ok((stream, _)) = connection else {
                    return;
                };
                if stream.peer_cred().is_ok_and(|peer| peer.uid() == uid) {
                    connections.spawn(reply(stream, load_session.clone(), current.clone()));
                }
            }
        }
    }
}

/// Read only the fixed request. Do not use a client-provided origin or workspace
/// ID and never start Cloud work for an invalid or partial request.
async fn reply<Session>(
    mut stream: UnixStream,
    load_session: impl FnOnce() -> Session,
    current: impl Fn(u64) -> bool,
) where
    Session: Future<Output = Option<(u64, DeviceSession)>>,
{
    let mut request = [0; REQUEST.len()];
    if !matches!(
        timeout(IO_DEADLINE, stream.read_exact(&mut request)).await,
        Ok(Ok(_))
    ) {
        return;
    }
    let response = if request == REQUEST
        && let Ok(Some((generation, session))) = timeout(SESSION_DEADLINE, load_session()).await
        && let Some(bytes) = encode(&session)
    {
        Some((generation, bytes))
    } else {
        None
    };
    let _ = timeout(IO_DEADLINE, async move {
        if let Some((generation, bytes)) = response {
            let mut written = 0;
            while written < bytes.len() {
                stream.writable().await?;
                if !current(generation) {
                    if written != 0 {
                        return Ok(());
                    }
                    break;
                }
                match stream.try_write(&bytes[written..]) {
                    Ok(0) => return Err(io::Error::from(ErrorKind::WriteZero)),
                    Ok(size) => written += size,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
                    Err(error) => return Err(error),
                }
            }
            if written == bytes.len() {
                return Ok(());
            }
        }
        stream.write_all(UNAVAILABLE).await
    })
    .await;
}

/// Serialize directly into a capped buffer. Never send a truncated claim.
fn encode(session: &DeviceSession) -> Option<Vec<u8>> {
    struct BoundedResponse(Vec<u8>);

    impl io::Write for BoundedResponse {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() >= MAX_RESPONSE - self.0.len() {
                return Err(io::Error::from(ErrorKind::InvalidData));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut response = BoundedResponse(Vec::new());
    serde_json::to_writer(&mut response, session).ok()?;
    response.0.push(b'\n');
    Some(response.0)
}

#[cfg(test)]
#[path = "../tests/device_broker_tests.rs"]
mod tests;

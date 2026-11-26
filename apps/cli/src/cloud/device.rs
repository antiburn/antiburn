//! Read the current device connection from the app's private local socket.

use serde::Deserialize;
use uuid::Uuid;

use crate::config::Config;

#[cfg(unix)]
#[derive(Deserialize)]
#[serde(untagged)]
enum DeviceReply {
    Connected(Connection),
    Unavailable { status: Unavailable },
}

#[cfg(unix)]
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Unavailable {
    Unavailable,
}

/// Read-only proof. The app alone owns the full connection and token rotation.
#[derive(Deserialize)]
pub(super) struct Connection {
    pub origin: String,
    pub read_token: String,
    pub personal_workspace_id: Uuid,
}

#[cfg(unix)]
pub(super) async fn current(config: &Config) -> Result<Option<Connection>, String> {
    use std::io::ErrorKind;
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    let unavailable = "could not read the local device connection";
    let Some(data) = config.data_root.clone().or_else(dirs::data_dir) else {
        return Ok(None);
    };
    let data = data.join(&config.app_identifier);
    let path = data.join("cloud-device.sock");
    let socket = match tokio::fs::symlink_metadata(&path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unavailable.into()),
    };
    let directory = tokio::fs::symlink_metadata(&data)
        .await
        .map_err(|_| unavailable)?;
    // SAFETY: geteuid takes no arguments and reads the current process identity.
    let user = unsafe { libc::geteuid() };
    if !directory.is_dir()
        || directory.uid() != user
        || directory.mode() & 0o022 != 0
        || !socket.file_type().is_socket()
        || socket.uid() != user
        || socket.mode() & 0o077 != 0
        || tokio::fs::canonicalize(&data)
            .await
            .map_err(|_| unavailable)?
            != data
    {
        return Err(unavailable.into());
    }

    // Desktop may need to refresh and verify a saved connection before replying.
    tokio::time::timeout(Duration::from_secs(55), async {
        let mut stream = match UnixStream::connect(path).await {
            Ok(stream) => stream,
            Err(error) if error.kind() == ErrorKind::ConnectionRefused => return Ok(None),
            Err(_) => return Err(unavailable.into()),
        };
        if stream.peer_cred().map_err(|_| unavailable)?.uid() != user {
            return Err(unavailable.into());
        }
        stream
            .write_all(b"device-read-session-v1\n")
            .await
            .map_err(|_| unavailable)?;
        let mut reply = Vec::new();
        stream
            .take(32 * 1024 + 1)
            .read_to_end(&mut reply)
            .await
            .map_err(|_| unavailable)?;
        if reply.len() > 32 * 1024 {
            return Err(unavailable.into());
        }
        match serde_json::from_slice(&reply).map_err(|_| unavailable)? {
            DeviceReply::Connected(connection) => Ok(Some(connection)),
            DeviceReply::Unavailable {
                status: Unavailable::Unavailable,
            } => Ok(None),
        }
    })
    .await
    .map_err(|_| unavailable)?
}

#[cfg(not(unix))]
pub(super) async fn current(_config: &Config) -> Result<Option<Connection>, String> {
    Ok(None)
}

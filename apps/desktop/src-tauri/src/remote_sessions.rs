//! Persisted remote hosts and the secure scan lifecycle.

use antiburn_remote::{PROTOCOL_VERSION, Request, Snapshot, transport};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

pub const HOSTS_CHANGED_EVENT: &str = "remote-hosts-changed";
const MAX_HOSTS: usize = 8;
const MAX_CONFIG_BYTES: u64 = 64 * 1024;
static HOST_CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());
static HOST_UPDATE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn lock_host_config() -> std::sync::MutexGuard<'static, ()> {
    HOST_CONFIG_WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct HostRecord {
    id: String,
    ssh_alias: String,
    display_name: Option<String>,
    #[serde(default = "default_sync_enabled")]
    automatic_sync_enabled: bool,
    last_successful_sync_epoch: Option<i64>,
    cached_session_count: u32,
    last_error: Option<RemoteHostError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHostError {
    pub category: RemoteErrorCategory,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteErrorCategory {
    SshUnavailable,
    AuthenticationFailed,
    HostKeyFailed,
    HelperMissing,
    IncompatibleProtocol,
    UnsupportedPlatform,
    CacheLimit,
    TransferFailed,
    AnalysisFailed,
    Cancelled,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteHostStatus {
    Idle,
    Syncing,
    Error,
}

fn default_sync_enabled() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHost {
    pub id: String,
    pub ssh_alias: String,
    pub display_name: Option<String>,
    pub automatic_sync_enabled: bool,
    pub status: RemoteHostStatus,
    pub last_successful_sync_epoch: Option<i64>,
    pub cached_session_count: u32,
    pub last_error: Option<RemoteHostError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHostPreflight {
    pub status: String,
    pub supported_agents: Vec<String>,
    pub platform: Option<String>,
    pub architecture: Option<String>,
    /// The release that shipped the installed helper. None when it did not answer.
    pub helper_version: Option<String>,
    pub message: Option<String>,
}

pub fn directory(app: &AppHandle) -> Result<PathBuf, String> {
    let path = app
        .path()
        .app_data_dir()
        .map_err(|_| "Remote storage is unavailable".to_owned())?
        .join("remote-sessions");
    private_dir(&path)?;
    Ok(path)
}

pub(crate) fn private_dir(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|_| "Remote storage is unavailable".to_owned())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "Remote storage is unavailable".to_owned())?;
    }
    Ok(())
}

fn private_nonce() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "Remote storage is unavailable".to_owned())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Remote storage is unavailable".to_owned())?;
    private_dir(parent)?;
    let temporary = parent.join(format!(".write-{}.tmp", private_nonce()?));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(|_| "Remote storage is unavailable".to_owned())?;
        file.write_all(bytes)
            .map_err(|_| "Remote storage is unavailable".to_owned())?;
        file.sync_all()
            .map_err(|_| "Remote storage is unavailable".to_owned())?;
        std::fs::rename(&temporary, path).map_err(|_| "Remote storage is unavailable".to_owned())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

fn hosts_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(directory(app)?.join("hosts.json"))
}

fn read_records(app: &AppHandle) -> Result<Vec<HostRecord>, String> {
    read_records_from(&hosts_path(app)?)
}

fn read_records_from(path: &Path) -> Result<Vec<HostRecord>, String> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err("Remote host settings are unavailable".into()),
    };
    if metadata.len() > MAX_CONFIG_BYTES {
        return Err("Remote host settings are invalid".into());
    }
    let records: Vec<HostRecord> = serde_json::from_slice(
        &std::fs::read(path).map_err(|_| "Remote host settings are unavailable".to_owned())?,
    )
    .map_err(|_| "Remote host settings are invalid".to_owned())?;
    validate_records(&records)?;
    Ok(records)
}

fn write_records(app: &AppHandle, records: &[HostRecord]) -> Result<(), String> {
    write_records_to(&hosts_path(app)?, records)
}

fn write_records_to(path: &Path, records: &[HostRecord]) -> Result<(), String> {
    validate_records(records)?;
    let bytes = serde_json::to_vec(records).map_err(|_| "Remote host settings are invalid")?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err("Remote host settings are invalid".into());
    }
    write_private(path, &bytes)
}

fn validate_records(records: &[HostRecord]) -> Result<(), String> {
    if records.len() > MAX_HOSTS {
        return Err("At most eight remote hosts are supported".into());
    }
    let mut ids = HashSet::new();
    let mut aliases = HashSet::new();
    for record in records {
        validate_host_id(&record.id)?;
        validate_alias(&record.ssh_alias)?;
        validate_display_name(record.display_name.as_deref())?;
        if !ids.insert(record.id.as_str()) {
            return Err("Remote host settings contain a duplicate ID".into());
        }
        if !aliases.insert(record.ssh_alias.to_ascii_lowercase()) {
            return Err("That SSH alias is already configured".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_host_id(id: &str) -> Result<(), String> {
    let valid = id.len() == 36
        && id.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        });
    if valid {
        Ok(())
    } else {
        Err("Remote host ID is invalid".into())
    }
}

fn validate_alias(alias: &str) -> Result<(), String> {
    transport::validate_host(alias).map_err(|_| "Enter a valid SSH host alias".to_owned())
}

fn validate_display_name(name: Option<&str>) -> Result<(), String> {
    if name.is_none_or(|name| {
        let name = name.trim();
        name.len() <= 80 && !name.chars().any(char::is_control)
    }) {
        Ok(())
    } else {
        Err("Display name must be 80 characters or fewer".into())
    }
}

fn normalized_name(name: Option<String>) -> Option<String> {
    name.map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

fn new_host_id() -> Result<String, String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "Could not create a remote host ID".to_owned())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

fn ipc_hosts(app: &AppHandle, records: Vec<HostRecord>) -> Result<Vec<RemoteHost>, String> {
    let counts = app
        .state::<crate::store::Store>()
        .remote_session_counts()
        .map_err(|_| "Saved remote session counts are unavailable".to_owned())?;
    let active = app
        .try_state::<crate::remote_sync::Scheduler>()
        .and_then(|_| crate::remote_sync::get_remote_sync_status(app.clone()).active)
        .map(|progress| progress.host_id);
    Ok(records
        .into_iter()
        .map(|record| RemoteHost {
            status: if active.as_deref() == Some(record.id.as_str()) {
                RemoteHostStatus::Syncing
            } else if record.last_error.is_some() {
                RemoteHostStatus::Error
            } else {
                RemoteHostStatus::Idle
            },
            cached_session_count: counts.get(&record.id).copied().unwrap_or(0),
            id: record.id,
            ssh_alias: record.ssh_alias,
            display_name: record.display_name,
            automatic_sync_enabled: record.automatic_sync_enabled,
            last_successful_sync_epoch: record.last_successful_sync_epoch,
            last_error: record.last_error,
        })
        .collect())
}

pub(crate) fn emit_hosts(app: &AppHandle) {
    if let Ok(hosts) = read_records(app).and_then(|records| ipc_hosts(app, records)) {
        let _ = app.emit(HOSTS_CHANGED_EVENT, hosts);
    }
}

#[tauri::command]
pub async fn get_remote_hosts(app: AppHandle) -> Result<Vec<RemoteHost>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let records = read_records(&app)?;
        ipc_hosts(&app, records)
    })
    .await
    .map_err(|_| "Remote host settings are unavailable".to_owned())?
}

fn preflight_error(error: &antiburn_remote::transport::PrerequisiteError) -> RemoteHostPreflight {
    use antiburn_remote::transport::PrerequisiteErrorCategory as Category;
    let (status, message) = match error.category() {
        Category::InvalidAlias => ("unknown", "Enter a valid SSH host alias"),
        Category::SshUnavailable => ("sshUnavailable", "SSH could not reach this host"),
        Category::HostKey => ("hostKeyFailed", "SSH could not verify this host key"),
        Category::Authentication => (
            "authenticationFailed",
            "SSH key authentication failed for this host",
        ),
        Category::HelperMissing => (
            "helperMissing",
            "Install the Antiburn remote helper on this host",
        ),
        Category::ProtocolMismatch => (
            "incompatibleProtocol",
            "Update the Antiburn remote helper on this host",
        ),
        Category::UnsupportedHost => ("unsupportedPlatform", "This host platform is not supported"),
        Category::InvalidResponse => (
            "incompatibleProtocol",
            "The remote helper returned an invalid response",
        ),
    };
    RemoteHostPreflight {
        status: status.into(),
        supported_agents: Vec::new(),
        platform: None,
        architecture: None,
        helper_version: None,
        message: Some(message.into()),
    }
}

async fn run_preflight(app: &AppHandle, ssh_alias: &str) -> RemoteHostPreflight {
    let result = match transport::check(ssh_alias).await {
        Ok(hello) => {
            let platform = hello.platform.to_ascii_lowercase();
            let architecture = hello.architecture.to_ascii_lowercase();
            if platform != "linux" || !matches!(architecture.as_str(), "x86_64" | "aarch64") {
                RemoteHostPreflight {
                    status: "unsupportedPlatform".into(),
                    supported_agents: Vec::new(),
                    platform: Some(platform),
                    architecture: Some(architecture),
                    helper_version: Some(hello.helper_version),
                    message: Some("Currently supports Linux x64 and ARM64 hosts".into()),
                }
            } else {
                RemoteHostPreflight {
                    status: "ready".into(),
                    supported_agents: hello.supported_agents,
                    platform: Some(platform),
                    architecture: Some(architecture),
                    helper_version: Some(hello.helper_version),
                    message: None,
                }
            }
        }
        Err(error) => preflight_error(&error),
    };
    use crate::analytics::event::RemoteConnectionOutcome as Outcome;
    let outcome = match result.status.as_str() {
        "ready" => Outcome::Ready,
        "authenticationFailed" => Outcome::Authentication,
        "hostKeyFailed" => Outcome::HostKey,
        "helperMissing" => Outcome::HelperMissing,
        "incompatibleProtocol" | "unsupportedPlatform" => Outcome::Incompatible,
        "sshUnavailable" => Outcome::Connection,
        _ => Outcome::Invalid,
    };
    crate::analytics::record_remote_host_connection_checked(app, outcome);
    result
}

#[tauri::command]
pub async fn check_remote_host(
    app: AppHandle,
    ssh_alias: String,
) -> Result<RemoteHostPreflight, String> {
    if let Err(error) = validate_alias(&ssh_alias) {
        crate::analytics::record_remote_host_connection_checked(
            &app,
            crate::analytics::event::RemoteConnectionOutcome::Invalid,
        );
        return Err(error);
    }
    Ok(run_preflight(&app, &ssh_alias).await)
}

async fn require_ready(app: &AppHandle, ssh_alias: &str) -> Result<(), String> {
    let result = run_preflight(app, ssh_alias).await;
    if result.status == "ready" {
        Ok(())
    } else {
        Err(result
            .message
            .unwrap_or_else(|| "Remote host prerequisite check failed".into()))
    }
}

#[tauri::command]
pub async fn add_remote_host(
    app: AppHandle,
    ssh_alias: String,
    display_name: Option<String>,
) -> Result<RemoteHost, String> {
    validate_alias(&ssh_alias)?;
    validate_display_name(display_name.as_deref())?;
    let existing = read_records(&app)?;
    if existing.len() >= MAX_HOSTS {
        return Err("At most eight remote hosts are supported".into());
    }
    if existing
        .iter()
        .any(|record| record.ssh_alias.eq_ignore_ascii_case(&ssh_alias))
    {
        return Err("That SSH alias is already configured".into());
    }
    require_ready(&app, &ssh_alias).await?;
    let id = new_host_id()?;
    let record = HostRecord {
        id: id.clone(),
        ssh_alias,
        display_name: normalized_name(display_name),
        automatic_sync_enabled: true,
        last_successful_sync_epoch: None,
        cached_session_count: 0,
        last_error: None,
    };
    let configured = {
        let _config = lock_host_config();
        let mut records = read_records(&app)?;
        if records.len() >= MAX_HOSTS
            || records
                .iter()
                .any(|other| other.ssh_alias.eq_ignore_ascii_case(&record.ssh_alias))
        {
            return Err("Remote host settings changed; check the host and try again".into());
        }
        records.push(record.clone());
        write_records(&app, &records)?;
        records.len()
    };
    crate::analytics::record_remote_host_changed(
        &app,
        crate::analytics::event::RemoteHostChange::Added,
        configured,
    );
    emit_hosts(&app);
    crate::remote_sync::enqueue_automatic(&app, &id);
    Ok(ipc_hosts(&app, vec![record])?.remove(0))
}

#[tauri::command]
pub async fn update_remote_host(
    app: AppHandle,
    id: String,
    ssh_alias: String,
    display_name: Option<String>,
) -> Result<RemoteHost, String> {
    validate_host_id(&id)?;
    validate_alias(&ssh_alias)?;
    validate_display_name(display_name.as_deref())?;
    let _update = HOST_UPDATE_LOCK.lock().await;
    let records = read_records(&app)?;
    let current = records
        .iter()
        .find(|record| record.id == id)
        .ok_or_else(|| "Remote host was removed".to_owned())?;
    if records
        .iter()
        .any(|record| record.id != id && record.ssh_alias.eq_ignore_ascii_case(&ssh_alias))
    {
        return Err("That SSH alias is already configured".into());
    }
    let requested_alias_change = current.ssh_alias != ssh_alias;
    if requested_alias_change {
        require_ready(&app, &ssh_alias).await?;
    }
    let update = || -> Result<(HostRecord, usize, bool), String> {
        let _config = lock_host_config();
        let mut records = read_records(&app)?;
        let record = records
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| "Remote host was removed".to_owned())?;
        let alias_changed = record.ssh_alias != ssh_alias;
        record.ssh_alias = ssh_alias;
        record.display_name = normalized_name(display_name);
        record.last_error = None;
        let result = record.clone();
        write_records(&app, &records)?;
        Ok((result, records.len(), alias_changed))
    };
    let (result, configured, alias_changed) = if requested_alias_change {
        crate::remote_sync::with_destructive_lifecycle_guard(
            &app,
            std::slice::from_ref(&id),
            || update().map_err(anyhow::Error::msg),
        )
        .map_err(|_| "Could not update the remote host".to_owned())?
    } else {
        update()?
    };
    crate::analytics::record_remote_host_changed(
        &app,
        crate::analytics::event::RemoteHostChange::Edited,
        configured,
    );
    emit_hosts(&app);
    if alias_changed {
        crate::remote_sync::enqueue_automatic(&app, &id);
    }
    Ok(ipc_hosts(&app, vec![result])?.remove(0))
}

#[tauri::command]
pub fn set_remote_host_sync_enabled(
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<(), String> {
    validate_host_id(&id)?;
    let configured = {
        let _config = lock_host_config();
        let mut records = read_records(&app)?;
        let record = records
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or_else(|| "Remote host was removed".to_owned())?;
        if record.automatic_sync_enabled == enabled {
            return Ok(());
        }
        record.automatic_sync_enabled = enabled;
        write_records(&app, &records)?;
        records.len()
    };
    crate::analytics::record_remote_host_changed(
        &app,
        if enabled {
            crate::analytics::event::RemoteHostChange::SyncEnabled
        } else {
            crate::analytics::event::RemoteHostChange::SyncDisabled
        },
        configured,
    );
    emit_hosts(&app);
    crate::remote_sync::host_sync_changed(&app, &id, enabled);
    Ok(())
}

pub(crate) fn automatic_host_ids(app: &AppHandle) -> Result<HashSet<String>, String> {
    Ok(read_records(app)?
        .into_iter()
        .filter(|record| record.automatic_sync_enabled)
        .map(|record| record.id)
        .collect())
}

#[tauri::command]
pub async fn remove_remote_host(app: AppHandle, id: String) -> Result<(), String> {
    validate_host_id(&id)?;
    if !host_exists(&app, &id)? {
        return Ok(());
    }
    let store = app.state::<crate::store::Store>().inner().clone();
    let root = directory(&app)?;
    let id_copy = id.clone();
    let worker_app = app.clone();
    let (removed, revision) = tauri::async_runtime::spawn_blocking(move || {
        crate::remote_sync::with_destructive_lifecycle_guard(
            &worker_app,
            std::slice::from_ref(&id_copy),
            || {
                let removed = store.delete_remote_host(&id_copy)?;
                let transcripts = root.join("transcripts").join(&id_copy);
                if transcripts.exists() {
                    std::fs::remove_dir_all(transcripts)?;
                }
                let snapshot = root.join(format!("{id_copy}.snapshot.json"));
                if snapshot.exists() {
                    std::fs::remove_file(snapshot)?;
                }
                let _config = lock_host_config();
                let mut records = read_records(&worker_app).map_err(anyhow::Error::msg)?;
                records.retain(|record| record.id != id_copy);
                write_records(&worker_app, &records).map_err(anyhow::Error::msg)?;
                Ok(removed)
            },
        )
        .map_err(|_| "Could not remove cached remote sessions".to_owned())
    })
    .await
    .map_err(|_| "Could not remove cached remote sessions".to_owned())??;
    if removed > 0 {
        crate::session_lifecycle::report(
            &app,
            crate::session_lifecycle::SyncObservation::Removed {
                scope: crate::session_lifecycle::RemovalScope::Broad,
                reason: crate::session_lifecycle::RemovalReason::Deleted,
                revision,
            },
        );
    }
    let configured = read_records(&app)?.len();
    crate::analytics::record_remote_host_changed(
        &app,
        crate::analytics::event::RemoteHostChange::Removed,
        configured,
    );
    emit_hosts(&app);
    Ok(())
}

#[tauri::command]
pub fn scan_remote_host(app: AppHandle, id: String) -> Result<(), String> {
    validate_host_id(&id)?;
    let records = read_records(&app)?;
    let host = records
        .iter()
        .find(|record| record.id == id)
        .ok_or_else(|| "Remote host was removed".to_owned())?;
    if !host.automatic_sync_enabled {
        return Err("Turn on syncing for this host first".into());
    }
    crate::remote_sync::enqueue_manual(&app, &id);
    Ok(())
}

pub(crate) fn lifecycle_host_ids(app: &AppHandle) -> Vec<String> {
    lifecycle_ids(read_records(app))
}

fn lifecycle_ids(records: Result<Vec<HostRecord>, String>) -> Vec<String> {
    match records {
        Ok(records) => records.into_iter().map(|record| record.id).collect(),
        Err(error) => {
            tracing::warn!(event = "remote_host_settings_unavailable", error = %error);
            Vec::new()
        }
    }
}

pub(crate) fn host_ids(app: &AppHandle) -> Result<Vec<String>, String> {
    Ok(read_records(app)?
        .into_iter()
        .map(|record| record.id)
        .collect())
}

pub(crate) fn host_exists(app: &AppHandle, id: &str) -> Result<bool, String> {
    Ok(read_records(app)?.iter().any(|record| record.id == id))
}

pub(crate) fn read_interval(app: &AppHandle) -> Result<u64, String> {
    let path = directory(app)?.join("scan-interval.json");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(300),
        Err(_) => return Err("Remote sync settings are unavailable".into()),
    };
    let seconds: u64 = serde_json::from_slice(&bytes)
        .map_err(|_| "Remote sync settings are invalid".to_owned())?;
    crate::remote_sync::validate_interval(seconds)?;
    Ok(seconds)
}

pub(crate) fn write_interval(app: &AppHandle, seconds: u64) -> Result<(), String> {
    write_private(
        &directory(app)?.join("scan-interval.json"),
        &serde_json::to_vec(&seconds).map_err(|_| "Remote sync settings are invalid")?,
    )
}

pub(crate) fn clear_cached_sessions_fenced(app: &AppHandle) -> Result<(), String> {
    let _config = lock_host_config();
    let mut records = read_records(app)?;
    let root = directory(app)?;
    for directory in [root.join("transcripts"), root.join("staging")] {
        if directory.exists() {
            std::fs::remove_dir_all(&directory)
                .map_err(|_| "Could not clear cached remote sessions".to_owned())?;
        }
    }
    for record in &mut records {
        let snapshot = root.join(format!("{}.snapshot.json", record.id));
        if snapshot.exists() {
            std::fs::remove_file(snapshot)
                .map_err(|_| "Could not clear cached remote sessions".to_owned())?;
        }
        record.cached_session_count = 0;
        record.last_successful_sync_epoch = None;
        record.last_error = None;
    }
    write_records(app, &records)?;
    emit_hosts(app);
    Ok(())
}

fn safe_scan_error(error: &anyhow::Error) -> RemoteHostError {
    let text = error.to_string();
    if text.contains("14 GiB") || text.contains("2 GiB") {
        RemoteHostError {
            category: RemoteErrorCategory::CacheLimit,
            message: "Remote session cache is full".into(),
        }
    } else if text == "scan cancelled" {
        RemoteHostError {
            category: RemoteErrorCategory::Cancelled,
            message: "Remote scan was cancelled".into(),
        }
    } else {
        RemoteHostError {
            category: RemoteErrorCategory::TransferFailed,
            message: "Some remote sessions could not be synced".into(),
        }
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct ScanSummary {
    attempted: usize,
    imported: usize,
    unchanged: usize,
    failed: usize,
}

async fn sync_snapshot<F, Fut, P>(
    count: usize,
    summary: &mut ScanSummary,
    mut sync: F,
    mut progress: P,
) -> anyhow::Result<()>
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<crate::remote_cache::SyncResult>>,
    P: FnMut(usize, usize),
{
    for index in 0..count {
        summary.attempted += 1;
        match sync(index).await {
            Ok(crate::remote_cache::SyncResult::Imported) => summary.imported += 1,
            Ok(crate::remote_cache::SyncResult::Unchanged) => summary.unchanged += 1,
            Err(error) if error.is::<transport::SessionRejected>() => summary.failed += 1,
            Err(error) => return Err(error),
        }
        progress(summary.attempted, count);
    }
    Ok(())
}

fn scan_error<T>(result: &anyhow::Result<T>, summary: &ScanSummary) -> Option<RemoteHostError> {
    match result {
        Err(error) => Some(safe_scan_error(error)),
        Ok(_) if summary.failed > 0 => Some(RemoteHostError {
            category: RemoteErrorCategory::TransferFailed,
            message: format!(
                "{} of {} sessions could not be synced",
                summary.failed, summary.attempted
            ),
        }),
        Ok(_) => None,
    }
}

fn update_scan_record(
    record: &mut HostRecord,
    error: Option<RemoteHostError>,
    saved_count: u32,
    now: i64,
) {
    if error.is_none() {
        record.last_successful_sync_epoch = Some(now);
    }
    record.last_error = error;
    record.cached_session_count = saved_count;
}

pub(crate) async fn perform_scan(
    app: &AppHandle,
    host_id: &str,
    generation: u64,
    origin: crate::remote_sync::ScanOrigin,
) -> anyhow::Result<()> {
    let record = read_records(app)
        .map_err(anyhow::Error::msg)?
        .into_iter()
        .find(|record| record.id == host_id)
        .ok_or_else(|| anyhow::anyhow!("scan cancelled"))?;
    if !record.automatic_sync_enabled {
        return Ok(());
    }
    emit_hosts(app);
    let mut summary = ScanSummary::default();
    let result = async {
        let bytes = transport::request(
            &record.ssh_alias,
            &Request::List {
                version: PROTOCOL_VERSION,
            },
        )
        .await?;
        let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
        snapshot.validate()?;
        let root = directory(app).map_err(anyhow::Error::msg)?;
        let store = app.state::<crate::store::Store>();
        sync_snapshot(
            snapshot.sessions.len(),
            &mut summary,
            |index| {
                let session = &snapshot.sessions[index];
                let root = &root;
                let store = &store;
                let alias = &record.ssh_alias;
                async move {
                    anyhow::ensure!(
                        crate::remote_sync::is_current(app, host_id, generation),
                        "scan cancelled"
                    );
                    crate::remote_cache::sync_session(
                        app, root, host_id, alias, generation, session, store,
                    )
                    .await
                }
            },
            |completed, total| crate::remote_sync::progress(app, host_id, completed, total),
        )
        .await?;
        anyhow::ensure!(
            crate::remote_sync::is_current(app, host_id, generation),
            "scan cancelled"
        );
        Ok::<(Snapshot, Vec<u8>, PathBuf), anyhow::Error>((snapshot, bytes, root))
    }
    .await;

    let error = scan_error(&result, &summary);
    let succeeded = error.is_none();
    if summary.imported > 0 {
        crate::insights_worker::wake(app);
        crate::session_lifecycle::report(
            app,
            crate::session_lifecycle::SyncObservation::IndexChanged {
                reason: crate::session_lifecycle::IndexChangeReason::ScanPass,
            },
        );
    }
    let cached_sessions = crate::remote_sync::with_commit_guard(app, host_id, generation, || {
        let _config = lock_host_config();
        if let Ok((_, bytes, root)) = &result
            && succeeded
        {
            write_private(&root.join(format!("{host_id}.snapshot.json")), bytes)
                .map_err(anyhow::Error::msg)?;
        }
        let mut records = read_records(app).map_err(anyhow::Error::msg)?;
        let current = records
            .iter_mut()
            .find(|item| item.id == host_id)
            .ok_or_else(|| anyhow::anyhow!("scan cancelled"))?;
        let saved_count = app
            .state::<crate::store::Store>()
            .remote_session_counts()?
            .get(host_id)
            .copied()
            .unwrap_or(0);
        update_scan_record(
            current,
            error.clone(),
            saved_count,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        );
        let cached_sessions = current.cached_session_count as usize;
        write_records(app, &records).map_err(anyhow::Error::msg)?;
        Ok(cached_sessions)
    })?;
    crate::analytics::record_remote_sync_completed(
        app,
        if succeeded {
            crate::analytics::event::RemoteSyncOutcome::Succeeded
        } else {
            crate::analytics::event::RemoteSyncOutcome::Failed
        },
        origin,
        cached_sessions,
    );
    emit_hosts(app);
    result?;
    anyhow::ensure!(succeeded, "Some remote sessions could not be synced");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejected_sessions_do_not_starve_later_imports() {
        use crate::remote_cache::SyncResult::{Imported, Unchanged};
        for results in [
            vec![Err(transport::SessionRejected.into()), Ok(Imported)],
            vec![
                Ok(Imported),
                Err(transport::SessionRejected.into()),
                Ok(Imported),
            ],
            vec![
                Ok(Unchanged),
                Err(transport::SessionRejected.into()),
                Ok(Imported),
            ],
        ] {
            let total = results.len();
            let mut results = results.into_iter();
            let mut summary = ScanSummary::default();
            let mut progress = Vec::new();
            sync_snapshot(
                total,
                &mut summary,
                |_| std::future::ready(results.next().unwrap()),
                |done, count| progress.push((done, count)),
            )
            .await
            .unwrap();
            assert_eq!(summary.attempted, total);
            assert_eq!(summary.failed, 1);
            assert!(summary.imported > 0);
            assert_eq!(progress.last(), Some(&(total, total)));
        }
    }

    #[tokio::test]
    async fn malformed_successful_bundle_stops_scan_and_preserves_earlier_import() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("malformed.bundle");
        let expected = antiburn_remote::RemoteSession {
            agent: "codex".into(),
            session_id: "expected".into(),
            title: "test".into(),
            cwd: None,
            surface: "cli".into(),
            updated_at: None,
        };
        for bytes in [b"garbage".as_slice(), b"ABR2DATA\x02\0\0\0{}".as_slice()] {
            std::fs::write(&bundle, bytes).unwrap();
            let protocol_error =
                crate::remote_cache::unpack(&bundle, temp.path(), &expected).unwrap_err();
            assert!(!protocol_error.is::<transport::SessionRejected>());
            let mut outcomes = vec![
                Ok(crate::remote_cache::SyncResult::Imported),
                Err(protocol_error),
                Ok(crate::remote_cache::SyncResult::Imported),
            ]
            .into_iter();
            let mut summary = ScanSummary::default();
            assert!(
                sync_snapshot(
                    3,
                    &mut summary,
                    |_| std::future::ready(outcomes.next().unwrap()),
                    |_, _| {}
                )
                .await
                .is_err()
            );
            assert_eq!(summary.imported, 1);
            assert_eq!(summary.attempted, 2);
            assert_eq!(outcomes.count(), 1);
        }
    }

    #[test]
    fn corrupt_host_records_do_not_block_lifecycle_operations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hosts.json");
        std::fs::write(&path, "invalid json").unwrap();
        assert!(read_records_from(&path).is_err());
        assert!(lifecycle_ids(read_records_from(&path)).is_empty());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(read_records_from(&path).is_err());
        assert!(lifecycle_ids(read_records_from(&path)).is_empty());
    }

    #[test]
    fn partial_scan_preserves_success_time_and_retry_clears_error() {
        let mut host = record("host", "test");
        host.last_successful_sync_epoch = Some(10);
        host.cached_session_count = 99;
        let summary = ScanSummary {
            attempted: 3,
            imported: 2,
            unchanged: 0,
            failed: 1,
        };
        update_scan_record(&mut host, scan_error(&Ok(()), &summary), 2, 20);
        assert_eq!(host.last_successful_sync_epoch, Some(10));
        assert_eq!(host.cached_session_count, 2);
        assert_eq!(
            host.last_error.as_ref().unwrap().message,
            "1 of 3 sessions could not be synced"
        );
        update_scan_record(
            &mut host,
            scan_error(&Ok(()), &ScanSummary::default()),
            3,
            30,
        );
        assert_eq!(host.last_successful_sync_epoch, Some(30));
        assert_eq!(host.cached_session_count, 3);
        assert!(host.last_error.is_none());
    }

    #[test]
    fn failed_only_and_empty_scans_have_distinct_status() {
        let mut host = record("host", "test");
        let failures = ScanSummary {
            attempted: 2,
            failed: 2,
            ..ScanSummary::default()
        };
        update_scan_record(&mut host, scan_error(&Ok(()), &failures), 1, 10);
        assert!(host.last_successful_sync_epoch.is_none());
        assert!(host.last_error.is_some());
        update_scan_record(
            &mut host,
            scan_error(&Ok(()), &ScanSummary::default()),
            1,
            20,
        );
        assert_eq!(host.last_successful_sync_epoch, Some(20));
        assert_eq!(host.cached_session_count, 1);
        assert!(host.last_error.is_none());
    }

    #[tokio::test]
    async fn transport_storage_and_cancellation_stop_without_losing_committed_progress() {
        for failure in ["SSH failed", "disk full", "scan cancelled"] {
            let mut results = vec![
                Ok(crate::remote_cache::SyncResult::Imported),
                Err(anyhow::anyhow!(failure)),
                Ok(crate::remote_cache::SyncResult::Imported),
            ]
            .into_iter();
            let mut summary = ScanSummary::default();
            assert!(
                sync_snapshot(
                    3,
                    &mut summary,
                    |_| std::future::ready(results.next().unwrap()),
                    |_, _| {}
                )
                .await
                .is_err()
            );
            assert_eq!(summary.imported, 1);
            assert_eq!(summary.attempted, 2);
            assert_eq!(results.count(), 1);
        }
    }

    #[tokio::test]
    async fn empty_unchanged_and_repaired_scans_finish_cleanly() {
        for (count, outcome) in [
            (0, crate::remote_cache::SyncResult::Imported),
            (2, crate::remote_cache::SyncResult::Unchanged),
            (2, crate::remote_cache::SyncResult::Imported),
        ] {
            let mut summary = ScanSummary::default();
            sync_snapshot(
                count,
                &mut summary,
                |_| std::future::ready(Ok(outcome)),
                |_, _| {},
            )
            .await
            .unwrap();
            assert_eq!(summary.failed, 0);
            assert_eq!(summary.imported + summary.unchanged, count);
        }
    }

    #[test]
    fn partial_summary_and_fatal_diagnostics_do_not_expose_private_text() {
        let fatal: anyhow::Result<()> = Err(anyhow::anyhow!("secret prompt /private/path"));
        let error = scan_error(&fatal, &ScanSummary::default()).unwrap();
        assert_eq!(error.message, "Some remote sessions could not be synced");
    }

    fn record(id: &str, alias: &str) -> HostRecord {
        HostRecord {
            id: id.into(),
            ssh_alias: alias.into(),
            display_name: None,
            automatic_sync_enabled: true,
            last_successful_sync_epoch: None,
            cached_session_count: 0,
            last_error: None,
        }
    }

    #[test]
    fn legacy_hosts_sync_by_default_and_pause_survives_persistence() {
        let mut value =
            serde_json::to_value(record("11111111-1111-4111-8111-111111111111", "orb")).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("automaticSyncEnabled");
        let mut host: HostRecord = serde_json::from_value(value).unwrap();
        assert!(host.automatic_sync_enabled);
        host.automatic_sync_enabled = false;
        host.cached_session_count = 2;
        let saved = serde_json::to_vec(&host).unwrap();
        let restored: HostRecord = serde_json::from_slice(&saved).unwrap();
        assert!(!restored.automatic_sync_enabled);
        assert_eq!(restored.cached_session_count, 2);
        assert_eq!(restored.ssh_alias, "orb");
    }

    #[test]
    fn records_enforce_uuid_alias_uniqueness_and_cap() {
        let one = "11111111-1111-4111-8111-111111111111";
        assert!(validate_records(&[record(one, "build-box")]).is_ok());
        assert!(validate_records(&[record(one, "one"), record(one, "two")]).is_err());
        assert!(
            validate_records(&[
                record("11111111-1111-4111-8111-111111111111", "Same"),
                record("22222222-2222-4222-8222-222222222222", "same"),
            ])
            .is_err()
        );
        let records = (0..9)
            .map(|index| {
                record(
                    &format!("{index:08x}-1111-4111-8111-111111111111"),
                    &format!("host-{index}"),
                )
            })
            .collect::<Vec<_>>();
        assert!(validate_records(&records).is_err());
    }

    #[test]
    fn concurrent_host_transactions_preserve_both_additions() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hosts.json");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for (id, alias) in [
            ("11111111-1111-4111-8111-111111111111", "one"),
            ("22222222-2222-4222-8222-222222222222", "two"),
        ] {
            let path = path.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let _guard = lock_host_config();
                let mut records = read_records_from(&path).unwrap();
                records.push(record(id, alias));
                write_records_to(&path, &records).unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let records = read_records_from(&path).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn a_status_transaction_cannot_restore_a_removed_host() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("hosts.json");
        let id = "11111111-1111-4111-8111-111111111111";
        write_records_to(&path, &[record(id, "one")]).unwrap();
        {
            let _guard = lock_host_config();
            let mut records = read_records_from(&path).unwrap();
            records.retain(|item| item.id != id);
            write_records_to(&path, &records).unwrap();
        }
        {
            let _guard = lock_host_config();
            let mut records = read_records_from(&path).unwrap();
            if let Some(current) = records.iter_mut().find(|item| item.id == id) {
                current.last_successful_sync_epoch = Some(1);
                write_records_to(&path, &records).unwrap();
            }
        }
        assert!(read_records_from(&path).unwrap().is_empty());
    }

    #[test]
    fn a_preflight_reports_the_helper_version_to_the_interface() {
        // Two contracts in one assertion. The interface reads `helperVersion`,
        // so a renamed field serializes to null and fails here. The helper also
        // ships as an asset of this release, so its version must equal this
        // crate's; a drifted manifest fails here instead of at the release tag.
        let hello = antiburn_remote::Hello::current();
        let ready = RemoteHostPreflight {
            status: "ready".into(),
            supported_agents: hello.supported_agents,
            platform: Some(hello.platform),
            architecture: Some(hello.architecture),
            helper_version: Some(hello.helper_version),
            message: None,
        };
        let encoded = serde_json::to_value(&ready).unwrap();
        assert_eq!(encoded["helperVersion"], env!("CARGO_PKG_VERSION"));
    }
}

//! Storage-neutral source identity and version values.

use super::SessionSource;
use crate::model::AgentKind;
use crate::platform::environment::DiscoveryEnvironment;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const FINGERPRINT_HEAD_BYTES: usize = 64 * 1024;

pub(crate) fn provider_db_fingerprint(latest: u64, rows: u64) -> String {
    format!("sv1:db:{latest}:{rows}")
}

#[derive(Debug, Clone)]
pub struct SourceDescriptor {
    pub agent: AgentKind,
    pub session_id: String,
    pub environment: DiscoveryEnvironment,
    pub source: SessionSource,
    pub updated_at_epoch: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceVersion {
    pub fingerprint: String,
    pub estimated_bytes: Option<u64>,
    pub streamability: Streamability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Streamability {
    RecordStream,
    DatabaseRows,
    WholeDocumentFallback,
    InlineMaterialized,
}

impl super::Explorers {
    pub async fn source_version(
        &self,
        descriptor: &SourceDescriptor,
        read: Option<&super::SourceRead>,
    ) -> Option<SourceVersion> {
        match &descriptor.source {
            SessionSource::File(_) => {
                let read = read?;
                let stat = read.stat.clone()?;
                let estimated_bytes = Some(stat.size);
                let fingerprint = FingerprintInputs {
                    stat,
                    head_hash: read.head_hash,
                }
                .fingerprint();
                let streamability =
                    if matches!(descriptor.agent, AgentKind::Claude | AgentKind::Codex) {
                        Streamability::RecordStream
                    } else {
                        Streamability::WholeDocumentFallback
                    };
                Some(SourceVersion {
                    fingerprint,
                    estimated_bytes,
                    streamability,
                })
            }
            SessionSource::ProviderDb {
                agent,
                db_path,
                session_id,
            } => {
                let (latest, rows) = self
                    .provider_db_fingerprint(agent, db_path, session_id)
                    .await?;
                Some(SourceVersion {
                    fingerprint: provider_db_fingerprint(latest, rows),
                    estimated_bytes: None,
                    streamability: Streamability::DatabaseRows,
                })
            }
            SessionSource::Inline { content, .. } => {
                let stat = SourceStat {
                    identity: None,
                    size: content.len() as u64,
                    modified_nanos: None,
                    changed_nanos: None,
                };
                Some(SourceVersion {
                    fingerprint: FingerprintInputs {
                        stat,
                        head_hash: Some(head_hash_of(content.as_bytes())),
                    }
                    .fingerprint(),
                    estimated_bytes: Some(content.len() as u64),
                    streamability: Streamability::InlineMaterialized,
                })
            }
        }
    }
}

/// Identity and time inputs from an open handle or a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceStat {
    pub identity: Option<String>,
    pub size: u64,
    pub modified_nanos: Option<i128>,
    pub changed_nanos: Option<i128>,
}

impl SourceStat {
    pub async fn from_open_file(file: &tokio::fs::File) -> Option<Self> {
        let metadata = file.metadata().await.ok()?;
        Some(Self::from_open_metadata(file, &metadata))
    }

    pub async fn from_path(path: &Path) -> Option<Self> {
        let metadata = tokio::fs::metadata(path).await.ok()?;
        Some(Self::from_path_metadata(&metadata))
    }

    pub fn from_open_std_file(file: &std::fs::File) -> Option<Self> {
        let metadata = file.metadata().ok()?;
        Some(Self::from_open_std_metadata(file, &metadata))
    }

    #[cfg(unix)]
    fn from_open_metadata(_file: &tokio::fs::File, metadata: &std::fs::Metadata) -> Self {
        Self::from_unix_metadata(metadata)
    }

    #[cfg(windows)]
    fn from_open_metadata(file: &tokio::fs::File, metadata: &std::fs::Metadata) -> Self {
        use std::os::windows::io::AsRawHandle;

        Self::from_windows_handle(file.as_raw_handle(), metadata)
    }

    #[cfg(unix)]
    fn from_open_std_metadata(_file: &std::fs::File, metadata: &std::fs::Metadata) -> Self {
        Self::from_unix_metadata(metadata)
    }

    #[cfg(windows)]
    fn from_open_std_metadata(file: &std::fs::File, metadata: &std::fs::Metadata) -> Self {
        use std::os::windows::io::AsRawHandle;

        Self::from_windows_handle(file.as_raw_handle(), metadata)
    }

    #[cfg(windows)]
    fn from_windows_handle(
        handle: std::os::windows::io::RawHandle,
        metadata: &std::fs::Metadata,
    ) -> Self {
        use std::mem::MaybeUninit;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx,
        };

        let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        let mut basic = MaybeUninit::<FILE_BASIC_INFO>::zeroed();
        // SAFETY: the file owns the valid handle, and both output buffers match the requested types.
        let (info_ok, basic_ok) = unsafe {
            (
                GetFileInformationByHandle(handle, info.as_mut_ptr()),
                GetFileInformationByHandleEx(
                    handle,
                    FileBasicInfo,
                    basic.as_mut_ptr().cast(),
                    std::mem::size_of::<FILE_BASIC_INFO>() as u32,
                ),
            )
        };
        let identity = (info_ok != 0).then(|| {
            // SAFETY: GetFileInformationByHandle initialized the buffer after it returned success.
            let info = unsafe { info.assume_init() };
            let file_index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
            format!("{}:{file_index}", info.dwVolumeSerialNumber)
        });
        let changed_nanos = (basic_ok != 0)
            .then(|| {
                // SAFETY: GetFileInformationByHandleEx initialized the buffer after it returned success.
                let basic = unsafe { basic.assume_init() };
                windows_change_time_to_unix_nanos(basic.ChangeTime)
            })
            .flatten();
        Self {
            identity,
            size: metadata.len(),
            modified_nanos: metadata.modified().ok().and_then(system_time_nanos),
            changed_nanos,
        }
    }

    #[cfg(unix)]
    fn from_path_metadata(metadata: &std::fs::Metadata) -> Self {
        Self::from_unix_metadata(metadata)
    }

    #[cfg(windows)]
    fn from_path_metadata(metadata: &std::fs::Metadata) -> Self {
        Self {
            identity: None,
            size: metadata.len(),
            modified_nanos: metadata.modified().ok().and_then(system_time_nanos),
            changed_nanos: None,
        }
    }

    #[cfg(unix)]
    fn from_unix_metadata(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;

        Self {
            identity: Some(format!("{}:{}", metadata.dev(), metadata.ino())),
            size: metadata.len(),
            modified_nanos: metadata.modified().ok().and_then(system_time_nanos),
            changed_nanos: Some(
                i128::from(metadata.ctime()) * 1_000_000_000 + i128::from(metadata.ctime_nsec()),
            ),
        }
    }
}

/// Fingerprint inputs are separate from the filesystem for deterministic tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintInputs {
    pub stat: SourceStat,
    pub head_hash: Option<u64>,
}

impl FingerprintInputs {
    pub fn fingerprint(&self) -> String {
        format!(
            "sv1:{}:{}:{}:{}:{}",
            self.stat.identity.as_deref().unwrap_or("-"),
            self.stat.size,
            optional_i128(self.stat.modified_nanos),
            optional_i128(self.stat.changed_nanos),
            self.head_hash
                .map(|hash| format!("{hash:016x}"))
                .unwrap_or_else(|| "-".to_string())
        )
    }
}

pub fn head_hash_of(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x100_0000_01b3;

    bytes
        .iter()
        .take(FINGERPRINT_HEAD_BYTES)
        .fold(OFFSET_BASIS, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
        })
}

fn optional_i128(value: Option<i128>) -> String {
    value.map_or_else(|| "-".to_string(), |value| value.to_string())
}

fn system_time_nanos(time: SystemTime) -> Option<i128> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i128::try_from(duration.as_nanos()).ok(),
        Err(error) => i128::try_from(error.duration().as_nanos())
            .ok()
            .map(|nanos| -nanos),
    }
}

#[cfg(windows)]
fn windows_change_time_to_unix_nanos(change_time: i64) -> Option<i128> {
    if change_time == 0 {
        return None;
    }
    Some((i128::from(change_time) - 116_444_736_000_000_000i128) * 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixed_inputs(bytes: &[u8]) -> FingerprintInputs {
        FingerprintInputs {
            stat: SourceStat {
                identity: Some("device:file".to_string()),
                size: 70_001,
                modified_nanos: Some(100),
                changed_nanos: Some(200),
            },
            head_hash: Some(head_hash_of(bytes)),
        }
    }

    #[test]
    fn the_head_hash_matches_the_canonical_fnv1a64_vectors() {
        assert_eq!(format!("{:016x}", head_hash_of(b"")), "cbf29ce484222325");
        assert_eq!(format!("{:016x}", head_hash_of(b"a")), "af63dc4c8601ec8c");
        assert_eq!(
            format!("{:016x}", head_hash_of(b"foobar")),
            "85944171f73967e8"
        );
    }

    #[test]
    fn a_rewrite_inside_the_head_region_changes_the_head_hash_component() {
        let bytes = vec![b'a'; 70_001];
        let mut rewritten = bytes.clone();
        rewritten[100] = b'b';

        assert_ne!(
            fixed_inputs(&bytes).fingerprint(),
            fixed_inputs(&rewritten).fingerprint()
        );
    }

    #[test]
    fn a_rewrite_below_the_head_region_leaves_the_head_hash_component() {
        let bytes = vec![b'a'; 70_001];
        let mut rewritten = bytes.clone();
        rewritten[70_000] = b'b';

        assert_eq!(
            fixed_inputs(&bytes).fingerprint(),
            fixed_inputs(&rewritten).fingerprint()
        );
    }

    #[test]
    fn an_absent_component_renders_a_placeholder() {
        let inputs = FingerprintInputs {
            stat: SourceStat {
                identity: None,
                size: 12,
                modified_nanos: None,
                changed_nanos: None,
            },
            head_hash: None,
        };

        assert_eq!(inputs.fingerprint(), "sv1:-:12:-:-:-");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_source_stat_reports_identity_and_change_time() {
        let dir = TempDir::new().expect("tempdir");
        let first_path = dir.path().join("first.jsonl");
        let second_path = dir.path().join("second.jsonl");
        tokio::fs::write(&first_path, b"first")
            .await
            .expect("write first");
        tokio::fs::write(&second_path, b"second")
            .await
            .expect("write second");
        let first_file = tokio::fs::File::open(&first_path)
            .await
            .expect("open first");
        let second_file = tokio::fs::File::open(&second_path)
            .await
            .expect("open second");

        let first = SourceStat::from_open_file(&first_file)
            .await
            .expect("first stat");
        let first_again = SourceStat::from_open_file(&first_file)
            .await
            .expect("second stat");
        let second = SourceStat::from_open_file(&second_file)
            .await
            .expect("other stat");

        assert!(
            first
                .identity
                .as_deref()
                .is_some_and(|value| value.contains(':'))
        );
        assert!(first.changed_nanos.is_some());
        assert_eq!(first, first_again);
        assert_ne!(first.identity, second.identity);
    }

    fn descriptor(agent: AgentKind, source: SessionSource) -> SourceDescriptor {
        SourceDescriptor {
            agent,
            session_id: "session-1".to_string(),
            environment: DiscoveryEnvironment::default(),
            source,
            updated_at_epoch: Some(100),
        }
    }

    #[tokio::test]
    async fn source_version_for_a_file_reports_size_and_record_stream() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        let content = b"{\"session_id\":\"session-1\"}\n";
        tokio::fs::write(&path, content)
            .await
            .expect("write source");
        let log = super::super::SessionLog {
            agent_type: AgentKind::Claude,
            source: SessionSource::File(path),
            updated_at: Some(100),
            environment: DiscoveryEnvironment::default(),
        };
        let read = super::super::session_log_read(&log)
            .await
            .expect("source read");
        let descriptor = descriptor(log.agent_type, log.source);

        let version = super::super::Explorers::DISK
            .source_version(&descriptor, Some(&read))
            .await
            .expect("source version");

        assert_eq!(version.estimated_bytes, Some(content.len() as u64));
        assert_eq!(version.streamability, Streamability::RecordStream);
        assert!(version.fingerprint.starts_with("sv1:"));
    }

    #[tokio::test]
    async fn source_version_for_a_codex_file_reports_a_record_stream() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        tokio::fs::write(&path, b"{}\n")
            .await
            .expect("write source");
        let log = super::super::SessionLog {
            agent_type: AgentKind::Codex,
            source: SessionSource::File(path),
            updated_at: Some(100),
            environment: DiscoveryEnvironment::default(),
        };
        let read = super::super::session_log_read(&log)
            .await
            .expect("source read");
        let descriptor = descriptor(log.agent_type, log.source);

        let version = super::super::Explorers::DISK
            .source_version(&descriptor, Some(&read))
            .await
            .expect("source version");

        assert_eq!(version.streamability, Streamability::RecordStream);
    }

    #[tokio::test]
    async fn source_version_is_none_when_the_source_cannot_be_read() {
        let descriptor = descriptor(
            AgentKind::Claude,
            SessionSource::File(std::path::PathBuf::from("/missing/session.jsonl")),
        );

        assert!(
            super::super::Explorers::DISK
                .source_version(&descriptor, None)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_inline_source_is_fingerprinted_from_its_content() {
        let content = "synthetic inline session";
        let descriptor = descriptor(
            AgentKind::Claude,
            SessionSource::Inline {
                label: "inline-1".to_string(),
                content: content.to_string(),
            },
        );

        let version = super::super::Explorers::DISK
            .source_version(&descriptor, None)
            .await
            .expect("source version");

        assert_eq!(version.estimated_bytes, Some(content.len() as u64));
        assert_eq!(version.streamability, Streamability::InlineMaterialized);
        assert_eq!(
            version.fingerprint,
            format!(
                "sv1:-:{}:-:-:{:016x}",
                content.len(),
                head_hash_of(content.as_bytes())
            )
        );
    }

    #[tokio::test]
    async fn a_provider_db_source_reuses_the_provider_fingerprint() {
        let dir = TempDir::new().expect("tempdir");
        let db_path = dir.path().join("opencode.db");
        let connection = rusqlite::Connection::open(&db_path).expect("database");
        connection
            .execute_batch(
                "CREATE TABLE session (
                     id TEXT PRIMARY KEY, parent_id TEXT,
                     time_created INTEGER, time_updated INTEGER);
                 CREATE TABLE message (
                     session_id TEXT, time_created INTEGER, time_updated INTEGER);
                 CREATE TABLE part (
                     session_id TEXT, time_created INTEGER, time_updated INTEGER);
                 INSERT INTO session VALUES ('session-1', NULL, 100, 120);",
            )
            .expect("schema");
        drop(connection);
        let descriptor = descriptor(
            AgentKind::OpenCode,
            SessionSource::ProviderDb {
                agent: AgentKind::OpenCode,
                db_path,
                session_id: "session-1".to_string(),
            },
        );

        let version = super::super::Explorers::DISK
            .source_version(&descriptor, None)
            .await
            .expect("source version");

        assert_eq!(version.fingerprint, "sv1:db:120:1");
        assert_eq!(version.estimated_bytes, None);
        assert_eq!(version.streamability, Streamability::DatabaseRows);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_source_stat_reports_identity_and_change_time() {
        use std::mem::MaybeUninit;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx,
        };

        let dir = TempDir::new().expect("tempdir");
        let first_path = dir.path().join("first.jsonl");
        let second_path = dir.path().join("second.jsonl");
        tokio::fs::write(&first_path, b"first")
            .await
            .expect("write first");
        tokio::fs::write(&second_path, b"second")
            .await
            .expect("write second");
        let first_file = tokio::fs::File::open(&first_path)
            .await
            .expect("open first");
        let second_file = tokio::fs::File::open(&second_path)
            .await
            .expect("open second");

        let first = SourceStat::from_open_file(&first_file)
            .await
            .expect("first stat");
        let first_again = SourceStat::from_open_file(&first_file)
            .await
            .expect("second stat");
        let second = SourceStat::from_open_file(&second_file)
            .await
            .expect("other stat");

        let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
        let mut basic = MaybeUninit::<FILE_BASIC_INFO>::zeroed();
        // SAFETY: the file owns the valid handle, and both output buffers match the requested types.
        let (info_ok, basic_ok) = unsafe {
            (
                GetFileInformationByHandle(first_file.as_raw_handle(), info.as_mut_ptr()),
                GetFileInformationByHandleEx(
                    first_file.as_raw_handle(),
                    FileBasicInfo,
                    basic.as_mut_ptr().cast(),
                    std::mem::size_of::<FILE_BASIC_INFO>() as u32,
                ),
            )
        };
        assert_ne!(info_ok, 0);
        assert_ne!(basic_ok, 0);
        // SAFETY: both handle queries initialized their output buffers after they returned success.
        let (info, basic) = unsafe { (info.assume_init(), basic.assume_init()) };
        let file_index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        let expected_identity = format!("{}:{file_index}", info.dwVolumeSerialNumber);

        assert_eq!(first.identity.as_deref(), Some(expected_identity.as_str()));
        assert_eq!(
            first.changed_nanos,
            windows_change_time_to_unix_nanos(basic.ChangeTime)
        );
        assert_eq!(first, first_again);
        assert_ne!(first.identity, second.identity);
        assert_eq!(windows_change_time_to_unix_nanos(0), None);
        assert!(windows_change_time_to_unix_nanos(1).is_some_and(|value| value < 0));
    }
}

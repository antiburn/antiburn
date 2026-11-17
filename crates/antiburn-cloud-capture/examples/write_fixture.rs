//! Regenerate public wire references from synthetic repository inputs only.

use antiburn_cloud_capture::*;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// Capture each synthetic package through the real spool without network I/O.
fn main() -> Result<()> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/cloud/capture-v1")
        .canonicalize()?;
    let candidate = discover_claude(&fixtures.join("project"), &Limits::default())?
        .into_iter()
        .find(|candidate| candidate.source_id() == "main")
        .ok_or(Error::InvalidSource)?;
    let temporary = tempfile::tempdir_in(std::env::temp_dir().canonicalize()?)?;
    let mut spool = Spool::open(&temporary.path().join("spool"), Limits::default())?;
    let entry = spool.capture(
        CaptureIdentity {
            capture_id: "fixture-main".into(),
            account_id: "synthetic-account".into(),
            destination_id: "synthetic-destination".into(),
            source_id: "main".into(),
        },
        &candidate,
        serde_json::from_slice(&fs::read(fixtures.join("sections.json"))?)?,
        &mut Reservation,
    )?;
    let reference = fixtures.join("reference");
    fs::create_dir_all(&reference)?;
    let manifest = entry.manifest.ok_or(Error::CorruptSpool)?;
    fs::write(
        reference.join("main.capture.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    let mut paths: Vec<String> = [
        "project/main.jsonl",
        "project/fork.jsonl",
        "project/unknown.jsonl",
        "project/main/subagents/agent-worker.jsonl",
        "project/main/subagents/agent-worker.meta.json",
        "project/main/subagents/agent-fork.jsonl",
        "project/main/subagents/agent-fork.meta.json",
        "project/main/subagents/agent-inherited-only.jsonl",
        "project/main/subagents/agent-inherited-only.meta.json",
        "project/main/subagents/agent-replay-only.jsonl",
        "project/main/subagents/agent-replay-only.meta.json",
        "project/main/subagents/agent-conflicting-fork.jsonl",
        "project/main/subagents/agent-conflicting-fork.meta.json",
        "project/audit.jsonl",
        "project/unowned.txt",
        "reference/main.capture.json",
        "sections.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for (key, _) in manifest.chunks() {
        let source = format!("chunk-{}-{:05}.gz", key.stream_id, key.index);
        let reference_name = format!("main.{}.chunk-{:05}.gz", key.stream_id, key.index);
        fs::copy(
            temporary.path().join("spool/fixture-main").join(source),
            reference.join(&reference_name),
        )?;
        paths.push(format!("reference/{reference_name}"));
    }
    let mut files = Vec::new();
    for path in paths {
        let bytes = fs::read(fixtures.join(&path))?;
        files.push(serde_json::json!({
            "path": path,
            "bytes": bytes.len(),
            "sha256": Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        }));
    }
    fs::write(
        fixtures.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "fixture_version": FIXTURE_VERSION,
            "capture_version": CAPTURE_VERSION,
            "synthetic": true,
            "files": files,
        }))?,
    )?;
    #[cfg(unix)]
    write_opencode_fixture()?;
    Ok(())
}

/// Keep SQLite references independent of temporary roots and physical database bytes.
#[cfg(unix)]
fn write_opencode_fixture() -> Result<()> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/cloud/opencode")
        .canonicalize()?;
    let temporary = tempfile::tempdir_in(std::env::temp_dir().canonicalize()?)?;
    let database = temporary.path().join("opencode.db");
    let connection = rusqlite::Connection::open(&database).map_err(|_| Error::InvalidSource)?;
    connection
        .execute_batch(&fs::read_to_string(fixtures.join("current.sql"))?)
        .map_err(|_| Error::InvalidSource)?;
    drop(connection);
    let candidate = discover_opencode(&database, &Limits::default())?
        .into_iter()
        .find(|candidate| candidate.native_session_id() == "ses_main")
        .ok_or(Error::InvalidSource)?
        .in_source_scope("synthetic-root")?;
    let mut spool = Spool::open(&temporary.path().join("spool"), Limits::default())?;
    let entry = spool.capture(
        CaptureIdentity {
            capture_id: "fixture-opencode-main".into(),
            account_id: "synthetic-account".into(),
            destination_id: "synthetic-destination".into(),
            source_id: candidate.source_id().into(),
        },
        &candidate,
        MetadataSections::new(),
        &mut Reservation,
    )?;
    let manifest = entry.manifest.ok_or(Error::CorruptSpool)?;
    let reference = fixtures.join("reference");
    fs::create_dir_all(&reference)?;
    fs::write(
        reference.join("main.capture.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    let mut paths: Vec<String> = [
        "README.md",
        "earlier.sql",
        "current.sql",
        "reference/main.capture.json",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for (key, _) in manifest.chunks() {
        let source = format!("chunk-{}-{:05}.gz", key.stream_id, key.index);
        let reference_name = format!("main.{}.chunk-{:05}.gz", key.stream_id, key.index);
        fs::copy(
            temporary
                .path()
                .join("spool/fixture-opencode-main")
                .join(source),
            reference.join(&reference_name),
        )?;
        paths.push(format!("reference/{reference_name}"));
    }
    let mut files = Vec::new();
    for path in paths {
        let bytes = fs::read(fixtures.join(&path))?;
        files.push(serde_json::json!({
            "path": path,
            "bytes": bytes.len(),
            "sha256": Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
        }));
    }
    fs::write(
        fixtures.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "fixture_version": "opencode-native-fixtures-v1",
            "capture_version": CAPTURE_VERSION,
            "synthetic": true,
            "files": files,
        }))?,
    )?;
    Ok(())
}

/// Reserve a synthetic ID only; every upload operation rejects without I/O.
struct Reservation;

impl Transport for Reservation {
    fn reserve(
        &mut self,
        _: &CaptureIdentity,
        _: u64,
    ) -> std::result::Result<String, TransportError> {
        Ok("synthetic-reservation".into())
    }

    fn negotiate(
        &mut self,
        _: &str,
        _: &CaptureManifest,
    ) -> std::result::Result<UploadPlan, TransportError> {
        Err(TransportError::Rejected)
    }

    fn status(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> std::result::Result<ServerStatus, TransportError> {
        Err(TransportError::Rejected)
    }

    fn upload_part(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
        _: &ChunkKey,
        _: &Chunk,
        _: &mut dyn std::io::Read,
    ) -> std::result::Result<(), TransportError> {
        Err(TransportError::Rejected)
    }

    fn complete(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> std::result::Result<(), TransportError> {
        Err(TransportError::Rejected)
    }
}

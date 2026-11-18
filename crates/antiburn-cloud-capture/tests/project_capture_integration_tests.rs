#![cfg(all(unix, feature = "cloud"))]

//! SQLite project permission checks at the desktop capture boundary.

use antiburn_cloud_capture::*;
use rusqlite::Connection;
use std::io::Read;
use std::path::PathBuf;

#[derive(Default)]
struct CaptureTransport {
    reserves: usize,
}

/// Permit reservation and reject network work that capture must not perform.
impl Transport for CaptureTransport {
    fn reserve(
        &mut self,
        _: &CaptureIdentity,
        _: u64,
    ) -> std::result::Result<String, TransportError> {
        self.reserves += 1;
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
        _: &mut dyn Read,
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

fn fixture() -> (tempfile::TempDir, PathBuf, Connection) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(include_str!("../../../fixtures/cloud/opencode/current.sql"))
        .unwrap();
    (temporary, path, connection)
}

#[test]
fn project_admission_rechecks_directory_in_the_payload_snapshot() {
    let (temporary, path, connection) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let project = root.join("project");
    let outside = root.join("outside");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&outside).unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
            [project.to_str().unwrap()],
        )
        .unwrap();
    let candidate = reopen_opencode(&path, "ses_main", &Limits::default()).unwrap();
    let admitted = ProjectSelection::new(vec![project])
        .unwrap()
        .admit(&candidate)
        .unwrap()
        .unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
            [outside.to_str().unwrap()],
        )
        .unwrap();
    let mut spool = Spool::open(&root.join("spool"), Limits::default()).unwrap();
    let mut transport = CaptureTransport::default();
    let identity = CaptureIdentity {
        account_id: "account".into(),
        destination_id: "destination".into(),
        source_id: admitted.source_id().into(),
        capture_id: "changed-project".into(),
    };
    assert!(matches!(
        spool.capture(identity, &admitted, MetadataSections::new(), &mut transport),
        Err(Error::SourceChanged)
    ));
    assert_eq!(transport.reserves, 1);
    assert!(spool.entries().unwrap()[0].manifest.is_none());
}

#[test]
fn project_admission_allows_sqlite_directory_changes_inside_the_approved_tree() {
    let (temporary, path, connection) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let project = root.join("project");
    let nested = project.join("nested");
    std::fs::create_dir_all(&nested).unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
            [project.to_str().unwrap()],
        )
        .unwrap();
    let candidate = reopen_opencode(&path, "ses_main", &Limits::default()).unwrap();
    let admitted = ProjectSelection::new(vec![project])
        .unwrap()
        .admit(&candidate)
        .unwrap()
        .unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
            [nested.to_str().unwrap()],
        )
        .unwrap();
    let mut spool = Spool::open(&root.join("spool"), Limits::default()).unwrap();
    let mut transport = CaptureTransport::default();
    let identity = CaptureIdentity {
        account_id: "account".into(),
        destination_id: "destination".into(),
        source_id: admitted.source_id().into(),
        capture_id: "changed-descendant".into(),
    };
    let entry = spool
        .capture(identity, &admitted, MetadataSections::new(), &mut transport)
        .unwrap();
    assert_eq!(entry.state, CaptureState::Queued);
    assert_eq!(transport.reserves, 1);
}

#[test]
fn project_admission_keeps_the_typed_sqlite_capture_unchanged() {
    let (temporary, path, connection) = fixture();
    let root = temporary.path().canonicalize().unwrap();
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory = ?1 WHERE id = 'ses_main'",
            [project.to_str().unwrap()],
        )
        .unwrap();
    let candidate = reopen_opencode(&path, "ses_main", &Limits::default()).unwrap();
    let admitted = ProjectSelection::new(vec![project])
        .unwrap()
        .admit(&candidate)
        .unwrap()
        .unwrap();
    let mut spool = Spool::open(&root.join("spool"), Limits::default()).unwrap();
    let mut transport = CaptureTransport::default();
    let mut fingerprints = Vec::new();
    for (id, source) in [("original", &candidate), ("project-selected", &admitted)] {
        let identity = CaptureIdentity {
            account_id: "account".into(),
            destination_id: "destination".into(),
            source_id: source.source_id().into(),
            capture_id: id.into(),
        };
        let entry = spool
            .capture(identity, source, MetadataSections::new(), &mut transport)
            .unwrap();
        fingerprints.push(entry.manifest.unwrap().source_fingerprint);
    }
    assert_eq!(fingerprints[0], fingerprints[1]);
}

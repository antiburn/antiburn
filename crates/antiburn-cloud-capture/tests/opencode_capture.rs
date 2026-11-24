#![cfg(all(unix, feature = "cloud"))]

use antiburn_cloud_capture::*;
use flate2::read::GzDecoder;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

const CURRENT: &str = include_str!("../../../fixtures/cloud/opencode/current.sql");

#[derive(Default)]
struct SavedTransport {
    confirmed: BTreeSet<ChunkKey>,
    uploads: Vec<Vec<u8>>,
    reserves: usize,
    offline: bool,
}

impl Transport for SavedTransport {
    fn reserve(
        &mut self,
        _: &CaptureIdentity,
        _: u64,
    ) -> std::result::Result<String, TransportError> {
        self.reserves += 1;
        if self.offline {
            return Err(TransportError::Offline);
        }
        Ok("synthetic-reservation".into())
    }

    fn negotiate(
        &mut self,
        _: &str,
        manifest: &CaptureManifest,
    ) -> std::result::Result<UploadPlan, TransportError> {
        Ok(UploadPlan {
            identity: manifest.identity.clone(),
            manifest_sha256: manifest.sha256().unwrap(),
            plan_id: "synthetic-plan".into(),
            chunks: manifest
                .chunks()
                .map(|(key, chunk)| PlannedChunk {
                    key,
                    compressed_sha256: chunk.compressed_sha256.clone(),
                    disposition: ChunkDisposition::Upload,
                })
                .collect(),
            required_transfer_bytes: manifest
                .chunks()
                .map(|(_, chunk)| chunk.compressed_bytes)
                .sum(),
            reused_bytes: 0,
        })
    }

    fn status(
        &mut self,
        _: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
    ) -> std::result::Result<ServerStatus, TransportError> {
        Ok(ServerStatus {
            identity: manifest.identity.clone(),
            manifest_sha256: manifest.sha256().unwrap(),
            plan_id: plan.plan_id.clone(),
            uploaded_parts: self
                .confirmed
                .iter()
                .map(|key| PartReceipt {
                    key: key.clone(),
                    compressed_sha256: manifest.chunk(key).unwrap().compressed_sha256.clone(),
                })
                .collect(),
            integrity_accepted: false,
        })
    }

    fn upload_part(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
        key: &ChunkKey,
        _: &Chunk,
        body: &mut dyn Read,
    ) -> std::result::Result<(), TransportError> {
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes).unwrap();
        self.uploads.push(bytes);
        self.confirmed.insert(key.clone());
        Ok(())
    }

    fn complete(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> std::result::Result<(), TransportError> {
        Ok(())
    }
}

fn fixture() -> (tempfile::TempDir, PathBuf, Connection) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(CURRENT).unwrap();
    (temporary, path, connection)
}

fn selected(path: &Path, id: &str) -> Candidate {
    discover_opencode(path, &Limits::default())
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.native_session_id() == id)
        .unwrap()
        .in_source_scope("synthetic-root")
        .unwrap()
}

fn identity(candidate: &Candidate, capture_id: &str) -> CaptureIdentity {
    CaptureIdentity {
        capture_id: capture_id.into(),
        account_id: "synthetic-account".into(),
        destination_id: "synthetic-destination".into(),
        source_id: candidate.source_id().into(),
    }
}

fn expanded(root: &Path, identity: &CaptureIdentity, stream: &CapturedStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    for chunk in &stream.chunks {
        let path = root
            .join(&identity.capture_id)
            .join(format!("chunk-{}-{:05}.gz", stream.stream_id, chunk.index));
        let file = std::fs::File::open(path).unwrap();
        GzDecoder::new(file).read_to_end(&mut bytes).unwrap();
    }
    bytes
}

#[test]
fn automation_scheduler_state_does_not_change_discovery_or_capture_bytes() {
    let (_temporary, database, connection) = fixture();
    let limits = Limits::default();
    let first = discover_opencode_page(&database, None, &limits).unwrap();
    let ids = |candidates: Vec<Candidate>| {
        candidates
            .into_iter()
            .map(|candidate| candidate.native_session_id().to_owned())
            .collect::<Vec<_>>()
    };
    let expected_ids = ids(first.candidates);
    let candidate = selected(&database, "ses_main");
    let identity = identity(&candidate, "same-source");
    let capture = |name: &str| {
        let directory = database.parent().unwrap().join(name);
        let entry = Spool::open(&directory, limits.clone())
            .unwrap()
            .capture(
                identity.clone(),
                &candidate,
                Default::default(),
                &mut SavedTransport::default(),
            )
            .unwrap();
        let manifest = entry.manifest.unwrap();
        let bytes = expanded(&directory, &identity, &manifest.streams[0]);
        assert!(!bytes.is_empty());
        (manifest, bytes)
    };
    let original = capture("without-automation");
    connection
        .execute_batch(
            "CREATE TABLE automation (
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL, session_id TEXT,
                directory TEXT NOT NULL, name TEXT NOT NULL, trigger TEXT NOT NULL,
                tz TEXT NOT NULL, action TEXT NOT NULL, status TEXT DEFAULT 'active' NOT NULL,
                next_due_at INTEGER, expires_at INTEGER, run_count INTEGER DEFAULT 0 NOT NULL,
                failures INTEGER DEFAULT 0 NOT NULL, last_run_at INTEGER, last_message_id TEXT,
                last_outcome TEXT, last_output TEXT, retry_at INTEGER, state TEXT,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL
             );
             INSERT INTO automation
                (id, project_id, session_id, directory, name, trigger, tz, action,
                 last_output, time_created, time_updated)
             VALUES ('synthetic-automation', 'synthetic-project', 'ses_main',
                     '/not-project-evidence', 'synthetic schedule', '{}', 'UTC',
                     'excluded automation action', 'excluded automation output', 1, 1);",
        )
        .unwrap();
    let page = discover_opencode_page(&database, None, &limits).unwrap();
    assert_eq!(ids(page.candidates), expected_ids);
    assert!(page.next_cursor.is_none());
    assert_eq!(capture("with-automation"), original);

    connection
        .execute_batch(
            "ALTER TABLE automation ADD COLUMN event_position TEXT;
             UPDATE automation SET event_position = 'excluded continuation';",
        )
        .unwrap();
    assert_eq!(capture("with-event-position"), original);
    connection
        .execute("ALTER TABLE automation ADD COLUMN unknown_owner TEXT", [])
        .unwrap();
    assert!(matches!(
        discover_opencode_page(&database, None, &limits),
        Err(Error::UnsupportedSource)
    ));
}

#[test]
fn session_digest_ignores_unrelated_writes_and_tracks_owned_values_and_dependencies() {
    let (_temporary, database, connection) = fixture();
    let project = database.parent().unwrap().join("project");
    std::fs::create_dir(&project).unwrap();
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id='ses_main'",
            [project.to_str().unwrap()],
        )
        .unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    let candidate = selected(&database, "ses_main");
    let limits = Limits::default();
    let original = candidate
        .opencode_content_digest(&limits, &selection)
        .unwrap();
    connection
        .execute(
            "UPDATE session_message SET data='unrelated change' WHERE id='msg_child'",
            [],
        )
        .unwrap();
    connection
        .execute("UPDATE credential SET value='unrelated global change'", [])
        .unwrap();
    assert_eq!(
        candidate
            .opencode_content_digest(&limits, &selection)
            .unwrap(),
        original
    );
    connection.execute("UPDATE session_part SET data='owned change' WHERE message_id='msg_split' AND ordinal=0", []).unwrap();
    let changed = candidate
        .opencode_content_digest(&limits, &selection)
        .unwrap();
    assert_ne!(changed, original);
    connection.execute("UPDATE blob SET data=X'6e6577', bytes=3 WHERE hash='2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824'", []).unwrap();
    assert_ne!(
        candidate
            .opencode_content_digest(&limits, &selection)
            .unwrap(),
        changed
    );
    assert!(
        candidate
            .opencode_content_digest(
                &Limits {
                    source_bytes: 1,
                    ..limits
                },
                &selection,
            )
            .is_err()
    );
}

#[test]
fn session_digest_rechecks_project_before_reading_changed_owned_content() {
    let (_temporary, database, connection) = fixture();
    let root = database.parent().unwrap();
    let approved = root.join("approved");
    let excluded = root.join("excluded");
    for project in [&approved, &excluded] {
        std::fs::create_dir(project).unwrap();
    }
    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id='ses_main'",
            [approved.to_str().unwrap()],
        )
        .unwrap();
    let selection = ProjectSelection::new(vec![approved]).unwrap();
    let candidate = selected(&database, "ses_main");
    let limits = Limits::default();
    let evidence = candidate.project_evidence_with_limits(&limits).unwrap();
    assert!(selection.matches(&evidence).unwrap());
    candidate
        .opencode_content_digest(&limits, &selection)
        .unwrap();

    connection
        .execute(
            "UPDATE session_v2 SET directory=?1 WHERE id='ses_main'",
            [excluded.to_str().unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE session_message SET data='not JSON' WHERE id='msg_user'",
            [],
        )
        .unwrap();
    assert!(selection.matches(&evidence).unwrap());
    assert!(matches!(
        candidate.opencode_content_digest(&limits, &selection),
        Err(Error::SourceChanged)
    ));
    let excluded_selection = ProjectSelection::new(vec![excluded]).unwrap();
    assert!(matches!(
        candidate.opencode_content_digest(&limits, &excluded_selection),
        Err(Error::InvalidSource)
    ));
}

#[test]
fn separate_source_captures_keep_unknown_runtime_and_scoped_native_edges() {
    let (_temporary, database, _connection) = fixture();
    let directory = database.parent().unwrap().join("spool");
    let mut spool = Spool::open(&directory, Limits::default()).unwrap();
    let mut transport = SavedTransport::default();
    for (id, capture_id, edge) in [
        ("ses_main", "main", None),
        ("ses_child", "child", Some("ses_main")),
        ("ses_detached", "detached", Some("ses_absent")),
        ("ses_fork", "fork", Some("ses_main")),
    ] {
        let candidate = selected(&database, id);
        let identity = identity(&candidate, capture_id);
        let entry = spool
            .capture(
                identity.clone(),
                &candidate,
                Default::default(),
                &mut transport,
            )
            .unwrap();
        let manifest = entry.manifest.unwrap();
        assert_eq!(manifest.streams[0].stream_id, "native-rows");
        assert_eq!(manifest.streams[0].schema, "antiburn-sqlite-native");
        let metadata: CaptureMetadata =
            serde_json::from_slice(&expanded(&directory, &identity, &manifest.streams[1])).unwrap();
        assert_eq!(metadata.agent, "opencode");
        assert_eq!(metadata.native_session_id, id);
        assert_eq!(metadata.source_agent_version, None);
        assert!(!metadata.owned_version_complete);
        assert!(metadata.version_observations.is_empty());
        if let Some(parent) = edge {
            let expected_parent = if parent == "ses_absent" {
                let main = selected(&database, "ses_main");
                format!(
                    "{}/{}",
                    main.source_id().rsplit_once('/').unwrap().0,
                    base64::Engine::encode(
                        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
                        parent
                    )
                )
            } else {
                selected(&database, parent).source_id().to_owned()
            };
            match metadata.relationship {
                Relationship::Child {
                    parent_source_id, ..
                } if id != "ses_fork" => assert_eq!(parent_source_id, expected_parent),
                Relationship::Fork {
                    parent_source_id, ..
                } if id == "ses_fork" => assert_eq!(parent_source_id, expected_parent),
                _ => panic!("missing proved synthetic relationship"),
            }
        } else {
            assert_eq!(metadata.relationship, Relationship::Unknown);
        }
    }
    assert_eq!(transport.reserves, 4);
}

#[test]
fn queued_restart_and_resume_never_reopen_the_native_database() {
    let (_temporary, database, connection) = fixture();
    let candidate = selected(&database, "ses_main");
    let identity = identity(&candidate, "saved");
    let directory = database.parent().unwrap().join("spool");
    let mut transport = SavedTransport::default();
    let mut spool = Spool::open(&directory, Limits::default()).unwrap();
    let entry = spool
        .capture(
            identity.clone(),
            &candidate,
            Default::default(),
            &mut transport,
        )
        .unwrap();
    let manifest = entry.manifest.as_ref().unwrap();
    let expected = manifest
        .chunks()
        .map(|(key, _)| {
            std::fs::read(
                directory
                    .join("saved")
                    .join(format!("chunk-{}-{:05}.gz", key.stream_id, key.index)),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();
    drop(spool);
    drop(connection);
    std::fs::remove_file(&database).unwrap();
    let mut reopened = Spool::open(&directory, Limits::default()).unwrap();
    let repeated = reopened
        .capture(
            identity.clone(),
            &candidate,
            Default::default(),
            &mut transport,
        )
        .unwrap();
    assert_eq!(entry, repeated);
    for _ in 0..expected.len() + 2 {
        reopened.resume_step(&identity, &mut transport).unwrap();
    }
    assert_eq!(transport.uploads, expected);
    assert_eq!(transport.reserves, 1);
}

#[test]
fn failed_reservation_does_not_read_malformed_payload() {
    let (_temporary, database, connection) = fixture();
    connection
        .execute(
            "UPDATE instruction_state SET initial_values = 'invalid dependency JSON'",
            [],
        )
        .unwrap();
    let candidate = selected(&database, "ses_main");
    let directory = database.parent().unwrap().join("spool");
    let mut spool = Spool::open(&directory, Limits::default()).unwrap();
    let mut transport = SavedTransport {
        offline: true,
        ..Default::default()
    };
    assert!(matches!(
        spool.capture(
            identity(&candidate, "offline"),
            &candidate,
            Default::default(),
            &mut transport
        ),
        Err(Error::Transport(TransportError::Offline))
    ));
    assert_eq!(spool.entries().unwrap()[0].state, CaptureState::Preparing);
}

#[test]
fn public_synthetic_source_manifest_pins_exact_input_bytes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/cloud/opencode");
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["synthetic"], true);
    assert_eq!(manifest["fixture_version"], "opencode-native-fixtures-v1");
    for file in manifest["files"].as_array().unwrap() {
        let bytes = std::fs::read(root.join(file["path"].as_str().unwrap())).unwrap();
        assert_eq!(bytes.len() as u64, file["bytes"].as_u64().unwrap());
        assert_eq!(
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
            file["sha256"].as_str().unwrap()
        );
    }
}

#[test]
fn newly_unsupported_layout_stays_private_after_reservation() {
    let (_temporary, database, connection) = fixture();
    let candidate = selected(&database, "ses_main");
    connection
        .execute_batch(
            "CREATE TABLE future_session_payload(id TEXT PRIMARY KEY, session_id TEXT, data TEXT);",
        )
        .unwrap();
    let directory = database.parent().unwrap().join("spool");
    let mut spool = Spool::open(&directory, Limits::default()).unwrap();
    let mut transport = SavedTransport::default();
    assert!(matches!(
        spool.capture(
            identity(&candidate, "unsupported"),
            &candidate,
            Default::default(),
            &mut transport
        ),
        Err(Error::UnsupportedSource)
    ));
    let entry = spool.entries().unwrap().pop().unwrap();
    assert_eq!(entry.state, CaptureState::Preparing);
    assert!(entry.manifest.is_none());
    assert_eq!(transport.reserves, 1);
    assert!(transport.uploads.is_empty());
}

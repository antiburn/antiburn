#![cfg(unix)]

use antiburn_cloud_capture::*;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/cloud/capture-v1")
        .canonicalize()
        .unwrap()
}

fn temporary() -> TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

fn identity(source_id: &str, capture_id: &str) -> CaptureIdentity {
    CaptureIdentity {
        capture_id: capture_id.into(),
        account_id: "synthetic-account".into(),
        destination_id: "synthetic-destination".into(),
        source_id: source_id.into(),
    }
}

fn candidate(source_id: &str) -> Candidate {
    discover_claude(&fixtures().join("project"), &Limits::default())
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.source_id() == source_id)
        .unwrap()
}

fn spool(temp: &TempDir) -> Spool {
    Spool::open(&temp.path().join("spool"), Limits::default()).unwrap()
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[derive(Default)]
struct FakeTransport {
    manifest: Option<CaptureManifest>,
    confirmed: BTreeSet<u32>,
    uploaded: Vec<(u32, Vec<u8>)>,
    reserve_calls: usize,
    complete_calls: usize,
    reuse_all: bool,
    accepted: bool,
    accept_on_complete: bool,
    fail_upload_after_receipt: bool,
    fail_complete: bool,
    bad_identity: bool,
    bad_hash: bool,
    duplicate_receipt: bool,
    reserve_mutation: Option<(PathBuf, Vec<u8>)>,
    reserve_failure: bool,
    drop_previous_receipt: bool,
    bad_manifest_digest: bool,
}

impl FakeTransport {
    fn response(&self, manifest: &CaptureManifest) -> ServerStatus {
        let mut identity = manifest.identity.clone();
        if self.bad_identity {
            identity.account_id = "another-account".into();
        }
        let mut parts: Vec<_> = self
            .confirmed
            .iter()
            .map(|index| PartReceipt {
                index: *index,
                compressed_sha256: if self.bad_hash {
                    "0".repeat(64)
                } else {
                    manifest.chunks[*index as usize].compressed_sha256.clone()
                },
            })
            .collect();
        if self.duplicate_receipt && !parts.is_empty() {
            parts.push(parts[0].clone());
        }
        ServerStatus {
            identity,
            manifest_sha256: if self.bad_manifest_digest {
                "0".repeat(64)
            } else {
                manifest.sha256().unwrap()
            },
            parts,
            integrity_accepted: self.accepted,
        }
    }
}

impl Transport for FakeTransport {
    fn reserve(
        &mut self,
        _: &CaptureIdentity,
        _: u64,
    ) -> std::result::Result<String, TransportError> {
        self.reserve_calls += 1;
        if let Some((path, bytes)) = self.reserve_mutation.take() {
            fs::write(path, bytes).unwrap();
        }
        if self.reserve_failure {
            return Err(TransportError::Offline);
        }
        Ok("synthetic-reservation".into())
    }

    fn negotiate(
        &mut self,
        _: &str,
        manifest: &CaptureManifest,
    ) -> std::result::Result<ServerStatus, TransportError> {
        self.manifest = Some(manifest.clone());
        if self.reuse_all {
            self.confirmed = manifest.chunks.iter().map(|chunk| chunk.index).collect();
        }
        Ok(self.response(manifest))
    }

    fn status(
        &mut self,
        _: &str,
        manifest: &CaptureManifest,
    ) -> std::result::Result<ServerStatus, TransportError> {
        if self.drop_previous_receipt && self.confirmed.len() > 1 {
            self.confirmed.remove(&0);
        }
        Ok(self.response(manifest))
    }

    fn upload_part(
        &mut self,
        _: &str,
        _: &CaptureIdentity,
        chunk: &Chunk,
        body: &mut dyn Read,
    ) -> std::result::Result<(), TransportError> {
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len() as u64, chunk.compressed_bytes);
        assert_eq!(sha(&bytes), chunk.compressed_sha256);
        self.uploaded.push((chunk.index, bytes));
        self.confirmed.insert(chunk.index);
        if self.fail_upload_after_receipt {
            self.fail_upload_after_receipt = false;
            return Err(TransportError::Offline);
        }
        Ok(())
    }

    fn complete(
        &mut self,
        _: &str,
        _: &CaptureManifest,
    ) -> std::result::Result<(), TransportError> {
        self.complete_calls += 1;
        if self.fail_complete {
            self.fail_complete = false;
            return Err(TransportError::Offline);
        }
        if self.accept_on_complete {
            self.accepted = true;
        }
        Ok(())
    }
}

fn expanded(root: &Path, manifest: &CaptureManifest) -> Vec<u8> {
    let mut bytes = Vec::new();
    for chunk in &manifest.chunks {
        let compressed = fs::read(root.join(format!("chunk-{:05}.gz", chunk.index))).unwrap();
        assert_eq!(sha(&compressed), chunk.compressed_sha256);
        let mut part = Vec::new();
        GzDecoder::new(compressed.as_slice())
            .read_to_end(&mut part)
            .unwrap();
        assert_eq!(part.len() as u64, chunk.expanded_bytes);
        assert_eq!(sha(&part), chunk.expanded_sha256);
        assert_eq!(&compressed[..10], &[31, 139, 8, 0, 0, 0, 0, 0, 0, 255]);
        bytes.extend(part);
    }
    assert_eq!(sha(&bytes), manifest.expanded_sha256);
    assert_eq!(bytes.len() as u64, manifest.expanded_bytes);
    bytes
}

fn metadata(bytes: &[u8], manifest: &CaptureManifest) -> CaptureMetadata {
    let file = manifest.files.last().unwrap();
    serde_json::from_slice(&bytes[file.offset as usize..(file.offset + file.bytes) as usize])
        .unwrap()
}

#[test]
fn discovery_is_bounded_and_child_and_fork_sources_are_independent() {
    let candidates = discover_claude(&fixtures().join("project"), &Limits::default()).unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(Candidate::source_id)
            .collect::<Vec<_>>(),
        [
            "fork",
            "main",
            "main/subagents/agent-fork",
            "main/subagents/agent-worker",
            "unknown"
        ]
    );
    let limits = Limits {
        discovery_entries: 1,
        ..Limits::default()
    };
    assert!(matches!(
        discover_claude(&fixtures().join("project"), &limits),
        Err(Error::Limit)
    ));
    let limits = Limits {
        discovery_sessions: 1,
        ..Limits::default()
    };
    assert!(matches!(
        discover_claude(&fixtures().join("project"), &limits),
        Err(Error::Limit)
    ));
}

#[test]
fn exact_native_and_companion_bytes_and_extensible_metadata_are_retained() {
    let temp = temporary();
    let mut spool = spool(&temp);
    let candidate = candidate("main/subagents/agent-fork");
    let mut transport = FakeTransport::default();
    let entry = spool
        .capture(
            identity(candidate.source_id(), "capture-fork"),
            &candidate,
            BTreeMap::from([(
                "synthetic.example.v1".into(),
                serde_json::json!({"keep": true}),
            )]),
            &mut transport,
        )
        .unwrap();
    assert_eq!(transport.reserve_calls, 1);
    assert!(transport.uploaded.is_empty());
    let manifest = entry.manifest.unwrap();
    let root = temp.path().join("spool/capture-fork");
    let bytes = expanded(&root, &manifest);
    for file in &manifest.files {
        let captured = &bytes[file.offset as usize..(file.offset + file.bytes) as usize];
        assert_eq!(sha(captured), file.sha256);
    }
    assert_eq!(
        &bytes[..manifest.files[0].bytes as usize],
        fs::read(candidate.transcript_path()).unwrap()
    );
    let native_companion = &manifest.files[1];
    assert_eq!(
        &bytes[native_companion.offset as usize
            ..(native_companion.offset + native_companion.bytes) as usize],
        fs::read(candidate.transcript_path().with_extension("meta.json")).unwrap()
    );
    let metadata = metadata(&bytes, &manifest);
    assert_eq!(metadata.source_agent_version, None);
    assert_eq!(metadata.recorded_agent_versions, ["2.1.278", "2.1.309"]);
    assert!(metadata.version_scan_complete);
    assert_eq!(metadata.extensions["synthetic.example.v1"]["keep"], true);
    assert_eq!(
        metadata.relationship,
        Relationship::Fork {
            parent_source_id: "main/subagents/agent-worker".into(),
            proof: "claude-native-meta-v1".into()
        }
    );
}

#[test]
fn historical_versions_and_unknown_relationships_are_not_inferred_from_installation() {
    for (source, version, complete, relation) in [
        ("main", Some("2.1.309"), true, "unknown"),
        (
            "main/subagents/agent-worker",
            Some("2.1.278"),
            true,
            "child",
        ),
        ("fork", Some("2.1.278"), true, "fork"),
        ("unknown", None, false, "unknown"),
    ] {
        let temp = temporary();
        let mut spool = spool(&temp);
        let entry = spool
            .capture(
                identity(source, "capture-version"),
                &candidate(source),
                BTreeMap::new(),
                &mut FakeTransport::default(),
            )
            .unwrap();
        let manifest = entry.manifest.unwrap();
        let bytes = expanded(&temp.path().join("spool/capture-version"), &manifest);
        let metadata = metadata(&bytes, &manifest);
        assert_eq!(metadata.source_agent_version.as_deref(), version);
        assert_eq!(metadata.version_scan_complete, complete);
        assert_eq!(metadata.history_completeness, "unknown");
        assert_eq!(
            serde_json::to_value(metadata.relationship).unwrap()["kind"],
            relation
        );
    }
}

#[test]
fn restart_reconciles_lost_part_response_without_reading_the_source_again() {
    let temp = temporary();
    let source_root = temp.path().join("project");
    fs::create_dir(&source_root).unwrap();
    let source = source_root.join("session.jsonl");
    fs::write(
        &source,
        b"{\"version\":\"2.1.278\"}\r\nexact native bytes\0\n",
    )
    .unwrap();
    let candidate = discover_claude(&source_root, &Limits::default())
        .unwrap()
        .remove(0);
    let identity = identity(candidate.source_id(), "capture-resume");
    let mut transport = FakeTransport {
        fail_upload_after_receipt: true,
        ..FakeTransport::default()
    };
    let mut queue = spool(&temp);
    queue
        .capture(
            identity.clone(),
            &candidate,
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    assert!(matches!(
        queue.resume(&identity, &mut transport),
        Err(Error::Transport(TransportError::Offline))
    ));
    assert_eq!(transport.uploaded.len(), 1);
    drop(queue);
    fs::remove_file(source).unwrap();
    let mut queue = spool(&temp);
    let entry = queue.resume(&identity, &mut transport).unwrap();
    assert_eq!(entry.state, CaptureState::AwaitingIntegrity);
    assert_eq!(transport.uploaded.len(), 1);
    assert!(
        temp.path()
            .join("spool/capture-resume/chunk-00000.gz")
            .exists()
    );
    transport.accepted = true;
    assert_eq!(
        queue.resume(&identity, &mut transport).unwrap().state,
        CaptureState::Accepted
    );
    assert!(!temp.path().join("spool/capture-resume").exists());
}

#[test]
fn reused_chunks_and_202_completion_keep_every_saved_byte() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let identity = identity("main", "capture-reuse");
    let mut transport = FakeTransport {
        reuse_all: true,
        ..FakeTransport::default()
    };
    queue
        .capture(
            identity.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let chunk = temp.path().join("spool/capture-reuse/chunk-00000.gz");
    let before = fs::read(&chunk).unwrap();
    let entry = queue.resume(&identity, &mut transport).unwrap();
    assert_eq!(entry.state, CaptureState::AwaitingIntegrity);
    assert!(transport.uploaded.is_empty());
    assert_eq!(fs::read(&chunk).unwrap(), before);
    drop(queue);
    let mut queue = spool(&temp);
    assert_eq!(fs::read(&chunk).unwrap(), before);
    queue.cancel(&identity).unwrap();
    assert!(!chunk.exists());
}

#[test]
fn lost_completion_response_preserves_durable_awaiting_integrity_state() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let identity = identity("main", "capture-complete");
    let mut transport = FakeTransport {
        fail_complete: true,
        ..FakeTransport::default()
    };
    queue
        .capture(
            identity.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    assert!(queue.resume(&identity, &mut transport).is_err());
    assert_eq!(
        queue.entries().unwrap()[0].state,
        CaptureState::AwaitingIntegrity
    );
    let count = transport.uploaded.len();
    transport.accept_on_complete = true;
    assert_eq!(
        queue.resume(&identity, &mut transport).unwrap().state,
        CaptureState::Accepted
    );
    assert_eq!(transport.uploaded.len(), count);
}

#[test]
fn account_destination_and_source_binding_cannot_change_on_retry() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let original = identity("main", "capture-binding");
    let mut transport = FakeTransport::default();
    queue
        .capture(
            original.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    for changed in [
        CaptureIdentity {
            account_id: "other".into(),
            ..original.clone()
        },
        CaptureIdentity {
            destination_id: "other".into(),
            ..original.clone()
        },
        CaptureIdentity {
            source_id: "other".into(),
            ..original.clone()
        },
    ] {
        assert!(matches!(
            queue.resume(&changed, &mut transport),
            Err(Error::IdentityMismatch)
        ));
        assert!(matches!(
            queue.cancel(&changed),
            Err(Error::IdentityMismatch)
        ));
    }
    let again = queue
        .capture(
            original,
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    assert_eq!(again.state, CaptureState::Queued);
    assert_eq!(transport.reserve_calls, 1);
    assert!(transport.uploaded.is_empty());
}

#[test]
fn source_mutation_after_reservation_stops_capture_without_recapture() {
    let temp = temporary();
    let source_root = temp.path().join("project");
    fs::create_dir(&source_root).unwrap();
    let path = source_root.join("changing.jsonl");
    fs::write(&path, b"before\n").unwrap();
    let candidate = discover_claude(&source_root, &Limits::default())
        .unwrap()
        .remove(0);
    let identity = identity(candidate.source_id(), "capture-changed");
    let mut transport = FakeTransport {
        reserve_mutation: Some((path, b"after\n".to_vec())),
        ..FakeTransport::default()
    };
    let mut queue = spool(&temp);
    assert!(matches!(
        queue.capture(
            identity.clone(),
            &candidate,
            BTreeMap::new(),
            &mut transport
        ),
        Err(Error::SourceChanged)
    ));
    assert!(matches!(
        queue.resume(&identity, &mut transport),
        Err(Error::InterruptedCapture)
    ));
    assert_eq!(queue.entries().unwrap()[0].state, CaptureState::Preparing);
    queue.cancel(&identity).unwrap();
}

#[test]
fn spool_is_private_and_rejects_symlink_and_shared_spool_paths() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let identity = identity("main", "capture-private");
    queue
        .capture(
            identity,
            &candidate("main"),
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    for directory in [
        temp.path().join("spool"),
        temp.path().join("spool/capture-private"),
    ] {
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                assert_eq!(
                    entry.metadata().unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
    }
    assert!(matches!(
        Spool::open(&temp.path().join("spool"), Limits::default()),
        Err(Error::Busy)
    ));
    let alias = temp.path().join("alias");
    symlink(temp.path().join("spool"), &alias).unwrap();
    assert!(Spool::open(&alias, Limits::default()).is_err());
    let shared = temp.path().join("shared");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(Spool::open(&shared, Limits::default()).is_err());
}

#[test]
fn source_symlink_and_hardlink_are_rejected_before_reservation() {
    let temp = temporary();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("main.jsonl"), b"{}\n").unwrap();
    let candidate = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let original = project.join("saved");
    fs::rename(candidate.transcript_path(), &original).unwrap();
    symlink(&original, candidate.transcript_path()).unwrap();
    let mut queue = spool(&temp);
    let mut transport = FakeTransport::default();
    assert!(
        queue
            .capture(
                identity("main", "capture-link"),
                &candidate,
                BTreeMap::new(),
                &mut transport
            )
            .is_err()
    );
    fs::remove_file(candidate.transcript_path()).unwrap();
    fs::hard_link(&original, candidate.transcript_path()).unwrap();
    assert!(matches!(
        queue.capture(
            identity("main", "capture-link"),
            &candidate,
            BTreeMap::new(),
            &mut transport
        ),
        Err(Error::InvalidSource)
    ));
    assert_eq!(transport.reserve_calls, 0);
}

#[test]
fn source_disk_and_queue_limits_stop_work_before_reservation() {
    for limits in [
        Limits {
            source_bytes: 1,
            ..Limits::default()
        },
        Limits {
            capture_bytes: 1,
            ..Limits::default()
        },
        Limits {
            spool_bytes: 1,
            ..Limits::default()
        },
    ] {
        let temp = temporary();
        let mut queue = Spool::open(&temp.path().join("spool"), limits).unwrap();
        let mut transport = FakeTransport::default();
        assert!(matches!(
            queue.capture(
                identity("main", "limited"),
                &candidate("main"),
                BTreeMap::new(),
                &mut transport
            ),
            Err(Error::Limit)
        ));
        assert_eq!(transport.reserve_calls, 0);
    }
    let temp = temporary();
    let mut queue = Spool::open(
        &temp.path().join("spool"),
        Limits {
            queue_entries: 2,
            ..Limits::default()
        },
    )
    .unwrap();
    let mut transport = FakeTransport::default();
    for id in ["first", "second"] {
        queue
            .capture(
                identity("main", id),
                &candidate("main"),
                BTreeMap::new(),
                &mut transport,
            )
            .unwrap();
    }
    assert!(matches!(
        queue.capture(
            identity("main", "third"),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport
        ),
        Err(Error::Limit)
    ));
    drop(queue);
    assert_eq!(
        Spool::open(
            &temp.path().join("spool"),
            Limits {
                queue_entries: 2,
                ..Limits::default()
            }
        )
        .unwrap()
        .entries()
        .unwrap()
        .len(),
        2
    );
}

#[test]
fn fixed_expanded_chunk_layout_spans_native_file_boundaries() {
    let temp = temporary();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let mut file = fs::File::create(project.join("large.jsonl")).unwrap();
    let block = vec![b'x'; 64 * 1024];
    for _ in 0..129 {
        file.write_all(&block).unwrap();
    }
    drop(file);
    let candidate = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity("large", "capture-large"),
            &candidate,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    assert_eq!(manifest.chunks.len(), 2);
    assert_eq!(manifest.chunks[0].expanded_bytes, CHUNK_BYTES);
    assert_eq!(manifest.chunks[1].expanded_offset, CHUNK_BYTES);
    let bytes = expanded(&temp.path().join("spool/capture-large"), &manifest);
    let metadata = metadata(&bytes, &manifest);
    assert_eq!(metadata.source_agent_version, None);
    assert!(!metadata.version_scan_complete);
    assert_eq!(&bytes[..129 * 64 * 1024], vec![b'x'; 129 * 64 * 1024]);
}

#[test]
fn corrupted_saved_bytes_block_upload_and_do_not_release_capture() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let identity = identity("main", "capture-corrupt");
    let mut transport = FakeTransport::default();
    queue
        .capture(
            identity.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let path = temp.path().join("spool/capture-corrupt/chunk-00000.gz");
    let mut bytes = fs::read(&path).unwrap();
    bytes[20] ^= 1;
    fs::write(&path, bytes).unwrap();
    assert!(matches!(
        queue.resume(&identity, &mut transport),
        Err(Error::CorruptSpool)
    ));
    assert!(transport.uploaded.is_empty());
    assert!(path.exists());
}

#[test]
fn mismatched_and_duplicate_server_receipts_never_release_bytes() {
    for mode in 0..5 {
        let temp = temporary();
        let mut queue = spool(&temp);
        let identity = identity("main", "capture-status");
        let mut transport = FakeTransport {
            reuse_all: mode != 4,
            accepted: true,
            bad_identity: mode == 0,
            bad_hash: mode == 1,
            duplicate_receipt: mode == 2,
            bad_manifest_digest: mode == 3,
            ..FakeTransport::default()
        };
        queue
            .capture(
                identity.clone(),
                &candidate("main"),
                BTreeMap::new(),
                &mut transport,
            )
            .unwrap();
        assert!(matches!(
            queue.resume(&identity, &mut transport),
            Err(Error::InvalidServerStatus)
        ));
        assert!(
            temp.path()
                .join("spool/capture-status/chunk-00000.gz")
                .exists()
        );
    }
}

#[test]
fn fixture_manifest_pins_every_native_input_hash() {
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(fixtures().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["fixture_version"], FIXTURE_VERSION);
    assert_eq!(manifest["capture_version"], CAPTURE_VERSION);
    for item in manifest["files"].as_array().unwrap() {
        let path = item["path"].as_str().unwrap();
        let bytes = fs::read(fixtures().join(path)).unwrap();
        assert_eq!(bytes.len() as u64, item["bytes"].as_u64().unwrap());
        assert_eq!(sha(&bytes), item["sha256"].as_str().unwrap());
    }
}

#[test]
fn capture_matches_the_public_saved_wire_reference() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity("main", "fixture-main"),
            &candidate("main"),
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let expected: CaptureManifest =
        serde_json::from_slice(&fs::read(fixtures().join("reference/main.capture.json")).unwrap())
            .unwrap();
    assert_eq!(entry.manifest.unwrap(), expected);
    assert_eq!(
        fs::read(temp.path().join("spool/fixture-main/chunk-00000.gz")).unwrap(),
        fs::read(fixtures().join("reference/main.chunk-00000.gz")).unwrap(),
    );
}

#[test]
fn source_scope_is_fixed_and_applies_to_relationships() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let candidate = candidate("main/subagents/agent-fork")
        .in_source_scope("synthetic-device-root")
        .unwrap();
    assert_eq!(
        candidate.source_id(),
        "synthetic-device-root/main/subagents/agent-fork"
    );
    assert!(candidate.in_source_scope("other-root").is_err());
    let entry = queue
        .capture(
            identity(candidate.source_id(), "capture-scope"),
            &candidate,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    let metadata = metadata(
        &expanded(&temp.path().join("spool/capture-scope"), &manifest),
        &manifest,
    );
    assert_eq!(
        metadata.relationship,
        Relationship::Fork {
            parent_source_id: "synthetic-device-root/main/subagents/agent-worker".into(),
            proof: "claude-native-meta-v1".into(),
        }
    );
}

#[test]
fn failed_reservation_leaves_an_interrupted_identity_without_native_reads() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let identity = identity("main", "capture-reservation");
    let mut transport = FakeTransport {
        reserve_failure: true,
        ..FakeTransport::default()
    };
    assert!(matches!(
        queue.capture(
            identity.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport
        ),
        Err(Error::Transport(TransportError::Offline))
    ));
    let paths = fs::read_dir(temp.path().join("spool/capture-reservation"))
        .unwrap()
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].as_ref().unwrap().file_name(), "record.json");
    assert_eq!(queue.entries().unwrap()[0].state, CaptureState::Preparing);
    drop(queue);
    let mut queue = spool(&temp);
    assert!(matches!(
        queue.resume(&identity, &mut transport),
        Err(Error::InterruptedCapture)
    ));
    queue.cancel(&identity).unwrap();
}

#[test]
fn partial_final_record_and_binary_native_bytes_survive_without_newline_repair() {
    let temp = temporary();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let original = b"{\"version\":\"2.1.309\"}\r\n\xff\0{\"incomplete\": ";
    fs::write(project.join("partial.jsonl"), original).unwrap();
    let candidate = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity("partial", "capture-partial"),
            &candidate,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    let bytes = expanded(&temp.path().join("spool/capture-partial"), &manifest);
    assert_eq!(&bytes[..original.len()], original);
    let metadata = metadata(&bytes, &manifest);
    assert_eq!(metadata.source_agent_version, None);
    assert!(!metadata.version_scan_complete);
    assert_eq!(metadata.recorded_agent_versions, ["2.1.309"]);
}

#[test]
fn restart_cleans_interrupted_publication_and_terminal_deletion_only() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "capture-survivor");
    queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let root = temp.path().join("spool");
    for name in [".new-unpublished", ".delete-accepted"] {
        let path = root.join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    drop(queue);
    let queue = spool(&temp);
    assert_eq!(queue.entries().unwrap()[0].identity, id);
    assert!(!root.join(".new-unpublished").exists());
    assert!(!root.join(".delete-accepted").exists());
    assert!(root.join("capture-survivor/chunk-00000.gz").exists());
}

#[test]
fn many_independent_pending_captures_survive_restart() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let candidate = candidate("main");
    for index in 0..40 {
        queue
            .capture(
                identity("main", &format!("capture-{index:03}")),
                &candidate,
                BTreeMap::new(),
                &mut FakeTransport::default(),
            )
            .unwrap();
    }
    drop(queue);
    let mut queue = spool(&temp);
    let entries = queue.entries().unwrap();
    assert_eq!(entries.len(), 40);
    queue.cancel(&entries[17].identity).unwrap();
    assert_eq!(queue.entries().unwrap().len(), 39);
    assert!(
        temp.path()
            .join("spool/capture-018/chunk-00000.gz")
            .exists()
    );
}

#[test]
fn oversized_extensions_fail_before_reservation() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let mut transport = FakeTransport::default();
    assert!(matches!(
        queue.capture(
            identity("main", "capture-extensions"),
            &candidate("main"),
            BTreeMap::from([(
                "synthetic.large.v1".into(),
                serde_json::json!("x".repeat(40 * 1024))
            )]),
            &mut transport,
        ),
        Err(Error::Limit)
    ));
    assert_eq!(transport.reserve_calls, 0);
}

#[test]
fn orphan_child_capture_does_not_require_or_read_a_parent_transcript() {
    let temp = temporary();
    let project = temp.path().join("project");
    let children = project.join("absent-parent/subagents");
    fs::create_dir_all(&children).unwrap();
    let original =
        b"{\"sessionId\":\"absent-parent\",\"agentId\":\"child\",\"version\":\"2.1.278\"}\n";
    fs::write(children.join("agent-child.jsonl"), original).unwrap();
    let candidates = discover_claude(&project, &Limits::default()).unwrap();
    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    assert_eq!(candidate.source_id(), "absent-parent/subagents/agent-child");
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity(candidate.source_id(), "capture-orphan"),
            candidate,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    assert_eq!(manifest.files.len(), 2);
    let bytes = expanded(&temp.path().join("spool/capture-orphan"), &manifest);
    assert_eq!(&bytes[..original.len()], original);
    let metadata = metadata(&bytes, &manifest);
    assert_eq!(metadata.native_agent_id.as_deref(), Some("child"));
    assert_eq!(
        metadata.relationship,
        Relationship::Child {
            parent_source_id: "absent-parent".into(),
            proof: "claude-subagents-layout-v1".into(),
        }
    );
}

#[test]
fn invalid_version_fields_and_conflicting_fork_edges_stay_unknown() {
    let temp = temporary();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::write(project.join("conflict.jsonl"), b"{\"version\":\"2.1.309\",\"forkedFrom\":{\"sessionId\":\"first\"}}\n{\"version\":42,\"forkedFrom\":{\"sessionId\":\"second\"}}\n").unwrap();
    let candidate = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity("conflict", "capture-conflict"),
            &candidate,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    let metadata = metadata(
        &expanded(&temp.path().join("spool/capture-conflict"), &manifest),
        &manifest,
    );
    assert_eq!(metadata.relationship, Relationship::Unknown);
    assert_eq!(metadata.source_agent_version, None);
    assert_eq!(metadata.recorded_agent_versions, ["2.1.309"]);
    assert!(!metadata.version_scan_complete);
}

#[test]
fn native_companion_has_a_strict_byte_limit_before_reservation() {
    let temp = temporary();
    let project = temp.path().join("project");
    let children = project.join("main/subagents");
    fs::create_dir_all(&children).unwrap();
    fs::write(children.join("agent-child.jsonl"), b"{}\n").unwrap();
    fs::write(
        children.join("agent-child.meta.json"),
        vec![b'x'; METADATA_BYTES as usize + 1],
    )
    .unwrap();
    let candidate = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let mut transport = FakeTransport::default();
    assert!(matches!(
        queue.capture(
            identity(candidate.source_id(), "capture-companion"),
            &candidate,
            BTreeMap::new(),
            &mut transport
        ),
        Err(Error::Limit)
    ));
    assert_eq!(transport.reserve_calls, 0);
}

#[test]
fn regressed_server_receipts_block_completion_and_keep_all_chunks() {
    let temp = temporary();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::File::create(project.join("large.jsonl"))
        .unwrap()
        .set_len(CHUNK_BYTES)
        .unwrap();
    let candidate = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let identity = identity("large", "capture-regression");
    let mut transport = FakeTransport {
        drop_previous_receipt: true,
        ..FakeTransport::default()
    };
    queue
        .capture(
            identity.clone(),
            &candidate,
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    assert!(matches!(
        queue.resume(&identity, &mut transport),
        Err(Error::InvalidServerStatus)
    ));
    assert_eq!(transport.complete_calls, 0);
    assert_eq!(transport.uploaded.len(), 2);
    for index in 0..2 {
        assert!(
            temp.path()
                .join(format!("spool/capture-regression/chunk-{index:05}.gz"))
                .exists()
        );
    }
}

#[test]
fn restart_releases_only_durable_terminal_records() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "capture-terminal");
    let mut entry = queue
        .capture(
            id,
            &candidate("main"),
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let record = temp.path().join("spool/capture-terminal/record.json");
    entry.state = CaptureState::Accepted;
    fs::write(&record, serde_json::to_vec(&entry).unwrap()).unwrap();
    drop(queue);
    let queue = spool(&temp);
    assert!(queue.entries().unwrap().is_empty());
    assert!(!temp.path().join("spool/capture-terminal").exists());
}

#[test]
fn spool_handles_do_not_follow_a_replaced_ancestor_after_admission() {
    let temp = temporary();
    let parent = temp.path().join("original-parent");
    fs::create_dir(&parent).unwrap();
    let mut queue = Spool::open(&parent.join("spool"), Limits::default()).unwrap();
    let first = identity("main", "capture-before-rename");
    let mut transport = FakeTransport::default();
    queue
        .capture(
            first.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let moved_parent = temp.path().join("moved-parent");
    fs::rename(&parent, &moved_parent).unwrap();
    let decoy = temp.path().join("decoy");
    fs::create_dir(&decoy).unwrap();
    symlink(&decoy, &parent).unwrap();
    let second = identity("main", "capture-after-rename");
    queue
        .capture(
            second.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    assert_eq!(queue.entries().unwrap().len(), 2);
    assert!(matches!(
        Spool::open(&moved_parent.join("spool"), Limits::default()),
        Err(Error::Busy)
    ));
    queue.resume(&first, &mut transport).unwrap();
    queue.cancel(&second).unwrap();
    assert_eq!(queue.entries().unwrap().len(), 1);
    assert!(
        moved_parent
            .join("spool/capture-before-rename/chunk-00000.gz")
            .exists()
    );
    assert!(fs::read_dir(&decoy).unwrap().next().is_none());
    transport.accepted = true;
    queue.resume(&first, &mut transport).unwrap();
    assert!(queue.entries().unwrap().is_empty());
    assert!(fs::read_dir(&decoy).unwrap().next().is_none());
}

#[test]
fn captured_sources_stay_bound_to_the_discovered_directory_after_rename() {
    let temp = temporary();
    let project = temp.path().join("project");
    let children = project.join("main/subagents");
    fs::create_dir_all(&children).unwrap();
    let original = b"{\"version\":\"2.1.278\",\"message\":\"synthetic original\"}\n";
    let companion = b"{\"isFork\":false,\"synthetic\":\"original companion\"}\n";
    fs::write(children.join("agent-child.jsonl"), original).unwrap();
    fs::write(children.join("agent-child.meta.json"), companion).unwrap();
    let candidate = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    fs::rename(&project, temp.path().join("moved")).unwrap();
    let decoy = temp.path().join("decoy");
    fs::create_dir_all(decoy.join("main/subagents")).unwrap();
    fs::write(
        decoy.join("main/subagents/agent-child.jsonl"),
        b"outside capture scope\n",
    )
    .unwrap();
    fs::write(
        decoy.join("main/subagents/agent-child.meta.json"),
        b"outside companion\n",
    )
    .unwrap();
    symlink(&decoy, &project).unwrap();
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity(candidate.source_id(), "capture-source-handle"),
            &candidate,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    let bytes = expanded(&temp.path().join("spool/capture-source-handle"), &manifest);
    assert_eq!(&bytes[..original.len()], original);
    assert_eq!(
        &bytes[original.len()..original.len() + companion.len()],
        companion
    );
}

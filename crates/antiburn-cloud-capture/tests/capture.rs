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

#[path = "capture/resume_tests.rs"]
mod resume_tests;

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

#[test]
fn project_admission_preserves_full_session_bytes_and_ignores_tool_paths() {
    let temp = temporary();
    let project = temp.path().join("project");
    let nested = project.join("nested");
    let source = temp.path().join("provider-storage");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir(&source).unwrap();
    let mut native = format!(
        "{}\n",
        serde_json::json!({
            "sessionId": "selected",
            "cwd": project,
            "tool": {"path": "/synthetic/outside/secret.txt", "cwd": "/synthetic/outside"},
            "message": {
                "text": "Keep all native content.",
                "thinking": "Synthetic private thinking.",
                "content": [{"type": "tool_result", "content": "synthetic output ".repeat(32 * 1024)}]
            },
            "unknown": [1, 2, 3]
        })
    );
    native.push_str(&format!(
        "{}\n",
        serde_json::json!({"sessionId": "selected", "cwd": nested})
    ));
    assert!(native.len() > 64 * 1024);
    fs::write(source.join("selected.jsonl"), &native).unwrap();
    let candidate = reopen_claude(&source, "selected", None, &Limits::default()).unwrap();
    let admitted = ProjectSelection::new(vec![project])
        .unwrap()
        .admit(&candidate)
        .unwrap()
        .unwrap();
    assert_eq!(admitted.admitted_project_paths().len(), 2);
    let mut spool = spool(&temp);
    let mut transport = FakeTransport::default();
    let entry = spool
        .capture(
            identity(admitted.source_id(), "project-capture"),
            &admitted,
            MetadataSections::new(),
            &mut transport,
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    assert_eq!(manifest.streams[0].expanded_bytes, native.len() as u64);
    assert_eq!(manifest.streams[0].expanded_sha256, sha(native.as_bytes()));
    assert_eq!(transport.reserve_calls, 1);
    assert!(transport.uploaded.is_empty());
}

#[test]
fn project_admission_captures_full_session_when_owned_cwd_changes() {
    let temp = temporary();
    let project = temp.path().join("project");
    let outside = temp.path().join("outside");
    let source = temp.path().join("provider-storage");
    for path in [&project, &outside, &source] {
        fs::create_dir(path).unwrap();
    }
    let mut native = format!(
        "{}\n",
        serde_json::json!({"sessionId": "selected", "cwd": project})
    );
    native.push_str(&"{\"noise\":true}\n".repeat(64 * 1024 / 10));
    native.push_str(&format!(
        "{}\n",
        serde_json::json!({"sessionId": "selected", "cwd": outside})
    ));
    fs::write(source.join("selected.jsonl"), &native).unwrap();
    let candidate = reopen_claude(&source, "selected", None, &Limits::default()).unwrap();
    let admitted = ProjectSelection::new(vec![project])
        .unwrap()
        .admit(&candidate)
        .unwrap()
        .unwrap();
    let mut spool = spool(&temp);
    let mut transport = FakeTransport::default();
    let result = spool
        .capture(
            identity(admitted.source_id(), "changing-cwd"),
            &admitted,
            MetadataSections::new(),
            &mut transport,
        )
        .unwrap();
    assert_eq!(result.state, CaptureState::Queued);
    assert_eq!(
        result.manifest.unwrap().streams[0].expanded_sha256,
        sha(native.as_bytes())
    );
    assert_eq!(transport.reserve_calls, 1);
    assert!(transport.uploaded.is_empty());
}

#[test]
fn project_admission_rechecks_frozen_root_policy_not_one_cwd() {
    let temp = temporary();
    let project = temp.path().join("project");
    let nested = project.join("nested");
    let outside = temp.path().join("outside");
    let source = temp.path().join("provider-storage");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::create_dir(&source).unwrap();
    let path = source.join("selected.jsonl");
    fs::write(
        &path,
        format!(
            "{}\n",
            serde_json::json!({"sessionId": "selected", "cwd": project})
        ),
    )
    .unwrap();
    let candidate = reopen_claude(&source, "selected", None, &Limits::default()).unwrap();
    let admitted = ProjectSelection::new(vec![project])
        .unwrap()
        .admit(&candidate)
        .unwrap()
        .unwrap();
    let mut spool = spool(&temp);
    let mut transport = FakeTransport::default();
    for (capture_id, cwd, allowed) in [
        ("changed-descendant", &nested, true),
        ("removed-approved-cwd", &outside, false),
    ] {
        fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::json!({"sessionId": "selected", "cwd": cwd})
            ),
        )
        .unwrap();
        let result = spool.capture(
            identity(admitted.source_id(), capture_id),
            &admitted,
            MetadataSections::new(),
            &mut transport,
        );
        if allowed {
            assert_eq!(result.unwrap().state, CaptureState::Queued);
        } else {
            assert!(matches!(result, Err(Error::SourceChanged)));
        }
    }
    assert_eq!(transport.reserve_calls, 2);
    assert!(transport.uploaded.is_empty());
}

#[derive(Default)]
struct FakeTransport {
    manifest: Option<CaptureManifest>,
    confirmed: BTreeSet<ChunkKey>,
    uploaded: Vec<(ChunkKey, Vec<u8>)>,
    reserve_calls: usize,
    negotiate_calls: usize,
    status_calls: usize,
    complete_calls: usize,
    reuse_all: bool,
    accepted: bool,
    accept_on_complete: bool,
    fail_upload_after_receipt: bool,
    fail_negotiate_after_registration: bool,
    fail_complete: bool,
    bad_identity: bool,
    bad_hash: bool,
    duplicate_receipt: bool,
    reserve_mutation: Option<(PathBuf, Vec<u8>)>,
    reserve_failure: bool,
    drop_previous_receipt: bool,
    keep_only_latest_receipt: bool,
    lose_latest_receipt: bool,
    bad_manifest_digest: bool,
    reuse_stream: Option<String>,
    change_plan: bool,
    wrong_plan_totals: bool,
    bad_receipt_hash: bool,
}

impl FakeTransport {
    fn response(&self, manifest: &CaptureManifest, plan: &UploadPlan) -> ServerStatus {
        let mut identity = manifest.identity.clone();
        if self.bad_identity {
            identity.account_id = "another-account".into();
        }
        let mut parts: Vec<_> = self
            .confirmed
            .iter()
            .map(|key| PartReceipt {
                key: key.clone(),
                compressed_sha256: if self.bad_receipt_hash {
                    "0".repeat(64)
                } else {
                    manifest.chunk(key).unwrap().compressed_sha256.clone()
                },
            })
            .collect();
        if self.duplicate_receipt {
            let (key, chunk) = manifest.chunks().next().unwrap();
            let part = PartReceipt {
                key,
                compressed_sha256: chunk.compressed_sha256.clone(),
            };
            parts.push(part.clone());
            parts.push(part);
        }
        ServerStatus {
            identity,
            manifest_sha256: if self.bad_manifest_digest {
                "0".repeat(64)
            } else {
                manifest.sha256().unwrap()
            },
            plan_id: if self.change_plan {
                "changed-plan".into()
            } else {
                plan.plan_id.clone()
            },
            uploaded_parts: parts,
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
    ) -> std::result::Result<UploadPlan, TransportError> {
        self.negotiate_calls += 1;
        self.manifest = Some(manifest.clone());
        if self.fail_negotiate_after_registration {
            self.fail_negotiate_after_registration = false;
            return Err(TransportError::Offline);
        }
        let mut required_transfer_bytes = 0;
        let mut reused_bytes = 0;
        let chunks = manifest
            .chunks()
            .map(|(key, chunk)| {
                let disposition = if self.reuse_all
                    || self.reuse_stream.as_deref() == Some(key.stream_id.as_str())
                {
                    reused_bytes += chunk.compressed_bytes;
                    ChunkDisposition::Reuse
                } else {
                    required_transfer_bytes += chunk.compressed_bytes;
                    ChunkDisposition::Upload
                };
                PlannedChunk {
                    key,
                    disposition,
                    compressed_sha256: if self.bad_hash {
                        "0".repeat(64)
                    } else {
                        chunk.compressed_sha256.clone()
                    },
                }
            })
            .collect();
        let mut identity = manifest.identity.clone();
        if self.bad_identity {
            identity.account_id = "another-account".into();
        }
        if self.wrong_plan_totals {
            required_transfer_bytes += 1;
        }
        Ok(UploadPlan {
            identity,
            manifest_sha256: if self.bad_manifest_digest {
                "0".repeat(64)
            } else {
                manifest.sha256().unwrap()
            },
            plan_id: if self.change_plan {
                "changed-plan".into()
            } else {
                "synthetic-plan".into()
            },
            chunks,
            required_transfer_bytes,
            reused_bytes,
        })
    }

    fn status(
        &mut self,
        _: &str,
        manifest: &CaptureManifest,
        plan: &UploadPlan,
    ) -> std::result::Result<ServerStatus, TransportError> {
        self.status_calls += 1;
        if self.keep_only_latest_receipt {
            let latest = self.uploaded.last().map(|(key, _)| key);
            self.confirmed.retain(|key| Some(key) == latest);
        }
        if self.lose_latest_receipt
            && let Some((key, _)) = self.uploaded.last()
        {
            self.confirmed.remove(key);
            self.lose_latest_receipt = false;
        }
        if self.drop_previous_receipt && self.confirmed.len() > 1 {
            self.drop_previous_receipt = false;
            let first = self.uploaded.first().unwrap().0.clone();
            self.confirmed.remove(&first);
        }
        Ok(self.response(manifest, plan))
    }

    fn upload_part(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
        key: &ChunkKey,
        chunk: &Chunk,
        body: &mut dyn Read,
    ) -> std::result::Result<(), TransportError> {
        let mut bytes = Vec::new();
        body.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes.len() as u64, chunk.compressed_bytes);
        assert_eq!(sha(&bytes), chunk.compressed_sha256);
        self.uploaded.push((key.clone(), bytes));
        self.confirmed.insert(key.clone());
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
        _: &UploadPlan,
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
    for stream in &manifest.streams {
        let part = expanded_stream(root, stream);
        assert_eq!(sha(&part), stream.expanded_sha256);
        assert_eq!(part.len() as u64, stream.expanded_bytes);
        bytes.extend(part);
    }
    bytes
}

fn expanded_stream(root: &Path, stream: &CapturedStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    for chunk in &stream.chunks {
        let compressed =
            fs::read(root.join(format!("chunk-{}-{:05}.gz", stream.stream_id, chunk.index)))
                .unwrap();
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
    bytes
}

fn metadata(bytes: &[u8], manifest: &CaptureManifest) -> CaptureMetadata {
    let start = manifest
        .streams
        .iter()
        .take(manifest.streams.len() - 1)
        .map(|stream| stream.expanded_bytes)
        .sum::<u64>() as usize;
    serde_json::from_slice(&bytes[start..]).unwrap()
}

fn local_section(data: serde_json::Value) -> MetadataSection {
    let data_sha256 = sha(&serde_json::to_vec(&data).unwrap());
    MetadataSection {
        schema: "synthetic-section-v1".into(),
        producer: MetadataProducer {
            name: "synthetic-producer".into(),
            version: "1.0".into(),
            unknown_fields: BTreeMap::new(),
        },
        observation: MetadataObservation::ObservedAt {
            unix_millis: 1_700_000_000_000,
            unknown_fields: BTreeMap::new(),
        },
        evidence: vec![MetadataEvidence::LocalObservation {
            method: "synthetic-observation".into(),
            data_sha256,
            unknown_fields: BTreeMap::new(),
        }],
        data,
        unknown_fields: BTreeMap::new(),
    }
}

fn saved_chunks(root: &Path, manifest: &CaptureManifest) -> BTreeMap<ChunkKey, Vec<u8>> {
    manifest
        .chunks()
        .map(|(key, _)| {
            let bytes = fs::read(root.join(format!("chunk-{}-{:05}.gz", key.stream_id, key.index)))
                .unwrap();
            (key, bytes)
        })
        .collect()
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
            "main/subagents/agent-conflicting-fork",
            "main/subagents/agent-fork",
            "main/subagents/agent-inherited-only",
            "main/subagents/agent-replay-only",
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
fn candidates_expose_native_identity_without_treating_every_source_as_a_transcript() {
    let root = candidate("main");
    assert_eq!(root.source_kind(), CaptureSourceKind::ClaudeJsonl);
    assert_eq!(root.source_path(), fixtures().join("project/main.jsonl"));
    assert_eq!(root.native_session_id(), "main");
    assert_eq!(root.native_agent_id(), None);
    let child = candidate("main/subagents/agent-worker")
        .in_source_scope("synthetic-root")
        .unwrap();
    assert_eq!(child.source_kind(), CaptureSourceKind::ClaudeJsonl);
    assert_eq!(child.native_session_id(), "main");
    assert_eq!(child.native_agent_id(), Some("worker"));
    assert_eq!(
        child.source_id(),
        "synthetic-root/main/subagents/agent-worker"
    );
}

#[test]
fn provider_stream_layouts_cannot_mix_roles_companions_or_schemas() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let source = candidate("main/subagents/agent-worker");
    let id = identity(source.source_id(), "stream-contract");
    let mut transport = FakeTransport::default();
    let entry = queue
        .capture(id.clone(), &source, BTreeMap::new(), &mut transport)
        .unwrap();
    let root = temp.path().join("spool/stream-contract");
    let manifest = entry.manifest.as_ref().unwrap();
    let before = saved_chunks(&root, manifest);
    for corruption in ["rows-with-companion", "role", "schema", "metadata"] {
        let mut invalid = entry.clone();
        let manifest = invalid.manifest.as_mut().unwrap();
        match corruption {
            "rows-with-companion" => {
                manifest.streams[0].stream_id = "native-rows".into();
                manifest.streams[0].role = "native_rows".into();
                manifest.streams[0].schema = "antiburn-sqlite-native".into();
            }
            "role" => manifest.streams[0].role = "native_rows".into(),
            "schema" => manifest.streams[0].schema = "antiburn-sqlite-native".into(),
            "metadata" => manifest.streams[2].schema = "claude-jsonl".into(),
            _ => unreachable!(),
        }
        manifest.source_fingerprint = manifest.calculate_source_fingerprint().unwrap();
        fs::write(
            root.join("record.json"),
            serde_json::to_vec(&invalid).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            queue.resume_step(&id, &mut transport),
            Err(Error::CorruptSpool)
        ));
        assert!(transport.uploaded.is_empty());
        assert_eq!(
            saved_chunks(&root, entry.manifest.as_ref().unwrap()),
            before
        );
    }
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
                local_section(serde_json::json!({"keep": true})),
            )]),
            &mut transport,
        )
        .unwrap();
    assert_eq!(transport.reserve_calls, 1);
    assert!(transport.uploaded.is_empty());
    let manifest = entry.manifest.unwrap();
    let root = temp.path().join("spool/capture-fork");
    let bytes = expanded(&root, &manifest);
    let mut offset = 0;
    for stream in &manifest.streams {
        let end = offset + stream.expanded_bytes as usize;
        let captured = &bytes[offset..end];
        assert_eq!(sha(captured), stream.expanded_sha256);
        offset = end;
    }
    assert_eq!(
        &bytes[..manifest.streams[0].expanded_bytes as usize],
        fs::read(candidate.source_path()).unwrap()
    );
    let companion_offset = manifest.streams[0].expanded_bytes as usize;
    let native_companion = &manifest.streams[1];
    assert_eq!(
        &bytes[companion_offset..companion_offset + native_companion.expanded_bytes as usize],
        fs::read(candidate.source_path().with_extension("meta.json")).unwrap()
    );
    let metadata = metadata(&bytes, &manifest);
    assert_eq!(metadata.source_agent_version.as_deref(), Some("2.1.309"));
    assert_eq!(
        metadata.version_observations,
        [
            VersionObservation {
                version: "2.1.278".into(),
                ownership: VersionOwnership::Inherited,
            },
            VersionObservation {
                version: "2.1.309".into(),
                ownership: VersionOwnership::Owned,
            }
        ]
    );
    assert!(metadata.version_scan_complete);
    assert_eq!(metadata.sections["synthetic.example.v1"].data["keep"], true);
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
    assert_eq!(transport.uploaded.len(), 2);
    assert!(
        temp.path()
            .join("spool/capture-resume/chunk-transcript-00000.gz")
            .exists()
    );
    transport.accepted = true;
    assert_eq!(
        queue.resume(&identity, &mut transport).unwrap().state,
        CaptureState::Accepted
    );
    assert!(
        !temp
            .path()
            .join("spool/capture-resume/chunk-transcript-00000.gz")
            .exists()
    );
    assert_eq!(queue.entries().unwrap()[0].state, CaptureState::Accepted);
    queue.acknowledge_accepted(&identity).unwrap();
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
    let chunk = temp
        .path()
        .join("spool/capture-reuse/chunk-transcript-00000.gz");
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
    fs::rename(candidate.source_path(), &original).unwrap();
    symlink(&original, candidate.source_path()).unwrap();
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
    fs::remove_file(candidate.source_path()).unwrap();
    fs::hard_link(&original, candidate.source_path()).unwrap();
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
            Err(Error::Limit | Error::SpoolCapacity)
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
        Err(Error::SpoolCapacity)
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
fn fixed_expanded_chunks_are_independent_for_each_stream() {
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
    assert_eq!(manifest.streams.len(), 2);
    assert_eq!(manifest.streams[0].chunks.len(), 2);
    assert_eq!(manifest.streams[0].chunks[0].expanded_bytes, CHUNK_BYTES);
    assert_eq!(manifest.streams[0].chunks[1].index, 1);
    assert_eq!(manifest.streams[1].chunks.len(), 1);
    assert_eq!(manifest.streams[1].chunks[0].index, 0);
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
    let path = temp
        .path()
        .join("spool/capture-corrupt/chunk-transcript-00000.gz");
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
                .join("spool/capture-status/chunk-transcript-00000.gz")
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
            serde_json::from_slice(&fs::read(fixtures().join("sections.json")).unwrap()).unwrap(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let expected: CaptureManifest =
        serde_json::from_slice(&fs::read(fixtures().join("reference/main.capture.json")).unwrap())
            .unwrap();
    assert_eq!(entry.manifest.unwrap(), expected);
    for (key, _) in expected.chunks() {
        assert_eq!(
            fs::read(temp.path().join(format!(
                "spool/fixture-main/chunk-{}-{:05}.gz",
                key.stream_id, key.index
            )))
            .unwrap(),
            fs::read(fixtures().join(format!(
                "reference/main.{}.chunk-{:05}.gz",
                key.stream_id, key.index
            )))
            .unwrap(),
        );
    }
}

#[test]
fn opencode_wire_reference_matches_real_capture_in_distinct_roots() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/cloud/opencode")
        .canonicalize()
        .unwrap();
    let expected: CaptureManifest =
        serde_json::from_slice(&fs::read(fixtures.join("reference/main.capture.json")).unwrap())
            .unwrap();
    for _ in 0..2 {
        let temporary = temporary();
        let database = temporary.path().join("opencode.db");
        let connection = rusqlite::Connection::open(&database).unwrap();
        connection
            .execute_batch(&fs::read_to_string(fixtures.join("current.sql")).unwrap())
            .unwrap();
        drop(connection);
        let candidate = discover_opencode(&database, &Limits::default())
            .unwrap()
            .into_iter()
            .find(|candidate| candidate.native_session_id() == "ses_main")
            .unwrap()
            .in_source_scope("synthetic-root")
            .unwrap();
        let mut queue = spool(&temporary);
        let mut transport = FakeTransport::default();
        let entry = queue
            .capture(
                identity(candidate.source_id(), "fixture-opencode-main"),
                &candidate,
                MetadataSections::new(),
                &mut transport,
            )
            .unwrap();
        assert_eq!(transport.reserve_calls, 1);
        assert!(transport.uploaded.is_empty());
        assert_eq!(entry.manifest.as_ref(), Some(&expected));
        let saved = temporary.path().join("spool/fixture-opencode-main");
        expanded(&saved, &expected);
        for (key, _) in expected.chunks() {
            assert_eq!(
                fs::read(saved.join(format!("chunk-{}-{:05}.gz", key.stream_id, key.index)))
                    .unwrap(),
                fs::read(fixtures.join(format!(
                    "reference/main.{}.chunk-{:05}.gz",
                    key.stream_id, key.index
                )))
                .unwrap()
            );
        }
    }
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
    assert_eq!(
        metadata.version_observations,
        [VersionObservation {
            version: "2.1.309".into(),
            ownership: VersionOwnership::Unknown,
        }]
    );
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
    assert!(
        root.join("capture-survivor/chunk-transcript-00000.gz")
            .exists()
    );
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
            .join("spool/capture-018/chunk-transcript-00000.gz")
            .exists()
    );
}

#[test]
fn oversized_sections_fail_before_reservation() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let mut transport = FakeTransport::default();
    assert!(matches!(
        queue.capture(
            identity("main", "capture-extensions"),
            &candidate("main"),
            BTreeMap::from([(
                "synthetic.large.v1".into(),
                local_section(serde_json::json!("x".repeat(40 * 1024)))
            )]),
            &mut transport,
        ),
        Err(Error::Limit)
    ));
    assert_eq!(transport.reserve_calls, 0);
}

#[test]
fn section_nesting_limits_preserve_the_supported_capture_boundary() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let mut transport = FakeTransport::default();
    for depth in [125, 129, 124] {
        let mut data = serde_json::Value::Null;
        for _ in 0..depth {
            data = serde_json::Value::Array(vec![data]);
        }
        let result = queue.capture(
            identity("main", &format!("capture-nesting-{depth}")),
            &candidate("main"),
            BTreeMap::from([("synthetic.nested".into(), local_section(data))]),
            &mut transport,
        );
        if depth == 124 {
            assert_eq!(result.unwrap().state, CaptureState::Queued);
            assert_eq!(transport.reserve_calls, 1);
            assert_eq!(queue.entries().unwrap().len(), 1);
        } else {
            assert!(matches!(result, Err(Error::Limit)));
            assert_eq!(transport.reserve_calls, 0);
            assert!(queue.entries().unwrap().is_empty());
        }
    }
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
    assert_eq!(manifest.streams.len(), 2);
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
    fs::write(project.join("conflict.jsonl"), b"{\"sessionId\":\"conflict\",\"version\":\"2.1.309\",\"forkedFrom\":{\"sessionId\":\"first\"}}\n{\"sessionId\":\"conflict\",\"version\":42,\"forkedFrom\":{\"sessionId\":\"second\"}}\n").unwrap();
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
    assert_eq!(
        metadata.version_observations,
        [VersionObservation {
            version: "2.1.309".into(),
            ownership: VersionOwnership::Owned,
        }]
    );
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
fn expired_server_receipts_reupload_saved_chunks_before_completion() {
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
    assert_eq!(
        queue.resume(&identity, &mut transport).unwrap().state,
        CaptureState::Queued
    );
    assert_eq!(
        queue.resume(&identity, &mut transport).unwrap().state,
        CaptureState::AwaitingIntegrity
    );
    assert_eq!(transport.complete_calls, 1);
    assert_eq!(transport.uploaded.len(), 3);
    assert_eq!(transport.uploaded[0], transport.uploaded[2]);
    let saved_status = queue.entries().unwrap().remove(0).server_status.unwrap();
    assert_eq!(saved_status.uploaded_parts.len(), 2);
    for stream in ["transcript", "capture-metadata"] {
        assert!(
            temp.path()
                .join(format!("spool/capture-regression/chunk-{stream}-00000.gz"))
                .exists()
        );
    }
}

#[test]
fn an_accepted_flag_without_integrity_evidence_never_releases_bytes() {
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
    assert!(matches!(
        Spool::open(&temp.path().join("spool"), Limits::default()),
        Err(Error::CorruptSpool)
    ));
    assert!(
        temp.path()
            .join("spool/capture-terminal/chunk-transcript-00000.gz")
            .exists()
    );
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
            .join("spool/capture-before-rename/chunk-transcript-00000.gz")
            .exists()
    );
    assert!(fs::read_dir(&decoy).unwrap().next().is_none());
    transport.accepted = true;
    queue.resume(&first, &mut transport).unwrap();
    queue.acknowledge_accepted(&first).unwrap();
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

#[test]
fn new_capture_identity_preserves_stable_streams_and_accepted_source_fingerprint() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let source = candidate("main");
    let first_id = identity("main", "stable-first");
    let mut first_transport = FakeTransport {
        accept_on_complete: true,
        ..FakeTransport::default()
    };
    let first = queue
        .capture(
            first_id.clone(),
            &source,
            BTreeMap::new(),
            &mut first_transport,
        )
        .unwrap();
    assert!(first.accepted_source_fingerprint().is_none());
    let first_manifest = first.manifest.unwrap();
    let accepted = queue.resume(&first_id, &mut first_transport).unwrap();
    let fingerprint = accepted.accepted_source_fingerprint().unwrap();
    let fingerprint: AcceptedSourceFingerprint =
        serde_json::from_slice(&serde_json::to_vec(&fingerprint).unwrap()).unwrap();
    queue.acknowledge_accepted(&first_id).unwrap();
    let second_id = identity("main", "stable-second");
    let second = queue
        .capture(
            second_id.clone(),
            &source,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let second_manifest = second.manifest.as_ref().unwrap();
    assert_ne!(
        first_manifest.sha256().unwrap(),
        second_manifest.sha256().unwrap()
    );
    assert_eq!(
        first_manifest.source_fingerprint,
        second_manifest.source_fingerprint
    );
    assert_eq!(first_manifest.streams, second_manifest.streams);
    assert!(fingerprint.matches_source(second_manifest));
    assert!(second.accepted_source_fingerprint().is_none());
    let bytes = expanded(&temp.path().join("spool/stable-second"), second_manifest);
    let metadata = serde_json::to_value(metadata(&bytes, second_manifest)).unwrap();
    assert!(metadata.get("identity").is_none());
    assert!(
        !serde_json::to_string(&metadata)
            .unwrap()
            .contains("stable-second")
    );
    drop(queue);
    let mut queue = spool(&temp);
    let pending = queue.entries().unwrap().remove(0);
    assert_eq!(pending.identity, second_id);
    assert!(fingerprint.matches_source(pending.manifest.as_ref().unwrap()));
    let before = saved_chunks(
        &temp.path().join("spool/stable-second"),
        pending.manifest.as_ref().unwrap(),
    );
    let fenced = CaptureIdentity {
        account_id: "other-account".into(),
        ..second_id.clone()
    };
    assert!(matches!(
        queue.resume_step(&fenced, &mut FakeTransport::default()),
        Err(Error::IdentityMismatch)
    ));
    let mut fenced_manifest = pending.manifest.unwrap();
    fenced_manifest.identity = fenced;
    assert!(!fingerprint.matches_source(&fenced_manifest));
    assert_eq!(
        before,
        saved_chunks(&temp.path().join("spool/stable-second"), &fenced_manifest)
    );
}

#[test]
fn metadata_only_changes_change_the_source_fingerprint_without_replacing_a_pending_retry() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let source = candidate("main");
    let sections = BTreeMap::from([(
        "synthetic.snapshot".into(),
        local_section(serde_json::json!({"checkout":"first"})),
    )]);
    let first_id = identity("main", "metadata-first");
    let first = queue
        .capture(
            first_id.clone(),
            &source,
            sections,
            &mut FakeTransport::default(),
        )
        .unwrap();
    let first_manifest = first.manifest.as_ref().unwrap();
    let before = saved_chunks(&temp.path().join("spool/metadata-first"), first_manifest);
    let changed = BTreeMap::from([(
        "synthetic.snapshot".into(),
        local_section(serde_json::json!({"checkout":"second"})),
    )]);
    let second = queue
        .capture(
            identity("main", "metadata-second"),
            &source,
            changed.clone(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let second_manifest = second.manifest.unwrap();
    assert_ne!(
        first_manifest.source_fingerprint,
        second_manifest.source_fingerprint
    );
    assert_eq!(first_manifest.streams[0], second_manifest.streams[0]);
    assert_ne!(
        first_manifest.streams.last(),
        second_manifest.streams.last()
    );
    let retried = queue
        .capture(first_id, &source, changed, &mut FakeTransport::default())
        .unwrap();
    assert_eq!(retried, first);
    assert_eq!(
        before,
        saved_chunks(
            &temp.path().join("spool/metadata-first"),
            retried.manifest.as_ref().unwrap()
        )
    );
    assert_eq!(queue.entries().unwrap().len(), 2);
}

#[test]
fn empty_native_streams_have_zero_chunks_and_a_separate_nonempty_metadata_stream() {
    let temp = temporary();
    let project = temp.path().join("project");
    let children = project.join("main/subagents");
    fs::create_dir_all(&children).unwrap();
    fs::write(children.join("agent-empty.jsonl"), b"").unwrap();
    fs::write(children.join("agent-empty.meta.json"), b"").unwrap();
    let source = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let id = identity(source.source_id(), "empty-streams");
    let mut transport = FakeTransport::default();
    let entry = queue
        .capture(id.clone(), &source, BTreeMap::new(), &mut transport)
        .unwrap();
    let manifest = entry.manifest.unwrap();
    assert_eq!(manifest.streams.len(), 3);
    for stream in &manifest.streams[..2] {
        assert_eq!(stream.expanded_bytes, 0);
        assert_eq!(stream.expanded_sha256, sha(b""));
        assert!(stream.chunks.is_empty());
    }
    assert_eq!(manifest.streams[2].stream_id, "capture-metadata");
    assert_eq!(manifest.streams[2].chunks.len(), 1);
    assert_eq!(manifest.chunks().count(), 1);
    let step = queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(step.uploaded_chunk.unwrap().stream_id, "capture-metadata");
    assert_eq!(transport.uploaded.len(), 1);
    assert_eq!(transport.complete_calls, 0);
}

#[test]
fn inherited_fork_versions_do_not_become_owned_source_versions() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let source = candidate("main/subagents/agent-inherited-only");
    let entry = queue
        .capture(
            identity(source.source_id(), "inherited-version"),
            &source,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    let metadata = metadata(
        &expanded(&temp.path().join("spool/inherited-version"), &manifest),
        &manifest,
    );
    assert_eq!(metadata.source_agent_version, None);
    assert!(metadata.version_scan_complete);
    assert!(!metadata.owned_version_complete);
    assert_eq!(
        metadata.version_observations,
        [VersionObservation {
            version: "2.1.278".into(),
            ownership: VersionOwnership::Inherited,
        }]
    );
}

#[test]
fn transfer_reuse_and_server_confirmation_have_separate_durable_totals() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let source = candidate("main/subagents/agent-fork");
    let id = identity(source.source_id(), "mixed-plan");
    let mut transport = FakeTransport {
        reuse_stream: Some("transcript".into()),
        ..FakeTransport::default()
    };
    let entry = queue
        .capture(id.clone(), &source, BTreeMap::new(), &mut transport)
        .unwrap();
    assert!(!entry.progress().unwrap().negotiated);
    let manifest = entry.manifest.unwrap();
    let transcript_bytes = manifest.streams[0].chunks[0].compressed_bytes;
    let companion_bytes = manifest.streams[1].chunks[0].compressed_bytes;
    let metadata_bytes = manifest.streams[2].chunks[0].compressed_bytes;
    let all_saved = saved_chunks(&temp.path().join("spool/mixed-plan"), &manifest);
    let first = queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(first.uploaded_chunk.unwrap().stream_id, "native-companion");
    assert!(first.progress.negotiated);
    assert_eq!(
        first.progress.required_transfer_bytes,
        companion_bytes + metadata_bytes
    );
    assert_eq!(first.progress.reused_bytes, transcript_bytes);
    assert_eq!(first.progress.confirmed_upload_bytes, companion_bytes);
    assert_eq!(
        first.progress.total_compressed_bytes,
        transcript_bytes + companion_bytes + metadata_bytes
    );
    drop(queue);
    let mut queue = spool(&temp);
    assert_eq!(
        queue.entries().unwrap()[0].progress().unwrap(),
        first.progress
    );
    let second = queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(second.uploaded_chunk.unwrap().stream_id, "capture-metadata");
    assert_eq!(
        second.progress.confirmed_upload_bytes,
        companion_bytes + metadata_bytes
    );
    assert_eq!(transport.complete_calls, 0);
    let submitted = queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(submitted.entry.state, CaptureState::AwaitingIntegrity);
    assert!(submitted.entry.accepted_source_fingerprint().is_none());
    assert_eq!(
        all_saved,
        saved_chunks(&temp.path().join("spool/mixed-plan"), &manifest)
    );
    assert_eq!(transport.complete_calls, 1);
    queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(transport.complete_calls, 1);
}

#[test]
fn invalid_transfer_totals_or_uploaded_receipts_fail_closed() {
    for bad_plan in [true, false] {
        let temp = temporary();
        let mut queue = spool(&temp);
        let id = identity("main", "invalid-plan-status");
        let mut transport = FakeTransport {
            wrong_plan_totals: bad_plan,
            bad_receipt_hash: !bad_plan,
            ..FakeTransport::default()
        };
        let entry = queue
            .capture(
                id.clone(),
                &candidate("main"),
                BTreeMap::new(),
                &mut transport,
            )
            .unwrap();
        let manifest = entry.manifest.unwrap();
        let before = saved_chunks(&temp.path().join("spool/invalid-plan-status"), &manifest);
        assert!(matches!(
            queue.resume_step(&id, &mut transport),
            Err(Error::InvalidServerStatus)
        ));
        assert_eq!(
            before,
            saved_chunks(&temp.path().join("spool/invalid-plan-status"), &manifest)
        );
        assert_eq!(transport.complete_calls, 0);
    }
}

#[test]
fn bounded_steps_allow_round_robin_upload_of_many_chunk_sources() {
    let temp = temporary();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::File::create(project.join("large.jsonl"))
        .unwrap()
        .set_len(CHUNK_BYTES + 1)
        .unwrap();
    let source = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let first = identity("large", "fair-first");
    let second = identity("large", "fair-second");
    let mut one = FakeTransport::default();
    let mut two = FakeTransport::default();
    queue
        .capture(first.clone(), &source, BTreeMap::new(), &mut one)
        .unwrap();
    queue
        .capture(second.clone(), &source, BTreeMap::new(), &mut two)
        .unwrap();
    for index in 0..2 {
        for (id, transport) in [(&first, &mut one), (&second, &mut two)] {
            let before = transport.uploaded.len();
            let step = queue.resume_step(id, transport).unwrap();
            assert_eq!(
                step.uploaded_chunk,
                Some(ChunkKey {
                    stream_id: "transcript".into(),
                    index
                })
            );
            assert_eq!(transport.uploaded.len(), before + 1);
            assert_eq!(transport.complete_calls, 0);
        }
    }
    assert_eq!(queue.entries().unwrap().len(), 2);
}

#[test]
fn a_resume_step_does_not_read_later_saved_chunks() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "bounded-read");
    let mut transport = FakeTransport::default();
    queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    let later = temp
        .path()
        .join("spool/bounded-read/chunk-capture-metadata-00000.gz");
    let mut bytes = fs::read(&later).unwrap();
    bytes[20] ^= 1;
    fs::write(&later, bytes).unwrap();
    let step = queue.resume_step(&id, &mut transport).unwrap();
    assert_eq!(step.uploaded_chunk.unwrap().stream_id, "transcript");
    assert_eq!(transport.uploaded.len(), 1);
    assert!(matches!(
        queue.resume_step(&id, &mut transport),
        Err(Error::CorruptSpool)
    ));
    assert!(later.exists());
}

#[test]
fn malformed_stream_chunk_layout_blocks_resume_without_releasing_bytes() {
    let temp = temporary();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    fs::File::create(project.join("large.jsonl"))
        .unwrap()
        .set_len(CHUNK_BYTES + 1)
        .unwrap();
    let source = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let id = identity("large", "malformed-stream-layout");
    let mut transport = FakeTransport::default();
    let entry = queue
        .capture(id.clone(), &source, BTreeMap::new(), &mut transport)
        .unwrap();
    let original = entry.manifest.clone().unwrap();
    let before = saved_chunks(
        &temp.path().join("spool/malformed-stream-layout"),
        &original,
    );
    for corruption in ["index", "non-final-length", "total-length", "empty-final"] {
        let mut entry = entry.clone();
        let chunks = &mut entry.manifest.as_mut().unwrap().streams[0].chunks;
        match corruption {
            "index" => chunks[0].index = 1,
            "non-final-length" => {
                chunks[0].expanded_bytes -= 1;
                chunks[1].expanded_bytes += 1;
            }
            "total-length" => chunks[1].expanded_bytes += 1,
            "empty-final" => chunks[1].expanded_bytes = 0,
            _ => unreachable!(),
        }
        fs::write(
            temp.path()
                .join("spool/malformed-stream-layout/record.json"),
            serde_json::to_vec(&entry).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            queue.resume_step(&id, &mut transport),
            Err(Error::CorruptSpool)
        ));
        assert!(transport.uploaded.is_empty());
        assert_eq!(
            before,
            saved_chunks(
                &temp.path().join("spool/malformed-stream-layout"),
                &original
            )
        );
    }
}

#[test]
fn an_owned_companion_only_change_changes_the_source_fingerprint() {
    let temp = temporary();
    let project = temp.path().join("project");
    let children = project.join("main/subagents");
    fs::create_dir_all(&children).unwrap();
    fs::write(
        children.join("agent-child.jsonl"),
        b"{\"sessionId\":\"main\",\"agentId\":\"child\",\"version\":\"2.1.309\"}\n",
    )
    .unwrap();
    let companion = children.join("agent-child.meta.json");
    fs::write(&companion, b"{\"isFork\":false,\"syntheticExtra\":1}\n").unwrap();
    let source = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let first = queue
        .capture(
            identity(source.source_id(), "companion-first"),
            &source,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap()
        .manifest
        .unwrap();
    fs::write(&companion, b"{\"isFork\":false,\"syntheticExtra\":2}\n").unwrap();
    let second = queue
        .capture(
            identity(source.source_id(), "companion-second"),
            &source,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap()
        .manifest
        .unwrap();
    assert_eq!(first.streams[0], second.streams[0]);
    assert_ne!(first.streams[1], second.streams[1]);
    assert_eq!(first.streams[2], second.streams[2]);
    assert_ne!(first.source_fingerprint, second.source_fingerprint);
    assert_eq!(queue.entries().unwrap().len(), 2);
}

#[test]
fn observation_timestamps_change_wire_bytes_but_not_stable_source_fingerprints() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let source = candidate("main");
    let first_section = local_section(serde_json::json!({"checkout":"same","unix_millis":7}));
    let mut later_section = first_section.clone();
    if let MetadataObservation::ObservedAt { unix_millis, .. } = &mut later_section.observation {
        *unix_millis += 123456;
    }
    let first = queue
        .capture(
            identity("main", "time-first"),
            &source,
            BTreeMap::from([("synthetic.snapshot".into(), first_section)]),
            &mut FakeTransport::default(),
        )
        .unwrap()
        .manifest
        .unwrap();
    let later = queue
        .capture(
            identity("main", "time-later"),
            &source,
            BTreeMap::from([("synthetic.snapshot".into(), later_section.clone())]),
            &mut FakeTransport::default(),
        )
        .unwrap()
        .manifest
        .unwrap();
    assert_ne!(first.streams[1], later.streams[1]);
    assert_ne!(first.sha256().unwrap(), later.sha256().unwrap());
    assert_eq!(first.metadata_fingerprint, later.metadata_fingerprint);
    assert_eq!(first.source_fingerprint, later.source_fingerprint);
    later_section.data["unix_millis"] = 8.into();
    if let MetadataEvidence::LocalObservation { data_sha256, .. } = &mut later_section.evidence[0] {
        *data_sha256 = sha(&serde_json::to_vec(&later_section.data).unwrap());
    }
    let changed = queue
        .capture(
            identity("main", "time-data"),
            &source,
            BTreeMap::from([("synthetic.snapshot".into(), later_section)]),
            &mut FakeTransport::default(),
        )
        .unwrap()
        .manifest
        .unwrap();
    assert_ne!(later.metadata_fingerprint, changed.metadata_fingerprint);
    assert_ne!(later.source_fingerprint, changed.source_fingerprint);
}

#[test]
fn accepted_receipts_survive_restart_until_the_host_acknowledges_them() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let id = identity("main", "accepted-receipt");
    let mut transport = FakeTransport {
        accept_on_complete: true,
        ..FakeTransport::default()
    };
    queue
        .capture(
            id.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    assert!(matches!(
        queue.acknowledge_accepted(&id),
        Err(Error::NotAccepted)
    ));
    let entry = queue.resume(&id, &mut transport).unwrap();
    let accepted = entry.accepted_source_fingerprint().unwrap();
    assert!(
        !temp
            .path()
            .join("spool/accepted-receipt/chunk-transcript-00000.gz")
            .exists()
    );
    assert!(matches!(queue.cancel(&id), Err(Error::AlreadyAccepted)));
    drop(queue);
    let mut queue = spool(&temp);
    assert_eq!(
        queue.entries().unwrap().as_slice(),
        std::slice::from_ref(&entry)
    );
    assert_eq!(
        queue.entries().unwrap()[0].accepted_source_fingerprint(),
        Some(accepted)
    );
    let fenced = CaptureIdentity {
        account_id: "wrong-account".into(),
        ..id.clone()
    };
    assert!(matches!(
        queue.acknowledge_accepted(&fenced),
        Err(Error::IdentityMismatch)
    ));
    assert_eq!(
        queue
            .resume_step(
                &id,
                &mut FakeTransport {
                    reserve_failure: true,
                    ..FakeTransport::default()
                }
            )
            .unwrap()
            .entry,
        entry
    );
    queue.acknowledge_accepted(&id).unwrap();
    assert!(queue.entries().unwrap().is_empty());
    drop(queue);
    assert!(spool(&temp).entries().unwrap().is_empty());
}

#[test]
fn inherited_only_native_records_cannot_establish_a_new_source_identity() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let source = candidate("main/subagents/agent-replay-only");
    let id = identity(source.source_id(), "replay-only");
    let mut transport = FakeTransport::default();
    assert!(matches!(
        queue.capture(id.clone(), &source, BTreeMap::new(), &mut transport),
        Err(Error::InvalidSource)
    ));
    assert_eq!(queue.entries().unwrap()[0].state, CaptureState::Preparing);
    assert!(transport.uploaded.is_empty());
    assert!(matches!(
        queue.resume_step(&id, &mut transport),
        Err(Error::InterruptedCapture)
    ));
    queue.cancel(&id).unwrap();
    assert!(queue.entries().unwrap().is_empty());
}

#[test]
fn conflicting_owned_fork_evidence_cannot_be_hidden_by_a_child_companion() {
    let temp = temporary();
    let source = candidate("main/subagents/agent-conflicting-fork");
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity(source.source_id(), "conflicting-child"),
            &source,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    let bytes = expanded(&temp.path().join("spool/conflicting-child"), &manifest);
    assert_eq!(
        &bytes[..manifest.streams[0].expanded_bytes as usize],
        fs::read(source.source_path()).unwrap()
    );
    let metadata = metadata(&bytes, &manifest);
    assert_eq!(metadata.relationship, Relationship::Unknown);
    assert_eq!(metadata.source_agent_version.as_deref(), Some("2.1.309"));
}

#[test]
fn malformed_ownership_fields_do_not_prove_a_historical_runtime_version() {
    for extra in [r#""agentId":42"#, r#""isSidechain":"false""#] {
        let temp = temporary();
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        fs::write(
            project.join("main.jsonl"),
            format!("{{\"sessionId\":\"main\",\"version\":\"2.1.309\",{extra}}}\n"),
        )
        .unwrap();
        let source = discover_claude(&project, &Limits::default())
            .unwrap()
            .remove(0);
        let mut queue = spool(&temp);
        let entry = queue
            .capture(
                identity("main", "invalid-owner"),
                &source,
                BTreeMap::new(),
                &mut FakeTransport::default(),
            )
            .unwrap();
        let manifest = entry.manifest.unwrap();
        let captured = metadata(
            &expanded(&temp.path().join("spool/invalid-owner"), &manifest),
            &manifest,
        );
        assert_eq!(captured.source_agent_version, None);
        assert!(!captured.owned_version_complete);
    }
}

#[test]
fn native_unknown_objects_do_not_change_claude_version_evidence() {
    let temp = temporary();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let original = b"{\"sessionId\":\"main\",\"version\":\"2.1.309\",\"future\":{\"$serde_json::private::Number\":\"not-a-number\"}}\n";
    fs::write(project.join("main.jsonl"), original).unwrap();
    let source = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity("main", "native-literal-object"),
            &source,
            MetadataSections::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    let bytes = expanded(&temp.path().join("spool/native-literal-object"), &manifest);
    assert_eq!(&bytes[..original.len()], original);
    let metadata = metadata(&bytes, &manifest);
    assert_eq!(metadata.source_agent_version.as_deref(), Some("2.1.309"));
    assert!(metadata.version_scan_complete);
    assert!(metadata.owned_version_complete);
}

#[test]
fn queue_admission_keeps_both_record_slots_reserved_for_every_entry() {
    let temp = temporary();
    let limits = Limits {
        spool_bytes: 2 * RECORD_BYTES + 200_000,
        ..Limits::default()
    };
    let mut queue = Spool::open(&temp.path().join("spool"), limits.clone()).unwrap();
    let first = identity("main", "record-headroom-first");
    let mut transport = FakeTransport::default();
    queue
        .capture(
            first.clone(),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport,
        )
        .unwrap();
    assert!(matches!(
        queue.capture(
            identity("main", "record-headroom-second"),
            &candidate("main"),
            BTreeMap::new(),
            &mut transport
        ),
        Err(Error::SpoolCapacity)
    ));
    assert_eq!(transport.reserve_calls, 1);
    queue.resume_step(&first, &mut transport).unwrap();
    drop(queue);
    let queue = Spool::open(&temp.path().join("spool"), limits).unwrap();
    let entry = &queue.entries().unwrap()[0];
    assert!(entry.progress().unwrap().negotiated);
    assert!(entry.progress().unwrap().confirmed_upload_bytes > 0);
}

#[test]
fn changed_native_bytes_do_not_replace_the_saved_generation() {
    let temp = temporary();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    let path = project.join("main.jsonl");
    fs::write(&path, b"{\"sessionId\":\"main\",\"synthetic\":1}\n").unwrap();
    let source = discover_claude(&project, &Limits::default())
        .unwrap()
        .remove(0);
    let mut queue = spool(&temp);
    let first_id = identity("main", "generation-first");
    let first = queue
        .capture(
            first_id.clone(),
            &source,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let first_manifest = first.manifest.as_ref().unwrap();
    let before = saved_chunks(&temp.path().join("spool/generation-first"), first_manifest);
    fs::write(&path, b"{\"sessionId\":\"main\",\"synthetic\":2}\n").unwrap();
    let later = queue
        .capture(
            identity("main", "generation-later"),
            &source,
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap()
        .manifest
        .unwrap();
    assert_ne!(first_manifest.source_fingerprint, later.source_fingerprint);
    assert_eq!(
        queue
            .capture(
                first_id,
                &source,
                BTreeMap::new(),
                &mut FakeTransport::default()
            )
            .unwrap(),
        first
    );
    assert_eq!(
        saved_chunks(&temp.path().join("spool/generation-first"), first_manifest),
        before
    );
}

#[test]
fn saved_acceptance_recovers_the_pending_state_before_cancellation() {
    for pending in [CaptureState::Queued, CaptureState::AwaitingIntegrity] {
        let temp = temporary();
        let mut queue = spool(&temp);
        let id = identity("main", "accepted-status");
        let mut transport = FakeTransport::default();
        let mut entry = queue
            .capture(
                id.clone(),
                &candidate("main"),
                BTreeMap::new(),
                &mut transport,
            )
            .unwrap();
        let manifest = entry.manifest.as_ref().unwrap();
        let plan = transport
            .negotiate(entry.reservation_id.as_ref().unwrap(), manifest)
            .unwrap();
        transport
            .confirmed
            .extend(manifest.chunks().map(|(key, _)| key));
        transport.accepted = true;
        entry.server_status = Some(transport.response(manifest, &plan));
        entry.upload_plan = Some(plan);
        entry.state = pending;
        entry.completion_submitted = pending == CaptureState::AwaitingIntegrity;
        let root = temp.path().join("spool/accepted-status");
        let chunks = saved_chunks(&root, manifest);
        fs::write(
            root.join("record.json"),
            serde_json::to_vec(&entry).unwrap(),
        )
        .unwrap();
        drop(queue);

        let mut queue = spool(&temp);
        let recovered = queue.entries().unwrap().remove(0);
        assert_eq!(recovered.state, CaptureState::Accepted);
        let fingerprint = recovered.accepted_source_fingerprint().unwrap();
        assert!(fingerprint.matches_source(manifest));
        assert!(matches!(queue.cancel(&id), Err(Error::AlreadyAccepted)));
        for key in chunks.keys() {
            assert!(
                !root
                    .join(format!("chunk-{}-{:05}.gz", key.stream_id, key.index))
                    .exists()
            );
        }
        drop(queue);

        let mut queue = spool(&temp);
        assert_eq!(queue.entries().unwrap()[0], recovered);
        assert_eq!(
            queue.entries().unwrap()[0].accepted_source_fingerprint(),
            Some(fingerprint)
        );
        queue.acknowledge_accepted(&id).unwrap();
        assert!(queue.entries().unwrap().is_empty());
    }
}

#[test]
fn capture_wire_uses_numeric_versions_without_derived_fields() {
    let temp = temporary();
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity("main", "numeric-format"),
            &candidate("main"),
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    let transcript = &manifest.streams[0];
    let source_preimage = format!(
        r#"{{"version":1,"metadata_fingerprint":"{}","streams":[{{"stream_id":"transcript","role":"native_transcript","schema":"claude-jsonl","expanded_bytes":{},"expanded_sha256":"{}"}}]}}"#,
        manifest.metadata_fingerprint, transcript.expanded_bytes, transcript.expanded_sha256
    );
    assert_eq!(manifest.source_fingerprint, sha(source_preimage.as_bytes()));
    let wire = serde_json::to_value(&manifest).unwrap();
    assert_eq!(wire["version"], 1);
    assert!(wire.get("capture_version").is_none());
    for stream in wire["streams"].as_array().unwrap() {
        for chunk in stream["chunks"].as_array().unwrap() {
            assert!(chunk.get("expanded_offset").is_none());
        }
    }
    let metadata_bytes = expanded_stream(
        &temp.path().join("spool/numeric-format"),
        &manifest.streams[1],
    );
    let metadata: serde_json::Value = serde_json::from_slice(&metadata_bytes).unwrap();
    assert_eq!(metadata["version"], 1);
    for removed in [
        "schema_version",
        "capture_version",
        "recorded_agent_versions",
    ] {
        assert!(metadata.get(removed).is_none());
    }
    for invalid in [
        serde_json::json!("1"),
        serde_json::json!("antiburn-native-capture-v1"),
        serde_json::json!(0),
        serde_json::json!(2),
        serde_json::json!(-1),
        serde_json::json!(1.0),
    ] {
        let mut bad_manifest = wire.clone();
        bad_manifest["version"] = invalid.clone();
        assert!(serde_json::from_value::<CaptureManifest>(bad_manifest).is_err());
        let mut bad_metadata = metadata.clone();
        bad_metadata["version"] = invalid;
        assert!(serde_json::from_value::<CaptureMetadata>(bad_metadata).is_err());
    }
    let mut missing_manifest_version = wire;
    missing_manifest_version
        .as_object_mut()
        .unwrap()
        .remove("version");
    assert!(serde_json::from_value::<CaptureManifest>(missing_manifest_version).is_err());
    let mut missing_metadata_version = metadata;
    missing_metadata_version
        .as_object_mut()
        .unwrap()
        .remove("version");
    assert!(serde_json::from_value::<CaptureMetadata>(missing_metadata_version).is_err());
}

#[test]
fn unsupported_saved_formats_preserve_the_record_and_every_chunk() {
    for version in [
        None,
        Some(serde_json::json!("1")),
        Some(serde_json::json!(2)),
    ] {
        let temp = temporary();
        let mut queue = spool(&temp);
        let entry = queue
            .capture(
                identity("main", "unsupported-format"),
                &candidate("main"),
                BTreeMap::new(),
                &mut FakeTransport::default(),
            )
            .unwrap();
        let manifest = entry.manifest.as_ref().unwrap();
        let root = temp.path().join("spool/unsupported-format");
        let chunks = saved_chunks(&root, manifest);
        let mut record = serde_json::to_value(&entry).unwrap();
        let saved_manifest = record["manifest"].as_object_mut().unwrap();
        if let Some(version) = version {
            saved_manifest.insert("version".into(), version);
        } else {
            saved_manifest.remove("version");
            saved_manifest.insert(
                "capture_version".into(),
                serde_json::json!("antiburn-native-capture-v1"),
            );
        }
        let record = serde_json::to_vec(&record).unwrap();
        fs::write(root.join("record.json"), &record).unwrap();
        drop(queue);
        assert!(matches!(
            Spool::open(&temp.path().join("spool"), Limits::default()),
            Err(Error::UnsupportedFormat)
        ));
        assert_eq!(fs::read(root.join("record.json")).unwrap(), record);
        assert_eq!(saved_chunks(&root, manifest), chunks);
    }
}

#[test]
#[cfg(target_os = "linux")]
fn discovery_ignores_non_utf8_names_but_counts_them_against_the_bound() {
    use std::os::unix::ffi::OsStringExt;

    let temp = temporary();
    let project = temp.path().join("project");
    let children = project.join("main/subagents");
    fs::create_dir_all(&children).unwrap();
    fs::write(project.join("main.jsonl"), b"{}\n").unwrap();
    fs::write(children.join("agent-worker.jsonl"), b"{}\n").unwrap();
    let invalid = std::ffi::OsString::from_vec(b"unowned-\xff".to_vec());
    fs::write(project.join(&invalid), b"outside capture scope").unwrap();
    fs::write(children.join(&invalid), b"outside capture scope").unwrap();
    let limits = Limits {
        discovery_entries: 5,
        ..Limits::default()
    };
    let candidates = discover_claude(&project, &limits).unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(Candidate::source_id)
            .collect::<Vec<_>>(),
        ["main", "main/subagents/agent-worker"]
    );
    assert!(matches!(
        discover_claude(
            &project,
            &Limits {
                discovery_entries: 4,
                ..limits
            }
        ),
        Err(Error::Limit)
    ));
}

#[test]
#[cfg(target_os = "linux")]
fn spool_rejects_non_utf8_names_without_releasing_pending_bytes() {
    use std::os::unix::ffi::OsStringExt;

    let temp = temporary();
    let mut queue = spool(&temp);
    let entry = queue
        .capture(
            identity("main", "strict-spool"),
            &candidate("main"),
            BTreeMap::new(),
            &mut FakeTransport::default(),
        )
        .unwrap();
    let root = temp.path().join("spool/strict-spool");
    let manifest = entry.manifest.unwrap();
    let before = saved_chunks(&root, &manifest);
    let invalid = std::ffi::OsString::from_vec(b"unexpected-\xff".to_vec());
    fs::write(root.join(invalid), b"unexpected spool file").unwrap();
    drop(queue);
    assert!(matches!(
        Spool::open(&temp.path().join("spool"), Limits::default()),
        Err(Error::CorruptSpool)
    ));
    assert_eq!(saved_chunks(&root, &manifest), before);
}

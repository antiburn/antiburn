#![cfg(all(unix, feature = "cloud"))]

use antiburn_cloud_capture::*;
use flate2::read::GzDecoder;
use serde_json::json;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const FORK: &[u8] = include_bytes!("fixtures/codex/rollout-synthetic-fork.jsonl");
const SPAWN: &[u8] = include_bytes!("fixtures/codex/rollout-synthetic-spawn.jsonl");

struct Reservation<F>(F);

impl<F: FnMut(u64)> Transport for Reservation<F> {
    fn reserve(&mut self, _: &CaptureIdentity, maximum: u64) -> TransportResult<String> {
        (self.0)(maximum);
        Ok("synthetic-reservation".into())
    }

    fn negotiate(&mut self, _: &str, _: &CaptureManifest) -> TransportResult<UploadPlan> {
        panic!("capture must not negotiate")
    }

    fn status(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> TransportResult<ServerStatus> {
        panic!("capture must not check status")
    }

    fn upload_part(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
        _: &ChunkKey,
        _: &Chunk,
        _: &mut dyn Read,
    ) -> TransportResult<()> {
        panic!("capture must not upload")
    }

    fn complete(&mut self, _: &str, _: &CaptureManifest, _: &UploadPlan) -> TransportResult<()> {
        panic!("capture must not submit completion")
    }
}

type TransportResult<T> = std::result::Result<T, TransportError>;

fn temporary() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    (temporary, root)
}

fn candidate(rollout: &Path, id: &str) -> Candidate {
    reopen_codex(rollout, id, &Limits::default())
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
        let file = fs::File::open(
            root.join(&identity.capture_id)
                .join(format!("chunk-{}-{:05}.gz", stream.stream_id, chunk.index)),
        )
        .unwrap();
        GzDecoder::new(file).read_to_end(&mut bytes).unwrap();
    }
    bytes
}

#[test]
fn saves_exact_bytes_and_only_first_owned_metadata() {
    let (_temporary, root) = temporary();
    let rollout = root.join("rollout-synthetic-fork.jsonl");
    let first_newline = FORK.iter().position(|byte| *byte == b'\n').unwrap();
    let mut bytes = FORK[..first_newline].to_vec();
    bytes.extend_from_slice(b"\r\n");
    bytes.extend_from_slice(&FORK[first_newline + 1..]);
    bytes.extend_from_slice(b"not-json-\xff\n{\"incomplete\":");
    fs::write(&rollout, &bytes).unwrap();

    let candidate = candidate(&rollout, "synthetic-fork");
    assert_eq!(candidate.source_kind(), CaptureSourceKind::CodexJsonl);
    assert_eq!(candidate.native_session_id(), "synthetic-fork");
    assert_eq!(candidate.native_agent_id(), None);
    assert_eq!(candidate.source_path(), rollout);
    assert_eq!(candidate.source_id(), "synthetic-root/codex/synthetic-fork");
    let evidence = candidate.project_evidence().unwrap();
    assert_eq!(
        evidence.project_paths(),
        [Path::new("/synthetic/work/fork")]
    );
    assert!(evidence.scan_complete());
    assert_eq!(evidence.source_digest(), None);

    let spool_path = root.join("spool");
    let mut spool = Spool::open(&spool_path, Limits::default()).unwrap();
    let identity = identity(&candidate, "synthetic-capture");
    let entry = spool
        .capture(
            identity.clone(),
            &candidate,
            MetadataSections::new(),
            &mut Reservation(|maximum| assert_eq!(maximum, bytes.len() as u64 + METADATA_BYTES)),
        )
        .unwrap();
    let manifest = entry.manifest.as_ref().unwrap();
    assert_eq!(manifest.streams.len(), 2);
    let transcript = &manifest.streams[0];
    assert_eq!(transcript.stream_id, "transcript");
    assert_eq!(transcript.role, "native_transcript");
    assert_eq!(transcript.schema, "codex-jsonl");
    assert_eq!(expanded(&spool_path, &identity, transcript), bytes);
    let metadata_bytes = expanded(&spool_path, &identity, &manifest.streams[1]);
    let metadata: CaptureMetadata = serde_json::from_slice(&metadata_bytes).unwrap();
    assert_eq!(metadata.agent, "codex");
    assert_eq!(metadata.source_format, "CodexJsonl");
    assert_eq!(metadata.native_session_id, "synthetic-fork");
    assert_eq!(metadata.native_agent_id, None);
    assert_eq!(
        metadata.relationship,
        Relationship::Fork {
            parent_source_id: "synthetic-root/codex/synthetic-parent".into(),
            proof: "codex-session-meta-fork-v1".into(),
        }
    );
    assert_eq!(
        metadata.version_observations,
        [VersionObservation {
            version: "0.999.0".into(),
            ownership: VersionOwnership::Owned,
        }]
    );
    assert_eq!(metadata.source_agent_version, None);
    assert!(!metadata.version_scan_complete);
    assert!(!metadata.owned_version_complete);
    assert_eq!(metadata.history_completeness, "unknown");
    let schema = serde_json::from_str(include_str!(
        "../../../docs/cloud/capture-metadata.schema.json"
    ))
    .unwrap();
    let value = serde_json::from_slice(&metadata_bytes).unwrap();
    jsonschema::draft202012::new(&schema)
        .unwrap()
        .validate(&value)
        .unwrap();

    drop(spool);
    fs::remove_file(rollout).unwrap();
    let spool = Spool::open(&spool_path, Limits::default()).unwrap();
    assert_eq!(spool.entries().unwrap()[0].manifest, entry.manifest);
    assert_eq!(expanded(&spool_path, &identity, transcript), bytes);
}

#[test]
fn spawn_is_an_independent_session_without_inherited_project_permission() {
    let (_temporary, root) = temporary();
    let rollout = root.join("rollout-synthetic-spawn.jsonl");
    fs::write(&rollout, SPAWN).unwrap();
    let candidate = candidate(&rollout, "synthetic-spawn");
    let selection = ProjectSelection::new(vec![root.clone()]).unwrap();
    assert!(selection.admit(&candidate).unwrap().is_none());

    let spool_path = root.join("spool");
    let mut spool = Spool::open(&spool_path, Limits::default()).unwrap();
    let identity = identity(&candidate, "synthetic-capture");
    let entry = spool
        .capture(
            identity.clone(),
            &candidate,
            MetadataSections::new(),
            &mut Reservation(|_| ()),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    let metadata: CaptureMetadata =
        serde_json::from_slice(&expanded(&spool_path, &identity, &manifest.streams[1])).unwrap();
    assert_eq!(
        metadata.relationship,
        Relationship::Child {
            parent_source_id: "synthetic-root/codex/synthetic-fork".into(),
            proof: "codex-session-meta-thread-spawn-v1".into(),
        }
    );
    assert_eq!(
        expanded(&spool_path, &identity, &manifest.streams[0]),
        SPAWN
    );
}

#[test]
fn rejects_unproved_identity_without_searching_a_later_header() {
    let (_temporary, root) = temporary();
    let rollout = root.join("rollout-synthetic-fork.jsonl");
    let invalid_first_lines: &[&[u8]] = &[
        b"\n",
        b"{broken}\n",
        b"[\"session_meta\",{\"id\":\"synthetic-fork\"}]\n",
        b"{\"type\":\"session_meta\",\"payload\":[\"synthetic-fork\",null,null,null,null]}\n",
        b"{\"type\":\"response_item\",\"payload\":{\"id\":\"synthetic-fork\"}}\n",
        b"{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"synthetic-fork\"}}\n",
        b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"synthetic-parent\"}}\n",
        b"{\"type\":\"session_meta\",\"payload\":{\"id\":\"copied\",\"id\":\"synthetic-fork\"}}\n",
    ];
    for first_line in invalid_first_lines {
        let bytes = [*first_line, FORK].concat();
        fs::write(&rollout, bytes).unwrap();
        assert!(matches!(
            reopen_codex(&rollout, "synthetic-fork", &Limits::default()),
            Err(Error::InvalidSource)
        ));
    }
    let first_newline = FORK.iter().position(|byte| *byte == b'\n').unwrap();
    fs::write(&rollout, &FORK[..first_newline]).unwrap();
    assert!(matches!(
        reopen_codex(&rollout, "synthetic-fork", &Limits::default()),
        Err(Error::InvalidSource)
    ));
    fs::write(&rollout, vec![b' '; 64 * 1024 + 1]).unwrap();
    assert!(matches!(
        reopen_codex(&rollout, "synthetic-fork", &Limits::default()),
        Err(Error::Limit)
    ));
    fs::write(&rollout, FORK).unwrap();
    let lower = Limits {
        source_bytes: FORK.len() as u64 - 1,
        ..Limits::default()
    };
    assert!(matches!(
        reopen_codex(&rollout, "synthetic-fork", &lower),
        Err(Error::Limit)
    ));
}

#[test]
fn never_follows_symlinks_or_shared_file_links() {
    let (_temporary, root) = temporary();
    let rollout = root.join("rollout-synthetic-fork.jsonl");
    fs::write(&rollout, FORK).unwrap();
    let alias = root.join("rollout-alias.jsonl");
    std::os::unix::fs::symlink(&rollout, &alias).unwrap();
    assert!(reopen_codex(&alias, "synthetic-fork", &Limits::default()).is_err());
    fs::remove_file(alias).unwrap();
    fs::hard_link(&rollout, root.join("rollout-shared.jsonl")).unwrap();
    assert!(matches!(
        reopen_codex(&rollout, "synthetic-fork", &Limits::default()),
        Err(Error::InvalidSource)
    ));
}

fn header(id: &str, cwd: &Path) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&json!({
        "type": "session_meta",
        "payload": {"id": id, "cwd": cwd}
    }))
    .unwrap();
    bytes.push(b'\n');
    bytes
}

#[test]
fn copied_header_cannot_grant_folder_permission_before_or_after_reservation() {
    let (_temporary, root) = temporary();
    let approved = root.join("approved");
    let excluded = root.join("excluded");
    fs::create_dir(&approved).unwrap();
    fs::create_dir(&excluded).unwrap();
    let rollout = root.join("rollout-synthetic-fork.jsonl");
    let selection = ProjectSelection::new(vec![approved.clone()]).unwrap();
    let outside = [
        header("synthetic-fork", &excluded),
        header("synthetic-parent", &approved),
        header("synthetic-fork", &approved),
    ]
    .concat();
    fs::write(&rollout, &outside).unwrap();
    let candidate = candidate(&rollout, "synthetic-fork");
    assert!(selection.admit(&candidate).unwrap().is_none());

    fs::write(&rollout, header("synthetic-fork", &approved)).unwrap();
    let admitted = selection.admit(&candidate).unwrap().unwrap();
    fs::write(&rollout, outside).unwrap();
    let spool_path = root.join("spool");
    let mut spool = Spool::open(&spool_path, Limits::default()).unwrap();
    let identity = identity(&admitted, "synthetic-capture");
    let mut reserved = false;
    assert!(matches!(
        spool.capture(
            identity,
            &admitted,
            MetadataSections::new(),
            &mut Reservation(|_| reserved = true),
        ),
        Err(Error::SourceChanged)
    ));
    assert!(reserved);
    let entries = spool.entries().unwrap();
    assert_eq!(entries[0].state, CaptureState::Preparing);
    assert!(entries[0].manifest.is_none());
    assert_eq!(
        fs::read_dir(spool_path.join("synthetic-capture"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn mutation_during_reservation_interrupts_without_publishing() {
    let (_temporary, root) = temporary();
    let rollout = root.join("rollout-synthetic-fork.jsonl");
    fs::write(&rollout, FORK).unwrap();
    let candidate = candidate(&rollout, "synthetic-fork");
    let mut spool = Spool::open(&root.join("spool"), Limits::default()).unwrap();
    assert!(matches!(
        spool.capture(
            identity(&candidate, "synthetic-capture"),
            &candidate,
            MetadataSections::new(),
            &mut Reservation(|_| fs::write(&rollout, b"changed").unwrap()),
        ),
        Err(Error::SourceChanged)
    ));
    let entries = spool.entries().unwrap();
    assert_eq!(entries[0].state, CaptureState::Preparing);
    assert!(entries[0].manifest.is_none());
}

#![cfg(all(unix, feature = "cloud"))]

use antiburn_cloud_capture::*;
use flate2::read::GzDecoder;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

const ID: &str = "019b76da-a800-7123-8abc-0123456789ab";
const FILENAME: &str = "2026-01-01T00-00-00-000Z_019b76da-a800-7123-8abc-0123456789ab.jsonl";
const SESSION: &[u8] = include_bytes!(
    "fixtures/pi/2026-01-01T00-00-00-000Z_019b76da-a800-7123-8abc-0123456789ab.jsonl"
);

struct Reservation<F>(F);

type TransportResult<T> = std::result::Result<T, TransportError>;

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

fn temporary() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    (temporary, root)
}

fn identity(candidate: &Candidate) -> CaptureIdentity {
    CaptureIdentity {
        capture_id: "synthetic-capture".into(),
        account_id: "synthetic-account".into(),
        destination_id: "synthetic-destination".into(),
        source_id: candidate.source_id().into(),
    }
}

fn header(id: &str, cwd: &Path) -> Vec<u8> {
    let mut line = serde_json::to_vec(
        &serde_json::json!({"type": "session", "version": 3, "id": id, "cwd": cwd}),
    )
    .unwrap();
    line.push(b'\n');
    line
}

fn expanded(root: &Path, stream: &CapturedStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    for chunk in &stream.chunks {
        let file = fs::File::open(
            root.join("synthetic-capture")
                .join(format!("chunk-{}-{:05}.gz", stream.stream_id, chunk.index)),
        )
        .unwrap();
        GzDecoder::new(file).read_to_end(&mut bytes).unwrap();
    }
    bytes
}

#[test]
fn preserves_the_whole_session_without_inventing_worker_or_runtime_metadata() {
    let (_temporary, root) = temporary();
    let file = root.join(FILENAME);
    let working_dir = root.join("project");
    fs::create_dir(&working_dir).unwrap();
    let header_end = SESSION.iter().position(|&byte| byte == b'\n').unwrap();
    let mut first: serde_json::Value = serde_json::from_slice(&SESSION[..header_end]).unwrap();
    first["cwd"] = serde_json::json!(working_dir);
    let first = serde_json::to_vec(&first).unwrap();
    let bytes = [
        first.as_slice(),
        b"\r\n",
        &SESSION[header_end + 1..],
        &header("synthetic-copied-session", Path::new("/synthetic/copied")),
        b"\xff\n{\"incomplete\":",
    ]
    .concat();
    fs::write(&file, &bytes).unwrap();

    let candidate = reopen_pi(&file, ID, &Limits::default())
        .unwrap()
        .in_source_scope("synthetic-root")
        .unwrap();
    assert_eq!(candidate.source_kind(), CaptureSourceKind::PiJsonl);
    assert_eq!(candidate.source_path(), file);
    assert_eq!(candidate.native_session_id(), ID);
    assert_eq!(candidate.source_id(), format!("synthetic-root/pi/{ID}"));
    assert_eq!(candidate.native_agent_id(), None);
    let evidence = candidate.project_evidence().unwrap();
    assert_eq!(evidence.project_paths(), std::slice::from_ref(&working_dir));
    assert!(evidence.scan_complete());
    assert_eq!(evidence.source_digest(), None);
    let selection = ProjectSelection::new(vec![working_dir]).unwrap();
    let candidate = selection.admit(&candidate).unwrap().unwrap();

    let spool_path = root.join("spool");
    let mut spool = Spool::open(&spool_path, Limits::default()).unwrap();
    let entry = spool
        .capture(
            identity(&candidate),
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
    assert_eq!(transcript.schema, "pi-jsonl");
    assert_eq!(expanded(&spool_path, transcript), bytes);
    let metadata_bytes = expanded(&spool_path, &manifest.streams[1]);
    let metadata: CaptureMetadata = serde_json::from_slice(&metadata_bytes).unwrap();
    assert_eq!(metadata.agent, "pi");
    assert_eq!(metadata.source_format, "PiJsonl");
    assert_eq!(metadata.native_session_id, ID);
    assert_eq!(metadata.native_agent_id, None);
    assert_eq!(metadata.relationship, Relationship::Unknown);
    assert_eq!(metadata.source_agent_version, None);
    assert!(metadata.version_observations.is_empty());
    assert!(!metadata.version_scan_complete);
    assert!(!metadata.owned_version_complete);
    assert_eq!(metadata.history_completeness, "unknown");
    let schema = serde_json::from_str(include_str!(
        "../../../docs/cloud/capture-metadata.schema.json"
    ))
    .unwrap();
    jsonschema::draft202012::new(&schema)
        .unwrap()
        .validate(&serde_json::from_slice(&metadata_bytes).unwrap())
        .unwrap();

    drop(spool);
    fs::remove_file(file).unwrap();
    let spool = Spool::open(&spool_path, Limits::default()).unwrap();
    assert_eq!(spool.entries().unwrap()[0].manifest, entry.manifest);
    assert_eq!(expanded(&spool_path, transcript), bytes);
}

#[test]
fn requires_an_exact_file_suffix_and_the_first_owned_header() {
    let (_temporary, root) = temporary();
    let file = root.join(FILENAME);
    let limits = Limits::default();
    fs::write(&file, SESSION).unwrap();
    for name in [
        format!("{ID}.jsonl"),
        format!("_{ID}.jsonl"),
        format!("synthetic_prefix-{ID}.jsonl"),
        format!("synthetic_{ID}.jsonl.backup"),
    ] {
        let other = root.join(name);
        fs::write(&other, SESSION).unwrap();
        assert!(matches!(
            reopen_pi(&other, ID, &limits),
            Err(Error::InvalidSource)
        ));
    }
    assert!(matches!(
        reopen_pi(&file, "other", &limits),
        Err(Error::InvalidSource)
    ));

    for line in [
        "\n".to_owned(),
        format!("[\"session\",\"{ID}\",\"/synthetic/work/project\"]\n"),
        format!("{{\"type\":\"message\",\"id\":\"{ID}\"}}\n"),
        format!("{{\"type\":\"session\",\"sessionId\":\"{ID}\"}}\n"),
        format!("{{\"type\":\"session\",\"id\":\"other\",\"id\":\"{ID}\"}}\n"),
        format!("{{\"type\":\"session\",\"id\":\"{ID}\",\"cwd\":null,\"cwd\":\"/synthetic\"}}\n"),
        "{\"type\":\"session\",\"id\":\"other\"}\n".to_owned(),
    ] {
        fs::write(&file, [line.as_bytes(), SESSION].concat()).unwrap();
        assert!(matches!(
            reopen_pi(&file, ID, &limits),
            Err(Error::InvalidSource)
        ));
    }
    let end = SESSION.iter().position(|&byte| byte == b'\n').unwrap();
    fs::write(&file, &SESSION[..end]).unwrap();
    assert!(matches!(
        reopen_pi(&file, ID, &limits),
        Err(Error::InvalidSource)
    ));
}

#[test]
fn rejects_links_and_exceeded_source_or_header_bounds() {
    let (_temporary, root) = temporary();
    let parent = root.join("sessions");
    fs::create_dir(&parent).unwrap();
    let file = parent.join(FILENAME);
    let limits = Limits::default();
    let symlink = root.join(FILENAME);
    fs::write(&file, SESSION).unwrap();
    std::os::unix::fs::symlink(&file, &symlink).unwrap();
    assert!(reopen_pi(&symlink, ID, &limits).is_err());
    fs::remove_file(symlink).unwrap();
    std::os::unix::fs::symlink(&parent, root.join("alias")).unwrap();
    assert!(reopen_pi(&root.join("alias").join(FILENAME), ID, &limits).is_err());
    assert!(reopen_pi(&parent.join(".").join(FILENAME), ID, &limits).is_err());

    let hard_link = root.join(FILENAME);
    fs::hard_link(&file, &hard_link).unwrap();
    assert!(matches!(
        reopen_pi(&file, ID, &limits),
        Err(Error::InvalidSource)
    ));
    fs::remove_file(hard_link).unwrap();
    let lower = Limits {
        source_bytes: SESSION.len() as u64 - 1,
        ..limits.clone()
    };
    assert!(matches!(reopen_pi(&file, ID, &lower), Err(Error::Limit)));
    fs::write(&file, vec![b' '; 64 * 1024 + 1]).unwrap();
    assert!(matches!(reopen_pi(&file, ID, &limits), Err(Error::Limit)));
}

#[test]
fn rechecks_only_the_first_cwd_after_reservation() {
    let (_temporary, root) = temporary();
    let approved = root.join("approved");
    let excluded = root.join("approved-other");
    fs::create_dir(&approved).unwrap();
    fs::create_dir(&excluded).unwrap();
    let file = approved.join(FILENAME);
    let limits = Limits::default();
    let selection = ProjectSelection::new(vec![approved.clone()]).unwrap();
    let copied = header(ID, &approved);
    let outside = [header(ID, &excluded), copied.clone()].concat();
    fs::write(&file, &outside).unwrap();
    let candidate = reopen_pi(&file, ID, &limits).unwrap();
    assert!(selection.admit(&candidate).unwrap().is_none());

    fs::write(&file, &copied).unwrap();
    let admitted = selection.admit(&candidate).unwrap().unwrap();
    fs::write(&file, &outside).unwrap();
    let spool_path = root.join("spool");
    let mut spool = Spool::open(&spool_path, limits).unwrap();
    let mut reserved = false;
    assert!(matches!(
        spool.capture(
            identity(&admitted),
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

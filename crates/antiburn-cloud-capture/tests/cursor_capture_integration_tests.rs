#![cfg(all(unix, feature = "cloud"))]

//! Exercise Cursor capture using only the public synthetic source layouts.

use antiburn_cloud_capture::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::read::GzDecoder;
use md5::{Digest, Md5};
use rusqlite::Connection;
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

type TransportResult<T> = std::result::Result<T, TransportError>;

#[derive(Default)]
struct Reserve<'a> {
    calls: usize,
    before_reply: Option<&'a dyn Fn()>,
    expected_generation: Option<&'a str>,
}

impl Transport for Reserve<'_> {
    fn expected_file_generation(&self) -> Option<&str> {
        self.expected_generation
    }

    fn reserve(&mut self, _: &CaptureIdentity, _: u64) -> TransportResult<String> {
        self.calls += 1;
        if let Some(before_reply) = self.before_reply {
            before_reply();
        }
        Ok("synthetic-reservation".into())
    }

    fn negotiate(&mut self, _: &str, _: &CaptureManifest) -> TransportResult<UploadPlan> {
        panic!("unexpected negotiation")
    }

    fn status(
        &mut self,
        _: &str,
        _: &CaptureManifest,
        _: &UploadPlan,
    ) -> TransportResult<ServerStatus> {
        panic!("unexpected status request")
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
        panic!("unexpected part upload")
    }

    fn complete(&mut self, _: &str, _: &CaptureManifest, _: &UploadPlan) -> TransportResult<()> {
        panic!("unexpected completion")
    }
}

fn seed(path: &Path, sql: &str) -> Connection {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let db = Connection::open(path).unwrap();
    db.execute_batch(sql).unwrap();
    db
}

fn ide(root: &Path, project: &Path) -> (PathBuf, Connection, Connection) {
    let user = root.join("Cursor/User");
    let workspace = user.join("workspaceStorage/synthetic-workspace");
    let db = seed(
        &workspace.join("state.vscdb"),
        include_str!("../../../fixtures/cloud/cursor/ide-workspace.sql"),
    );
    let global = seed(
        &user.join("globalStorage/state.vscdb"),
        include_str!("../../../fixtures/cloud/cursor/ide-global.sql"),
    );
    write_workspace_folder(&workspace, project);
    (workspace, db, global)
}

fn write_workspace_folder(workspace: &Path, project: &Path) {
    fs::write(
        workspace.join("workspace.json"),
        serde_json::json!({ "folder": format!("file://{}", project.display()) }).to_string(),
    )
    .unwrap();
}

const CLI_TRANSCRIPT: &[u8] = include_bytes!("../../../fixtures/cloud/cursor/cli-agent.jsonl");

fn cli(root: &Path, project: &Path) -> (PathBuf, PathBuf) {
    let cwd = project.to_str().unwrap();
    let project_key = cwd.strip_prefix('/').unwrap().replace('/', "-");
    let workspace_key = Md5::digest(cwd)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let transcripts = root.join(format!(".cursor/projects/{project_key}/agent-transcripts"));
    let transcript = transcripts.join("synthetic-cli/synthetic-cli.jsonl");
    fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    fs::write(transcript, CLI_TRANSCRIPT).unwrap();
    let sidecar = root.join(format!(
        ".cursor/chats/{workspace_key}/synthetic-cli/meta.json"
    ));
    fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
    let mut metadata: Value =
        serde_json::from_str(include_str!("../../../fixtures/cloud/cursor/cli-meta.json")).unwrap();
    metadata["cwd"] = serde_json::json!(project);
    fs::write(&sidecar, metadata.to_string()).unwrap();
    (transcripts, sidecar)
}

fn test_root() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let project = root.join("project");
    fs::create_dir(&project).unwrap();
    (temporary, root, project)
}

fn identity(candidate: &Candidate) -> CaptureIdentity {
    CaptureIdentity {
        capture_id: "synthetic-capture".into(),
        account_id: "synthetic-account".into(),
        destination_id: "synthetic-destination".into(),
        source_id: candidate.source_id().into(),
    }
}

fn expanded(spool: &Path, manifest: &CaptureManifest, index: usize) -> Vec<u8> {
    let stream = &manifest.streams[index];
    let mut bytes = Vec::new();
    for chunk in &stream.chunks {
        let file = fs::File::open(
            spool
                .join(&manifest.identity.capture_id)
                .join(format!("chunk-{}-{:05}.gz", stream.stream_id, chunk.index)),
        )
        .unwrap();
        GzDecoder::new(file).read_to_end(&mut bytes).unwrap();
    }
    bytes
}

fn native_records(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn native_data(records: &[Value]) -> Vec<Vec<u8>> {
    records
        .iter()
        .filter(|value| value["kind"] == "data")
        .map(|value| STANDARD.decode(value["base64"].as_str().unwrap()).unwrap())
        .collect()
}

fn capture_metadata(path: &Path, manifest: &CaptureManifest) -> Value {
    let metadata: Value = serde_json::from_slice(&expanded(path, manifest, 1)).unwrap();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../docs/cloud/capture-metadata.schema.json"
    ))
    .unwrap();
    assert!(jsonschema::is_valid(&schema, &metadata));
    metadata
}

#[test]
fn ide_captures_only_exact_composer_and_bubble_rows_in_recorded_order() {
    let (_temporary, root, project) = test_root();
    let (workspace, _db, global) = ide(&root, &project);
    let limits = Limits {
        discovery_sessions: 1,
        ..Limits::default()
    };
    let page = discover_cursor_ide_page(&workspace, None, &limits).unwrap();
    assert_eq!(page.next_cursor.as_deref(), Some("synthetic-main"));
    let second =
        discover_cursor_ide_page(&workspace, page.next_cursor.as_deref(), &limits).unwrap();
    assert_eq!(second.candidates[0].native_session_id(), "synthetic-other");
    assert!(second.next_cursor.is_none());
    let candidate = page.candidates[0]
        .in_source_scope("workspace-scope")
        .unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    let admitted = selection.admit(&candidate).unwrap().unwrap();
    let digest = admitted.cursor_content_digest(&limits, &selection).unwrap();
    global
        .execute(
            "UPDATE cursorDiskKV SET value = 'unrelated change'
             WHERE key = 'bubbleId:synthetic-main:unreferenced'",
            [],
        )
        .unwrap();
    assert_eq!(
        admitted.cursor_content_digest(&limits, &selection).unwrap(),
        digest
    );
    let path = root.join("spool");
    let mut spool = Spool::open(&path, limits).unwrap();
    let entry = spool
        .capture(
            identity(&admitted),
            &admitted,
            MetadataSections::new(),
            &mut Reserve::default(),
        )
        .unwrap();
    let manifest = entry.manifest.unwrap();
    assert_eq!(manifest.streams[0].stream_id, "cursor-ide-rows");
    assert_eq!(manifest.streams[0].expanded_sha256, digest);
    let records = native_records(&expanded(&path, &manifest, 0));
    assert_eq!(records.last().unwrap()["kind"], "end");
    let data = native_data(&records);
    assert_eq!(data.len(), 6);
    assert_eq!(data[0], b"composerData:synthetic-main");
    assert_eq!(data[2], b"bubbleId:synthetic-main:z-user");
    assert_eq!(data[4], b"bubbleId:synthetic-main:a-assistant");
    assert!(records.iter().any(|value| value["type"] == "blob"));
    let metadata = capture_metadata(&path, &manifest);
    assert_eq!(metadata["agent"], "cursor");
    assert_eq!(metadata["source_format"], "CursorIdeComposer");
}

#[test]
fn ide_rejects_unowned_or_missing_bubbles_and_workspace_changes() {
    let (_temporary, root, project) = test_root();
    let (workspace, _db, global) = ide(&root, &project);
    let limits = Limits::default();
    let candidate = reopen_cursor_ide(&workspace, "synthetic-main", &limits).unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    let admitted = selection.admit(&candidate).unwrap().unwrap();
    global
        .execute(
            "DELETE FROM cursorDiskKV WHERE key = 'bubbleId:synthetic-main:z-user'",
            [],
        )
        .unwrap();
    assert!(matches!(
        admitted.cursor_content_digest(&limits, &selection),
        Err(Error::SourceChanged)
    ));
    let outside = root.join("outside");
    fs::create_dir(&outside).unwrap();
    let mutate = || write_workspace_folder(&workspace, &outside);
    let mut transport = Reserve {
        calls: 0,
        before_reply: Some(&mutate),
        ..Reserve::default()
    };
    let mut spool = Spool::open(&root.join("spool"), limits).unwrap();
    assert!(matches!(
        spool.capture(
            identity(&admitted),
            &admitted,
            MetadataSections::new(),
            &mut transport
        ),
        Err(Error::SourceChanged)
    ));
    assert_eq!(transport.calls, 1);
    assert!(spool.entries().unwrap()[0].manifest.is_none());
}

#[test]
fn cli_captures_exact_transcript_and_keeps_project_sidecar_private() {
    let (_temporary, root, project) = test_root();
    let (transcripts, sidecar) = cli(&root, &project);
    let limits = Limits::default();
    let candidate = reopen_cursor_cli(&transcripts, "synthetic-cli", &limits).unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    let admitted = selection.admit(&candidate).unwrap().unwrap();
    let path = root.join("spool");
    let mut spool = Spool::open(&path, limits.clone()).unwrap();
    let manifest = spool
        .capture(
            identity(&admitted),
            &admitted,
            MetadataSections::new(),
            &mut Reserve::default(),
        )
        .unwrap()
        .manifest
        .unwrap();
    assert_eq!(manifest.streams.len(), 2);
    assert_eq!(manifest.streams[0].stream_id, "transcript");
    assert_eq!(manifest.streams[0].schema, "cursor-cli-jsonl");
    assert_eq!(expanded(&path, &manifest, 0), CLI_TRANSCRIPT);
    let metadata = capture_metadata(&path, &manifest);
    assert_eq!(metadata["source_format"], "CursorCliAgentJsonl");

    fs::remove_file(sidecar).unwrap();
    assert!(matches!(
        reopen_cursor_cli(&transcripts, "synthetic-cli", &limits),
        Err(Error::Io(std::io::ErrorKind::NotFound))
    ));
}

#[test]
fn cli_discovery_ignores_transcript_mirrors_without_same_session_sidecar() {
    let (_temporary, root, project) = test_root();
    let (transcripts, _) = cli(&root, &project);
    let mirror = transcripts.join("mirror/mirror.jsonl");
    fs::create_dir_all(mirror.parent().unwrap()).unwrap();
    fs::write(mirror, CLI_TRANSCRIPT).unwrap();
    let limits = Limits {
        discovery_sessions: 1,
        ..Limits::default()
    };
    let first = discover_cursor_cli_page(&transcripts, None, &limits).unwrap();
    assert!(first.candidates.is_empty());
    assert!(!first.partial);
    assert_eq!(first.next_cursor.as_deref(), Some("mirror"));
    let second =
        discover_cursor_cli_page(&transcripts, first.next_cursor.as_deref(), &limits).unwrap();
    assert_eq!(second.candidates[0].native_session_id(), "synthetic-cli");
    assert!(!second.partial);
    assert!(second.next_cursor.is_none());
}

#[test]
fn cli_oversized_sidecar_leaves_a_partial_page_and_a_healthy_continuation() {
    let (_temporary, root, project) = test_root();
    let (transcripts, sidecar) = cli(&root, &project);
    let oversized = transcripts.join("oversized");
    fs::create_dir(&oversized).unwrap();
    fs::write(oversized.join("oversized.jsonl"), CLI_TRANSCRIPT).unwrap();
    let oversized_sidecar = sidecar.parent().unwrap().with_file_name("oversized");
    fs::create_dir(&oversized_sidecar).unwrap();
    fs::write(oversized_sidecar.join("meta.json"), vec![b' '; 65_537]).unwrap();

    let limits = Limits {
        discovery_sessions: 1,
        ..Limits::default()
    };
    let first = discover_cursor_cli_page(&transcripts, None, &limits).unwrap();
    assert!(first.partial);
    assert!(first.candidates.is_empty());
    assert_eq!(first.visited, 1);
    assert_eq!(first.next_cursor.as_deref(), Some("oversized"));
    let next =
        discover_cursor_cli_page(&transcripts, first.next_cursor.as_deref(), &limits).unwrap();
    assert!(!next.partial);
    assert_eq!(next.candidates[0].source_id(), "cursor/cli/synthetic-cli");
}

#[test]
fn cli_sidecar_without_a_conversation_is_not_an_incomplete_result() {
    let (_temporary, root, project) = test_root();
    let (transcripts, sidecar) = cli(&root, &project);
    let draft = transcripts.join("draft");
    fs::create_dir(&draft).unwrap();
    fs::write(draft.join("draft.jsonl"), CLI_TRANSCRIPT).unwrap();
    let draft_metadata = sidecar.parent().unwrap().with_file_name("draft");
    fs::create_dir(&draft_metadata).unwrap();
    fs::write(
        draft_metadata.join("meta.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "hasConversation": false,
            "cwd": project,
        })
        .to_string(),
    )
    .unwrap();

    let limits = Limits::default();
    let page = discover_cursor_cli_page(&transcripts, None, &limits).unwrap();
    assert!(!page.partial);
    assert_eq!(page.candidates.len(), 1);
    assert_eq!(page.candidates[0].source_id(), "cursor/cli/synthetic-cli");
    assert!(matches!(
        reopen_cursor_cli(&transcripts, "draft", &limits),
        Err(Error::UnsupportedSource)
    ));
}

#[test]
fn ide_excludes_malformed_keys_and_keeps_pages_with_ambiguous_index_proof() {
    let (_temporary, root, project) = test_root();
    let (workspace, _local, global) = ide(&root, &project);
    global
        .execute_batch(
            "CREATE TABLE composerHeaders(composerId TEXT, workspaceId TEXT);
             INSERT INTO composerHeaders VALUES
               ('duplicate', 'synthetic-workspace'), ('duplicate', 'synthetic-workspace');
             INSERT INTO cursorDiskKV VALUES ('composerData:duplicate', '{}');",
        )
        .unwrap();
    global
        .execute(
            "INSERT INTO cursorDiskKV VALUES (?1, '{}')",
            ["composerData:a\0invalid"],
        )
        .unwrap();
    let limits = Limits {
        discovery_sessions: 1,
        ..Limits::default()
    };
    let first = discover_cursor_ide_page(&workspace, None, &limits).unwrap();
    assert!(first.partial);
    assert!(first.candidates.is_empty());
    assert_eq!(first.next_cursor.as_deref(), Some("duplicate"));
    let next = discover_cursor_ide_page(&workspace, first.next_cursor.as_deref(), &limits).unwrap();
    assert!(!next.partial);
    assert_eq!(next.candidates[0].source_id(), "cursor/ide/synthetic-main");
}

#[test]
fn cli_rechecks_exact_sidecar_after_reservation_and_rejects_duplicate_owners() {
    let (_temporary, root, project) = test_root();
    let (transcripts, sidecar) = cli(&root, &project);
    let limits = Limits::default();
    let candidate = reopen_cursor_cli(&transcripts, "synthetic-cli", &limits).unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    let admitted = selection.admit(&candidate).unwrap().unwrap();
    let duplicate = root.join(".cursor/chats/other-workspace/synthetic-cli/meta.json");
    let mutate = || {
        fs::create_dir_all(duplicate.parent().unwrap()).unwrap();
        fs::copy(&sidecar, &duplicate).unwrap();
    };
    let mut transport = Reserve {
        calls: 0,
        before_reply: Some(&mutate),
        ..Reserve::default()
    };
    let mut spool = Spool::open(&root.join("spool"), limits).unwrap();
    assert!(matches!(
        spool.capture(
            identity(&admitted),
            &admitted,
            MetadataSections::new(),
            &mut transport
        ),
        Err(Error::UnsupportedSource)
    ));
    assert_eq!(transport.calls, 1);
    assert!(spool.entries().unwrap()[0].manifest.is_none());
}

#[test]
fn cli_pins_transcript_before_reservation_and_rejects_invalid_framing() {
    let (_temporary, root, project) = test_root();
    let (transcripts, _) = cli(&root, &project);
    let limits = Limits::default();
    let candidate = reopen_cursor_cli(&transcripts, "synthetic-cli", &limits).unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    let admitted = selection.admit(&candidate).unwrap().unwrap();
    let transcript = admitted.source_path();
    let replacement = transcript.with_extension("new");
    fs::write(&replacement, CLI_TRANSCRIPT).unwrap();
    let mutate = || fs::rename(&replacement, transcript).unwrap();
    let mut transport = Reserve {
        calls: 0,
        before_reply: Some(&mutate),
        ..Reserve::default()
    };
    let mut spool = Spool::open(&root.join("spool"), limits.clone()).unwrap();
    assert!(matches!(
        spool.capture(
            identity(&admitted),
            &admitted,
            MetadataSections::new(),
            &mut transport
        ),
        Err(Error::SourceChanged)
    ));
    assert_eq!(transport.calls, 1);
    assert!(spool.entries().unwrap()[0].manifest.is_none());
    fs::write(transcript, [CLI_TRANSCRIPT, b"not a record\n"].concat()).unwrap();
    assert!(matches!(
        admitted.cursor_content_digest(&limits, &selection),
        Err(Error::InvalidSource)
    ));
}

#[test]
fn cli_refuses_a_different_scheduler_generation_before_reservation() {
    let (_temporary, root, project) = test_root();
    let (transcripts, _) = cli(&root, &project);
    let limits = Limits::default();
    let candidate = reopen_cursor_cli(&transcripts, "synthetic-cli", &limits).unwrap();
    let admitted = ProjectSelection::new(vec![project])
        .unwrap()
        .admit(&candidate)
        .unwrap()
        .unwrap();
    let transcript = admitted.source_path();
    let generation = file_generation_key(&[fs::metadata(transcript).unwrap()]).unwrap();
    fs::write(transcript, [CLI_TRANSCRIPT, b"{}\n"].concat()).unwrap();
    let mut transport = Reserve {
        expected_generation: Some(&generation),
        ..Reserve::default()
    };
    let mut spool = Spool::open(&root.join("spool"), limits).unwrap();
    assert!(matches!(
        spool.capture(
            identity(&admitted),
            &admitted,
            MetadataSections::new(),
            &mut transport
        ),
        Err(Error::FileGenerationChanged)
    ));
    assert_eq!(transport.calls, 0);
    assert!(spool.entries().unwrap().is_empty());
}

#[test]
fn migrated_ide_focus_list_cannot_assign_another_composers_workspace() {
    let (_temporary, root, project) = test_root();
    let (workspace, db, _global) = ide(&root, &project);
    db.execute(
        "UPDATE ItemTable SET value = ?1 WHERE key = 'composer.composerData'",
        [r#"{"hasMigratedComposerData":true,"selectedComposerIds":["synthetic-main","synthetic-other"]}"#],
    )
    .unwrap();
    let limits = Limits::default();
    let candidate = reopen_cursor_ide(&workspace, "synthetic-other", &limits).unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    assert!(selection.admit(&candidate).unwrap().is_none());
    let candidate = reopen_cursor_ide(&workspace, "synthetic-main", &limits).unwrap();
    assert!(selection.admit(&candidate).unwrap().is_some());
}

#[test]
fn desktop_composer_keeps_exact_sparse_chunks_then_ordered_bubbles() {
    let (_temporary, root, project) = test_root();
    let (workspace, db, global) = ide(&root, &project);
    db.execute(
        "UPDATE ItemTable SET value = ?1 WHERE key = 'composer.composerData'",
        [r#"{"hasMigratedComposerData":false,"selectedComposerIds":["synthetic-other"],"lastFocusedComposerIds":["synthetic-other"]}"#],
    )
    .unwrap();
    global
        .execute_batch(include_str!(
            "../../../fixtures/cloud/cursor/ide-chunked.sql"
        ))
        .unwrap();
    let limits = Limits::default();
    let page = discover_cursor_ide_page(&workspace, None, &limits).unwrap();
    let candidate = page
        .candidates
        .iter()
        .find(|source| source.native_session_id() == "synthetic-chunked")
        .unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    let admitted = selection.admit(candidate).unwrap().unwrap();
    let path = root.join("spool");
    let mut spool = Spool::open(&path, limits).unwrap();
    let manifest = spool
        .capture(
            identity(&admitted),
            &admitted,
            MetadataSections::new(),
            &mut Reserve::default(),
        )
        .unwrap()
        .manifest
        .unwrap();
    let records = native_data(&native_records(&expanded(&path, &manifest, 0)));
    let keys = records
        .iter()
        .step_by(2)
        .map(Vec::as_slice)
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        [
            b"composerData:synthetic-chunked".as_slice(),
            b"composerConvHeaders:synthetic-chunked:000000",
            b"composerConvHeaders:synthetic-chunked:000001",
            b"bubbleId:synthetic-chunked:z-user",
            b"bubbleId:synthetic-chunked:a-assistant",
        ]
    );
    assert_eq!(records[3], br#"[ {"bubbleId":"z-user","type":1} ]"#);
    assert_eq!(
        capture_metadata(&path, &manifest)["source_format"],
        "CursorIdeComposer"
    );
}

#[test]
fn desktop_chunked_composer_requires_current_exact_workspace_index() {
    let (_temporary, root, project) = test_root();
    let (workspace, _db, global) = ide(&root, &project);
    global
        .execute_batch(include_str!(
            "../../../fixtures/cloud/cursor/ide-chunked.sql"
        ))
        .unwrap();
    let limits = Limits::default();
    let candidate = reopen_cursor_ide(&workspace, "synthetic-chunked", &limits).unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    global
        .execute(
            "UPDATE composerHeaders SET workspaceId='foreign-workspace'
             WHERE composerId='synthetic-chunked'",
            [],
        )
        .unwrap();
    assert!(selection.admit(&candidate).unwrap().is_none());
    global.execute(
        "UPDATE composerHeaders SET workspaceId='synthetic-workspace' WHERE composerId='synthetic-chunked'",
        [],
    ).unwrap();
    let admitted = selection.admit(&candidate).unwrap().unwrap();
    let path = root.join("spool");
    let mut spool = Spool::open(&path, limits).unwrap();
    let mutate = || {
        global
            .execute(
                "DELETE FROM composerHeaders WHERE composerId='synthetic-chunked'",
                [],
            )
            .unwrap();
    };
    let mut transport = Reserve {
        before_reply: Some(&mutate),
        ..Reserve::default()
    };
    assert!(matches!(
        spool.capture(
            identity(&admitted),
            &admitted,
            MetadataSections::new(),
            &mut transport
        ),
        Err(Error::SourceChanged)
    ));
    assert_eq!(transport.calls, 1);
    assert!(spool.entries().unwrap()[0].manifest.is_none());
}

#[test]
fn desktop_chunked_composer_rejects_invalid_counts_refs_and_roles() {
    for mutation in [
        "UPDATE cursorDiskKV SET value=json_set(value,'$.headerLog.count',3) WHERE key='composerData:synthetic-chunked'",
        "UPDATE cursorDiskKV SET value=json_set(value,'$.headerLog.chunkCount',257) WHERE key='composerData:synthetic-chunked'",
        "DELETE FROM cursorDiskKV WHERE key='composerConvHeaders:synthetic-chunked:000000'",
        "DELETE FROM cursorDiskKV WHERE key='bubbleId:synthetic-chunked:z-user'",
        r#"UPDATE cursorDiskKV SET value='[{"bubbleId":"z-user","type":1}]' WHERE key='composerConvHeaders:synthetic-chunked:000001'"#,
        "UPDATE cursorDiskKV SET value=json_set(value,'$.type',2) WHERE key='bubbleId:synthetic-chunked:z-user'",
    ] {
        let (_temporary, root, project) = test_root();
        let (workspace, _db, global) = ide(&root, &project);
        global
            .execute_batch(include_str!(
                "../../../fixtures/cloud/cursor/ide-chunked.sql"
            ))
            .unwrap();
        let limits = Limits::default();
        let candidate = reopen_cursor_ide(&workspace, "synthetic-chunked", &limits).unwrap();
        let selection = ProjectSelection::new(vec![project]).unwrap();
        let admitted = selection.admit(&candidate).unwrap().unwrap();
        global.execute_batch(mutation).unwrap();
        assert!(
            matches!(
                admitted.cursor_content_digest(&limits, &selection),
                Err(Error::UnsupportedSource | Error::SourceChanged | Error::Limit)
            ),
            "{mutation}"
        );
    }
}

#[test]
fn ide_workspace_replacement_cannot_bind_new_rows_to_prior_source() {
    let (_temporary, root, project) = test_root();
    let (workspace, _db, _global) = ide(&root, &project);
    let limits = Limits::default();
    let candidate = reopen_cursor_ide(&workspace, "synthetic-main", &limits).unwrap();
    let selection = ProjectSelection::new(vec![project.clone()]).unwrap();
    assert!(selection.admit(&candidate).unwrap().is_some());
    fs::rename(&workspace, root.join("moved-workspace")).unwrap();
    let _replacement = seed(
        &workspace.join("state.vscdb"),
        include_str!("../../../fixtures/cloud/cursor/ide-workspace.sql"),
    );
    write_workspace_folder(&workspace, &project);
    assert!(matches!(
        candidate.cursor_content_digest(&limits, &selection),
        Err(Error::SourceChanged)
    ));
}

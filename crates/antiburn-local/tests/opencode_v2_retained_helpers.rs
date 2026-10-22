use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
use antiburn_local::analysis::jev_evidence::*;
use antiburn_local::analysis::session_scope::{
    SCOPE_SELECTION, ScopeAuthority, ScopeMissingReason, SessionScopeBoundary, SessionScopeBranch,
    SessionScopeBuilder,
};
use antiburn_local::analysis::*;
use antiburn_local::checks::skill_opportunities::*;
use antiburn_local::model::AgentKind;
use antiburn_local::pricing::ModelTokens;
use rusqlite::{Connection, params};
use serde_json::{Value, json};

const READ_PIN: &str = "opencode/tool/plugin/read.ts@843a6c514e606b436a7fe713422ace0a93ad3d61";
const READ_SELECTION: JevInputSelection = JevInputSelection::from_fields(&[
    JevInputField::ReadFilePath,
    JevInputField::ReadFileRequest,
    JevInputField::ReadFileResult,
]);

fn key() -> TurnSessionKey<'static> {
    TurnSessionKey {
        environment_key: "native",
        agent: "opencode",
        session_id: "root",
    }
}

struct FileTurnRowStore(Mutex<Connection>);

impl FileTurnRowStore {
    fn create(path: &Path) -> Arc<Self> {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
             CREATE TABLE session (environment_key TEXT, agent TEXT, session_id TEXT,
                 PRIMARY KEY (environment_key, agent, session_id));
             INSERT INTO session VALUES ('native', 'opencode', 'root');",
            )
            .unwrap();
        for migration in TURN_MIGRATIONS {
            connection.execute_batch(migration).unwrap();
        }
        Arc::new(Self(Mutex::new(connection)))
    }
}

impl TurnRowStore for FileTurnRowStore {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError> {
        insert_turn_rows(&self.0.lock().unwrap(), &key(), 1, rows).map_err(Into::into)
    }
    fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
        query_turn_facts(&self.0.lock().unwrap(), &key(), &FenceScope::single(1))
            .map_err(Into::into)
    }
    fn query_model_breakdown(&self) -> Result<BTreeMap<String, ModelTokens>, TurnRowError> {
        query_model_breakdown(&self.0.lock().unwrap(), &key(), &FenceScope::single(1))
            .map_err(Into::into)
    }
    fn query_model_runs(&self) -> Result<Vec<ModelRun>, TurnRowError> {
        query_model_runs(&self.0.lock().unwrap(), &key(), &FenceScope::single(1))
            .map_err(Into::into)
    }
    fn write_coverage_record(&self, record: &SessionCoverageRecord) -> Result<(), TurnRowError> {
        insert_coverage_record(&self.0.lock().unwrap(), &key(), 1, record).map_err(Into::into)
    }
    fn query_coverage_record(&self) -> Result<Option<SessionCoverageRecord>, TurnRowError> {
        query_coverage_record(&self.0.lock().unwrap(), &key(), &FenceScope::single(1))
            .map_err(Into::into)
    }
    fn read_resume(&self, source_key: &str) -> Result<Option<StoredResume>, TurnRowError> {
        query_source_resume(&self.0.lock().unwrap(), &key(), source_key).map_err(Into::into)
    }
    fn write_resume(&self, source_key: &str, resume: StoredResume) -> Result<(), TurnRowError> {
        insert_source_resume(&self.0.lock().unwrap(), &key(), source_key, &resume)
            .map_err(Into::into)
    }
    fn drop_resume(&self, source_key: &str) -> Result<(), TurnRowError> {
        delete_source_resume(&self.0.lock().unwrap(), &key(), source_key).map_err(Into::into)
    }
    fn delete_rows_for_source(&self, source_key: &str) -> Result<(), TurnRowError> {
        delete_source_rows_at_fence(&self.0.lock().unwrap(), &key(), source_key, 1)
            .map(|_| ())
            .map_err(Into::into)
    }
}

fn assistant(content: Vec<Value>) -> Value {
    json!({
        "id":"message", "type":"assistant", "time":{"created":1767225601000i64},
        "agent":"build", "model":{"id":"model", "providerID":"provider"},
        "tokens":{"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}},
        "content":content
    })
}

fn tool(name: &str, input: Value, body: &str) -> Value {
    json!({"type":"tool","id":"call","name":name,
        "state":{"status":"completed","input":input,
            "content":[{"type":"text","text":body}]}})
}

fn published(
    source_format: SourceFormat,
    messages: Vec<Value>,
    selection: JevInputSelection,
) -> PublishedContent {
    let directory = tempfile::tempdir().unwrap();
    let native = directory.path().join("source.db");
    let source = if source_format == SourceFormat::OpenCodeSqliteV2 {
        let connection = Connection::open(&native).unwrap();
        connection.execute_batch(
            "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, parent_id TEXT,
                directory TEXT NOT NULL, title TEXT NOT NULL DEFAULT '',
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL);
             CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                type TEXT NOT NULL, seq INTEGER NOT NULL, data TEXT NOT NULL,
                time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL);
             CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message(session_id, seq);
              INSERT INTO session_v2 VALUES ('root', NULL, '/synthetic', '', 0, 0);",
        ).unwrap();
        for (index, message) in messages.iter().enumerate() {
            let mut data = message.clone();
            data.as_object_mut().unwrap().remove("id");
            data.as_object_mut().unwrap().remove("type");
            connection
                .execute(
                    "INSERT INTO session_message VALUES (?1, 'root', ?2, ?3, ?4, 0, 0)",
                    params![
                        message["id"].as_str().unwrap(),
                        message["type"].as_str().unwrap(),
                        index as i64,
                        data.to_string()
                    ],
                )
                .unwrap();
        }
        RawSource::Sqlite(native)
    } else {
        RawSource::Jsonl(json!({"info":{"id":"root"},"messages":messages}).to_string())
    };
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "root".into(),
        source,
        source_format,
        fork_parent_session_id: None,
    };
    let retained = directory.path().join("retained.db");
    let store = FileTurnRowStore::create(&retained);
    let mut sink = TurnRowSink::new(Arc::clone(&store) as Arc<dyn TurnRowStore>, "root", None);
    reader_for_input(&input).visit(&input, &mut sink).unwrap();
    assert!(!sink.has_error());
    drop(sink);
    drop(store);
    let connection = Connection::open(&retained).unwrap();
    query_turn_content_offset_selected(
        &connection,
        &key(),
        &FenceScope::single(1),
        None,
        &Default::default(),
        0,
        selection,
    )
    .unwrap()
}

fn prepared(
    source: SourceFormat,
    messages: Vec<Value>,
    selection: JevInputSelection,
) -> SessionContentEvidence {
    prepare_session_content(
        "session",
        source,
        published(source, messages, selection),
        vec![],
    )
}

fn boundary(content: &SessionContentEvidence) -> SkillUseBoundary {
    SkillUseBoundary {
        session_identity: content.session_identity_digest.clone(),
        native_session_id: "root".into(),
        publication_fence: content.publication_fence,
        scope: SkillScope {
            agent: AgentKind::OpenCode,
            project_identity: None,
            environment_identity: "native".into(),
        },
    }
}

#[test]
fn file_read_pin_and_range_survive_durable_selected_publication() {
    for source in [
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        let content = prepared(
            source,
            vec![assistant(vec![
                json!({"type":"text","text":"answer"}),
                tool(
                    "read",
                    json!({"path":"src/lib.rs","offset":7,"limit":2}),
                    "Read file src/lib.rs, lines 7-8\n7: first\n8: second",
                ),
            ])],
            READ_SELECTION,
        );
        let request = content
            .actions
            .iter()
            .find_map(|action| action.metadata.read_request.as_ref())
            .unwrap();
        let result = content
            .actions
            .iter()
            .find_map(|action| action.metadata.read_result.as_ref())
            .unwrap();
        assert_eq!(
            request.extent_contract.as_deref(),
            Some(READ_PIN),
            "{content:#?}"
        );
        assert_eq!(request.extent.unit, JevReadUnit::Lines);
        assert_eq!(request.extent.offset, Some(7));
        assert_eq!(request.extent.limit, Some(2));
        assert_eq!(result.extent_contract.as_deref(), Some(READ_PIN));
        assert_eq!(
            result.returned_extent,
            Some(JevReadExtent {
                unit: JevReadUnit::Lines,
                offset: Some(7),
                limit: Some(2),
                end_inclusive: Some(8)
            })
        );
        assert_eq!(result.status, JevReadStatus::Success);
        assert_eq!(
            result.request_reference_id.as_deref(),
            Some(request.reference_id.as_str())
        );
        assert_eq!(result.recorded_file_version, None);
    }
}

#[test]
fn selected_request_without_raw_text_retains_only_bound_v2_ranges() {
    let source = SourceFormat::OpenCodeSqliteV2;
    let messages = vec![assistant(vec![tool(
        "read",
        json!({"path":"src/lib.rs","offset":7,"limit":2}),
        "Read file src/lib.rs, lines 7-8\n7: first\n8: second",
    )])];
    let selection = JevInputSelection::from_fields(&[
        JevInputField::ReadFileRequest,
        JevInputField::ReadFileResult,
    ]);
    let page = published(source, messages, selection);
    assert!(
        page.parts
            .iter()
            .find(|part| part.part.kind == ContentKind::ToolInput)
            .unwrap()
            .part
            .text
            .is_empty()
    );
    let content = prepare_session_content("session", source, page, vec![]);
    let request = content
        .actions
        .iter()
        .find_map(|action| action.metadata.read_request.as_ref())
        .unwrap();
    assert_eq!(request.extent_contract.as_deref(), Some(READ_PIN));
    assert_eq!(request.extent.offset, Some(7));
    assert!(content.actions.iter().any(|action| {
        action
            .metadata
            .read_result
            .as_ref()
            .is_some_and(|result| result.returned_extent.is_some())
    }));
}

#[test]
fn mismatched_and_native_alternate_filenames_do_not_borrow_requested_ranges() {
    for source in [
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        for selection in [
            READ_SELECTION,
            JevInputSelection::from_fields(&[
                JevInputField::ReadFileRequest,
                JevInputField::ReadFileResult,
            ]),
        ] {
            for (requested, returned) in [
                ("src/lib.rs", "src/other.rs"),
                ("docs/reader's guide.md", "docs/reader\u{2019}s guide.md"),
            ] {
                let body = format!("Read file {returned}, lines 7-8\n7: first\n8: second");
                let content = prepared(
                    source,
                    vec![assistant(vec![tool(
                        "read",
                        json!({"path":requested,"offset":7,"limit":2}),
                        &body,
                    )])],
                    selection,
                );
                let request = content
                    .actions
                    .iter()
                    .find_map(|action| action.metadata.read_request.as_ref())
                    .unwrap();
                let action = content
                    .actions
                    .iter()
                    .find(|action| action.metadata.read_result.is_some())
                    .unwrap();
                let result = action.metadata.read_result.as_ref().unwrap();
                assert_eq!(request.paths, [requested]);
                assert_eq!(action.text, body);
                assert_eq!(result.status, JevReadStatus::Success);
                assert_eq!(result.kind, JevReadResultKind::File);
                assert_eq!(result.returned_extent, None, "{content:#?}");
                assert_eq!(result.recorded_file_version, None);
            }
        }
    }
}

#[test]
fn retained_read_proofs_cannot_attach_a_range_to_a_different_returned_filename() {
    let source = SourceFormat::OpenCodeSqliteV2;
    let mut page = published(
        source,
        vec![assistant(vec![tool(
            "read",
            json!({"path":"src/lib.rs","offset":7,"limit":2}),
            "Read file src/lib.rs, lines 7-8\n7: first\n8: second",
        )])],
        READ_SELECTION,
    );
    let part = page
        .parts
        .iter_mut()
        .find(|part| part.part.kind == ContentKind::ToolResult)
        .unwrap();
    part.part.text = "Read file src/new.rs, lines 7-8\n7: first\n8: second".into();
    let result = part.part.metadata.read_result.as_mut().unwrap();
    result.recorded_output_digest =
        antiburn_local::checks::ignored_instructions::sha256_hex(part.part.text.as_bytes());
    let content = prepare_session_content("session", source, page, vec![]);
    let result = content
        .actions
        .iter()
        .find_map(|action| action.metadata.read_result.as_ref())
        .unwrap();
    assert_eq!(result.returned_extent, None, "{content:#?}");
    assert_eq!(result.extent_contract, None);
}

#[test]
fn missing_and_background_output_stays_partial_after_durable_selected_publication() {
    for source in [
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        for (body, state) in [
            ("Shell command output is no longer available.", "completed"),
            ("[Old tool result content cleared]", "completed"),
            ("Command moved to the background", "running"),
        ] {
            let mut shell = tool("shell", json!({"command":"synthetic"}), body);
            shell["state"]["metadata"] = json!({"status":state,"exit":7,"truncated":false});
            let selection = JevInputSelection::from_fields(&[JevInputField::BashCommandOutput]);
            let page = published(source, vec![assistant(vec![shell])], selection);
            assert_eq!(page.coverage.stored_truncated_parts, 1, "{page:#?}");
            let content = select_session_content(
                &prepare_session_content("session", source, page, vec![]),
                selection,
            );
            assert_eq!(content.actions.len(), 1);
            let action = &content.actions[0];
            assert_eq!(action.text, body);
            assert!(action.truncated, "{content:#?}");
            assert_eq!(action.metadata.state, JevOperationState::Completed);
            assert_eq!(
                action.metadata.outcome,
                Some(NativeToolOutcome::Shell {
                    state: Some(if state == "running" {
                        ShellState::Running
                    } else {
                        ShellState::Completed
                    }),
                    exit: Some(7),
                    signal: None,
                    timed_out: None,
                    shell_id: None,
                })
            );
            assert!(!content.complete, "{content:#?}");
            assert!(
                content
                    .limitations
                    .iter()
                    .any(|value| value == "truncated_source_content")
            );
        }
    }
}

#[test]
fn migrated_cleared_tool_content_stays_partial_without_rewriting_legacy_arguments() {
    for source in [
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        let migrated = json!({
            "type":"tool", "id":"legacy-read", "name":"read",
            "state":{
                "status":"completed", "input":{"filePath":"src/lib.rs"},
                "content":[{"type":"text","text":"[Old tool result content cleared]"}],
                "metadata":{}
            },
            "time":{"created":100,"completed":101}
        });
        let content = prepared(source, vec![assistant(vec![migrated])], READ_SELECTION);
        let action = content
            .actions
            .iter()
            .find(|action| action.kind == "tool_result")
            .unwrap();
        assert_eq!(action.text, "[Old tool result content cleared]");
        assert!(action.truncated, "{content:#?}");
        assert_eq!(action.metadata.state, JevOperationState::Completed);
        assert!(
            action
                .metadata
                .read_result
                .as_ref()
                .unwrap()
                .returned_extent
                .is_none()
        );
        assert!(
            content
                .actions
                .iter()
                .all(|action| action.metadata.read_request.is_none())
        );
        assert!(!content.complete);
    }
}

#[test]
fn unavailable_shell_message_output_stays_partial_and_keeps_the_program_outcome() {
    for source in [
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        let body = "Shell command output is no longer available.";
        let shell = json!({
            "id":"shell-message", "type":"shell", "time":{"created":100},
            "shellID":"sh_missing", "command":"synthetic", "status":"exited", "exit":7,
            "output":{"output":body,"truncated":false,"cursor":body.len(),"size":body.len()}
        });
        let selection = JevInputSelection::from_fields(&[JevInputField::BashCommandOutput]);
        let content = prepared(source, vec![shell], selection);
        assert_eq!(content.actions.len(), 1);
        let action = &content.actions[0];
        assert_eq!(action.text, body);
        assert!(action.truncated);
        assert_eq!(action.metadata.state, JevOperationState::Completed);
        assert_eq!(
            action.metadata.outcome,
            Some(NativeToolOutcome::Shell {
                state: Some(ShellState::Exited),
                exit: Some(7),
                signal: None,
                timed_out: None,
                shell_id: Some("sh_missing".into()),
            })
        );
        assert!(!content.complete, "{content:#?}");
    }
}

#[test]
fn directories_and_media_do_not_prove_file_body_ranges_or_versions() {
    for body in [
        "Read directory src, entries 4-5\na.rs\nb.rs",
        "Image read successfully",
        "PDF read successfully",
    ] {
        let content = prepared(
            SourceFormat::OpenCodeSqliteV2,
            vec![assistant(vec![tool(
                "read",
                json!({"path":"src","offset":4,"limit":2}),
                body,
            )])],
            READ_SELECTION,
        );
        let request = content
            .actions
            .iter()
            .find_map(|action| action.metadata.read_request.as_ref())
            .unwrap();
        let result = content
            .actions
            .iter()
            .find_map(|action| action.metadata.read_result.as_ref())
            .unwrap();
        assert_eq!(request.extent.unit, JevReadUnit::Unknown, "{content:#?}");
        assert_eq!(result.returned_extent, None);
        assert_eq!(result.recorded_file_version, None);
        assert_ne!(result.kind, JevReadResultKind::File);
    }
}

#[test]
fn altered_native_proofs_cannot_promote_v2_read_output() {
    let source = SourceFormat::OpenCodeSqliteV2;
    let page = published(
        source,
        vec![assistant(vec![tool(
            "read",
            json!({"path":"src/lib.rs"}),
            "Read file src/lib.rs, lines 1-1\n1: first",
        )])],
        READ_SELECTION,
    );
    for mutation in 0..8 {
        let mut changed = page.clone();
        let part = changed
            .parts
            .iter_mut()
            .find(|part| part.part.kind == ContentKind::ToolResult)
            .unwrap();
        match mutation {
            0 => {
                part.part
                    .metadata
                    .read_result
                    .as_mut()
                    .unwrap()
                    .extent_contract =
                    Some("opencode/read.ts@652c090dc119b5f3dc1e5e0bf1c4b40d9721f0ef".into())
            }
            1 => part.part.metadata.bindings[0].pointer = "/content/0/state.content/0/text".into(),
            2 => part.part.metadata.bindings[0].native_record_id = Some("other".into()),
            3 => part.part.tool_call_id = Some("other".into()),
            4 => part.thread_id = "other".into(),
            5 => {
                part.part
                    .metadata
                    .read_result
                    .as_mut()
                    .unwrap()
                    .recorded_output_digest = "0".repeat(64)
            }
            6 => {
                part.part
                    .metadata
                    .read_result
                    .as_mut()
                    .unwrap()
                    .returned_extent
                    .as_mut()
                    .unwrap()
                    .offset = Some(2)
            }
            7 => {
                part.part
                    .metadata
                    .read_result
                    .as_mut()
                    .unwrap()
                    .recorded_file_version = Some("invented".into())
            }
            _ => unreachable!(),
        }
        let content = prepare_session_content("session", source, changed, vec![]);
        let result = content
            .actions
            .iter()
            .find_map(|action| action.metadata.read_result.as_ref())
            .unwrap();
        assert_eq!(
            result.extent_contract, None,
            "mutation {mutation}: {content:#?}"
        );
        assert_eq!(result.returned_extent, None);
    }
}

#[test]
fn unbound_v2_requests_and_duplicate_calls_cannot_prove_returned_ranges() {
    let source = SourceFormat::OpenCodeSqliteV2;
    let page = published(
        source,
        vec![assistant(vec![tool(
            "read",
            json!({"path":"src/lib.rs","offset":7,"limit":2}),
            "Read file src/lib.rs, lines 7-8\n7: first\n8: second",
        )])],
        READ_SELECTION,
    );
    for mutation in 0..7 {
        let mut changed = page.clone();
        let request = changed
            .parts
            .iter_mut()
            .find(|part| part.part.kind == ContentKind::ToolInput)
            .unwrap();
        match mutation {
            0 => {
                request
                    .part
                    .metadata
                    .read_request
                    .as_mut()
                    .unwrap()
                    .extent_contract = Some("wrong-pin".into())
            }
            1 => {
                for range in &mut request.part.metadata.bindings {
                    range.pointer = range.pointer.replace("/content/0/", "/content/00/");
                }
            }
            2 => request.part.metadata.bindings.clear(),
            3 => {
                request
                    .part
                    .metadata
                    .read_request
                    .as_mut()
                    .unwrap()
                    .native_extent
                    .insert("start_line".into(), json!(7));
            }
            4 => {
                request
                    .part
                    .metadata
                    .read_request
                    .as_mut()
                    .unwrap()
                    .extent
                    .offset = Some(8)
            }
            5 => {
                request.part.metadata.read_request.as_mut().unwrap().paths[0] =
                    "different.rs".into()
            }
            6 => {
                let duplicate = request.clone();
                changed.parts.insert(0, duplicate);
            }
            _ => unreachable!(),
        }
        let content = prepare_session_content("session", source, changed, vec![]);
        let result = content
            .actions
            .iter()
            .find_map(|action| action.metadata.read_result.as_ref())
            .unwrap();
        assert_eq!(
            result.returned_extent, None,
            "mutation {mutation}: {content:#?}"
        );
        assert_eq!(result.extent_contract, None);
    }
}

#[test]
fn request_units_remain_unknown_when_the_file_result_is_not_selected() {
    let selection = JevInputSelection::from_fields(&[
        JevInputField::ReadFilePath,
        JevInputField::ReadFileRequest,
    ]);
    let content = prepared(
        SourceFormat::OpenCodeSqliteV2,
        vec![assistant(vec![tool(
            "read",
            json!({"path":"src/lib.rs","offset":7,"limit":2}),
            "Read file src/lib.rs, lines 7-8\n7: first\n8: second",
        )])],
        selection,
    );
    let request = content
        .actions
        .iter()
        .find_map(|action| action.metadata.read_request.as_ref())
        .unwrap();
    assert_eq!(request.extent.unit, JevReadUnit::Unknown);
}

#[test]
fn v2_skill_metadata_display_name_does_not_become_a_requested_id_or_success() {
    let mut skill = tool(
        "skill",
        json!({"id":"review-id"}),
        "<skill_content name=\"Review\">\n# Skill: Review\n</skill_content>",
    );
    skill["state"]["metadata"] = json!({"name":"Review","directory":"/synthetic/skills/review"});
    for source in [
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        let content = prepared(
            source,
            vec![assistant(vec![skill.clone()])],
            SKILL_USE_SELECTION,
        );
        for action in &content.actions {
            if let Some(fact) = &action.metadata.recorded_skill_result {
                assert!(
                    fact.matches_action(action, source),
                    "invalid native proof: {action:#?}"
                );
            }
        }
        let snapshot = SkillUseSnapshot::from_published_content(
            &select_session_content(&content, SKILL_USE_SELECTION),
            &boundary(&content),
        )
        .unwrap();
        assert_eq!(snapshot.events().len(), 2, "{content:#?}");
        assert_eq!(
            snapshot.events()[0].skill,
            RecordedSkillIdentity::NativeId {
                id: "review-id".into(),
                display_name: None,
            }
        );
        assert_eq!(
            snapshot.events()[1].skill,
            RecordedSkillIdentity::NativeId {
                id: "review-id".into(),
                display_name: Some("Review".into()),
            }
        );
        assert_eq!(snapshot.events()[0].lifecycle, SkillUseLifecycle::Requested);
        assert_eq!(snapshot.events()[1].lifecycle, SkillUseLifecycle::Unknown);
        assert!(!snapshot.events()[0].injected_context && !snapshot.events()[1].injected_context);
        assert_eq!(
            snapshot.events()[0].producer,
            SkillUseProducer::OpenCodeV2Skill843a6c51
        );
        assert!(!snapshot.proves_session_wide_absence());
    }
}

fn injected_user(skills: Vec<Value>) -> Value {
    json!({"id":"user", "type":"user", "time":{"created":1767225600000i64},
        "text":"Review the code", "skills":skills})
}

fn skill_snapshot(content: &SessionContentEvidence) -> SkillUseSnapshot {
    SkillUseSnapshot::from_published_content(
        &select_session_content(content, SKILL_USE_SELECTION),
        &boundary(content),
    )
    .unwrap()
}

#[test]
fn injected_native_ids_and_display_names_survive_without_document_paths_or_calls() {
    for source in [
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        let content = prepared(
            source,
            vec![injected_user(vec![
                json!({"id":"review-id", "name":"Review", "text":"Review instructions"}),
                json!({"id":"audit-id", "name":"Audit", "text":"Audit instructions"}),
            ])],
            SKILL_USE_SELECTION,
        );
        let selected = select_session_content(&content, SKILL_USE_SELECTION);
        let snapshot = skill_snapshot(&content);
        assert_eq!(snapshot.events().len(), 2, "{content:#?}");
        assert_eq!(
            snapshot.events()[0].skill,
            RecordedSkillIdentity::NativeId {
                id: "review-id".into(),
                display_name: Some("Review".into())
            }
        );
        assert_eq!(
            snapshot.events()[1].skill,
            RecordedSkillIdentity::NativeId {
                id: "audit-id".into(),
                display_name: Some("Audit".into())
            }
        );
        for event in snapshot.events() {
            assert!(event.injected_context);
            assert_eq!(event.native_body, Some(JevNativeSkillBody::Recorded));
            assert_eq!(event.lifecycle, SkillUseLifecycle::Unknown);
            assert_eq!(event.authority, "unknown");
            assert!(event.tool_call_id.is_none());
        }
        assert!(snapshot.evidence().events.is_empty());
        for action in &selected.actions {
            let Some(fact) = action.metadata.recorded_skill_result.as_ref() else {
                continue;
            };
            assert!(fact.matches_action(action, source));
            assert!(fact.name.is_none() && fact.location.is_none());
            assert!(action.metadata.human_text.is_none());
            assert!(action.metadata.user_text_history.is_none());
        }
        assert_eq!(
            selected
                .actions
                .iter()
                .filter(|action| action.metadata.recorded_skill_result.is_some())
                .count(),
            2
        );
        assert!(!snapshot.proves_session_wide_absence());
    }
}

#[test]
fn injected_missing_bodies_keep_native_identity_with_partial_coverage() {
    for source in [
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        let content = prepared(
            source,
            vec![injected_user(vec![json!({
            "id":"review-id", "name":"Review"})])],
            SKILL_USE_SELECTION,
        );
        let snapshot = skill_snapshot(&content);
        assert_eq!(snapshot.events().len(), 1, "{content:#?}");
        assert_eq!(
            snapshot.events()[0].skill,
            RecordedSkillIdentity::NativeId {
                id: "review-id".into(),
                display_name: Some("Review".into())
            }
        );
        assert_eq!(
            snapshot.events()[0].native_body,
            Some(JevNativeSkillBody::Unavailable)
        );
        assert!(!select_session_content(&content, SKILL_USE_SELECTION).complete);
        assert_eq!(snapshot.coverage().status, SkillUseStatus::Partial);
        assert!(
            snapshot
                .coverage()
                .limitations
                .contains(&SkillUseLimit::IncompleteSelectedContent)
        );
        assert!(snapshot.events()[0].injected_context);
        assert!(snapshot.evidence().events.is_empty());
    }
}

#[test]
fn duplicate_injected_ids_and_raw_name_collisions_remain_distinct_and_partial() {
    let content = prepared(
        SourceFormat::OpenCodeSqliteV2,
        vec![injected_user(vec![
            json!({"id":"review-id", "name":"Review", "text":"first body"}),
            json!({"id":"review-id", "name":"Review", "text":"second body"}),
            json!({"id":"Review", "name":"Other", "text":"third body"}),
        ])],
        SKILL_USE_SELECTION,
    );
    let snapshot = skill_snapshot(&content);
    assert_eq!(snapshot.events().len(), 3, "{content:#?}");
    assert_eq!(snapshot.coverage().status, SkillUseStatus::Partial);
    assert!(
        snapshot
            .coverage()
            .limitations
            .contains(&SkillUseLimit::DuplicateNativeSkillId)
    );
    assert!(
        snapshot
            .coverage()
            .limitations
            .contains(&SkillUseLimit::NativeSkillIdentityCollision)
    );
    assert_ne!(snapshot.events()[0].skill, snapshot.events()[2].skill);
    assert_ne!(
        snapshot.events()[0].reference,
        snapshot.events()[1].reference
    );
    assert!(snapshot.evidence().events.is_empty());
}

#[test]
fn injected_proof_changes_cannot_grant_authority_or_borrow_another_native_field() {
    let source = SourceFormat::OpenCodeSqliteV2;
    let page = published(
        source,
        vec![injected_user(vec![json!({
        "id":"review-id", "name":"Review", "text":"Review instructions"})])],
        SKILL_USE_SELECTION,
    );
    for mutation in 0..7 {
        let mut changed = page.clone();
        let part = changed
            .parts
            .iter_mut()
            .find(|part| part.part.metadata.recorded_skill_result.is_some())
            .unwrap();
        match mutation {
            0 => {
                part.part.authority = ContentAuthority::User;
                part.part.metadata.user_text_history = Some(UserTextHistoryProof {
                    source_format: source,
                    session_id: "root".into(),
                    message_id: "user".into(),
                    revision: 1,
                });
            }
            1 => {
                part.part
                    .metadata
                    .recorded_skill_result
                    .as_mut()
                    .unwrap()
                    .source_format = SourceFormat::OpenCodeSqliteV1
            }
            2 => {
                part.part
                    .metadata
                    .recorded_skill_result
                    .as_mut()
                    .unwrap()
                    .text_digest = "0".repeat(64)
            }
            3 => {
                part.part
                    .metadata
                    .recorded_skill_result
                    .as_mut()
                    .unwrap()
                    .native_skill
                    .as_mut()
                    .unwrap()
                    .origin = JevNativeSkillOrigin::PromptAttachment { index: 1 }
            }
            4 => {
                part.part
                    .metadata
                    .recorded_skill_result
                    .as_mut()
                    .unwrap()
                    .native_skill
                    .as_mut()
                    .unwrap()
                    .body = JevNativeSkillBody::Unavailable
            }
            5 => {
                part.part.metadata.bindings[0].pointer = "/skills.0/id".into();
            }
            6 => part.thread_id = "different".into(),
            7 => {
                let fact = part.part.metadata.recorded_skill_result.as_mut().unwrap();
                let body = fact
                    .ranges
                    .iter_mut()
                    .find(|range| range.pointer.ends_with("/text"))
                    .unwrap();
                body.pointer = "/content/99/state/content/0/text".into();
                let binding = part
                    .part
                    .metadata
                    .bindings
                    .iter_mut()
                    .find(|range| range.pointer.ends_with("/text"))
                    .unwrap();
                binding.pointer = body.pointer.clone();
            }
            8 => {
                let fact = part.part.metadata.recorded_skill_result.as_mut().unwrap();
                fact.ranges
                    .retain(|range| !range.pointer.ends_with("/text"));
                part.part
                    .metadata
                    .bindings
                    .retain(|range| !range.pointer.ends_with("/text"));
            }
            _ => unreachable!(),
        }
        let content = prepare_session_content("session", source, changed, vec![]);
        if mutation == 0 {
            let snapshot = skill_snapshot(&content);
            assert_eq!(snapshot.events().len(), 1);
            assert!(
                snapshot
                    .events()
                    .iter()
                    .all(|event| event.authority == "unknown" && event.injected_context)
            );
        } else {
            assert!(
                content
                    .actions
                    .iter()
                    .all(|action| action.metadata.recorded_skill_result.is_none()),
                "mutation {mutation}: {content:#?}"
            );
            assert!(skill_snapshot(&content).events().is_empty());
        }
        assert!(
            content
                .actions
                .iter()
                .all(|action| action.metadata.human_text.is_none())
        );
    }
}

#[test]
fn skill_injection_and_question_answers_do_not_supply_human_approval() {
    let messages = vec![
        json!({"id":"user", "type":"user", "time":{"created":1767225600000i64},
        "text":"Review the code", "skills":[{"id":"review-id", "name":"Review", "text":"skill instructions"}]}),
        assistant(vec![{
            let mut question = tool(
                "question",
                json!({"questions":[{"question":"Proceed?", "header":"Scope", "options":[{"label":"Yes","description":"Proceed"}]}]}),
                "answered",
            );
            question["state"]["metadata"] = json!({"answers":[["Yes"]]});
            question
        }]),
    ];
    let selection =
        JevInputSelection::from_fields(&[JevInputField::UserMessage, JevInputField::UserAnswer]);
    let content = prepared(SourceFormat::OpenCodeSqliteV2, messages, selection);
    assert!(
        content
            .actions
            .iter()
            .all(|action| action.metadata.human_text.is_none())
    );
    let answer = content
        .actions
        .iter()
        .flat_map(|action| &action.metadata.user_answers)
        .next()
        .unwrap();
    assert_eq!(answer.origin, JevUserAnswerOrigin::UnknownOrigin);
    assert!(!answer.is_authoritative_user_response());
}

#[test]
fn scope_preparation_keeps_v2_answers_unknown_and_does_not_infer_plan_approval() {
    let mut question = tool(
        "question",
        json!({"questions":[{
        "question":"Proceed?", "header":"Scope", "options":[{"label":"Yes","description":"Proceed"}]}]}),
        "answered",
    );
    question["state"]["metadata"] = json!({"answers":[["Yes"]]});
    let mut plan = tool("plan_exit", json!({"plan":"src/plan.md"}), "approved");
    plan["id"] = json!("plan-call");
    for source in [
        SourceFormat::OpenCodeSqliteV2,
        SourceFormat::OpenCodeV2SessionExport,
    ] {
        let mut page = published(
            source,
            vec![
                injected_user(vec![json!({
            "id":"review-id", "name":"Review", "text":"Review instructions"})]),
                assistant(vec![question.clone(), plan.clone()]),
            ],
            SCOPE_SELECTION,
        );
        page.source_generation = Some(7);
        let last = page.parts.last().unwrap();
        let scope_boundary = SessionScopeBoundary {
            source_key: last.source_key.clone(),
            thread_id: last.thread_id.clone(),
            turn_index: last.turn_index,
            part_index: last.part_index,
            branch: SessionScopeBranch::ProvenLinear,
        };
        let mut builder =
            SessionScopeBuilder::new(source, scope_boundary, page.publication_fence, 7, true)
                .unwrap();
        builder.push_page(page, false).unwrap();
        let scope = builder.finish().unwrap();
        assert!(
            scope
                .occurrences()
                .iter()
                .all(|occurrence| occurrence.authority != ScopeAuthority::User)
        );
        assert!(
            scope
                .occurrences()
                .iter()
                .any(|occurrence| occurrence.field == JevInputField::UserAnswer
                    && occurrence.authority == ScopeAuthority::UnknownInfluence)
        );
        assert!(
            scope
                .occurrences()
                .iter()
                .all(|occurrence| occurrence.field != JevInputField::PlanReference)
        );
        assert!(
            scope
                .limitations()
                .contains(&ScopeMissingReason::NoUserContext)
        );
        assert!(
            scope
                .limitations()
                .contains(&ScopeMissingReason::UnresolvedInfluence)
        );
    }
}

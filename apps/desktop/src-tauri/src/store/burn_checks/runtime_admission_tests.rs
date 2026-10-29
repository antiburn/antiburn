use serde_json::Value;
use std::io::Write;

use antiburn_checks::source::PublishedEvidenceSource;
use antiburn_local::analysis::file_live::{
    NATIVE_FILE_PAGE_BYTES, NativeFilePage, read_native_file_page,
};
use antiburn_local::analysis::{NativeEvidenceFact, RawSource, SessionInput, SourceFormat};

use super::*;
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use crate::store::native_tool_results::{admission, evidence};
use crate::store::{SessionKey, SessionRecord};

#[path = "runtime_request_origin_tests.rs"]
mod request_origin;

const NOW: i64 = 1000;
const LOCAL_RESULT_CHECK: &str = "name: Local result\non:\n  tool_result:\nactions:\n  - type: status\n    if: ${{ message.tool.result.status == 'error' }}\n    input: {result: failure, message: Synthetic command failed.}\n  - type: status\n    if: ${{ message.tool.result.status == 'completed' }}\n    input: {result: success, message: Synthetic command passed.}\n";

struct FileFixture {
    directory: tempfile::TempDir,
    store: Arc<Store>,
    adapter: Arc<StoreCheckStore>,
    active: EffectiveActivation,
    engine: CheckEngine<StoreCheckStore, NoEvaluator>,
    input: SessionInput,
    key: SessionKey,
    call: bool,
}

impl FileFixture {
    fn new(claude: bool, call: bool, idle: bool) -> Self {
        Self::with_history(claude, call, idle, false)
    }

    fn with_history(claude: bool, call: bool, idle: bool, clipped: bool) -> Self {
        let definition = if idle {
            "name: Idle\non:\n  session_idle:\nactions:\n  - type: status\n    input: {result: failure}\n".into()
        } else {
            CHECK
                .replace(
                    "  tool_result:",
                    if call {
                        "  tool_call:"
                    } else {
                        "  user_message:"
                    },
                )
                .replace("message.tool.result.status", "message")
        };
        Self::with_source(
            if claude {
                SourceFormat::ClaudeJsonl
            } else {
                SourceFormat::PiV3Jsonl
            },
            &definition,
            call,
            clipped,
            true,
        )
    }

    fn with_source(
        source_format: SourceFormat,
        definition: &str,
        call: bool,
        clipped: bool,
        warm: bool,
    ) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.jsonl");
        let (agent, header) = match source_format {
            SourceFormat::ClaudeJsonl => (
                "claude-code",
                json!({"type":"user","uuid":"root","parentUuid":null,"sessionId":"session","cwd":"/repo","isSidechain":false,"version":"2.1.278","entrypoint":"cli",
                    "message":{"role":"user","content":[{"type":"text","text":"Baseline request."}]}}),
            ),
            SourceFormat::CodexRolloutJsonl => (
                "codex",
                json!({"timestamp":"2026-10-07T10:00:00Z","type":"session_meta",
                    "payload":{"id":"session","cwd":"/repo","cli_version":"0.160.1","source":"cli","thread_source":"user","history_mode":"legacy"}}),
            ),
            SourceFormat::PiV3Jsonl => (
                "pi",
                json!({"type":"session","version":3,"id":"session","cwd":"/repo","timestamp":"2026-01-01T00:00:00Z"}),
            ),
            _ => panic!("the fixture requires a supported native file format"),
        };
        std::fs::write(&path, format!("{header}\n")).unwrap();
        if clipped {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            for index in 0..16 {
                let record = json!({"type":"label","id":format!("baseline-{index}"),
                    "parentId":if index == 0 {Value::Null} else {json!(format!("baseline-{}", index - 1))},
                    "label":"x".repeat(128 * 1024)});
                writeln!(file, "{record}").unwrap();
            }
        }
        let (store, adapter, active) = fixture_with_definition(
            Arc::new(Store::open(&directory.path().join("app")).unwrap()),
            definition,
        );
        let key = SessionKey::new("native", agent, "session");
        store
            .upsert_sessions(
                &[SessionRecord {
                    key: key.clone(),
                    source_kind: "file".into(),
                    source_label: path.to_string_lossy().into_owned(),
                    cwd: Some("/repo".into()),
                    wsl_distro: None,
                    title: None,
                    title_source: None,
                    surface: "cli".into(),
                    updated_at_epoch: Some(NOW),
                    activity_cursor: String::new(),
                    activity_source: "native".into(),
                    subagent_count: 0,
                    fork_parent_session_id: None,
                    source_fingerprint: Some("synthetic".into()),
                }],
                &crate::agents::evidence_cohort(),
            )
            .unwrap();
        let input = SessionInput {
            agent: if agent == "claude-code" {
                "claude"
            } else {
                agent
            }
            .into(),
            session_id: "session".into(),
            source: RawSource::File(path),
            source_format,
            fork_parent_session_id: None,
        };
        let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
        let fixture = Self {
            directory,
            store,
            adapter,
            active,
            engine,
            input,
            key,
            call,
        };
        if warm {
            let mut drained = false;
            for _ in 0..32 {
                let page = fixture.publish(NOW);
                if !page.has_more {
                    drained = true;
                    break;
                }
            }
            assert!(drained);
            fixture.sync();
        }
        fixture
    }

    fn append(&self, id: &str, parent: &str, content: &str) {
        let claude = self.key.agent == "claude-code";
        let record = if claude {
            json!({"type":if self.call {"assistant"} else {"user"},"uuid":id,"parentUuid":parent,"sessionId":"session","cwd":"/repo","isSidechain":false,"version":"2.1.278","entrypoint":"cli",
                "message":{"id":id,"role":if self.call {"assistant"} else {"user"},"model":"synthetic",
                    "content":if self.call {json!([{"type":"tool_use","id":"call-one","name":"bash","input":{"command":content}}])}
                        else {json!([{"type":"text","text":content}])}}})
        } else {
            json!({"type":"message","id":id,"parentId":if parent == "root" {Value::Null} else {json!(parent)},
                "message":{"role":if self.call {"assistant"} else {"user"},"content":if self.call {
                    json!([{"type":"toolCall","id":"call-one","name":"bash","arguments":{"command":content}}])
                } else {json!([{"type":"text","text":content}])}}})
        };
        let RawSource::File(path) = &self.input.source else {
            unreachable!()
        };
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file, "{record}").unwrap();
    }

    fn append_bytes(&self, bytes: &[u8]) {
        let RawSource::File(path) = &self.input.source else {
            unreachable!()
        };
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    fn result_records(&self, index: usize, error: bool) -> (String, String) {
        let call_id = format!("call-{index}");
        let call_record = format!("call-record-{index}");
        let result_record = format!("result-record-{index}");
        let parent = if index == 0 {
            "root".into()
        } else {
            format!("result-record-{}", index - 1)
        };
        let output = format!("Synthetic command output {index}.");
        let (call, result) = match self.input.source_format {
            SourceFormat::ClaudeJsonl => (
                json!({"type":"assistant","uuid":call_record,"parentUuid":parent,
                    "sessionId":"session","cwd":"/repo","isSidechain":false,"version":"2.1.278","entrypoint":"cli",
                    "message":{"id":format!("assistant-{index}"),"role":"assistant","model":"synthetic",
                        "content":[{"type":"tool_use","id":call_id,"name":"Bash","input":{"command":"synthetic-command"}}]}}),
                json!({"type":"user","uuid":result_record,"parentUuid":call_record,
                    "sessionId":"session","cwd":"/repo","isSidechain":false,"version":"2.1.278","entrypoint":"cli",
                    "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":call_id,"content":output,"is_error":error}]}}),
            ),
            SourceFormat::CodexRolloutJsonl => (
                json!({"timestamp":"2026-10-07T10:00:01Z","type":"response_item",
                    "payload":{"type":"function_call","id":call_record,"call_id":call_id,"name":"exec_command",
                        "arguments":"{\"cmd\":\"synthetic-command\"}"}}),
                json!({"timestamp":"2026-10-07T10:00:02Z","type":"response_item",
                    "payload":{"type":"function_call_output","id":result_record,"call_id":call_id,
                        "status":if error {"failed"} else {"completed"},"output":output}}),
            ),
            SourceFormat::PiV3Jsonl => (
                json!({"type":"message","id":call_record,"parentId":if index == 0 {Value::Null} else {json!(parent)},
                    "message":{"role":"assistant","model":"synthetic","content":[{"type":"toolCall","id":call_id,"name":"bash",
                        "arguments":{"command":"synthetic-command"}}]}}),
                json!({"type":"message","id":result_record,"parentId":call_record,
                    "message":{"role":"toolResult","toolCallId":call_id,"toolName":"bash",
                        "content":[{"type":"text","text":output}],"isError":error}}),
            ),
            _ => panic!("the fixture requires a supported native file format"),
        };
        (format!("{call}\n"), format!("{result}\n"))
    }

    fn read(
        &self,
        now: i64,
    ) -> (
        crate::store::native_tool_results::source_delivery::native_file::NativeFileClaim,
        NativeFilePage,
    ) {
        let claim = self.store.claim_next_native_file(now, 60).unwrap().unwrap();
        let cursor = claim.cursor().unwrap();
        let page = read_native_file_page(&self.input, cursor.as_ref(), &|| false).unwrap();
        (claim, page)
    }

    fn publish(&self, now: i64) -> NativeFilePage {
        let (claim, page) = self.read(now);
        self.store
            .publish_native_file_page(&claim, &page, &[], now)
            .unwrap();
        page
    }

    fn source(&self) -> PublishedEvidenceSource<StorePublishedEvidenceReader<'_>> {
        PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_live_project(&self.store, "native", "/repo")
                .with_activation(&self.active.id),
            40,
        )
        .unwrap()
    }

    fn sync(&self) {
        for _ in 0..128 {
            let progress = self.adapter.load_source_progress(&self.active).unwrap();
            let page = self
                .source()
                .read_batch(progress.publication_cursor)
                .unwrap();
            if !page.events.is_empty() {
                self.engine.apply_source_page(&self.active, &page).unwrap();
            }
            let mut attached = false;
            for session in self
                .adapter
                .startup_sessions(&self.active, NOW - 180)
                .unwrap()
            {
                let events =
                    StorePublishedEvidenceReader::for_project(&self.store, "native", "/repo")
                        .recent_results_before(&session.key, session.before_publication)
                        .unwrap();
                self.adapter
                    .initialize_recent(&self.active, &session, &events)
                    .unwrap()
                    .unwrap();
                self.engine.refresh_source_progress(&self.active).unwrap();
                attached = true;
            }
            if !page.has_more && !attached {
                break;
            }
        }
        self.engine.recover_pending().unwrap();
    }

    fn pending(&self) -> FrozenCandidate {
        let result = self.store.lock().query_row(
            "SELECT frozen_input_json FROM burn_check_candidate WHERE state = 'pending'",
            [],
            |row| row.get::<_, String>(0),
        );
        let saved = result.expect("an admitted public event must create pending work");
        serde_json::from_str(&saved).unwrap()
    }

    fn reopen(&mut self) {
        let store = Arc::new(Store::open(&self.directory.path().join("app")).unwrap());
        let adapter = Arc::new(
            StoreCheckStore::new(
                store.clone(),
                "reopened".into(),
                self.active.id.scope.clone(),
            )
            .unwrap(),
        );
        adapter.register_activation(&self.active).unwrap();
        self.engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
        self.adapter = adapter;
        self.store = store;
    }

    fn assert_local_not_due(&self, now: i64) {
        assert!(
            matches!(
                self.engine
                    .start_one_due(&self.active, u64::try_from(now).unwrap() * 1000)
                    .unwrap(),
                StartOutcome::NotDue
            ),
            "{} has unexpected local work at {now}",
            self.key.agent
        );
    }

    fn settle_local_result(&self, now: i64, status: &str, assessments: i64) {
        let candidate = self.pending();
        let expected_result = if status == "failure" {
            "error"
        } else {
            "completed"
        };
        assert_eq!(
            candidate.context.value["message"]["tool"]["result"]["status"], expected_result,
            "{} freezes the expected result at {now}: {:?}",
            self.key.agent, candidate.context.value
        );
        let StartOutcome::Started(task) = self
            .engine
            .start_one_due(&self.active, u64::try_from(now).unwrap() * 1000)
            .unwrap()
        else {
            panic!(
                "{} must queue its published terminal result at {now}",
                self.key.agent
            )
        };
        let settled = poll_settlement(task);
        assert_eq!(
            settled.outcome,
            RunOutcome::Accepted,
            "{} at {now}",
            self.key.agent
        );
        assert_eq!(settled.attempts, 0, "{} at {now}", self.key.agent);
        let saved: (String, i64) = self
            .store
            .lock()
            .query_row(
                "SELECT status, evidence_valid FROM burn_check_current_status",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            saved,
            (status.into(), 1),
            "{} at {now}, frozen context: {:?}",
            self.key.agent,
            candidate.context.value
        );
        assert_eq!(
            self.store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_assessment_history",
                    [],
                    |row| { row.get::<_, i64>(0) }
                )
                .unwrap(),
            assessments
        );
        for table in ["burn_check_evaluation_attempt", "burn_check_decision_usage"] {
            assert_eq!(
                self.store
                    .lock()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                0
            );
        }
        let usage = self.store.burn_check_usage_summary().unwrap();
        assert_eq!(usage.confirmed_calls, 0);
        assert_eq!(usage.unknown_outcomes, 0);
        self.assert_local_not_due(now);
    }

    fn assert_unsafe(&self) {
        assert_eq!(
            admission::source_reason_in(&self.store.lock(), &self.key)
                .unwrap()
                .as_deref(),
            Some("native-unsafe-identity")
        );
        let ready = crate::insights_report::installed_checks::source_ready_sql(1, 2, 3);
        assert!(!self.store.lock().query_row(&format!("SELECT ({ready}) FROM session s LEFT JOIN session_evidence e USING(environment_key, agent, session_id)"),
            params![antiburn_local::analysis::PARSER_REVISION, antiburn_local::analysis::ANALYZER_REVISION, antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION],
            |row| row.get::<_, bool>(0)).unwrap());
        let can_arrive = crate::insights_report::installed_checks::source_can_arrive_sql(1, 2);
        assert!(!self.store.lock().query_row(&format!("SELECT ({can_arrive}) FROM session s LEFT JOIN session_evidence e USING(environment_key, agent, session_id)"),
            params![antiburn_local::analysis::PARSER_REVISION, NOW - 180], |row| row.get::<_, bool>(0)).unwrap());
        let identities = self
            .store
            .lock()
            .prepare("SELECT native_identity FROM native_tool_result_current")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect::<Vec<_>>();
        for identity in identities {
            assert!(
                evidence::current_binding_in(&self.store.lock(), &self.key, &identity)
                    .unwrap()
                    .is_none()
            );
        }
    }
}

#[test]
fn native_file_appends_save_local_status_after_an_incomplete_record_and_restart() {
    for source_format in [
        SourceFormat::ClaudeJsonl,
        SourceFormat::CodexRolloutJsonl,
        SourceFormat::PiV3Jsonl,
    ] {
        for restart in [false, true] {
            let mut fixture =
                FileFixture::with_source(source_format, LOCAL_RESULT_CHECK, false, false, true);
            assert!(fixture.active.compiled.is_local());
            let (call, result) = fixture.result_records(0, true);
            fixture.append_bytes(call.as_bytes());
            let page = fixture.publish(NOW + 1);
            assert!(page.bytes_read <= NATIVE_FILE_PAGE_BYTES);
            fixture.sync();
            fixture.assert_local_not_due(NOW + 1);

            let split = result.len() / 2;
            fixture.append_bytes(&result.as_bytes()[..split]);
            let page = fixture.publish(NOW + 2);
            assert!(page.pending_tail && !page.has_more);
            assert!(page.bytes_read <= NATIVE_FILE_PAGE_BYTES);
            assert!(page.results.is_empty());
            fixture.sync();
            fixture.assert_local_not_due(NOW + 2);
            assert_eq!(
                fixture
                    .store
                    .lock()
                    .query_row(
                        "SELECT COUNT(*) FROM burn_check_current_status",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0,
                "an unfinished result cannot save a clean status"
            );
            if restart {
                fixture.reopen();
                fixture.engine.recover_pending().unwrap();
                fixture.assert_local_not_due(NOW + 2);
            }

            fixture.append_bytes(&result.as_bytes()[split..]);
            let page = fixture.publish(NOW + 3);
            assert!(!page.pending_tail && !page.has_more);
            assert!(page.bytes_read <= NATIVE_FILE_PAGE_BYTES);
            assert_eq!(page.results.len(), 1);
            fixture.sync();
            let candidate = fixture.pending();
            assert_eq!(candidate.messages.len(), 1);
            assert_eq!(
                candidate.messages[0].value["tool"]["result"]["status"],
                "error"
            );
            assert!(
                candidate.messages[0]
                    .value
                    .to_string()
                    .contains("Synthetic command output 0.")
            );
            fixture.settle_local_result(NOW + 3, "failure", 1);

            fixture.reopen();
            fixture.engine.recover_pending().unwrap();
            let page = fixture.publish(NOW + 4);
            assert!(page.results.is_empty());
            fixture.sync();
            fixture.assert_local_not_due(NOW + 4);

            let (call, result) = fixture.result_records(1, false);
            fixture.append_bytes(call.as_bytes());
            fixture.append_bytes(result.as_bytes());
            fixture.publish(NOW + 5);
            fixture.sync();
            fixture.settle_local_result(NOW + 5, "success", 2);
        }
    }
}

#[test]
fn native_file_startup_snapshot_and_later_append_save_local_status_without_provider_work() {
    for source_format in [
        SourceFormat::ClaudeJsonl,
        SourceFormat::CodexRolloutJsonl,
        SourceFormat::PiV3Jsonl,
    ] {
        let mut fixture =
            FileFixture::with_source(source_format, LOCAL_RESULT_CHECK, false, false, false);
        let (call, result) = fixture.result_records(0, true);
        fixture.append_bytes(call.as_bytes());
        fixture.append_bytes(result.as_bytes());
        let page = fixture.publish(NOW);
        assert_eq!(page.results.len(), 1);
        assert!(!page.results[0].live_eligible);
        fixture.reopen();
        fixture.engine.recover_pending().unwrap();
        fixture.sync();
        fixture.settle_local_result(NOW, "failure", 1);

        let (call, result) = fixture.result_records(1, false);
        fixture.append_bytes(call.as_bytes());
        fixture.append_bytes(result.as_bytes());
        let page = fixture.publish(NOW + 1);
        assert_eq!(page.results.len(), 1);
        assert!(page.results[0].live_eligible);
        fixture.sync();
        fixture.settle_local_result(NOW + 1, "success", 2);
    }
}

#[test]
fn real_file_conflicts_block_public_evidence_in_one_page_separate_pages_and_cursor_restore() {
    for claude in [false, true] {
        for call in [false, true] {
            for mode in ["one-page", "separate-pages", "reopen"] {
                let mut fixture = FileFixture::new(claude, call, false);
                fixture.append("public-one", "root", "Original public work.");
                let candidate = if mode != "one-page" {
                    fixture.publish(NOW + 1);
                    fixture.sync();
                    let candidate = fixture.pending();
                    if mode == "reopen" {
                        fixture.reopen();
                    }
                    Some(candidate)
                } else {
                    None
                };
                fixture.append("public-one", "public-one", "Conflicting public work.");
                let page = fixture.publish(NOW + 2);
                assert!(
                    page.evidence
                        .events
                        .iter()
                        .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_))),
                    "{claude}/{call}/{mode}"
                );
                fixture.assert_unsafe();
                if let Some(candidate) = &candidate {
                    assert!(matches!(
                        fixture.adapter.claim_due(candidate, 100_000, true).unwrap(),
                        CandidateClaim::Stale
                    ));
                }
                let progress = fixture
                    .adapter
                    .load_source_progress(&fixture.active)
                    .unwrap();
                let page = fixture
                    .source()
                    .read_batch(progress.publication_cursor)
                    .unwrap();
                assert!(
                    page.events
                        .iter()
                        .any(|event| matches!(event, SourceEvent::UnsafeUpdate { .. }))
                );
                fixture.sync();
                fixture.reopen();
                fixture.append("later", "public-one", "Later public work.");
                fixture.publish(NOW + 3);
                fixture.sync();
                fixture.assert_unsafe();
                assert_eq!(fixture.store.lock().query_row("SELECT COUNT(*) FROM burn_check_candidate WHERE state IN ('pending', 'claimed', 'dispatching', 'answered')", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
            }
        }
    }
}

#[test]
fn unsafe_file_admission_fences_claim_dispatch_acceptance_and_keeps_usage_and_unknown_attempts() {
    for claude in [false, true] {
        for call in [false, true] {
            for stage in ["claimed", "dispatching", "answered", "accepted", "unknown"] {
                let mut fixture = FileFixture::new(claude, call, false);
                fixture.append("public-one", "root", "Original public work.");
                fixture.publish(NOW + 1);
                fixture.sync();
                let candidate = fixture.pending();
                let claimed = claim(&fixture.adapter, &candidate);
                let acceptance = if matches!(stage, "answered" | "accepted") {
                    Some(answer(
                        &fixture.adapter,
                        &fixture.active,
                        claimed.clone(),
                        0.99,
                    ))
                } else {
                    if matches!(stage, "dispatching" | "unknown") {
                        assert_eq!(
                            fixture
                                .adapter
                                .dispatch_review(&claimed, 100_000, Duration::ZERO, 1)
                                .unwrap(),
                            ClaimOutcome::Current
                        );
                    }
                    None
                };
                if stage == "accepted" {
                    assert_eq!(
                        fixture
                            .adapter
                            .accept(acceptance.as_ref().unwrap())
                            .unwrap(),
                        AcceptanceOutcome::Accepted
                    );
                }
                if stage == "unknown" {
                    fixture
                        .adapter
                        .fail_review(
                            &claimed,
                            1,
                            AttemptFailure::Unknown,
                            &transient_diagnostic(),
                        )
                        .unwrap();
                }
                if stage != "claimed" {
                    fixture
                        .store
                        .record_burn_check_decision_usage(
                            &hex(&candidate.fingerprint),
                            &review_attempt(1),
                            "retry",
                            ("custom-systemone", "synthetic-model"),
                            (stage != "unknown").then_some(
                                antiburn_local::analysis::jev::JevUsage {
                                    input_tokens: 7,
                                    output_tokens: 3,
                                },
                            ),
                            100,
                        )
                        .unwrap();
                }
                let usage = fixture.store.burn_check_usage_summary().unwrap();
                let attempts: String = fixture.store.lock().query_row("SELECT json_group_array(json_array(attempt_id, state, outcome, response_json)) FROM burn_check_evaluation_attempt", [], |row| row.get(0)).unwrap();
                fixture.append("public-one", "public-one", "Conflicting public work.");
                fixture.publish(NOW + 2);
                fixture.assert_unsafe();
                assert_eq!(
                    fixture
                        .adapter
                        .dispatch_review(&claimed, 100_001, Duration::ZERO, 1)
                        .unwrap(),
                    ClaimOutcome::Stale
                );
                if stage == "answered" {
                    assert_eq!(
                        fixture
                            .adapter
                            .accept(acceptance.as_ref().unwrap())
                            .unwrap(),
                        AcceptanceOutcome::Stale
                    );
                }
                if stage == "accepted" {
                    assert_eq!(
                        fixture
                            .store
                            .lock()
                            .query_row(
                                "SELECT evidence_valid FROM burn_check_assessment_history",
                                [],
                                |row| row.get::<_, i64>(0)
                            )
                            .unwrap(),
                        0
                    );
                    assert_eq!(
                        fixture
                            .store
                            .lock()
                            .query_row(
                                "SELECT evidence_valid FROM burn_check_current_status",
                                [],
                                |row| row.get::<_, i64>(0)
                            )
                            .unwrap(),
                        0
                    );
                }
                fixture.sync();
                fixture.reopen();
                fixture.engine.recover_pending().unwrap();
                assert_eq!(fixture.store.burn_check_usage_summary().unwrap(), usage);
                if stage == "dispatching" {
                    assert_eq!(
                        fixture
                            .store
                            .lock()
                            .query_row(
                                "SELECT outcome FROM burn_check_evaluation_attempt",
                                [],
                                |row| row.get::<_, String>(0)
                            )
                            .unwrap(),
                        "unknown"
                    );
                } else {
                    assert_eq!(fixture.store.lock().query_row("SELECT json_group_array(json_array(attempt_id, state, outcome, response_json)) FROM burn_check_evaluation_attempt", [], |row| row.get::<_, String>(0)).unwrap(), attempts);
                }
                assert!(
                    fixture
                        .adapter
                        .load_recovery(None, 100)
                        .unwrap()
                        .iter()
                        .all(|record| !matches!(record.work, RecoveryWork::Pending { .. }))
                );
            }
        }
    }
}

#[test]
fn unsafe_file_pages_advance_one_publication_at_a_time_without_stale_queue_work() {
    for existing in [false, true] {
        let fixture = FileFixture::new(false, false, false);
        fixture.append("public-one", "root", "Original public work.");
        fixture.publish(NOW + 1);
        let candidate = if existing {
            fixture.sync();
            Some(fixture.pending())
        } else {
            None
        };
        fixture.append("public-two", "public-one", "Unconsumed public work.");
        fixture.publish(NOW + 2);
        fixture.append("public-two", "public-two", "Conflicting public work.");
        fixture.publish(NOW + 3);
        if let Some(candidate) = candidate {
            assert!(matches!(
                fixture
                    .adapter
                    .claim_due(&candidate, 100_000, true)
                    .unwrap(),
                CandidateClaim::Stale
            ));
        }
        let source = PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_live_project(&fixture.store, "native", "/repo")
                .with_activation(&fixture.active.id),
            1,
        )
        .unwrap();
        let mut unsafe_events = 0;
        let mut drained = false;
        for _ in 0..16 {
            let before = fixture
                .adapter
                .load_source_progress(&fixture.active)
                .unwrap()
                .publication_cursor;
            let page = source.read_batch(before).unwrap();
            if !page.events.is_empty() {
                assert!(page.next > before);
                unsafe_events += page
                    .events
                    .iter()
                    .filter(|event| matches!(event, SourceEvent::UnsafeUpdate { .. }))
                    .count();
                assert_eq!(
                    fixture
                        .engine
                        .apply_source_page(&fixture.active, &page)
                        .unwrap()
                        .candidates,
                    0
                );
                assert_eq!(
                    fixture
                        .adapter
                        .load_source_progress(&fixture.active)
                        .unwrap()
                        .publication_cursor,
                    page.next
                );
            }
            if !page.has_more {
                drained = true;
                break;
            }
        }
        assert!(drained && unsafe_events >= 2);
        assert_eq!(
            fixture
                .store
                .lock()
                .query_row(
                    "SELECT COUNT(*) FROM burn_check_candidate WHERE state = 'pending'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }
}

#[test]
fn known_unsafe_file_fence_precedes_output_budget_and_invalid_reasons_roll_back() {
    for defer_failure in [false, true] {
        assert_known_unsafe_output_fence(defer_failure);
    }
}

fn assert_known_unsafe_output_fence(defer_failure: bool) {
    use antiburn_local::analysis::ToolResultCompleteness;
    let fixture = FileFixture::new(false, false, false);
    fixture.append("public-one", "root", "Original public work.");
    fixture.publish(NOW + 1);
    fixture.sync();
    let candidate = fixture.pending();
    fixture.append("public-two", "public-one", "Unconsumed public work.");
    fixture.append("public-two", "public-two", "Conflicting public work.");
    let (claim, mut page) = fixture.read(NOW + 2);
    let fact = page
        .evidence
        .events
        .iter()
        .find(|event| matches!(event.fact, NativeEvidenceFact::Message(_)))
        .unwrap()
        .clone();
    page.evidence
        .events
        .splice(0..0, std::iter::repeat_n(fact, 256));
    for reason in [
        ToolResultCompleteness::Complete,
        ToolResultCompleteness::Incomplete,
    ] {
        let mut invalid = page.clone();
        let event = invalid
            .evidence
            .events
            .iter_mut()
            .find(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
            .unwrap();
        let NativeEvidenceFact::AdmissionFailure(failure) = &mut event.fact else {
            unreachable!()
        };
        failure.reason = reason;
        assert!(
            fixture
                .store
                .publish_native_file_page(&claim, &invalid, &[], NOW + 2)
                .is_err()
        );
        assert!(
            admission::source_reason_in(&fixture.store.lock(), &fixture.key)
                .unwrap()
                .is_none()
        );
        let mut invalid = page.clone();
        invalid.admission_failure.as_mut().unwrap().reason = reason;
        assert!(
            fixture
                .store
                .publish_native_file_page(&claim, &invalid, &[], NOW + 2)
                .is_err()
        );
    }
    if defer_failure {
        page.evidence
            .events
            .retain(|event| !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)));
    }
    assert!(
        fixture
            .store
            .publish_native_file_page(&claim, &page, &[], NOW + 2)
            .unwrap()
    );
    fixture.assert_unsafe();
    assert!(matches!(
        fixture
            .adapter
            .claim_due(&candidate, 100_000, true)
            .unwrap(),
        CandidateClaim::Stale
    ));
    fixture.sync();
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row("SELECT COUNT(*) FROM burn_check_candidate", [], |row| row
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1
    );
}

#[test]
fn clipped_file_history_keeps_public_messages_usable_without_unsafe_admission() {
    let fixture = FileFixture::with_history(false, false, false, true);
    fixture.append(
        "public-one",
        "baseline-15",
        "Usable work after clipped history.",
    );
    let page = fixture.publish(NOW + 1);
    assert!(!page.history_complete);
    assert!(
        !page
            .evidence
            .events
            .iter()
            .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
    );
    fixture.sync();
    assert!(
        admission::source_reason_in(&fixture.store.lock(), &fixture.key)
            .unwrap()
            .is_none()
    );
    let candidate = fixture.pending();
    assert_eq!(candidate.messages.len(), 1);
    let claimed = claim(&fixture.adapter, &candidate);
    let acceptance = answer(&fixture.adapter, &fixture.active, claimed, 0.99);
    assert_eq!(
        fixture.adapter.accept(&acceptance).unwrap(),
        AcceptanceOutcome::Accepted
    );
}

#[test]
fn unsafe_file_admission_keeps_accepted_idle_boundary_history() {
    let fixture = FileFixture::new(false, false, true);
    fixture.append("public-one", "root", "Public work before idle.");
    fixture.publish(NOW + 1);
    fixture.sync();
    let deadline = fixture.store.next_session_idle_deadline().unwrap().unwrap();
    fixture
        .store
        .request_due_session_idle_catchup(deadline)
        .unwrap();
    fixture.publish(deadline / 1000);
    fixture.sync();
    let StartOutcome::Started(task) = fixture
        .engine
        .start_one_due(&fixture.active, u64::try_from(deadline).unwrap())
        .unwrap()
    else {
        panic!("idle work must be due")
    };
    assert_eq!(poll_settlement(task).outcome, RunOutcome::Accepted);
    fixture.append("public-one", "public-one", "Conflicting public work.");
    fixture.publish(deadline / 1000 + 30);
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row(
                "SELECT evidence_valid FROM burn_check_assessment_history",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1,
        "source transaction must keep the accepted idle boundary"
    );
    fixture.sync();
    fixture.assert_unsafe();
    assert_eq!(
        fixture
            .store
            .lock()
            .query_row(
                "SELECT evidence_valid FROM burn_check_assessment_history",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        fixture
            .store
            .burn_check_usage_summary()
            .unwrap()
            .confirmed_calls,
        0
    );
}

#[test]
fn unsafe_file_binding_survives_read_failure_and_clears_only_after_new_binding_validates() {
    use antiburn_local::analysis::file_live::NativeFileReadError;
    let fixture = FileFixture::new(false, false, false);
    fixture.append("public-one", "root", "Original public work.");
    fixture.publish(NOW + 1);
    fixture.sync();
    fixture.append("public-one", "public-one", "Conflicting public work.");
    fixture.publish(NOW + 2);
    let claim = fixture
        .store
        .claim_next_native_file(NOW + 3, 60)
        .unwrap()
        .unwrap();
    fixture
        .store
        .fail_native_file(&claim, NativeFileReadError::ReadFailed, NOW + 3, NOW + 4)
        .unwrap();
    fixture.assert_unsafe();
    fixture.publish(NOW + 4);
    fixture.assert_unsafe();
    let old_identity: String = fixture
        .store
        .lock()
        .query_row(
            "SELECT native_identity FROM native_tool_result_current LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let path = fixture.directory.path().join("new-session.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"session\",\"version\":3,\"id\":\"session\",\"cwd\":\"/repo\",\"timestamp\":\"2026-01-01T00:00:00Z\"}\n",
    )
    .unwrap();
    fixture
        .store
        .lock()
        .execute(
            "UPDATE session SET source_label = ?1, incarnation = incarnation + 1",
            [path.to_string_lossy().as_ref()],
        )
        .unwrap();
    let claim = fixture
        .store
        .claim_next_native_file(NOW + 5, 60)
        .unwrap()
        .unwrap();
    assert!(claim.cursor_json.is_none());
    let input = SessionInput {
        source: RawSource::File(path),
        ..fixture.input.clone()
    };
    let page = read_native_file_page(&input, None, &|| false).unwrap();
    assert!(
        fixture
            .store
            .publish_native_file_page(&claim, &page, &[], NOW + 5)
            .unwrap()
    );
    assert!(
        admission::source_reason_in(&fixture.store.lock(), &fixture.key)
            .unwrap()
            .is_none()
    );
    assert!(
        evidence::current_binding_in(&fixture.store.lock(), &fixture.key, &old_identity)
            .unwrap()
            .is_none()
    );
}

#[test]
fn unsafe_file_report_distinguishes_unsafe_records_from_missing_source_access() {
    let fixture = FileFixture::new(false, false, false);
    fixture.append("public-one", "root", "Original public work.");
    fixture.append("public-one", "public-one", "Conflicting public work.");
    fixture.publish(NOW + 1);
    fixture.sync();
    let mut definition = crate::insights_report::installed_checks::tests::definition("/repo");
    definition.check_id = "retry".into();
    let request = crate::insights_report::ReportRequest {
        environment_key: "native".into(),
        window: antiburn_local::insights::ReportWindow {
            start_epoch: NOW - 100,
            end_epoch: NOW + 100,
        },
        computed_at_epoch: NOW + 10,
    };
    let report = crate::insights_report::installed_checks::read_installed_checks_in_window(
        &fixture.store,
        &[definition],
        &request,
    )
    .unwrap();
    assert!(report[0].diagnostics.iter().any(|diagnostic| {
        diagnostic.code == "source-unsafe-admission"
            && diagnostic
                .message
                .contains("Start a new supported agent session")
    }));
    assert!(
        !report[0]
            .waiting_for_events
            .is_some_and(|waiting| waiting.catching_up)
    );
}

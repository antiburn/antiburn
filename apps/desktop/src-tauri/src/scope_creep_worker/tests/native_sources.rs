use super::*;

pub(crate) fn sources() -> [(&'static str, &'static str, SourceFormat, &'static str); 3] {
    let claude = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/claude_characterization/retained_native_results.jsonl"
    ));
    let claude_root_end = claude
        .match_indices('\n')
        .nth(6)
        .expect("seven accepted retained-root rows")
        .0;
    [
        (
            "codex",
            "synthetic-root",
            SourceFormat::CodexRolloutJsonl,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-local/tests/fixtures/codex_characterization/paginated_completed.jsonl"
            )),
        ),
        (
            "claude-code",
            "scope-test",
            SourceFormat::ClaudeJsonl,
            &claude[..claude_root_end],
        ),
        (
            "pi",
            "synthetic",
            SourceFormat::PiV3Jsonl,
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-local/tests/fixtures/pi_characterization/core_linear_v3.jsonl"
            )),
        ),
    ]
}

pub(crate) fn read_sources() -> Vec<(String, String, SourceFormat, String)> {
    let codex_base = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/codex_characterization/paginated_completed.jsonl"
    ));
    let codex_rows = codex_base
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let mut codex_read = codex_rows[8].clone();
    codex_read["ordinal"] = json!(4);
    codex_read["timestamp"] = json!("2026-10-07T10:00:12Z");
    let command = "nl -ba sample.py | sed -n '2,4p'";
    codex_read["payload"]["item"]["command"][2] = json!(command);
    codex_read["payload"]["item"]["parsed_cmd"][0]["cmd"] = json!(command);
    let output = "     2\t    return 7\n     3\t    return second\n     4\t    return 9\n";
    codex_read["payload"]["item"]["stdout"] = json!(output);
    codex_read["payload"]["item"]["aggregated_output"] = json!(output);
    let mut codex_edit = codex_rows[6].clone();
    codex_edit["ordinal"] = json!(5);
    codex_edit["timestamp"] = json!("2026-10-07T10:00:13Z");
    let codex = codex_rows
        .iter()
        .take(4)
        .chain([&codex_read, &codex_edit])
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let claude = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/claude_characterization/retained_native_results.jsonl"
    ));
    let claude_root_end = claude
        .match_indices('\n')
        .nth(4)
        .expect("five accepted retained-root rows")
        .0;
    let mut claude = claude[..claude_root_end].to_owned();
    claude.push('\n');
    claude.push_str(
        r#"{"type":"user","uuid":"00000000-0000-0000-0000-000000000006","parentUuid":"00000000-0000-0000-0000-000000000005","sessionId":"scope-test","version":"2.1.278","entrypoint":"cli","isSidechain":false,"origin":{"kind":"human"},"promptSource":"typed","turnOrigin":"human","message":{"role":"user","content":"The read and tests are complete. Stop here."}}"#,
    );
    let pi = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../crates/antiburn-local/tests/fixtures/pi_characterization/core_linear_v3.jsonl"
    ));
    let pi_root_end = pi
        .match_indices('\n')
        .nth(5)
        .expect("six accepted native-read rows")
        .0;
    let mut pi_rows = pi[..pi_root_end]
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    pi_rows[4]["message"]["content"][0]["arguments"] = json!({"path":"src/example.py"});
    pi_rows[5]["message"]["content"][0]["text"] = json!("return café\nreturn 2\nreturn 3");
    let mut pi = pi_rows
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    pi.push('\n');
    pi.push_str(
        r#"{"type":"message","id":"edit-call","parentId":"result","timestamp":"2026-01-01T00:00:06Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"edit-id","name":"edit","arguments":{"path":"src/example.py","oldText":"return café","newText":"return 9"}}]}}"#,
    );
    pi.push('\n');
    pi.push_str(
        r#"{"type":"message","id":"edit-result","parentId":"edit-call","timestamp":"2026-01-01T00:00:07Z","message":{"role":"toolResult","toolCallId":"edit-id","toolName":"edit","content":[{"type":"text","text":"Text replaced."}],"isError":false}}"#,
    );
    pi.push('\n');
    pi.push_str(
        r#"{"type":"message","id":"human-after-read","parentId":"edit-result","timestamp":"2026-01-01T00:00:08Z","message":{"role":"user","content":[{"type":"text","text":"The read and edit are complete. Stop here."}]}}"#,
    );
    vec![
        (
            "codex".into(),
            "synthetic-root".into(),
            SourceFormat::CodexRolloutJsonl,
            codex,
        ),
        ("pi".into(), "synthetic".into(), SourceFormat::PiV3Jsonl, pi),
        (
            "claude-code".into(),
            "scope-test".into(),
            SourceFormat::ClaudeJsonl,
            claude,
        ),
    ]
}

pub(crate) fn publish(
    store: &Store,
    agent: &str,
    session_id: &str,
    format: SourceFormat,
    records: &str,
    cwd: &std::path::Path,
) -> BurnCheckCandidate {
    let source_directory = tempfile::tempdir().unwrap();
    let source_path = source_directory.path().join("session.jsonl");
    std::fs::write(&source_path, format!("{}\n", records.trim_end())).unwrap();
    let source_claim = antiburn_local::analysis::SourceClaim::from_fingerprint_inputs(
        &antiburn_local::discovery::FingerprintInputs {
            stat: antiburn_local::discovery::SourceStat::from_open_std_file(
                &std::fs::File::open(&source_path).unwrap(),
            )
            .unwrap(),
            head_hash: Some(antiburn_local::discovery::source_version::head_hash_of(
                &std::fs::read(&source_path).unwrap(),
            )),
        },
    );
    let fingerprint = antiburn_local::checks::ignored_instructions::sha256_hex(records.as_bytes());
    let key = SessionKey::new("native", agent, session_id);
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "file".into(),
                source_label: "synthetic-native".into(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: Some(cwd.to_string_lossy().into_owned()),
                surface: "cli".into(),
                updated_at_epoch: Some(9),
                activity_cursor: fingerprint.clone(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some(fingerprint.clone()),
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&[agent], 100, 60)
        .unwrap()
        .unwrap();
    let writer = Arc::new(FencedTurnRowStore::new(
        store.clone(),
        key.clone(),
        claim.claim_fence,
    ));
    let reader_agent = if agent == "claude-code" {
        "claude"
    } else {
        agent
    };
    let input = SessionInput {
        agent: reader_agent.into(),
        session_id: session_id.into(),
        source: RawSource::File(source_path),
        source_format: format,
        fork_parent_session_id: None,
    };
    let reader = reader_for(reader_agent);
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(agent, session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.into(),
            session_id: session_id.into(),
            kind: SourceKind::File,
            capabilities: reader.capabilities(&input),
        }),
        TurnRowSink::new(writer.clone() as Arc<dyn TurnRowStore>, session_id, None),
    );
    let outcome = reader
        .visit_claimed(
            &input,
            &source_claim,
            antiburn_local::analysis::AppendOnlyGuarantee::Absent,
            &|| false,
            &mut sink,
        )
        .unwrap();
    sink.observe_source_outcome(outcome);
    assert!(!sink.turn_row_write_failed(), "{agent}");
    writer
        .write_coverage_record(&sink.coverage_record().unwrap())
        .unwrap();
    let revisions = crate::analysis::projection_revisions();
    store
        .publish_projections(
            &AnalysisRecord {
                key: key.clone(),
                model_breakdown_json: "{}".into(),
                pricing_breakdown_json: "{}".into(),
                inclusive_models_json: "[]".into(),
                initial_context_json: None,
                source_summaries_json: None,
                provider_hints_json: None,
                source_fingerprint: fingerprint,
                pricing_generation: 1,
                analyzed_generation: claim.source_generation,
                parser_revision: revisions.parser_revision,
                analyzer_revision: revisions.analyzer_revision,
                metrics_schema_revision: revisions.metrics_schema_revision,
            },
            Some(1),
            &EvidenceCompletion {
                claim_fence: claim.claim_fence,
                status: PublishedEvidence::Ready,
                evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                evidence_json: serde_json::to_string(&sink.evidence().unwrap()).unwrap(),
            },
            &[],
            &[],
        )
        .unwrap();
    store
        .burn_check_candidates_for_revision(
            CHECK_ID,
            &CHECK.evaluator_revision(),
            1000,
            POLICY.idle_secs,
            16,
        )
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.session.key == key)
        .unwrap()
}

pub(crate) fn records_with_work(agent: &str, records: &str) -> String {
    if agent != "pi" {
        return records.to_owned();
    }
    let work = json!({"type":"message", "id":"work", "parentId":"selected", "timestamp":"2026-01-01T00:00:07Z", "message":{"role":"assistant", "content":[{"type":"toolCall", "id":"work-call", "name":"bash", "arguments":{"command":"cargo test --test billing"}}]}});
    let result = json!({"type":"message", "id":"work-result", "parentId":"work", "timestamp":"2026-01-01T00:00:08Z", "message":{"role":"toolResult", "toolCallId":"work-call", "toolName":"bash", "content":[{"type":"text", "text":"test result: ok. 2 passed; 0 failed"}], "isError":false, "details":{"exitCode":0}}});
    format!("{}\n{work}\n{result}\n", records.trim_end())
}

fn append_approval(agent: &str, records: &str) -> String {
    let rows = records
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let text = "I approve and accept the completed billing feature.";
    let approval = match agent {
        "codex" => {
            let mut row = rows
                .iter()
                .find(|row| row["metadata"]["retained_source"]["id"]["role"] == "user")
                .unwrap()
                .clone();
            row["timestamp"] = json!("2026-10-07T10:00:12Z");
            row["ordinal"] = json!(12);
            row["payload"]["id"] = json!("human-approval");
            row["payload"]["content"][0]["text"] = json!(text);
            row["metadata"]["retained_source"]["id"]["message_id"] = json!("human-approval");
            row["metadata"]["user_input_order"] = json!(1);
            row
        }
        "claude-code" => {
            let mut row = rows[0].clone();
            row["uuid"] = json!("00000000-0000-0000-0000-00000000000b");
            row["parentUuid"] = rows.last().unwrap()["uuid"].clone();
            row["message"]["content"] = json!(text);
            row
        }
        "pi" => {
            let mut row = rows.iter().find(|row| row["id"] == "task").unwrap().clone();
            row["id"] = json!("approval");
            row["parentId"] = rows.last().unwrap()["id"].clone();
            row["timestamp"] = json!("2026-01-01T00:00:09Z");
            row["message"]["content"][0]["text"] = json!(text);
            row
        }
        _ => unreachable!(),
    };
    format!("{}\n{approval}\n", records.trim_end())
}

#[test]
fn codex_prepares_publishes_restores_and_invalidates_scope_results() {
    assert_native_scope_source(sources()[0]);
}

#[test]
fn claude_history_prepares_publishes_restores_and_invalidates_scope_results() {
    assert_native_scope_source(sources()[1]);
}

#[test]
fn pi_prepares_publishes_restores_and_invalidates_scope_results() {
    assert_native_scope_source(sources()[2]);
}

fn assert_native_scope_source((agent, session, format, records): (&str, &str, SourceFormat, &str)) {
    let records = records_with_work(agent, records);
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.capture_burn_check_boundaries(&[CHECK_ID], 0).unwrap();
    let mut candidate = publish(&store, agent, session, format, &records, directory.path());
    if agent == "claude-code" {
        assert!(records.lines().all(|line| {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap()
                .get("timestamp")
                .is_none()
        }));
        candidate.historical = true;
    }
    let input = load_input(&store, &candidate, &ModelCapabilities::jev_default())
        .unwrap_or_else(|error| panic!("{agent}: {error:?}"));
    assert!(!input.plan.prepared.groups.is_empty(), "{agent}");
    assert!(input.plan.prepared.session_limitation.is_none(), "{agent}");
    let result = input
        .check
        .reduce(&input.plan, &results(&input, false), true)
        .unwrap();
    assert!(!result.findings.is_empty(), "{agent}");
    let finding_id = result.findings[0].id.clone();
    persist(&store, &input, result);
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    assert!(
        saved_finding_citations(&store.lock(), &SourceFence::from(&candidate), &finding_id)
            .unwrap()
            .is_some(),
        "{agent}"
    );
    let restored = load_input(&store, &candidate, &ModelCapabilities::jev_default()).unwrap();
    assert_eq!(
        restored.durable.input_revision, input.durable.input_revision,
        "{agent}"
    );
    let changed = append_approval(agent, &records);
    assert_ne!(changed, records, "{agent}");
    let mut current = publish(&store, agent, session, format, &changed, directory.path());
    current.historical = candidate.historical;
    let changed_input = load_input(&store, &current, &ModelCapabilities::jev_default()).unwrap();
    assert!(!changed_input.plan.prepared.groups.is_empty(), "{agent}");
    assert_ne!(
        changed_input.durable.input_revision, input.durable.input_revision,
        "{agent}"
    );
    assert!(
        changed_input
            .plan
            .shared_context
            .as_ref()
            .unwrap()
            .fields
            .to_string()
            .contains("I approve and accept the completed billing feature."),
        "{agent}"
    );
    assert!(
        current_publication(&store.lock(), &SourceFence::from(&candidate))
            .unwrap()
            .is_none(),
        "{agent}"
    );
    assert!(
        saved_finding_citations(&store.lock(), &SourceFence::from(&current), &finding_id)
            .unwrap()
            .is_none(),
        "{agent}"
    );
    let accepted = changed_input
        .check
        .reduce(&changed_input.plan, &results(&changed_input, true), true)
        .unwrap();
    assert!(accepted.findings.is_empty(), "{agent}");
    persist(&store, &changed_input, accepted);
    assert!(
        current_publication(&store.lock(), &SourceFence::from(&current))
            .unwrap()
            .is_some(),
        "{agent}"
    );
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET parser_revision = parser_revision - 1",
            [],
        )
        .unwrap();
    assert!(
        current_publication(&store.lock(), &SourceFence::from(&current))
            .unwrap()
            .is_none(),
        "{agent}"
    );
}

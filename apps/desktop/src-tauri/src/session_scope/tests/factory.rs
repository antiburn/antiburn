use std::sync::Arc;

use crate::store::{
    AnalysisRecord, EvidenceCompletion, FencedTurnRowStore, PublishedEvidence, SessionRecord,
};
use antiburn_local::analysis::reader_for;
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, RawSource, SessionEvidenceAccumulator, SessionInput,
    SessionMetricsAccumulator, SourceKind, TurnRowSink, TurnRowStore,
};
use antiburn_local::discovery::agents::opencode::db_session_fingerprint;

use super::*;

fn publish(environment: &str, synthetic: bool) -> (Store, SessionKey, i64, i64) {
    publish_with_extra_part(environment, synthetic, None)
}

fn publish_with_extra_part(
    environment: &str,
    synthetic: bool,
    extra_part: Option<serde_json::Value>,
) -> (Store, SessionKey, i64, i64) {
    publish_into(
        Store::open_in_memory(Path::new("/tmp/antiburn-scope-factory-tests")).unwrap(),
        environment,
        synthetic,
        extra_part,
    )
}

fn publish_into(
    store: Store,
    environment: &str,
    synthetic: bool,
    extra_part: Option<serde_json::Value>,
) -> (Store, SessionKey, i64, i64) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let source = rusqlite::Connection::open(&path).unwrap();
    source.execute_batch(
        "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
         INSERT INTO session VALUES ('scope', NULL, 1000, 2000);",
    ).unwrap();
    for index in 0..603 {
        let role = if index == 600 || index == 602 {
            "assistant"
        } else {
            "user"
        };
        let text = match index {
            600 => "May I change billing?".to_owned(),
            601 => "Yes, change billing now.".to_owned(),
            602 => "I will change billing.".to_owned(),
            _ => format!("{environment}: instruction {index}\n  Keep billing unchanged."),
        };
        let message = format!("m{index:04}");
        source
            .execute(
                "INSERT INTO message VALUES (?1, 'scope', ?2, ?2, ?3)",
                params![
                    message,
                    1001 + index,
                    serde_json::json!({"role": role}).to_string()
                ],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES (?1, ?2, 'scope', ?3, ?3, ?4)",
                params![
                    format!("p{index:04}"),
                    message,
                    1001 + index,
                    serde_json::json!({"type": "text", "text": text,
                    "synthetic": synthetic && index == 10})
                    .to_string()
                ],
            )
            .unwrap();
    }
    source
        .execute(
            "INSERT INTO part VALUES ('z-final', 'm0602', 'scope', 1603, 1603, ?1)",
            params![
                serde_json::json!({"type": "text", "text": "Final activity part."}).to_string()
            ],
        )
        .unwrap();
    if let Some(part) = extra_part {
        source
            .execute(
                "INSERT INTO part VALUES ('z-extra', 'm0010', 'scope', 1011, 1011, ?1)",
                params![part.to_string()],
            )
            .unwrap();
    }
    let (latest, rows) = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(db_session_fingerprint(path.clone(), "scope".into()))
        .unwrap();
    let fingerprint = format!("sv1:db:{latest}:{rows}");
    let key = SessionKey::new(environment, "opencode", "scope");
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "providerDb".into(),
                source_label: "synthetic-db".into(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: None,
                surface: "cli".into(),
                updated_at_epoch: Some(2),
                activity_cursor: String::new(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some(fingerprint.clone()),
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&["opencode"], 100, 60)
        .unwrap()
        .unwrap();
    let writer = Arc::new(FencedTurnRowStore::new(
        store.clone(),
        key.clone(),
        claim.claim_fence,
    ));
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "scope".into(),
        source: RawSource::Sqlite(path),
        source_format: SourceFormat::OpenCodeSqliteV2,
        fork_parent_session_id: None,
    };
    let reader = reader_for("opencode");
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("opencode", "scope"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "opencode".into(),
            session_id: "scope".into(),
            kind: SourceKind::Sqlite,
            capabilities: reader.capabilities(&input),
        }),
        TurnRowSink::new(writer.clone() as Arc<dyn TurnRowStore>, "scope", None),
    );
    let outcome = reader
        .visit_db_claimed(&input, &fingerprint, &|| false, &mut sink)
        .unwrap();
    sink.observe_source_outcome(outcome);
    assert!(!sink.turn_row_write_failed());
    writer
        .write_coverage_record(&sink.coverage_record().unwrap())
        .unwrap();
    let revisions = crate::analysis::projection_revisions();
    let projection = AnalysisRecord {
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
    };
    let completion = EvidenceCompletion {
        claim_fence: claim.claim_fence,
        status: PublishedEvidence::Ready,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        evidence_json: serde_json::to_string(&sink.evidence().unwrap()).unwrap(),
    };
    assert!(
        store
            .publish_projections(&projection, None, &completion, &[], &[])
            .unwrap()
    );
    (store, key, claim.claim_fence, claim.source_generation)
}

#[test]
fn real_parse_publish_factory_load_keeps_early_scope_and_later_approval() {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-scope-factory-tests")).unwrap();
    let publications = ["native", "wsl:synthetic", "ssh:synthetic"]
        .map(|environment| publish_into(store.clone(), environment, false, None));
    for (store, key, fence, generation) in publications {
        let environment = &key.environment_key;
        let request = store
            .session_scope_request(&key, fence, generation)
            .unwrap();
        assert_eq!(request.source_format, SourceFormat::OpenCodeSqliteV2);
        assert_eq!(
            (request.boundary.turn_index, request.boundary.part_index),
            (602, 1)
        );
        let scope = store.load_session_scope(&key, request).unwrap();
        assert_eq!(scope.occurrences().len(), 602);
        assert_eq!(
            scope.values()[0],
            format!("{environment}: instruction 0\n  Keep billing unchanged.")
        );
        assert_eq!(scope.values().last().unwrap(), "Yes, change billing now.");
        assert_eq!(
            scope.occurrences().last().unwrap().reference.turn_index,
            601
        );
    }
}

#[test]
fn factory_rejects_unknown_user_authority_before_selection_drops_it() {
    let (store, key, fence, generation) = publish("native", true);
    assert!(matches!(
        store.session_scope_request(&key, fence, generation),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::IncompleteSource
        )))
    ));
}

#[test]
fn factory_keeps_lost_history_and_compaction_as_partial_context() {
    for mutation in [
        "DELETE FROM turn WHERE turn_index = 0",
        "DELETE FROM turn_content WHERE turn_rowid IN (SELECT rowid FROM turn WHERE turn_index = 0)",
        "UPDATE turn SET is_compaction_boundary = 1 WHERE turn_index = 10",
    ] {
        let (store, key, fence, generation) = publish("native", false);
        store.lock().execute(mutation, []).unwrap();
        let request = store
            .session_scope_request(&key, fence, generation)
            .unwrap();
        assert!(!request.source_complete, "{mutation}");
        let scope = store.load_session_scope(&key, request).unwrap();
        assert!(
            scope
                .limitations()
                .contains(&ScopeMissingReason::IncompleteSource),
            "{mutation}"
        );
        assert!(!scope.occurrences().is_empty(), "{mutation}");
    }
}

#[test]
fn factory_rejects_invalid_identity_and_forks() {
    for mutation in [
        "UPDATE turn SET thread_id = 'sibling' WHERE turn_index = 10",
        "UPDATE turn_content SET normalized_fields_json = NULL WHERE turn_rowid IN
         (SELECT rowid FROM turn WHERE turn_index = 10)",
        "INSERT INTO session_relation (environment_key, agent, session_id, kind, related_id)
         VALUES ('native', 'opencode', 'scope', 'forkParent', 'parent')",
    ] {
        let (store, key, fence, generation) = publish("native", false);
        store.lock().execute(mutation, []).unwrap();
        assert!(
            matches!(
                store.session_scope_request(&key, fence, generation),
                Err(ScopeLoadError::Scope(SessionScopeError::Missing(
                    ScopeMissingReason::IncompleteSource
                )))
            ),
            "{mutation}"
        );
    }
}

#[test]
fn omitted_native_attachments_cannot_attest_complete_user_history() {
    let (store, key, fence, generation) = publish_with_extra_part(
        "native",
        false,
        Some(
            serde_json::json!({"type": "file", "url": "file:///synthetic/plan.md", "mime": "text/plain"}),
        ),
    );
    assert!(matches!(
        store.session_scope_request(&key, fence, generation),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::IncompleteSource
        )))
    ));
}

#[test]
fn factory_and_load_reject_publication_races_and_cross_environment_requests() {
    let (store, key, fence, generation) = publish("native", false);
    assert!(matches!(
        store.session_scope_request(&key, fence + 1, generation),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::PublicationChanged
        )))
    ));
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    let other = SessionKey::new("ssh:synthetic", "opencode", "scope");
    assert!(matches!(
        store.load_session_scope(&other, request),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::InvalidEvidence
        )))
    ));
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET published_fence = published_fence + 1",
            [],
        )
        .unwrap();
    assert!(matches!(
        store.load_session_scope(&key, request),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::PublicationChanged
        )))
    ));
    store
        .lock()
        .execute(
            "UPDATE session_evidence SET published_fence = published_fence - 1",
            [],
        )
        .unwrap();
    let request = store
        .session_scope_request(&key, fence, generation)
        .unwrap();
    store
        .lock()
        .execute(
            "UPDATE session SET source_generation = source_generation + 1",
            [],
        )
        .unwrap();
    assert!(matches!(
        store.session_scope_request(&key, fence, generation),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::PublicationChanged
        )))
    ));
    assert!(matches!(
        store.load_session_scope(&key, request),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::PublicationChanged
        )))
    ));
}

#[test]
fn accepted_full_exports_and_missing_coverage_do_not_attest_history() {
    let (store, key, fence, generation) = publish("native", false);
    let mut coverage = store.published_coverage_record(&key).unwrap().unwrap();
    coverage.capabilities.source_format = SourceFormat::OpenCodeJsonl;
    antiburn_local::analysis::insert_coverage_record(
        &store.lock(),
        &crate::session_scope::factory::turn_session_key(&key),
        fence,
        &coverage,
    )
    .unwrap();
    assert!(matches!(
        store.session_scope_request(&key, fence, generation),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::UnsupportedSource
        )))
    ));
    store
        .lock()
        .execute("DELETE FROM session_coverage", [])
        .unwrap();
    assert!(matches!(
        store.session_scope_request(&key, fence, generation),
        Err(ScopeLoadError::Scope(SessionScopeError::Missing(
            ScopeMissingReason::IncompleteSource
        )))
    ));
}

pub(super) fn publish_jsonl(
    agent: &str,
    session: &str,
    format: SourceFormat,
    text: &str,
) -> (Store, SessionKey, i64, i64) {
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-scope-source-tests")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scope.jsonl");
    std::fs::write(&path, format!("{text}\n")).unwrap();
    let source_claim = antiburn_local::analysis::SourceClaim::from_fingerprint_inputs(
        &antiburn_local::discovery::FingerprintInputs {
            stat: antiburn_local::discovery::SourceStat::from_open_std_file(
                &std::fs::File::open(&path).unwrap(),
            )
            .unwrap(),
            head_hash: Some(antiburn_local::discovery::source_version::head_hash_of(
                &std::fs::read(&path).unwrap(),
            )),
        },
    );
    let key = SessionKey::new(
        "native",
        if agent == "claude" {
            "claude-code"
        } else {
            agent
        },
        session,
    );
    let fingerprint = "sv1:synthetic-jsonl-v1";
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "file".into(),
                source_label: "synthetic-jsonl".into(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: None,
                surface: "cli".into(),
                updated_at_epoch: Some(1),
                activity_cursor: String::new(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some(fingerprint.into()),
            }],
            &crate::agents::evidence_cohort(),
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&[key.agent.as_str()], 100, 60)
        .unwrap()
        .unwrap();
    let writer = Arc::new(FencedTurnRowStore::new(
        store.clone(),
        key.clone(),
        claim.claim_fence,
    ));
    let input = SessionInput {
        agent: agent.into(),
        session_id: session.into(),
        source: RawSource::File(path),
        source_format: format,
        fork_parent_session_id: None,
    };
    let reader = reader_for(agent);
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(&key.agent, session),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: key.agent.clone(),
            session_id: session.into(),
            kind: SourceKind::from(&input.source),
            capabilities: reader.capabilities(&input),
        }),
        TurnRowSink::new(writer.clone() as Arc<dyn TurnRowStore>, session, None),
    );
    let outcome = if matches!(
        format,
        SourceFormat::ClaudeJsonl | SourceFormat::CodexRolloutJsonl | SourceFormat::PiV3Jsonl
    ) {
        reader
            .visit_claimed(
                &input,
                &source_claim,
                antiburn_local::analysis::AppendOnlyGuarantee::Absent,
                &|| false,
                &mut sink,
            )
            .unwrap()
    } else {
        reader.visit(&input, &mut sink).unwrap()
    };
    sink.observe_source_outcome(outcome);
    assert!(!sink.turn_row_write_failed());
    writer
        .write_coverage_record(&sink.coverage_record().unwrap())
        .unwrap();
    let revisions = crate::analysis::projection_revisions();
    assert!(
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
                    source_fingerprint: fingerprint.into(),
                    pricing_generation: 1,
                    analyzed_generation: claim.source_generation,
                    parser_revision: revisions.parser_revision,
                    analyzer_revision: revisions.analyzer_revision,
                    metrics_schema_revision: revisions.metrics_schema_revision,
                },
                None,
                &EvidenceCompletion {
                    claim_fence: claim.claim_fence,
                    status: PublishedEvidence::Ready,
                    evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                    evidence_json: serde_json::to_string(&sink.evidence().unwrap()).unwrap(),
                },
                &[],
                &[]
            )
            .unwrap()
    );
    (store, key, claim.claim_fence, claim.source_generation)
}

#[test]
fn first_tier_parsed_text_and_optional_scope_evidence_do_not_prove_full_history() {
    macro_rules! fixture {
        ($name:literal) => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../../crates/antiburn-local/tests/fixtures/",
                $name
            ))
        };
    }
    for (agent, session, format, text, optional) in [
        (
            "claude",
            "root",
            SourceFormat::ClaudeJsonl,
            fixture!("claude_characterization/scope_records.jsonl"),
            true,
        ),
        (
            "codex",
            "synthetic-scope",
            SourceFormat::CodexRolloutJsonl,
            fixture!("codex_characterization/scope_records.jsonl"),
            true,
        ),
        (
            "cursor",
            "scope",
            SourceFormat::CursorCliAgentJsonl,
            fixture!("cursor_characterization/optional_evidence_negative.jsonl"),
            false,
        ),
        (
            "antigravity",
            "scope",
            SourceFormat::AntigravityBrainJsonl,
            fixture!("antigravity_characterization/scope_records.jsonl"),
            false,
        ),
    ] {
        let (store, key, fence, generation) = publish_jsonl(agent, session, format, text);
        let coverage = store.published_coverage_record(&key).unwrap().unwrap();
        assert_eq!(coverage.capabilities.source_format, format);
        let (user_text, history_proofs, optional_records): (i64, i64, i64) = store.lock().query_row(
            "SELECT COUNT(*) FILTER (WHERE kind = 'user'),
                COUNT(*) FILTER (WHERE json_extract(normalized_fields_json, '$.metadata.user_text_history') IS NOT NULL),
                COUNT(*) FILTER (WHERE json_array_length(json_extract(normalized_fields_json, '$.metadata.user_answers')) > 0
                    OR json_array_length(json_extract(normalized_fields_json, '$.metadata.plan_references')) > 0)
             FROM turn_content", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
        assert!(user_text > 0, "{agent}");
        assert!(history_proofs < user_text, "{agent}");
        assert_eq!(optional_records > 0, optional, "{agent}");
        assert!(
            matches!(
                store.session_scope_request(&key, fence, generation),
                Err(ScopeLoadError::Scope(SessionScopeError::Missing(
                    ScopeMissingReason::UnsupportedSource | ScopeMissingReason::IncompleteSource
                )))
            ),
            "{agent}"
        );
        for detector in [
            crate::smart_check_inputs::DetectorInput::ScopeCreep,
            crate::smart_check_inputs::DetectorInput::OverExploring,
            crate::smart_check_inputs::DetectorInput::SkillOpportunities,
        ] {
            assert!(
                matches!(
                    store.load_smart_check_inputs(&key, fence, generation, detector),
                    Err(crate::smart_check_inputs::InputLoadError::Scope(
                        ScopeLoadError::Scope(SessionScopeError::Missing(
                            ScopeMissingReason::UnsupportedSource
                                | ScopeMissingReason::IncompleteSource
                        ))
                    ))
                ),
                "{agent} {detector:?}"
            );
        }
    }
}

use std::path::Path;
use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EVIDENCE_SCHEMA_REVISION, EvidenceSource, RawSource, SessionEvidenceAccumulator,
    SessionInput, SessionMetricsAccumulator, SourceFormat, SourceKind, TurnRowSink, TurnRowStore,
    reader_for,
};
use antiburn_local::discovery::agents::opencode::db_session_fingerprint;
use antiburn_local::model::AgentKind;
use serde_json::json;

use super::*;
use crate::agent_config::ConfigContext;
use crate::store::{
    AnalysisRecord, EvidenceCompletion, FencedTurnRowStore, PublishedEvidence, SessionRecord,
};

const OUTPUT: &str = "<path>/synthetic/parser.rs</path>\n<type>file</type>\n<content>\n1: parser\n\n(End of file - total 1 lines)\n</content>";

fn publish(
    environment: &str,
    fillers: usize,
    ending: &str,
    input: serde_json::Value,
) -> (Store, SessionKey, i64, i64) {
    publish_tool(environment, fillers, ending, "read", input, OUTPUT)
}

fn publish_tool(
    environment: &str,
    fillers: usize,
    ending: &str,
    tool: &str,
    input: serde_json::Value,
    output: &str,
) -> (Store, SessionKey, i64, i64) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("opencode.db");
    let source = rusqlite::Connection::open(&path).unwrap();
    source.execute_batch(
        "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, time_created INTEGER, time_updated INTEGER);
         CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
         CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
         INSERT INTO session VALUES ('input', NULL, 1000, 99999);",
    ).unwrap();
    let mut records = vec![(
        "user",
        json!({"type":"text", "text": format!("{environment}: Fix the parser.\n  Keep billing unchanged.")}),
    )];
    for _ in 0..fillers {
        records.push((
            "assistant",
            json!({"type":"text", "text":"Recorded context."}),
        ));
    }
    let mut tool_part = json!({"type":"tool", "tool":tool, "callID":"read-call",
        "state":{"status":"completed", "input":input, "output": output}});
    if tool == "skill" && output.starts_with("<skill_content ") {
        tool_part["state"]["metadata"] = json!({"name":"review", "dir":"/synthetic/skills/review", "truncated":false, "interrupted":false});
    }
    records.push(("assistant", tool_part));
    match ending {
        "user" => records.push(("user", json!({"type":"text", "text":"Now inspect tests."}))),
        "unknown" => records.push(("assistant", json!({"type":"text", "text":"Complete."}))),
        "edit" | "pending-edit" => records.push(("assistant", json!({"type":"tool", "tool":"edit", "callID":"edit-call",
            "state":{"status":if ending == "edit" { "completed" } else { "running" },
                "input":{"filePath":"/synthetic/parser.rs", "oldString":"parser", "newString":"fixed parser"},
                "output":"Edit applied."}}))),
        _ => {}
    }
    for (index, (role, part)) in records.into_iter().enumerate() {
        let message = format!("m{index:06}");
        source
            .execute(
                "INSERT INTO message VALUES (?1, 'input', ?2, ?2, ?3)",
                rusqlite::params![message, index + 1001, json!({"role":role}).to_string()],
            )
            .unwrap();
        source
            .execute(
                "INSERT INTO part VALUES (?1, ?2, 'input', ?3, ?3, ?4)",
                rusqlite::params![
                    format!("p{index:06}"),
                    message,
                    index + 1001,
                    part.to_string()
                ],
            )
            .unwrap();
    }
    let (latest, rows) = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(db_session_fingerprint(path.clone(), "input".into()))
        .unwrap();
    let fingerprint = format!("sv1:db:{latest}:{rows}");
    let store = Store::open_in_memory(Path::new("/tmp/antiburn-smart-input-tests")).unwrap();
    let key = SessionKey::new(environment, "opencode", "input");
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
                updated_at_epoch: Some(99),
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
        session_id: "input".into(),
        source: RawSource::Sqlite(path),
        source_format: SourceFormat::OpenCodeSqliteV2,
        fork_parent_session_id: None,
    };
    let reader = reader_for("opencode");
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("opencode", "input"),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "opencode".into(),
            session_id: "input".into(),
            kind: SourceKind::Sqlite,
            capabilities: reader.capabilities(&input),
        }),
        TurnRowSink::new(writer.clone() as Arc<dyn TurnRowStore>, "input", None),
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
fn parse_publish_load_joins_read_across_pages_and_keeps_full_scope() {
    let (store, key, fence, generation) = publish(
        "native",
        254,
        "user",
        json!({"filePath":"/synthetic/parser.rs", "offset":1, "limit":1}),
    );
    let snapshot = store
        .load_smart_check_inputs(&key, fence, generation, DetectorInput::OverExploring)
        .unwrap();
    assert_eq!(snapshot.content().actions.len(), 258);
    let request = &snapshot.content().actions[255];
    let result = &snapshot.content().actions[256];
    assert_eq!(
        result
            .metadata
            .read_result
            .as_ref()
            .unwrap()
            .request_reference_id
            .as_deref(),
        Some(request.reference.id.as_str())
    );
    assert_eq!(
        request.metadata.read_request.as_ref().unwrap().extent.limit,
        Some(1)
    );
    assert_eq!(
        request.reference.native_record_id.as_deref(),
        Some("m000255")
    );
    assert_eq!(
        snapshot.scope().values()[0],
        "native: Fix the parser.\n  Keep billing unchanged."
    );
    assert!(matches!(
        snapshot.investigation_spans()[0].completion,
        EpisodeCompletion::NextAuthoritativeUser { .. }
    ));
    let episodes = snapshot.over_exploring_input().unwrap();
    assert_eq!(
        episodes.episodes[0].state,
        over_exploring::EpisodeState::Complete
    );
    assert_eq!(
        episodes.episodes[0].reads[0].output.as_deref(),
        Some(OUTPUT)
    );
    assert_eq!(
        episodes.episodes[0].subsequent[0].text,
        "Now inspect tests."
    );
    assert_eq!(
        snapshot.input_revision(),
        store
            .load_smart_check_inputs(&key, fence, generation, DetectorInput::OverExploring)
            .unwrap()
            .input_revision()
    );
}

#[test]
fn current_episode_requires_completed_task_continuation_after_reads() {
    for (ending, complete) in [
        ("tool", false),
        ("unknown", false),
        ("edit", true),
        ("pending-edit", false),
    ] {
        let (store, key, fence, generation) = publish(
            "native",
            0,
            ending,
            json!({"filePath":"/synthetic/parser.rs"}),
        );
        let snapshot = store
            .load_smart_check_inputs(&key, fence, generation, DetectorInput::OverExploring)
            .unwrap();
        let spans = snapshot.investigation_spans();
        assert_eq!(
            spans[0].span.state == over_exploring::EpisodeState::Complete,
            complete
        );
        if !complete {
            assert_eq!(spans[0].completion, EpisodeCompletion::Unknown);
            assert_eq!(
                snapshot.over_exploring_input().unwrap().episodes[0].state,
                over_exploring::EpisodeState::Deferred
            );
        } else {
            assert!(matches!(
                spans[0].completion,
                EpisodeCompletion::RecordedOperation { .. }
            ));
        }
    }
}

#[test]
fn candidate_binding_rejects_identity_generation_and_publication_mismatches() {
    let (store, key, fence, generation) = publish(
        "native",
        0,
        "user",
        json!({"filePath":"/synthetic/parser.rs"}),
    );
    let snapshot = store
        .load_smart_check_inputs(&key, fence, generation, DetectorInput::OverExploring)
        .unwrap();
    let candidate = crate::store::BurnCheckCandidate {
        session: store.session(&key).unwrap().unwrap(),
        incarnation: 1,
        source_generation: generation,
        source_fingerprint: None,
        activity_cursor: "synthetic".into(),
        published_fence: fence,
        boundary_at_epoch: 0,
        boundary_positions: BTreeMap::new(),
        historical: true,
    };
    assert!(snapshot.clone().for_candidate(&candidate).is_ok());
    for field in ["identity", "generation", "fence"] {
        let mut changed = candidate.clone();
        match field {
            "identity" => changed.session.key.session_id.push_str("-changed"),
            "generation" => changed.source_generation += 1,
            "fence" => changed.published_fence += 1,
            _ => unreachable!(),
        }
        assert!(
            matches!(
                snapshot.clone().for_candidate(&changed),
                Err(InputLoadError::Unavailable(
                    InputUnavailable::PublicationChanged
                ))
            ),
            "{field}"
        );
    }
}

#[test]
fn next_user_does_not_complete_unfinished_or_ambiguous_operations() {
    for mutation in [
        "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.metadata.state', 'running') WHERE kind = 'tool_input'",
        "UPDATE turn_content SET tool_call_id = 'orphan' WHERE kind = 'tool_result'",
        "DELETE FROM turn_content WHERE kind = 'tool_result'",
    ] {
        let (store, key, fence, generation) = publish(
            "native",
            0,
            "user",
            json!({"filePath":"/synthetic/parser.rs"}),
        );
        store.lock().execute(mutation, []).unwrap();
        let snapshot = store
            .load_smart_check_inputs(&key, fence, generation, DetectorInput::OverExploring)
            .unwrap();
        assert_eq!(
            snapshot.investigation_spans()[0].completion,
            EpisodeCompletion::Unknown,
            "{mutation}"
        );
        assert_eq!(
            snapshot.over_exploring_input().unwrap().episodes[0].state,
            over_exploring::EpisodeState::Deferred
        );
    }
}

#[test]
fn source_and_selected_evidence_limits_are_typed_unavailable() {
    let (store, key, fence, generation) = publish("native", 0, "user", json!({}));
    assert!(matches!(
        store.load_smart_check_inputs(&key, fence, generation, DetectorInput::OverExploring),
        Err(InputLoadError::Unavailable(
            InputUnavailable::IncompleteEvidence
        ))
    ));
    for mutation in [
        "UPDATE turn_content SET authority = 'unknown' WHERE kind = 'user'",
        "UPDATE turn_content SET truncated = 1 WHERE kind = 'tool_result'",
        "UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.malformed', 1) WHERE kind = 'tool_input'",
        "UPDATE session SET source_generation = source_generation + 1",
        "UPDATE session_evidence SET published_fence = published_fence + 1",
    ] {
        let (store, key, fence, generation) = publish(
            "native",
            0,
            "user",
            json!({"filePath":"/synthetic/parser.rs"}),
        );
        store.lock().execute(mutation, []).unwrap();
        assert!(
            store
                .load_smart_check_inputs(&key, fence, generation, DetectorInput::OverExploring)
                .is_err(),
            "{mutation}"
        );
    }
    let (store, key, fence, generation) = publish(
        "native",
        4096,
        "user",
        json!({"filePath":"/synthetic/parser.rs"}),
    );
    assert!(matches!(
        store.load_smart_check_inputs(&key, fence, generation, DetectorInput::OverExploring),
        Err(InputLoadError::Unavailable(
            InputUnavailable::AssemblyLimitReached
        ))
    ));
}

#[test]
fn scope_creep_constructor_accepts_production_snapshot() {
    let (store, key, fence, generation) = publish(
        "native",
        254,
        "user",
        json!({"filePath":"/synthetic/parser.rs"}),
    );
    let snapshot = store
        .load_smart_check_inputs(&key, fence, generation, DetectorInput::ScopeCreep)
        .unwrap();
    assert!(!snapshot.content().actions[255].metadata.bindings.is_empty());
    let check =
        scope_creep::ScopeCreepCheck::new(snapshot.scope_creep_input(BTreeSet::new()).unwrap())
            .unwrap();
    assert!(!check.context().input_revision.is_empty());
}

fn inventory_context() -> (tempfile::TempDir, ConfigContext) {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    std::fs::create_dir_all(home.join(".config/opencode/skills/review")).unwrap();
    std::fs::write(
        home.join(".config/opencode/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review parser correctness.\n---\nPrivate body.\n",
    )
    .unwrap();
    (
        directory,
        ConfigContext::native(AgentKind::OpenCode, home, None),
    )
}

#[test]
fn skills_use_same_fence_and_never_claim_session_wide_absence() {
    let (_directory, context) = inventory_context();
    let (store, key, fence, generation) = publish(
        "native",
        254,
        "user",
        json!({"filePath":"/synthetic/parser.rs"}),
    );
    let snapshot = store
        .load_smart_check_inputs(&key, fence, generation, DetectorInput::SkillOpportunities)
        .unwrap();
    let skills = store
        .load_smart_check_skill_inputs(snapshot, &context)
        .unwrap();
    assert_eq!(skills.inventory().skills().len(), 1);
    assert_eq!(skills.usage().publication_fence(), Some(fence));
    assert!(!skills.usage().proves_session_wide_absence());
    skills.check().unwrap();
}

#[test]
fn native_codex_skill_inventory_requires_workspace_trust_and_valid_definitions() {
    use crate::scope_creep_worker::tests::native_sources;
    use antiburn_local::analysis::jev::JevInputField;
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let workspace = directory.path().join("workspace");
    let skill_path = home.join(".codex/skills/verify/SKILL.md");
    std::fs::create_dir_all(skill_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        &skill_path,
        "---\nname: verify\ndescription: Review code and run tests.\n---\n",
    )
    .unwrap();
    let store = Store::open(directory.path()).unwrap();
    store
        .capture_burn_check_boundaries(
            &[crate::scope_creep_worker::CHECK_ID, "skill_opportunities"],
            0,
        )
        .unwrap();
    let (agent, session, format, records) = native_sources::sources()
        .into_iter()
        .find(|(agent, _, _, _)| *agent == "codex")
        .unwrap();
    let candidate = native_sources::publish(&store, agent, session, format, records, &workspace);
    let context = ConfigContext::native(AgentKind::Codex, &home, Some(workspace.clone()));
    let load = || {
        let snapshot = store
            .load_smart_check_inputs(
                &candidate.session.key,
                candidate.published_fence,
                candidate.source_generation,
                DetectorInput::SkillOpportunities,
            )
            .unwrap();
        store.load_smart_check_skill_inputs(snapshot, &context)
    };
    assert!(matches!(
        load(),
        Err(InputLoadError::Unavailable(
            InputUnavailable::InventoryIncomplete
        ))
    ));
    let config_path = home.join(".codex/config.toml");
    let trusted_config = format!(
        "[projects.{}]\ntrust_level = \"trusted\"\n",
        serde_json::to_string(workspace.canonicalize().unwrap().to_str().unwrap()).unwrap()
    );
    std::fs::write(&config_path, &trusted_config).unwrap();
    let inputs = load().unwrap();
    assert_eq!(inputs.inventory().skills().len(), 1);
    assert_eq!(inputs.inventory().skills()[0].name, "verify");
    assert!(inputs.input().content().actions.iter().any(|action| {
        action.authority == "unknown"
            && action
                .metadata
                .recorded_skill_result
                .as_ref()
                .is_some_and(|fact| fact.field == JevInputField::UserMessage)
    }));
    assert!(
        inputs.usage().events().iter().any(
            |event| event.lifecycle == skill_opportunities::SkillUseLifecycle::DocumentSelected
        )
    );
    std::fs::remove_file(&config_path).unwrap();
    assert!(matches!(
        load(),
        Err(InputLoadError::Unavailable(
            InputUnavailable::InventoryIncomplete
        ))
    ));
    std::fs::write(&config_path, &trusted_config).unwrap();
    std::fs::write(&skill_path, "---\nname: verify\n---\n").unwrap();
    assert!(matches!(
        load(),
        Err(InputLoadError::Unavailable(
            InputUnavailable::InventoryIncomplete
        ))
    ));
}

#[test]
fn accepted_skill_request_survives_page_boundary_without_invented_native_result_metadata() {
    let (_directory, context) = inventory_context();
    let (store, key, fence, generation) = publish_tool(
        "native",
        254,
        "user",
        "skill",
        json!({"name":"review"}),
        "Skill selected.",
    );
    let snapshot = store
        .load_smart_check_inputs(&key, fence, generation, DetectorInput::SkillOpportunities)
        .unwrap();
    assert_eq!(snapshot.content().actions[255].kind, "tool_input");
    assert_eq!(snapshot.content().actions[256].kind, "tool_result");
    let skills = store
        .load_smart_check_skill_inputs(snapshot, &context)
        .unwrap();
    assert!(skills.usage().events().iter().any(|event| {
        event.lifecycle == skill_opportunities::SkillUseLifecycle::Requested
            && event.skill
                == skill_opportunities::RecordedSkillIdentity::Name {
                    name: "review".into(),
                }
    }));
    assert!(
        skills
            .usage()
            .coverage()
            .limitations
            .contains(&skill_opportunities::SkillUseLimit::NativeMetadataUnavailable)
    );
    assert!(
        !skills
            .usage()
            .events()
            .iter()
            .any(|event| event.lifecycle == skill_opportunities::SkillUseLifecycle::Succeeded)
    );
    skills.check().unwrap();
}

#[test]
fn native_skill_output_proof_never_enters_input_only_projection() {
    use antiburn_local::analysis::jev::{JevInputField, JevInputSelection};
    let native: serde_json::Value = serde_json::from_str(include_str!("../../../../../crates/antiburn-local/tests/fixtures/skill_use_characterization/opencode.jsonl")).unwrap();
    let output = native["state"]["output"].as_str().unwrap();
    let (store, key, fence, generation) = publish_tool(
        "native",
        0,
        "user",
        "skill",
        json!({"name":"review"}),
        output,
    );
    let snapshot = store
        .load_smart_check_inputs(&key, fence, generation, DetectorInput::SkillOpportunities)
        .unwrap();
    let proof = snapshot
        .content()
        .actions
        .iter()
        .filter(|action| action.kind == "tool_result")
        .find_map(|action| action.metadata.recorded_skill_result.as_ref())
        .unwrap();
    assert_eq!(
        proof.text_digest,
        antiburn_local::analysis::jev_evidence::skill_text_digest(output)
    );
    assert_eq!(proof.session_id.as_deref(), Some(key.session_id.as_str()));
    assert_eq!(
        proof.status,
        antiburn_local::analysis::jev_evidence::JevRecordedSkillStatus::DocumentSelected
    );
    let input_only = JevInputSelection::from_fields(&[JevInputField::OtherToolInput]);
    assert!(snapshot.content().actions.iter().all(|action| {
        action
            .metadata
            .selected(input_only)
            .recorded_skill_result
            .as_ref()
            .is_none_or(|fact| fact.field == JevInputField::OtherToolInput)
    }));
    let content = store
        .published_turn_content_keyset_selected(
            &key,
            SelectedContentRequest {
                source_generation: generation,
                after_ms: None,
                source_positions: &BTreeMap::new(),
                selection: input_only,
                cursor: None,
            },
        )
        .unwrap()
        .unwrap()
        .content;
    assert_eq!(content.parts.len(), 1);
    assert!(
        content.parts[0]
            .part
            .metadata
            .recorded_skill_result
            .as_ref()
            .is_none_or(|fact| fact.field == JevInputField::OtherToolInput)
    );
    let part = &content.parts[0].part;
    let serialized =
        serde_json::to_string(&(&part.text, &part.normalized_fields, &part.metadata)).unwrap();
    assert!(!serialized.contains("skill_content"));

    store.lock().execute("UPDATE turn_content SET normalized_fields_json = json_set(normalized_fields_json, '$.metadata.recorded_skill_result.location', ?1) WHERE json_extract(normalized_fields_json, '$.metadata.recorded_skill_result.field') = 'other_tool_output'", ["x".repeat(1024 * 1024)]).unwrap();
    let content = store
        .published_turn_content_keyset_selected(
            &key,
            SelectedContentRequest {
                source_generation: generation,
                after_ms: None,
                source_positions: &BTreeMap::new(),
                selection: input_only,
                cursor: None,
            },
        )
        .unwrap()
        .unwrap()
        .content;
    assert_eq!(content.parts.len(), 1);
    assert_eq!(content.coverage.oversized_parts, 0);
    assert!(
        content.parts[0]
            .part
            .metadata
            .recorded_skill_result
            .as_ref()
            .is_none_or(|fact| fact.field == JevInputField::OtherToolInput)
    );
    assert_eq!(content.publication_fence, fence);
}

#[test]
fn inventory_rejects_remote_wsl_and_wrong_workspace_without_host_substitution() {
    let (_directory, context) = inventory_context();
    for environment in ["ssh:synthetic", "wsl:synthetic"] {
        let (store, key, fence, generation) = publish(
            environment,
            0,
            "user",
            json!({"filePath":"/synthetic/parser.rs"}),
        );
        let snapshot = store
            .load_smart_check_inputs(&key, fence, generation, DetectorInput::SkillOpportunities)
            .unwrap();
        assert!(snapshot.content().actions[0].text.starts_with(environment));
        assert!(matches!(
            store.load_smart_check_skill_inputs(snapshot, &context),
            Err(InputLoadError::Unavailable(
                InputUnavailable::UnsupportedInventoryEnvironment
            ))
        ));
    }
    let (store, key, fence, generation) = publish(
        "native",
        0,
        "user",
        json!({"filePath":"/synthetic/parser.rs"}),
    );
    let snapshot = store
        .load_smart_check_inputs(&key, fence, generation, DetectorInput::SkillOpportunities)
        .unwrap();
    let mut wrong = context.clone();
    wrong.workspace_cwd = Some(wrong.home_root.clone());
    assert!(matches!(
        store.load_smart_check_skill_inputs(snapshot, &wrong),
        Err(InputLoadError::Unavailable(
            InputUnavailable::InventoryContextMismatch
        ))
    ));
}

#[test]
fn independent_inventory_revision_detects_edits_removals_and_availability_changes() {
    let (_directory, context) = inventory_context();
    let path = context
        .home_root
        .join(".config/opencode/skills/review/SKILL.md");
    let mut observer = InventoryRevisionObserver::default();
    let first = observer.observe("native", &context).unwrap().unwrap();
    assert!(first.previous_revision.is_none());
    assert!(observer.observe("native", &context).unwrap().is_none());
    std::fs::write(
        &path,
        "---\nname: review\ndescription: Inspect parser invariants.\n---\n",
    )
    .unwrap();
    let changed = observer.observe("native", &context).unwrap().unwrap();
    assert_eq!(changed.previous_revision, Some(first.current_revision));
    assert_ne!(
        changed.current_revision,
        changed.previous_revision.clone().unwrap()
    );
    std::fs::write(&path, "---\nname: review\n---\n").unwrap();
    assert!(observer.observe("native", &context).unwrap().is_some());
    std::fs::remove_file(&path).unwrap();
    assert!(observer.observe("native", &context).unwrap().is_some());
    assert!(observer.observe("wsl:synthetic", &context).is_err());
    observer.forget(&first.context_identity);
    assert!(
        observer
            .observe("native", &context)
            .unwrap()
            .unwrap()
            .previous_revision
            .is_none()
    );
}

#[test]
fn independent_observation_has_a_hard_context_bound_without_eviction() {
    let (_directory, context) = inventory_context();
    let mut observer = InventoryRevisionObserver::default();
    for index in 0..InventoryRevisionObserver::MAX_CONTEXTS {
        let mut scoped = context.clone();
        scoped.home_root = context.home_root.join(format!("missing-{index}"));
        assert!(observer.observe("native", &scoped).unwrap().is_some());
    }
    assert!(matches!(
        observer.observe("native", &context),
        Err(InputLoadError::Unavailable(
            InputUnavailable::AssemblyLimitReached
        ))
    ));
}

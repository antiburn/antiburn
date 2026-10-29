use super::*;
use crate::agent_config::opencode_config_authority::CurrentConfigAuthority;
use crate::store::{
    AnalysisRecord, EvidenceCompletion, FencedTurnRowStore, PublishedEvidence, SessionRecord,
};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, METRICS_SCHEMA_REVISION, PARSER_REVISION, RawSource,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SourceKind, TurnRowSink,
    TurnRowStore, reader_for_input,
};
use antiburn_local::insights::ReportWindow;
use std::sync::Arc;

struct Fixture {
    temporary: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
    config: PathBuf,
    database: PathBuf,
    store: Store,
    controller: RemediationController,
}

struct Session<'a> {
    id: &'a str,
    started_at_epoch: i64,
    observed_at_ms: i64,
    model: &'a str,
    cwd: &'a Path,
    source: SourceFormat,
    prompt: Option<&'a str>,
    attributed: bool,
}

impl Fixture {
    fn new() -> Self {
        install_synthetic_pricing();
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let home = root.join("home");
        let cwd = root.join("repo");
        let config = home.join(".config/opencode/opencode.json");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(&config, r#"{"model":"openai/gpt-5.5"}"#).unwrap();
        let database = root.join("native.db");
        rusqlite::Connection::open(&database).unwrap().execute_batch(
            "CREATE TABLE session_v2 (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT NOT NULL, title TEXT, time_created INTEGER, time_updated INTEGER);
             CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER, data TEXT, time_created INTEGER, time_updated INTEGER);
             CREATE UNIQUE INDEX native_order ON session_message(session_id, seq);"
        ).unwrap();
        let store = Store::open(&root).unwrap();
        store
            .replace_repositories(&[crate::store::RepositoryRecord {
                key: "repo".into(),
                repo_name: "repo".into(),
                full_name: "synthetic/repo".into(),
                status: "accessible".into(),
                repo_root: Some(cwd.to_string_lossy().into_owned()),
                suspected_path: None,
                worktree_count: 1,
                session_count: 1,
                wsl_distro: None,
                enabled: true,
            }])
            .unwrap();
        Self {
            controller: RemediationController::new(root),
            temporary,
            home,
            cwd,
            config,
            database,
            store,
        }
    }

    fn session<'a>(&'a self, id: &'a str, started_at_epoch: i64, model: &'a str) -> Session<'a> {
        Session {
            id,
            started_at_epoch,
            observed_at_ms: started_at_epoch * 1_000 + 10,
            model,
            cwd: &self.cwd,
            source: SourceFormat::OpenCodeSqliteV2,
            prompt: None,
            attributed: true,
        }
    }

    fn authority(&self, cwd: &Path) -> CurrentConfigAuthority {
        let mut entries = Vec::new();
        for path in [&self.config, &cwd.join("opencode.json")] {
            if path.is_file() {
                let raw: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                let (provider, model) = raw["model"].as_str().unwrap().split_once('/').unwrap();
                entries.push(json!({"type":"document","path":path,"info":{"model":{"providerID":provider,"model":model}}}));
            }
        }
        CurrentConfigAuthority::fixture(cwd, json!(entries))
    }

    fn publish(&self, session: Session<'_>) -> SessionKey {
        let connection = rusqlite::Connection::open(&self.database).unwrap();
        connection
            .execute(
                "INSERT INTO session_v2 VALUES (?1, NULL, ?2, '', ?3, ?4)",
                rusqlite::params![
                    session.id,
                    session.cwd.to_string_lossy(),
                    session.started_at_epoch * 1_000,
                    session.observed_at_ms
                ],
            )
            .unwrap();
        let mut messages = Vec::new();
        if let Some(prompt) = session.prompt {
            messages.push((
                "user",
                json!({"time":{"created":session.started_at_epoch * 1_000},"text":prompt}),
            ));
        }
        messages.push(("assistant", json!({"time":{"created":session.observed_at_ms},"agent":"build","model":{"id":session.model,"providerID":"openai"},"tokens":{"input":100,"output":10,"reasoning":0,"cache":{"read":0,"write":0}},"cost":0.0,"content":[]})));
        for (index, (kind, data)) in messages.into_iter().enumerate() {
            connection
                .execute(
                    "INSERT INTO session_message VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                    rusqlite::params![
                        format!("{}-{index}", session.id),
                        session.id,
                        kind,
                        index as i64,
                        data.to_string(),
                        session.observed_at_ms
                    ],
                )
                .unwrap();
        }
        let key = SessionKey::new("native", "opencode", session.id);
        let fingerprint = format!("synthetic-{}", session.id);
        self.store
            .upsert_sessions(
                &[SessionRecord {
                    key: key.clone(),
                    source_kind: "providerDb".into(),
                    source_label: format!("opencode:{}:open_code_v2_sqlite", session.id),
                    wsl_distro: None,
                    title: None,
                    title_source: None,
                    cwd: Some(session.cwd.to_string_lossy().into_owned()),
                    surface: "cli".into(),
                    updated_at_epoch: Some(session.observed_at_ms / 1_000),
                    activity_cursor: fingerprint.clone(),
                    activity_source: "event".into(),
                    subagent_count: 0,
                    fork_parent_session_id: None,
                    source_fingerprint: Some(fingerprint.clone()),
                }],
                &["opencode"],
            )
            .unwrap();
        let claim = self
            .store
            .claim_next_evidence(&["opencode"], now_epoch(), 60)
            .unwrap()
            .unwrap();
        assert_eq!(claim.key, key);
        let input = SessionInput {
            agent: "opencode".into(),
            session_id: session.id.into(),
            source: RawSource::Sqlite(self.database.clone()),
            source_format: SourceFormat::OpenCodeSqliteV2,
            fork_parent_session_id: None,
        };
        let reader = reader_for_input(&input);
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "opencode".into(),
            session_id: session.id.into(),
            kind: SourceKind::from(&input.source),
            capabilities: reader.capabilities(&input),
        });
        let writer = Arc::new(FencedTurnRowStore::new(
            self.store.clone(),
            key.clone(),
            claim.claim_fence,
        )) as Arc<dyn TurnRowStore>;
        let rows = TurnRowSink::new(writer, session.id, None);
        let mut sink = CompositeSink::with_turn_rows(
            SessionMetricsAccumulator::new("opencode", session.id),
            evidence,
            rows,
        );
        let outcome = reader.visit(&input, &mut sink).unwrap();
        sink.observe_source_outcome(outcome);
        assert!(!sink.turn_row_write_failed());
        let mut evidence = sink.evidence().unwrap();
        evidence.capabilities.source_format = session.source;
        let evidence_json = serde_json::to_string(&evidence).unwrap();
        assert!(
            self.store
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
                        pricing_generation: antiburn_local::analysis::pricing_generation() as i64,
                        analyzed_generation: claim.source_generation,
                        parser_revision: PARSER_REVISION,
                        analyzer_revision: ANALYZER_REVISION,
                        metrics_schema_revision: METRICS_SCHEMA_REVISION,
                    },
                    Some(session.started_at_epoch),
                    &EvidenceCompletion {
                        claim_fence: claim.claim_fence,
                        status: PublishedEvidence::Ready,
                        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                        evidence_json: evidence_json.clone()
                    },
                    &[],
                    &[]
                )
                .unwrap()
        );
        if session.attributed {
            let authority = self.authority(session.cwd);
            let attribution = config::publication_config_attribution_with_authority(
                &self.store,
                &key,
                PublishedEvidence::Ready,
                &evidence_json,
                &self.home,
                Some(&authority),
            )
            .unwrap();
            let (physical, scope, value) = attribution
                .model
                .expect("the fixture must prove current publication attribution");
            self.store.lock().execute("UPDATE session_evidence SET effective_model_target_hash=?1,effective_model_scope=?2,effective_model=?3 WHERE environment_key=?4 AND agent=?5 AND session_id=?6", rusqlite::params![physical,scope,value,key.environment_key,key.agent,key.session_id]).unwrap();
        }
        key
    }

    fn watching(&self) -> RemediationRecord {
        self.publish(self.session("old", now_epoch() - 10, "gpt-5.5"));
        let listed = self
            .controller
            .list_burn_check_targets_with_home(
                &self.store,
                DetectorId::OldModelUsage,
                BurnCheckTargetContext {
                    environment_key: "native".into(),
                    window: ReportWindow {
                        start_epoch: 0,
                        end_epoch: i64::MAX,
                    },
                },
                &self.home,
            )
            .unwrap();
        let target = listed.targets.first().unwrap();
        assert_eq!(target.prompt_fix, PromptFixAvailability::Available);
        let result = self
            .controller
            .copy_prompt_fix_burn_check_target(&self.store, &target.action_id)
            .unwrap();
        let waiting = result.watch.unwrap();
        assert_eq!(waiting.lifecycle, RemediationState::WaitingForPromptUse);
        assert!(
            matches!(
                waiting.verification,
                VerificationStatus::VerificationUnavailable
            ),
            "unexpected verification status: {:?}; definition: {}",
            waiting.verification,
            self.store
                .remediation(&waiting.watch_id)
                .unwrap()
                .unwrap()
                .definition_json
        );
        let mut marker = self.session("prompt-use", now_epoch(), "gpt-5.5");
        marker.prompt = Some(&result.prompt);
        marker.attributed = false;
        self.publish(marker);
        let watch = self.store.remediation(&waiting.watch_id).unwrap().unwrap();
        assert_eq!(watch.state, RemediationState::Watching);
        assert!(watch.effective_boundary_ms.is_some());
        assert!(
            parse_watch_definition(&watch.definition_json)
                .unwrap()
                .prompt_action
        );
        let copied = self
            .controller
            .copy_prompt_fix_burn_check_target_current(
                &self.store,
                DetectorId::OldModelUsage,
                &target.finding_id,
                BurnCheckTargetContext {
                    environment_key: "native".into(),
                    window: ReportWindow {
                        start_epoch: 0,
                        end_epoch: i64::MAX,
                    },
                },
            )
            .unwrap();
        assert!(copied.prompt.contains("gpt-5.5"));
        assert!(!copied.prompt.contains(PROMPT_REFERENCE_PREFIX));
        assert_eq!(copied.watch.unwrap().lifecycle, RemediationState::Watching);
        assert_eq!(
            self.store
                .remediation(&watch.remediation_id)
                .unwrap()
                .unwrap(),
            watch
        );
        watch
    }

    fn evaluate(&self, watch: &RemediationRecord) -> RemediationRecord {
        let current = self
            .store
            .remediation(&watch.remediation_id)
            .unwrap()
            .unwrap();
        assert!(
            evaluate_dirty_remediation(
                self.temporary.path(),
                &self.store,
                &current,
                now_epoch() + 100
            )
            .unwrap()
        );
        self.store
            .remediation(&watch.remediation_id)
            .unwrap()
            .unwrap()
    }

    fn replacement_config(&self) {
        std::fs::write(&self.config, r#"{"model":"openai/gpt-5.6-sol"}"#).unwrap();
    }
}

fn install_synthetic_pricing() {
    antiburn_local::analysis::install_runtime_pricing(std::collections::HashMap::from([
        (
            "gpt-5.5".into(),
            ModelPricing {
                input_cost_per_token: 10e-6,
                output_cost_per_token: 60e-6,
                cache_read_cost_per_token: 1e-6,
                cache_write_cost_per_token: 12.5e-6,
            },
        ),
        (
            "gpt-5.6-sol".into(),
            ModelPricing {
                input_cost_per_token: 5e-6,
                output_cost_per_token: 30e-6,
                cache_read_cost_per_token: 0.5e-6,
                cache_write_cost_per_token: 6.25e-6,
            },
        ),
    ]));
}

#[test]
fn native_o_prompt_enrollment_does_not_claim_model_change_verification_without_api_route() {
    let fixture = Fixture::new();
    let watch = fixture.watching();
    fixture.replacement_config();
    let boundary = watch.effective_boundary_ms.unwrap();
    assert_eq!(fixture.evaluate(&watch).state, RemediationState::Watching);
    let mut active = fixture.session("active-before-change", boundary / 1_000 - 1, "gpt-5.6-sol");
    active.observed_at_ms = boundary + 10_000;
    fixture.publish(active);
    assert_eq!(fixture.evaluate(&watch).state, RemediationState::Watching);
    let mut unbound = fixture.session(
        "without-current-proof",
        boundary / 1_000 + 20,
        "gpt-5.6-sol",
    );
    unbound.attributed = false;
    fixture.publish(unbound);
    assert_eq!(fixture.evaluate(&watch).state, RemediationState::Watching);
    fixture.publish(fixture.session("replacement", boundary / 1_000 + 30, "gpt-5.6-sol"));
    let unresolved = fixture.evaluate(&watch);
    assert_eq!(unresolved.state, RemediationState::Watching);
    let result = stored_result(&unresolved.result_json).unwrap();
    assert!(matches!(
        result.verification,
        VerificationStatus::VerificationUnavailable
    ));
    assert!(matches!(result.savings, SavingsStatus::Unavailable));
    let progress = fixture
        .controller
        .burn_check_remediation_progress(&fixture.store)
        .unwrap();
    assert!(
        !progress
            .attempts
            .iter()
            .any(|attempt| attempt.watch_id == watch.remediation_id
                && attempt.outcome == BurnCheckRemediationOutcome::Passed)
    );
    std::fs::write(&fixture.config, r#"{"model":"openai/gpt-5.5"}"#).unwrap();
    fixture.publish(fixture.session("recurrence", boundary / 1_000 + 40, "gpt-5.5"));
    let recurred = fixture.evaluate(&watch);
    assert_eq!(recurred.state, RemediationState::Watching);
    assert!(matches!(
        stored_result(&recurred.result_json).unwrap().verification,
        VerificationStatus::VerificationUnavailable
    ));
}

#[test]
fn native_o_watch_rejects_other_physical_targets_routes_and_source_formats() {
    let fixture = Fixture::new();
    let watch = fixture.watching();
    fixture.replacement_config();
    let after = watch.effective_boundary_ms.unwrap() / 1_000 + 10;
    let nested = fixture.cwd.join("nested");
    std::fs::create_dir(&nested).unwrap();
    std::fs::write(
        nested.join("opencode.json"),
        r#"{"model":"openai/gpt-5.6-sol"}"#,
    )
    .unwrap();
    let mut other = fixture.session("other-target", after, "gpt-5.6-sol");
    other.cwd = &nested;
    fixture.publish(other);
    assert_eq!(fixture.evaluate(&watch).state, RemediationState::Watching);
    let wrong_route = fixture.publish(fixture.session("other-route", after + 1, "gpt-5.6-sol"));
    fixture
        .store
        .lock()
        .execute(
            "UPDATE turn SET api='custom-api' WHERE session_id=?1",
            [wrong_route.session_id],
        )
        .unwrap();
    assert_eq!(fixture.evaluate(&watch).state, RemediationState::Watching);
    let mut legacy = fixture.session("legacy-source", after + 2, "gpt-5.6-sol");
    legacy.source = SourceFormat::OpenCodeSqliteV1;
    fixture.publish(legacy);
    assert_eq!(fixture.evaluate(&watch).state, RemediationState::Watching);
}

#[test]
fn native_o_future_default_watch_does_not_verify_continued_children_or_forks() {
    for kind in ["subagent", "forkParent"] {
        let fixture = Fixture::new();
        let watch = fixture.watching();
        fixture.replacement_config();
        let after = watch.effective_boundary_ms.unwrap() / 1_000 + 10;
        fixture.publish(fixture.session("continued", after, "gpt-5.6-sol"));
        let (session_id, related_id) = if kind == "subagent" {
            ("old", "continued")
        } else {
            ("continued", "old")
        };
        fixture.store.lock().execute(
            "INSERT INTO session_relation (environment_key,agent,session_id,kind,related_id) VALUES ('native','opencode',?1,?2,?3)",
            rusqlite::params![session_id,kind,related_id],
        ).unwrap();
        assert_eq!(
            fixture.evaluate(&watch).state,
            RemediationState::Watching,
            "{kind}"
        );
    }
}

#[test]
fn native_o_watch_does_not_verify_without_reviewed_pricing() {
    for field in [
        "pricingRevision",
        "oldPricing",
        "replacementPricing",
        "provider",
        "api",
    ] {
        let fixture = Fixture::new();
        let watch = fixture.watching();
        fixture.replacement_config();
        fixture.publish(fixture.session(
            "replacement",
            watch.effective_boundary_ms.unwrap() / 1_000 + 10,
            "gpt-5.6-sol",
        ));
        let mut definition: serde_json::Value =
            serde_json::from_str(&watch.definition_json).unwrap();
        definition[field] = serde_json::Value::Null;
        fixture
            .store
            .lock()
            .execute(
                "UPDATE remediation SET definition_json=?1 WHERE remediation_id=?2",
                rusqlite::params![definition.to_string(), watch.remediation_id],
            )
            .unwrap();
        let unresolved = fixture.evaluate(&watch);
        assert_eq!(unresolved.state, RemediationState::Watching, "{field}");
        assert!(matches!(
            stored_result(&unresolved.result_json).unwrap().verification,
            VerificationStatus::VerificationUnavailable
        ));
    }
}

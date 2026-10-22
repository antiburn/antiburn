use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use antiburn_checks::actions::ChoiceResult;
use antiburn_checks::check_store::{AttemptFailure, EvaluatedCandidate};
use antiburn_checks::compiler::StatusResult;
use antiburn_checks::definition_lifecycle::EffectiveActivation;
use antiburn_checks::domain::{
    ActivationId, ActivationScope, FrozenCandidate, Generation, ObservationKind,
    ObservationPosition, RunScope,
};
use antiburn_checks::engine::{DecisionEvaluator, DecisionFuture, EvaluationFailure};
use antiburn_checks::replay::{
    ReplayEnd, ReplayEventSource, ReplayLimits, ReplayReport, ReplayRunId, replay,
};
use antiburn_checks::source::{FileEventSource, PublicationCursor, SourceEvent};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::cases::{Case, Expected, Fixture};
use crate::check_store::StoreCheckStore;
use crate::jev::decision_evaluator::{AttemptOutcome, ProviderAttemptDiagnostic};
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use crate::store::{
    AnalysisRecord, EvidenceCompletion, PublishedEvidence, SessionKey, SessionRecord, Store,
    ToolResultBatch, ToolResultPublishOptions,
};

pub(super) const MAX_EVENTS: usize = 16;
pub(super) const CASE_TIMEOUT: Duration = Duration::from_secs(180);

pub(super) struct SessionReplay {
    pub store: Arc<Store>,
    adapter: Arc<StoreCheckStore>,
    pub activation: EffectiveActivation,
    source: Events,
    _directory: tempfile::TempDir,
}

struct Events {
    remaining: std::vec::IntoIter<SourceEvent>,
    store: Arc<Store>,
    previous: Option<u64>,
    statuses: Vec<Value>,
}

impl ReplayEventSource for Events {
    fn next_event(&mut self) -> Result<Option<SourceEvent>, String> {
        if let Some(cursor) = self.previous.take() {
            let rows = current_status(&self.store).map_err(|error| error.to_string())?;
            self.statuses.push(json!({"cursor": cursor, "rows": rows}));
        }
        let event = self.remaining.next();
        self.previous = event.as_ref().map(|event| match event {
            SourceEvent::Ignored { cursor }
            | SourceEvent::Observation { cursor, .. }
            | SourceEvent::Removal { cursor, .. }
            | SourceEvent::Gap { cursor, .. }
            | SourceEvent::UnsafeUpdate { cursor, .. }
            | SourceEvent::SegmentReset { cursor, .. }
            | SourceEvent::CoverageRestored { cursor, .. } => cursor.0,
        });
        Ok(event)
    }
}

fn publish_fixture(store: &Store, project: &str, fixture: Fixture) -> Result<ToolResultBatch> {
    let key = SessionKey::new(
        "replay-fixture",
        antiburn_local::model::AgentKind::Claude.slug(),
        "synthetic",
    );
    let batch = fixture.batch();
    ensure!(
        batch.results.len() <= MAX_EVENTS,
        "fixture exceeds event limit"
    );
    ensure!(
        batch.results.iter().all(
            |result| result.context.as_ref().is_none_or(|context| context
                .user_messages
                .iter()
                .all(|message| message.is_bounded()))
        ),
        "fixture has invalid request evidence"
    );
    store.upsert_sessions(
        &[SessionRecord {
            key: key.clone(),
            source_kind: "file".into(),
            source_label: "synthetic-session-evidence".into(),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: Some(project.into()),
            surface: "cli".into(),
            updated_at_epoch: Some(100),
            activity_cursor: String::new(),
            activity_source: "event".into(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: Some("synthetic-evidence".into()),
        }],
        &crate::agents::evidence_cohort(),
    )?;
    let claim = store
        .claim_next_evidence(&[key.agent.as_str()], 100, 60)?
        .context("claim synthetic evidence")?;
    let revisions = crate::analysis::projection_revisions();
    let record = AnalysisRecord {
        key: key.clone(),
        model_breakdown_json: "{}".into(),
        pricing_breakdown_json: "{}".into(),
        inclusive_models_json: "[]".into(),
        initial_context_json: None,
        source_summaries_json: None,
        provider_hints_json: None,
        source_fingerprint: "synthetic-evidence".into(),
        pricing_generation: 1,
        analyzed_generation: claim.source_generation,
        parser_revision: revisions.parser_revision,
        analyzer_revision: revisions.analyzer_revision,
        metrics_schema_revision: revisions.metrics_schema_revision,
    };
    ensure!(
        store.publish_projections_with_native_tool_results(
            &record,
            None,
            &EvidenceCompletion {
                claim_fence: claim.claim_fence,
                status: PublishedEvidence::Ready,
                evidence_schema_revision: antiburn_local::analysis::EVIDENCE_SCHEMA_REVISION,
                evidence_json: crate::store::test_support::evidence_json(&key),
            },
            &[],
            &[],
            ToolResultPublishOptions {
                source_fingerprint: Some("synthetic-evidence"),
                batch: Some(&batch)
            },
        )?,
        "synthetic publication lost its claim"
    );
    Ok(batch)
}

fn read_published_events(
    store: &Store,
    project: &str,
    batch: &ToolResultBatch,
) -> Result<Vec<SourceEvent>> {
    let source = FileEventSource::new(
        StorePublishedEvidenceReader::for_project(store, "replay-fixture", project),
        1,
    )
    .map_err(|error| anyhow::anyhow!("source setup: {error:?}"))?;
    let mut events = Vec::new();
    let mut after = PublicationCursor(0);
    loop {
        let page = source
            .read_batch(after)
            .map_err(|error| anyhow::anyhow!("source read: {error:?}"))?;
        if page.events.is_empty() {
            break;
        }
        ensure!(
            events.len() + page.events.len() <= MAX_EVENTS,
            "publication exceeds event limit"
        );
        after = page.next;
        events.extend(page.events);
    }
    ensure!(
        events.len() == batch.results.len(),
        "publication lost synthetic results"
    );
    for (event, result) in events.iter().zip(&batch.results) {
        if result.context.is_none() {
            ensure!(
                matches!(event, SourceEvent::UnsafeUpdate { .. }),
                "missing result context must remain unavailable"
            );
            continue;
        }
        let SourceEvent::Observation {
            revision,
            observation,
            ..
        } = event
        else {
            anyhow::bail!("synthetic result did not convert to an observation")
        };
        ensure!(
            *revision == result.revision
                && observation.complete
                    == (result.finality
                        && result.completeness
                            == antiburn_local::analysis::ToolResultCompleteness::Complete),
            "production conversion changed revision or result completeness"
        );
        let ObservationKind::ToolResult {
            message,
            category,
            status,
        } = &observation.kind
        else {
            anyhow::bail!("synthetic result did not convert to a tool result")
        };
        ensure!(
            category == "shell"
                && message.value.pointer("/tool/arguments") == result.arguments.as_ref()
                && message.value["tool"]["result"]["content"].as_str() == result.content.as_deref()
                && status
                    == if result.status == antiburn_local::analysis::ToolResultStatus::Error {
                        "error"
                    } else {
                        "completed"
                    },
            "production conversion changed tool evidence"
        );
        let requests = result.context.as_ref()
                .map(|context| context.user_messages.iter().map(|message| json!({
                    "id":message.message_id,
                    "source":"claude-code",
                    "kind":"user_message",
                    "text":message.parts.iter().map(|part| part.text.as_str()).collect::<Vec<_>>().join("\n")
                })).collect::<Vec<_>>()).unwrap_or_default();
        ensure!(
            observation.context.value["session"]["requests"] == json!(requests),
            "production conversion changed the frozen request prefix"
        );
        ensure!(
            observation.context.value["session"]["requests_complete"]
                == result
                    .context
                    .as_ref()
                    .is_some_and(|context| context.complete_through_event),
            "production conversion changed request-history quality"
        );
    }
    Ok(events)
}

impl SessionReplay {
    pub fn for_case(case: &Case) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let store = Arc::new(Store::open(directory.path())?);
        let project = directory.path().to_string_lossy().into_owned();
        let batch = publish_fixture(&store, &project, case.fixture)?;
        let events = read_published_events(&store, &project, &batch)?;
        let run_id = ReplayRunId(
            directory
                .path()
                .file_name()
                .context("replay identity")?
                .to_string_lossy()
                .into_owned(),
        );
        let activation = EffectiveActivation {
            id: ActivationId {
                scope: ActivationScope {
                    environment: "replay-fixture".into(),
                    session_cwd: project,
                    run: RunScope::Replay(run_id.0.clone()),
                },
                definition_id: case.definition.id.into(),
                generation: Generation(1),
            },
            revision: Sha256::digest(case.definition.yaml.as_bytes()).into(),
            definition_scope: antiburn_checks::discovery::DefinitionScope::Global,
            source_paths: vec![case.definition.path.into()],
            project_root: None,
            compiled: Arc::new(
                antiburn_checks::compiler::compile(case.definition.yaml)
                    .map_err(|error| anyhow::anyhow!("definition compilation failed: {error:?}"))?,
            ),
            bootstrap_observation_position: ObservationPosition(0),
            bootstrap_publication: PublicationCursor(0),
        };
        let adapter = Arc::new(StoreCheckStore::new(
            store.clone(),
            run_id.0,
            activation.id.scope.clone(),
        )?);
        Ok(Self {
            source: Events {
                remaining: events.into_iter(),
                store: store.clone(),
                previous: None,
                statuses: Vec::new(),
            },
            store,
            adapter,
            activation,
            _directory: directory,
        })
    }

    pub async fn run<E: DecisionEvaluator>(
        &mut self,
        evaluator: Arc<E>,
        limits: ReplayLimits,
        cancelled: impl Fn() -> bool,
    ) -> Result<ReplayReport> {
        let RunScope::Replay(id) = &self.activation.id.scope.run else {
            anyhow::bail!("replay scope required")
        };
        Ok(replay(
            ReplayRunId(id.clone()),
            self.activation.clone(),
            self.adapter.clone(),
            evaluator,
            &mut self.source,
            limits,
            cancelled,
        )
        .await?)
    }

    pub fn snapshot(&self) -> Result<Value> {
        let usage = self.store.burn_check_usage_summary()?;
        let status = current_status(&self.store)?;
        let connection = self.store.lock();
        let assessments = connection.prepare("SELECT assessment_id, trigger_position, answer_json, actions_json FROM burn_check_assessment_history WHERE outcome = 'assessed' ORDER BY trigger_position")?
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))?
            .map(|row| {
                let (id, trigger, answer, actions) = row?;
                Ok(json!({"id":id, "trigger":trigger, "answer":serde_json::from_str::<EvaluatedCandidate>(&answer)?, "actions":serde_json::from_str::<Value>(&actions)?}))
            }).collect::<Result<Vec<_>>>()?;
        let attempts = connection.prepare("SELECT attempt_number, state, outcome FROM burn_check_evaluation_attempt ORDER BY candidate_id, attempt_number")?
            .query_map([], |row| Ok(json!({"attempt":row.get::<_, u8>(0)?, "state":row.get::<_, String>(1)?, "outcome":row.get::<_, Option<String>>(2)?})))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let other_activations: usize = connection.query_row(
            "SELECT COUNT(*) FROM burn_check_activation WHERE activation_scope_key != ?1",
            [StoreCheckStore::activation_scope_key(
                &self.activation.id.scope,
            )],
            |row| row.get(0),
        )?;
        Ok(
            json!({"activation_scope_key":StoreCheckStore::activation_scope_key(&self.activation.id.scope),
            "assessments":assessments, "attempts":attempts, "current_status":status, "status_trace":self.source.statuses,
            "other_activation_rows":other_activations, "usage":usage}),
        )
    }
}

fn current_status(store: &Store) -> Result<Vec<Value>> {
    Ok(store.lock().prepare("SELECT check_id, activation_scope_key, branch_key, generation, status, assessment_id, position, evidence_valid, updated_at_epoch FROM burn_check_current_status ORDER BY check_id, activation_scope_key, branch_key")?
        .query_map([], |row| Ok(json!({"check_id":row.get::<_, String>(0)?, "activation_scope_key":row.get::<_, String>(1)?, "branch_key":row.get::<_, String>(2)?,
            "generation":row.get::<_, u64>(3)?, "status":row.get::<_, String>(4)?, "assessment_id":row.get::<_, String>(5)?,
            "position":row.get::<_, u64>(6)?, "evidence_valid":row.get::<_, bool>(7)?, "updated_at_epoch":row.get::<_, i64>(8)?})))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

pub(super) fn limits(case: &Case) -> ReplayLimits {
    ReplayLimits {
        events: MAX_EVENTS,
        evaluations: case.expected.len(),
        attempts: case.expected.len() * 2,
    }
}

pub(super) struct CheckedEvaluator<E> {
    pub inner: E,
    fixture: Fixture,
    expected: &'static [Expected],
    pub inputs: Mutex<Vec<Value>>,
}

impl<E> CheckedEvaluator<E> {
    pub fn new(case: &Case, inner: E) -> Self {
        Self {
            inner,
            fixture: case.fixture,
            expected: case.expected,
            inputs: Mutex::new(Vec::new()),
        }
    }
}

impl<E: DecisionEvaluator> DecisionEvaluator for CheckedEvaluator<E> {
    fn prepare(
        &self,
        activation: &EffectiveActivation,
        input: Value,
    ) -> Result<antiburn_checks::fit::PreparedDecision, antiburn_checks::fit::PreparationFailure>
    {
        self.inner.prepare(activation, input)
    }

    fn validate_prepared(
        &self,
        activation: &EffectiveActivation,
        prepared: &antiburn_checks::fit::PreparedDecision,
    ) -> Result<(), antiburn_checks::fit::PreparationFailure> {
        self.inner.validate_prepared(activation, prepared)
    }

    fn evaluate_prepared<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        prepared: &'a antiburn_checks::fit::PreparedDecision,
        attempt: u8,
    ) -> DecisionFuture<'a> {
        Box::pin(async move {
            self.check_input(candidate, &prepared.input, attempt)?;
            self.inner
                .evaluate_prepared(activation, candidate, prepared, attempt)
                .await
        })
    }

    fn evaluate<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        candidate: &'a FrozenCandidate,
        input: &'a Value,
        attempt: u8,
    ) -> DecisionFuture<'a> {
        Box::pin(async move {
            self.check_input(candidate, input, attempt)?;
            self.inner
                .evaluate(activation, candidate, input, attempt)
                .await
        })
    }
}

impl<E> CheckedEvaluator<E> {
    fn check_input(
        &self,
        candidate: &FrozenCandidate,
        input: &Value,
        attempt: u8,
    ) -> Result<(), EvaluationFailure> {
        self.expected
            .iter()
            .find(|expected| expected.trigger == candidate.trigger_position.0)
            .context("unexpected evaluation trigger")
            .and_then(|expected| self.fixture.check_input(expected, input))
            .map_err(|error| EvaluationFailure {
                code: antiburn_checks::engine::EvaluationFailureCode::InvalidRequest,
                kind: AttemptFailure::Permanent,
                detail: format!("replay input: {error:#}"),
                attempts: 0,
            })?;
        self.inputs
            .lock()
            .expect("input mutex is not poisoned")
            .push(
                json!({"trigger": candidate.trigger_position.0, "attempt":attempt, "input":input}),
            );
        Ok(())
    }
}

pub(super) struct ScriptedEvaluator(Mutex<VecDeque<Expected>>);

impl ScriptedEvaluator {
    pub fn new(expected: &[Expected]) -> Self {
        Self(Mutex::new(expected.iter().copied().collect()))
    }
    pub fn exhausted(&self) -> bool {
        self.0
            .lock()
            .expect("answer mutex is not poisoned")
            .is_empty()
    }
}

impl DecisionEvaluator for ScriptedEvaluator {
    fn evaluate<'a>(
        &'a self,
        activation: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a Value,
        _: u8,
    ) -> DecisionFuture<'a> {
        let next = self
            .0
            .lock()
            .expect("answer mutex is not poisoned")
            .pop_front();
        Box::pin(async move {
            let failure = |detail| EvaluationFailure {
                code: antiburn_checks::engine::EvaluationFailureCode::InvalidResponse,
                kind: AttemptFailure::Permanent,
                detail,
                attempts: 0,
            };
            let next = next.ok_or_else(|| failure("script exhausted".into()))?;
            let decision = &activation.compiled.decision;
            let labels = decision
                .criteria
                .iter()
                .map(|(label, _)| label.clone())
                .collect::<Vec<_>>();
            let probabilities = labels
                .iter()
                .map(|label| (label.clone(), if label == next.label { 1.0 } else { 0.0 }))
                .collect();
            let choice = ChoiceResult::validate(
                &decision.name,
                &decision.question_id,
                &labels,
                next.label,
                probabilities,
            )
            .map_err(|error| failure(error.message))?;
            Ok(EvaluatedCandidate {
                choices: vec![choice],
                attempts: 1,
            })
        })
    }
}

pub(super) fn status_write(assessment: &Value) -> Result<Option<StatusResult>> {
    assessment["actions"]["outcomes"]
        .as_array()
        .context("action outcomes")?
        .iter()
        .filter_map(|action| {
            action["outcome"]
                .get("Applied")
                .map(|applied| &applied["result"])
        })
        .next_back()
        .map(|result| serde_json::from_value(result.clone()))
        .transpose()
        .map_err(Into::into)
}

pub(super) fn check_integrity(case: &Case, report: &ReplayReport, stored: &Value) -> Result<()> {
    ensure!(
        report.end == ReplayEnd::Complete,
        "replay stopped: {:?}",
        report.end
    );
    ensure!(
        report.evaluations == case.expected.len()
            && report.events == case.fixture.results.len()
            && report.events == report.steps.len(),
        "replay counts differ from the declared trace"
    );
    ensure!(
        report.attempts >= report.evaluations && report.attempts <= limits(case).attempts,
        "attempt count is outside bounds"
    );
    let accepted = stored["assessments"]
        .as_array()
        .context("assessment array")?;
    ensure!(
        accepted.len() == case.expected.len(),
        "accepted assessment count differs from the declared trace"
    );
    for (assessment, expected) in accepted.iter().zip(case.expected) {
        ensure!(
            assessment["trigger"] == expected.trigger,
            "saved trigger differs from the declared trace"
        );
        ensure!(
            assessment["answer"]["choices"]
                .as_array()
                .context("saved choices")?
                .len()
                == 1,
            "each assessment requires one complete Choice"
        );
    }
    let attempts = stored["attempts"].as_array().context("stored attempts")?;
    ensure!(
        attempts.len() == report.attempts
            && attempts.iter().all(|attempt| attempt["state"] == "settled"),
        "durable attempts do not match replay settlement"
    );
    let triggers = report
        .steps
        .iter()
        .filter(|step| step.candidates > 0)
        .map(|step| step.cursor.0)
        .collect::<Vec<_>>();
    ensure!(
        triggers
            == case
                .expected
                .iter()
                .map(|expected| expected.trigger)
                .collect::<Vec<_>>(),
        "trigger order differs from the declared trace"
    );
    let trace = stored["status_trace"].as_array().context("status trace")?;
    ensure!(
        trace.len() == report.events,
        "missing persisted status snapshots"
    );
    let mut writer: Option<&Value> = None;
    let mut previous_rows = &Value::Null;
    for (index, step) in trace.iter().enumerate() {
        ensure!(
            step["cursor"] == u64::try_from(index + 1)?,
            "status snapshot is out of order"
        );
        let assessment = accepted
            .iter()
            .find(|assessment| assessment["trigger"] == step["cursor"]);
        let write = assessment.map(status_write).transpose()?.flatten();
        if write.is_some() {
            writer = assessment;
        }
        let rows = step["rows"].as_array().context("persisted status rows")?;
        if let Some(writer) = writer {
            ensure!(rows.len() == 1, "missing or extra persisted current status");
            let row = &rows[0];
            ensure!(
                row["check_id"] == case.definition.id
                    && row["activation_scope_key"] == stored["activation_scope_key"]
                    && row["generation"] == 1
                    && row["branch_key"] == r#"["replay-fixture","claude-code","synthetic"]"#
                    && row["assessment_id"] == writer["id"]
                    && row["position"] == writer["trigger"]
                    && row["status"]
                        == match status_write(writer)?.context("writing status")? {
                            StatusResult::Failure => "failure",
                            StatusResult::Success => "success",
                        }
                    && row["evidence_valid"] == true,
                "persisted current status differs from its last writing assessment: {row}"
            );
            if write.is_none() {
                ensure!(
                    step["rows"] == *previous_rows,
                    "no-write changed persisted status or provenance"
                );
            }
        } else {
            ensure!(rows.is_empty(), "status exists before an authored write");
        }
        previous_rows = &step["rows"];
    }
    ensure!(
        stored["current_status"] == *previous_rows,
        "final status differs from the settled prefix"
    );
    ensure!(
        stored["other_activation_rows"] == 0,
        "replay escaped its isolated scope"
    );
    Ok(())
}

pub(super) fn report_value(report: &ReplayReport) -> Value {
    json!({"run_id":report.run_id.0, "definition_id":report.definition_id, "definition_revision":report.definition_revision, "model":report.model,
        "terminal":format!("{:?}",report.end), "events":report.events, "evaluations":report.evaluations, "attempts":report.attempts,
        "steps":report.steps.iter().map(|step|json!({"cursor":step.cursor.0, "candidates":step.candidates, "outcome":format!("{:?}",step.outcome)})).collect::<Vec<_>>()})
}

pub(super) fn attempt_value(record: ProviderAttemptDiagnostic) -> Value {
    let outcome = match record.outcome {
        AttemptOutcome::Answer(answer) => {
            json!({"state":"answer", "choice":answer.choice, "usage":answer.usage})
        }
        AttemptOutcome::Unknown => json!({"state":"unknown"}),
        AttemptOutcome::KnownTransientFailure(error) => {
            json!({"state":"safe_transient", "error":format!("{error:?}")})
        }
        AttemptOutcome::Failed {
            error,
            usage,
            rejected_choice,
        } => {
            json!({"state":"failed", "error":format!("{error:?}"), "usage":usage, "rejected_choice":rejected_choice})
        }
    };
    json!({"attempt":record.attempt_number, "latency_ms":record.latency.as_secs_f64()*1000.0, "outcome":outcome})
}

pub(super) fn expected_value(case: &Case) -> Value {
    json!({"case":case.id, "definition":case.definition.path, "input_representation":"synthetic session evidence",
        "expected":case.expected.iter().map(|expected|json!({"trigger":expected.trigger, "label":expected.label, "scripted_status_write":expected.status_write})).collect::<Vec<_>>(),
        "limits":{"events":limits(case).events, "evaluations":limits(case).evaluations, "attempts":limits(case).attempts, "execution_seconds":CASE_TIMEOUT.as_secs()}})
}

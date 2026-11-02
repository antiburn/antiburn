use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use antiburn_checks::check_store::{CandidateClaim, CheckStore, ClaimOutcome};
use antiburn_checks::compiler::compile;
use antiburn_checks::decision::{DecisionAttempt, DecisionInput};
use antiburn_checks::definition_lifecycle::{EffectiveActivation, LifecycleTransition};
use antiburn_checks::discovery::DefinitionScope;
use antiburn_checks::domain::{
    ActivationId, ActivationScope, FrozenCandidate, Generation, ObservationPosition, RunScope,
};
use antiburn_checks::engine::{
    CheckEngine, DecisionEvaluator, DecisionFuture, PreparationValidationPurpose, RunOutcome,
    StartOutcome,
};
use antiburn_checks::expression_eval::EvaluationBudget;
use antiburn_checks::fit::PreparationFailure;
use antiburn_checks::prepared_assessment::PreparedAssessment;
use antiburn_checks::source::{PublicationCursor, PublishedEvidenceSource, SourceEvent};
use antiburn_local::analysis::{
    NativeEvidenceFact, ToolResultAdmission, ToolResultCompleteness,
    read_opencode_live_tool_result_page,
};
use serde_json::json;

use super::*;
use crate::check_store::StoreCheckStore;
use crate::insights_worker::journal::tests::SyntheticJournal;
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use crate::store::native_tool_results::{ToolResultPublicationOutcome, admission};

struct NoProvider;

impl DecisionEvaluator for NoProvider {
    fn prepare(
        &self,
        _: &EffectiveActivation,
        _: Vec<DecisionInput>,
        _: EvaluationBudget,
    ) -> Result<PreparedAssessment, PreparationFailure> {
        panic!("journal regressions must not prepare provider requests")
    }

    fn validate_prepared(
        &self,
        _: &EffectiveActivation,
        _: &PreparedAssessment,
        _: PreparationValidationPurpose,
    ) -> Result<(), PreparationFailure> {
        panic!("journal regressions must not validate provider requests")
    }

    fn evaluate_prepared<'a>(
        &'a self,
        _: &'a EffectiveActivation,
        _: &'a FrozenCandidate,
        _: &'a PreparedAssessment,
        _: &'a DecisionAttempt,
    ) -> DecisionFuture<'a> {
        panic!("journal regressions must not send provider requests")
    }
}

struct Runtime {
    adapter: Arc<StoreCheckStore>,
    active: EffectiveActivation,
    engine: CheckEngine<StoreCheckStore, NoProvider>,
    cursor: PublicationCursor,
}

impl Runtime {
    fn new(store: Arc<Store>, model: bool) -> Self {
        store
            .set_internal_value_checked("internal:yamlBurnChecksDiscoveryEnabledV1", "true")
            .unwrap();
        let scope = ActivationScope {
            environment: "native".into(),
            session_cwd: "/synthetic".into(),
            run: RunScope::Live,
        };
        let adapter = Arc::new(
            StoreCheckStore::new(store, "journal-regression".into(), scope.clone()).unwrap(),
        );
        let decisions = if model {
            "decisions:\n  review:\n    input: {session: '${{ session.id }}'}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {yes: Yes., no: No.}\n"
        } else {
            ""
        };
        let active = EffectiveActivation {
            id: ActivationId {
                scope,
                definition_id: "journal-regression".into(),
                generation: Generation(1),
            },
            revision: [9; 32],
            definition_scope: DefinitionScope::Global,
            source_paths: Vec::new(),
            project_root: None,
            compiled: Arc::new(compile(&format!(
                "name: Journal regression\non:\n  tool_result:\n  cooldown: 0s\n{decisions}actions:\n  - type: status\n    input: {{result: failure}}\n"
            )).unwrap()),
            bootstrap_observation_position: ObservationPosition(0),
            bootstrap_publication: PublicationCursor(0),
        };
        adapter
            .activate(
                &active.id.definition_id,
                &LifecycleTransition::Activated(active.clone()),
                Some(&active),
            )
            .unwrap();
        Self {
            engine: CheckEngine::new(adapter.clone(), Arc::new(NoProvider)),
            adapter,
            active,
            cursor: PublicationCursor(0),
        }
    }

    fn sync(&mut self, store: &Store) -> usize {
        let mut candidates = 0;
        for _ in 0..32 {
            let page = PublishedEvidenceSource::new(
                StorePublishedEvidenceReader::for_project(store, "native", "/synthetic")
                    .with_activation(&self.active.id),
                100,
            )
            .unwrap()
            .read_batch(self.cursor)
            .unwrap();
            candidates += self
                .engine
                .apply_source_page(&self.active, &page)
                .unwrap()
                .candidates;
            self.cursor = page.next;
            if !page.has_more {
                return candidates;
            }
        }
        panic!("journal regression exceeded its source-page bound")
    }

    fn candidate(&self, store: &Store) -> FrozenCandidate {
        let encoded: String = store.lock().query_row(
            "SELECT frozen_input_json FROM burn_check_candidate WHERE check_id = 'journal-regression'
             ORDER BY created_at_epoch DESC, rowid DESC LIMIT 1", [], |row| row.get(0)).unwrap();
        serde_json::from_str(&encoded).unwrap()
    }

    fn accept(&self) {
        let StartOutcome::Started(mut task) =
            self.engine.start_one_due(&self.active, 101_000).unwrap()
        else {
            panic!("the journal result must create local work")
        };
        for _ in 0..8 {
            if let Poll::Ready(settlement) =
                task.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            {
                assert_eq!(settlement.unwrap().outcome, RunOutcome::Accepted);
                return;
            }
        }
        panic!("local journal work must settle without I/O")
    }
}

fn next_page(
    store: &Store,
    source: &SyntheticJournal,
    now: i64,
) -> (OpenCodeJournalClaim, OpenCodeToolResultPage) {
    source.index(store, "/synthetic");
    let claim = store.claim_next_opencode_journal(now, 60).unwrap().unwrap();
    let page = read_opencode_live_tool_result_page(
        &source.path,
        "native-session",
        claim.cursor().unwrap().as_ref(),
        claim.baseline_sequence,
        &|| false,
    )
    .unwrap();
    (claim, page)
}

fn publish(store: &Store, source: &SyntheticJournal, now: i64) -> OpenCodeToolResultPage {
    let (claim, page) = next_page(store, source, now);
    assert!(
        store
            .publish_opencode_journal_page(&claim, &page, now)
            .unwrap()
    );
    page
}

fn validity(store: &Store) -> (bool, bool) {
    store.lock().query_row("SELECT h.evidence_valid, s.evidence_valid FROM burn_check_assessment_history h
        JOIN burn_check_current_status s USING(assessment_id) WHERE h.check_id = 'journal-regression'",
        [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
}

#[test]
fn tool_only_owner_conflict_fences_claims_and_revokes_accepted_evidence_at_publication() {
    for (accepted, evicted_header) in [(false, false), (true, false), (false, true), (true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(directory.path()).unwrap());
        let mut runtime = Runtime::new(store.clone(), !accepted);
        let mut source = SyntheticJournal::new(16);
        publish(&store, &source, 100);
        source.start_result(0);
        source.result(0);
        let first = publish(&store, &source, 101);
        assert!(first.evidence.events.iter().any(|event| matches!(
            event.fact,
            NativeEvidenceFact::Message(
                antiburn_local::analysis::NativeMessageObservation::Public(_)
            )
        )));
        assert_eq!(runtime.sync(&store), 1);
        let candidate = runtime.candidate(&store);
        let claimed = if accepted {
            runtime.accept();
            assert_eq!(validity(&store), (true, true));
            None
        } else {
            let CandidateClaim::Claimed(claimed) = runtime
                .adapter
                .claim_due(&candidate, 101_000, false)
                .unwrap()
            else {
                panic!("the safe result must be claimable")
            };
            Some(claimed)
        };
        let mut now = 102;
        if evicted_header {
            for index in 0..140 {
                source.event(
                    "message.updated.1",
                    json!({"info":{
                        "id":format!("later-assistant-{index}"), "role":"assistant",
                        "sessionID":"native-session", "parentID":"request"
                    }}),
                );
            }
            let evicted =
                loop {
                    let page = publish(&store, &source, now);
                    assert!(page.history_complete);
                    assert!(page.evidence.events.iter().all(|event| !matches!(
                        event.fact,
                        NativeEvidenceFact::AdmissionFailure(_)
                    )));
                    now += 1;
                    if !page.has_more {
                        break page;
                    }
                };
            let saved = serde_json::to_value(&evicted.cursor).unwrap();
            assert!(
                saved["live"]["requests"]["messages"]
                    .get("assistant")
                    .is_none()
            );
            assert_eq!(saved["live"]["parts"]["part-0"]["unsafe_ownership"], false);
            assert!(!evicted.evidence.history_complete);
            if accepted {
                assert_eq!(validity(&store), (true, true));
            }
        } else {
            for index in 1..=20 {
                source.start_result(index);
            }
        }
        let (reason, gap_reason) = if evicted_header {
            source.event("message.updated.1", json!({"info":{
                "id":"assistant", "role":"user", "parentID":"request", "sessionID":"native-session"
            }}));
            (
                ToolResultCompleteness::UnsafeOwnership,
                "native-unsafe-ownership",
            )
        } else {
            source.event(
                "message.updated.1",
                json!({"info":{
                    "id":"assistant", "role":"assistant", "parentID":"request", "sessionID":"other"
                }}),
            );
            (
                ToolResultCompleteness::UnsafeIdentity,
                "native-unsafe-identity",
            )
        };
        let (claim, page) = loop {
            let (claim, page) = next_page(&store, &source, now);
            assert!(page.evidence.events.len() <= 40);
            if page
                .evidence
                .events
                .iter()
                .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
            {
                break (claim, page);
            }
            assert!(page.has_more);
            assert!(
                store
                    .publish_opencode_journal_page(&claim, &page, now)
                    .unwrap()
            );
            now += 1;
        };
        assert!(
            evicted_header || now > 102,
            "the conflict must follow a bounded publication page"
        );
        let before = store.native_tool_results_current(&claim.key).unwrap();
        assert_eq!(page.evidence.events.len(), 1);
        assert!(
            matches!(&page.evidence.events[0].fact, NativeEvidenceFact::AdmissionFailure(failure)
            if failure.reason == reason)
        );
        store.lock().execute_batch("CREATE TEMP TRIGGER reject_journal_admission BEFORE INSERT ON native_tool_result_publication
            WHEN NEW.gap_reason IN ('native-unsafe-identity', 'native-unsafe-ownership')
            BEGIN SELECT RAISE(ABORT, 'synthetic rollback'); END;").unwrap();
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, now)
                .is_err()
        );
        if accepted {
            assert_eq!(validity(&store), (true, true));
        } else {
            assert_eq!(store.lock().query_row("SELECT state FROM burn_check_candidate WHERE check_id = 'journal-regression'", [],
                |row| row.get::<_, String>(0)).unwrap(), "claimed");
        }
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_journal_admission")
            .unwrap();
        assert!(
            store
                .publish_opencode_journal_page(&claim, &page, now)
                .unwrap()
        );
        if let Some(claimed) = claimed {
            assert_eq!(
                runtime
                    .adapter
                    .record_dispatch_intent(
                        &claimed,
                        now as u64 * 1000,
                        Duration::ZERO,
                        &DecisionAttempt::new("review", 1).unwrap()
                    )
                    .unwrap(),
                ClaimOutcome::Stale
            );
        } else {
            assert_eq!(validity(&store), (false, false));
        }
        assert_eq!(
            store.native_tool_results_current(&claim.key).unwrap(),
            before
        );
        let head = store
            .lock()
            .query_row(
                "SELECT MAX(publication_sequence) FROM native_tool_result_publication",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(store.lock().query_row("SELECT gap_reason FROM native_tool_result_publication WHERE publication_sequence = ?1",
            [head], |row| row.get::<_, String>(0)).unwrap(), gap_reason);
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT last_error FROM opencode_journal_delivery",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            gap_reason
        );
        drop(runtime);
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        let (idle_claim, idle) = next_page(&store, &source, now + 1);
        assert!(idle_claim.requires_full_read);
        assert!(
            store
                .publish_opencode_journal_page(&idle_claim, &idle, now + 1)
                .unwrap()
        );
        assert_eq!(idle.live_input_events, 0);
        assert!(idle.results.is_empty());
        assert_eq!(
            store
                .lock()
                .query_row(
                    "SELECT MAX(publication_sequence) FROM native_tool_result_publication",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            head
        );
        assert!(
            admission::publication_reason_in(&store.lock(), before[0].publication_sequence)
                .unwrap()
                .is_some()
        );
        if accepted {
            assert_eq!(validity(&store), (false, false));
        }
    }
}

fn append_owned_result(source: &mut SyntheticJournal, owner: &str, id: &str) {
    source.event(
        "message.updated.1",
        json!({"info":{
            "id":owner, "role":"assistant", "sessionID":"native-session", "parentID":"request"
        }}),
    );
    for status in ["running", "error"] {
        source.event("message.part.updated.1", json!({"part":{
            "id":id, "messageID":owner, "type":"tool", "callID":format!("call-{id}"), "tool":"bash",
            "state":{"status":status,"input":{"command":"false"},"error":"Synthetic result"}
        }}));
    }
}

#[test]
fn validated_owner_removal_after_header_eviction_preserves_unrelated_status_and_suffix() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path()).unwrap());
    let mut runtime = Runtime::new(store.clone(), false);
    let mut source = SyntheticJournal::new(16);
    publish(&store, &source, 100);
    append_owned_result(&mut source, "unrelated", "unrelated-tool");
    publish(&store, &source, 101);
    assert_eq!(runtime.sync(&store), 1);
    runtime.accept();
    source.start_result(0);
    source.result(0);
    publish(&store, &source, 102);
    for index in 0..140 {
        source.event(
            "message.updated.1",
            json!({"info":{
                "id":format!("later-assistant-{index}"), "role":"assistant",
                "sessionID":"native-session", "parentID":"request"
            }}),
        );
    }
    let evicted = publish(&store, &source, 103);
    assert!(!evicted.has_more && evicted.history_complete && !evicted.evidence.history_complete);
    let saved = serde_json::to_value(&evicted.cursor).unwrap();
    assert!(
        saved["live"]["requests"]["messages"]
            .get("assistant")
            .is_none()
    );
    assert_eq!(saved["live"]["parts"]["part-0"]["unsafe_ownership"], false);
    assert_eq!(validity(&store), (true, true));
    source.event("message.removed.1", json!({"messageID":"assistant"}));
    let removed = publish(&store, &source, 104);
    assert!(
        removed
            .evidence
            .events
            .iter()
            .any(|event| matches!(&event.fact,
        NativeEvidenceFact::MessageRemoval(removal) if removal.message_id == "assistant"))
    );
    assert!(
        removed
            .evidence
            .events
            .iter()
            .all(|event| !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
    );
    assert_eq!(validity(&store), (true, true));
    let key = SessionKey::new("native", "opencode", "native-session");
    let current = store.native_tool_results_current(&key).unwrap();
    assert!(
        current
            .iter()
            .any(|entry| entry.result.record_id.as_deref() == Some("assistant") && entry.removed)
    );
    assert!(
        current
            .iter()
            .any(|entry| entry.result.record_id.as_deref() == Some("unrelated") && !entry.removed)
    );
    drop(runtime);
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let idle = publish(&store, &source, 105);
    assert!(idle.evidence.events.is_empty() && idle.results.is_empty());
    assert_eq!(validity(&store), (true, true));
    append_owned_result(&mut source, "suffix", "suffix-tool");
    let suffix = publish(&store, &source, 106);
    assert!(
        suffix
            .results
            .iter()
            .any(|result| result.finality && result.admission() == ToolResultAdmission::Admitted)
    );
    assert!(
        suffix
            .evidence
            .events
            .iter()
            .all(|event| !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
    );
    assert!(!suffix.evidence.history_complete);
    assert_eq!(validity(&store), (true, true));
}

#[test]
fn public_limits_reach_the_outbox_reader_and_window_without_revoking_valid_results() {
    let tombstone_owner = "tombstone-owner";
    for limit in ["count", "bytes", "tombstones"] {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(directory.path()).unwrap());
        let mut runtime = Runtime::new(store.clone(), false);
        let mut source = SyntheticJournal::new(16);
        if limit == "tombstones" {
            for index in 1..64 {
                source.event(
                    "message.updated.1",
                    json!({"info":{
                        "id":format!("historical-{index}"), "role":"user"
                    }}),
                );
            }
        }
        publish(&store, &source, 100);
        source.start_result(0);
        source.result(0);
        publish(&store, &source, 101);
        assert_eq!(runtime.sync(&store), 1);
        runtime.accept();
        let head = store
            .lock()
            .query_row(
                "SELECT MAX(publication_sequence) FROM native_tool_result_publication",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        match limit {
            "count" => {
                for index in 0..140 {
                    source.event("message.updated.1", json!({"info":{
                        "id":format!("assistant-{index}"), "role":"assistant", "parentID":"request"
                    }}));
                }
            }
            "bytes" => {
                for index in 0..16 {
                    let id = format!("assistant-{index}");
                    source.event(
                        "message.updated.1",
                        json!({"info":{"id":id,"role":"assistant","parentID":"request"}}),
                    );
                    source.event(
                        "message.part.updated.1",
                        json!({"part":{
                            "id":format!("text-{index}"), "messageID":id, "type":"text",
                            "text":"x".repeat(16 * 1024), "time":{"end":2}
                        }}),
                    );
                    source.event("message.updated.1", json!({"info":{
                        "id":id, "role":"assistant", "parentID":"request", "time":{"completed":3}
                    }}));
                }
            }
            "tombstones" => {
                // Keep this owner newer than the historical headers that the request cache evicts.
                source.event("message.updated.1", json!({"info":{
                    "id":tombstone_owner, "role":"assistant", "sessionID":"native-session", "parentID":"request"
                }}));
                for index in 0..65 {
                    let id = format!("step-{index}");
                    // Keep the request-side binding after the public cache evicts the part.
                    source.event(
                        "message.part.updated.1",
                        json!({"part":{
                            "id":id, "messageID":tombstone_owner, "sessionID":"native-session", "type":"step-start"
                        }}),
                    );
                    source.event(
                        "message.part.removed.1",
                        json!({"messageID":tombstone_owner,"partID":id}),
                    );
                }
            }
            _ => unreachable!(),
        }
        let mut now = 102;
        loop {
            let page = publish(&store, &source, now);
            let saved = serde_json::to_value(&page.cursor).unwrap();
            assert!(
                page.evidence
                    .events
                    .iter()
                    .all(|event| !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_))),
                "{limit}"
            );
            assert!(
                page.history_complete,
                "{limit}: history_safe={}, evidence_gap={}, request_unsafe_identity={}, public_disabled={}, untyped_request_removals={:?}",
                saved["live"]["history_safe"],
                saved["live"]["evidence_gap"],
                saved["live"]["requests"]["unsafe_identity"],
                saved["live"]["public"]["disabled"],
                saved["live"]["requests"]["removed_parts"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(_, part)| part["kind"].is_null())
                    .map(|(id, _)| id)
                    .collect::<Vec<_>>()
            );
            assert!(page.evidence.events.len() <= 40);
            now += 1;
            if !page.has_more {
                assert!(!page.evidence.history_complete, "{limit}");
                if limit == "tombstones" {
                    let public = &saved["live"]["public"];
                    assert_eq!(public["historical_users"].as_object().unwrap().len(), 64);
                    assert_eq!(public["removed_parts"].as_array().unwrap().len(), 65);
                    assert_eq!(public["disabled"], true);
                    let requests = &saved["live"]["requests"];
                    let owner = &requests["messages"][tombstone_owner][1];
                    assert_eq!(owner["role"], "assistant");
                    assert_eq!(owner["sessionID"], "native-session");
                    let removals = requests["removed_parts"].as_object().unwrap();
                    assert_eq!(removals.len(), 65);
                    assert!(removals.values().all(
                        |part| part["owner"] == tombstone_owner && part["kind"] == "step-start"
                    ));
                    assert_eq!(requests["incomplete"], true);
                    assert_eq!(requests["unsafe_identity"], false);
                    assert_eq!(saved["live"]["history_safe"], true);
                    assert_eq!(saved["live"]["evidence_gap"], false);
                }
                break;
            }
        }
        let publications = store
            .native_evidence_publications_after(head, 1000)
            .unwrap();
        assert!(
            publications
                .iter()
                .any(|event| event.gap_kind == PublicationGapKind::Partial
                    && event.gap_reason.as_deref() == Some("journal-history-incomplete")
                    && !event.complete),
            "{limit}"
        );
        assert!(
            !publications
                .iter()
                .any(|event| event.outcome == ToolResultPublicationOutcome::CoverageRestored),
            "{limit}"
        );
        let batch = PublishedEvidenceSource::new(
            StorePublishedEvidenceReader::for_project(&store, "native", "/synthetic")
                .with_activation(&runtime.active.id),
            1000,
        )
        .unwrap()
        .read_batch(PublicationCursor(head as u64))
        .unwrap();
        assert!(
            batch
                .events
                .iter()
                .any(|event| matches!(event, SourceEvent::Gap { .. })),
            "{limit}"
        );
        runtime.sync(&store);
        assert_eq!(validity(&store), (true, true), "{limit}");
        assert!(
            store
                .lock()
                .query_row(
                    "SELECT history_gap FROM opencode_journal_delivery",
                    [],
                    |row| row.get::<_, bool>(0)
                )
                .unwrap(),
            "{limit}"
        );
        let idle = publish(&store, &source, now);
        assert!(
            idle.evidence.events.is_empty() && idle.results.is_empty(),
            "{limit}"
        );
        source.event(
            "message.updated.1",
            json!({"info":{
                "id":"suffix", "role":"assistant", "parentID":"request"
            }}),
        );
        for status in ["running", "error"] {
            source.event("message.part.updated.1", json!({"part":{
                "id":"suffix-tool", "messageID":"suffix", "type":"tool", "callID":"suffix-call", "tool":"bash",
                "state":{"status":status,"input":{"command":"false"},"error":"Synthetic suffix failure"}
            }}));
        }
        let suffix = publish(&store, &source, now + 1);
        assert!(
            suffix.results.iter().any(
                |result| result.finality && result.admission() == ToolResultAdmission::Admitted
            ),
            "{limit}"
        );
        assert_eq!(runtime.sync(&store), 1, "{limit}");
        assert_eq!(
            runtime.candidate(&store).context.value["session"]["source_coverage"],
            "partial",
            "{limit}"
        );
        assert_eq!(validity(&store), (true, true), "{limit}");
    }
}

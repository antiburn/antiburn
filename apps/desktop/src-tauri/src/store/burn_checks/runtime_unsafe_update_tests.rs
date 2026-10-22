use antiburn_checks::source::PublishedEvidenceSource;
use antiburn_local::analysis::{
    SourceFormat, ToolResultChange, ToolResultCompleteness, ToolResultContext,
    ToolResultObservation, ToolResultPosition, ToolResultStatus,
};

use super::*;
use crate::published_evidence_reader::StorePublishedEvidenceReader;
use crate::store::SessionKey;

fn publication(
    store: &Store,
    sequence: u64,
    identity: &str,
    session: &str,
    unsafe_case: Option<&str>,
) -> SourceBatch {
    let revision = if unsafe_case.is_some() { 2 } else { 1 };
    store
        .lock()
        .execute(
            "INSERT INTO session (environment_key, agent, session_id, source_kind,
            source_label, first_seen_at, last_seen_at, cwd)
         VALUES ('native', 'fixture', ?1, 'file', 'fixture', 'now', 'now', '/repo')
         ON CONFLICT DO NOTHING",
            [session],
        )
        .unwrap();
    let position = ToolResultPosition::Jsonl {
        record_index: sequence,
        item_index: 0,
    };
    let mut result = ToolResultObservation {
        outcome: None,
        provider_executed: None,
        source_format: SourceFormat::PiV3Jsonl,
        session_id: session.into(),
        record_id: Some(identity.into()),
        call_id: Some(identity.into()),
        result_id: Some(identity.into()),
        position,
        revision,
        change: ToolResultChange::Update,
        tool_name: Some("bash".into()),
        tool_category: None,
        arguments: Some(json!({"command": "false"})),
        content: Some("Synthetic result".into()),
        status: ToolResultStatus::Error,
        finality: true,
        completeness: ToolResultCompleteness::Complete,
        context: Some(ToolResultContext {
            result_position: position,
            main_session: true,
            complete_through_event: true,
            user_messages: vec![],
        }),
    };
    match unsafe_case {
        None => {}
        Some("identity") => result.completeness = ToolResultCompleteness::UnsafeIdentity,
        Some("order") => result.completeness = ToolResultCompleteness::UnsafeOrder,
        Some("ownership") => result.completeness = ToolResultCompleteness::UnsafeOwnership,
        Some("context") => result.context = None,
        Some("scope") => result.context.as_mut().unwrap().main_session = false,
        Some(_) => unreachable!(),
    }
    store
        .lock()
        .execute(
            "INSERT INTO native_tool_result_publication (
                publication_sequence, outcome, environment_key, agent, session_id,
                source_format, native_identity, native_position_json, native_revision,
                payload_json, complete, source_generation, claim_fence)
             VALUES (?1, 'update', 'native', 'fixture', ?2, ?3, ?4, ?5, ?6, ?7, 1, 1, 1)",
            params![
                sequence,
                session,
                serde_json::to_string(&result.source_format).unwrap(),
                identity,
                serde_json::to_string(&position).unwrap(),
                revision,
                serde_json::to_string(&result).unwrap()
            ],
        )
        .unwrap();
    PublishedEvidenceSource::new(StorePublishedEvidenceReader::new(store), 1)
        .unwrap()
        .read_batch(PublicationCursor(sequence - 1))
        .unwrap()
}

fn validity(store: &Store) -> Vec<(String, bool, bool)> {
    store
        .lock()
        .prepare(
            "SELECT s.branch_key, s.evidence_valid, h.evidence_valid
           FROM burn_check_current_status s
           JOIN burn_check_assessment_history h USING(assessment_id)
          ORDER BY s.branch_key",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn unsafe_publication_revokes_saved_status_and_pending_work_without_reusing_old_results() {
    for unsafe_case in ["identity", "order", "ownership", "context", "scope"] {
        for work_state in ["pending", "claimed", "dispatching", "answered"] {
            let directory = tempfile::tempdir().unwrap();
            let source = CHECK
                .replace(
                    "  tool_result:\n",
                    "  tool_result:\n  cooldown: 0s\n  window: {size: 2, min: 2}\n",
                )
                .replace("result: failure", "result: success");
            let (store, adapter, active) =
                fixture_with_definition(Arc::new(Store::open(directory.path()).unwrap()), &source);
            for (sequence, session) in [(1, "branch-a"), (3, "branch-b")] {
                let first = publication(&store, sequence, "first", session, None);
                CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
                    .apply_source_page(&active, &first)
                    .unwrap();
                let page = publication(&store, sequence + 1, "second", session, None);
                let candidate = apply(&adapter, &active, &page);
                let assessment = answer(&adapter, &active, claim(&adapter, &candidate), 0.99);
                assert_eq!(
                    adapter.accept(&assessment).unwrap(),
                    AcceptanceOutcome::Accepted
                );
            }
            let pending_page = publication(&store, 5, "pending", "branch-a", None);
            let pending = apply(&adapter, &active, &pending_page);
            let answered = match work_state {
                "pending" => None,
                "claimed" => {
                    claim(&adapter, &pending);
                    None
                }
                "dispatching" => {
                    adapter
                        .record_dispatch_intent(
                            &claim(&adapter, &pending),
                            100_000,
                            Duration::ZERO,
                            1,
                        )
                        .unwrap();
                    None
                }
                "answered" => Some(answer(&adapter, &active, claim(&adapter, &pending), 0.99)),
                _ => unreachable!(),
            };
            let before = adapter.load_source_progress(&active).unwrap();
            let correction = publication(&store, 6, "first", "branch-a", Some(unsafe_case));
            assert!(
                matches!(correction.events[0], SourceEvent::UnsafeUpdate { .. }),
                "{unsafe_case}"
            );
            let engine = CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator));
            let outcome = engine.apply_source_page(&active, &correction).unwrap();
            assert_eq!((outcome.observations, outcome.candidates), (0, 0));
            let branch_a = SessionKey::new("native", "fixture", "branch-a").to_identity_json();
            let branch_b = SessionKey::new("native", "fixture", "branch-b").to_identity_json();
            let expected = [
                (branch_a.clone(), false, false),
                (branch_b.clone(), true, true),
            ];
            assert_eq!(validity(&store), expected, "{unsafe_case}, {work_state}");
            assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
            assert_eq!(
                adapter.claim_due(&pending, 200_000, true).unwrap(),
                CandidateClaim::Stale
            );
            if let Some(answered) = answered {
                assert_eq!(adapter.accept(&answered).unwrap(), AcceptanceOutcome::Stale);
            }
            let progress = adapter.load_source_progress(&active).unwrap();
            assert!(
                progress.selection.branches[&BranchId(branch_a.clone())]
                    .results
                    .is_empty()
            );
            assert!(!progress.selection.branches[&BranchId(branch_a)].coverage_complete);
            assert_eq!(
                progress.selection.branches[&BranchId(branch_b.clone())],
                before.selection.branches[&BranchId(branch_b)]
            );
            drop(engine);
            drop(adapter);
            drop(store);

            let store = Arc::new(Store::open(directory.path()).unwrap());
            let adapter = Arc::new(
                StoreCheckStore::new(
                    store.clone(),
                    "unsafe-restart".into(),
                    active.id.scope.clone(),
                )
                .unwrap(),
            );
            adapter.register_activation(&active).unwrap();
            assert_eq!(adapter.load_source_progress(&active).unwrap(), progress);
            assert_eq!(validity(&store), expected);
            assert!(adapter.load_recovery(None, 10).unwrap().is_empty());
            let first = publication(&store, 7, "fresh-first", "branch-a", None);
            assert_eq!(
                CheckEngine::new(adapter.clone(), Arc::new(NoEvaluator))
                    .apply_source_page(&active, &first)
                    .unwrap()
                    .candidates,
                0
            );
            let second = publication(&store, 8, "fresh-second", "branch-a", None);
            let fresh = apply(&adapter, &active, &second);
            assert_eq!(
                fresh
                    .messages
                    .iter()
                    .map(|message| message.id.as_str())
                    .collect::<Vec<_>>(),
                ["fresh-first", "fresh-second"]
            );
        }
    }
}

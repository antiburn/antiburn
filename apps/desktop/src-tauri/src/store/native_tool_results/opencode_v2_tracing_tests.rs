use super::*;
use crate::store::burn_checks::trace_capture::LogCapture;

#[test]
fn capture_trace_summarizes_publications_and_gap_transitions_without_private_source_fields() {
    use antiburn_checks::source::{PublicationCursor, PublicationRead, PublishedEvidenceReader};
    let (_directory, store, _journal, start) = setup();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        let claim = store
            .start_opencode_v2_capture_epoch(std::slice::from_ref(&start), "PRIVATE_EPOCH", 100, 60)
            .unwrap()
            .unwrap()
            .remove(0);
        let mut output = result(5, true);
        output.content = Some("PRIVATE_RESULT_BODY".into());
        output.arguments = Some(json!({"credential": "PRIVATE_KEY"}));
        assert!(
            store
                .advance_opencode_v2_capture(&claim, 5, Some(&output), 101, 60)
                .unwrap()
        );
        for sequence in 6..106 {
            assert!(
                store
                    .advance_opencode_v2_capture(&claim, sequence, None, 102, 60)
                    .unwrap()
            );
        }
        for _ in 0..100 {
            store
                .mark_opencode_v2_capture_gap(
                    std::slice::from_ref(&claim),
                    "private-gap-canary",
                    103,
                )
                .unwrap();
        }
        let reader = crate::published_evidence_reader::StorePublishedEvidenceReader::for_project(
            &store,
            "native",
            "/synthetic",
        );
        let page = reader
            .read_batch(PublicationRead {
                after: PublicationCursor(0),
                limit: 200,
            })
            .unwrap();
        assert!(!page.events.is_empty());
        for _ in 0..100 {
            assert!(
                reader
                    .read_batch(PublicationRead {
                        after: page.next,
                        limit: 200
                    })
                    .unwrap()
                    .events
                    .is_empty()
            );
        }
    });
    assert_eq!(captured.events("opencode_v2_capture_started").len(), 1);
    assert_eq!(captured.events("opencode_v2_evidence_published").len(), 1);
    assert_eq!(captured.events("burn_check_publications_read").len(), 1);
    let transitions = captured.events("native_evidence_transition_committed");
    assert_eq!(transitions.len(), 3);
    let allowed = [
        "event",
        "publication_origin",
        "reason",
        "publication_after_cursor",
        "publication_through_cursor",
        "capture_claim_fence",
        "capture_source_generation",
        "updates",
        "removals",
        "gaps",
        "restored",
        "accepted_evidence_revoked",
        "current_status_revoked",
        "unfinished_stale",
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    for transition in &transitions {
        let fields = transition
            .fields
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        let expected = if transition.fields["publication_origin"] == "\"live_capture\"" {
            allowed.clone()
        } else {
            allowed
                .iter()
                .copied()
                .filter(|field| {
                    !matches!(*field, "capture_claim_fence" | "capture_source_generation")
                })
                .collect()
        };
        assert_eq!(fields, expected);
        assert!(transition.fields.contains_key("publication_origin"));
        assert!(transition.fields.contains_key("accepted_evidence_revoked"));
        assert!(
            transition.fields["publication_through_cursor"]
                .parse::<i64>()
                .unwrap()
                > transition.fields["publication_after_cursor"]
                    .parse::<i64>()
                    .unwrap()
        );
    }
    assert_eq!(
        transitions[1].fields["publication_origin"],
        "\"live_capture\""
    );
    assert_eq!(transitions[1].fields["capture_claim_fence"], "1");
    let published_allowed = [
        "event",
        "status",
        "claim_fence",
        "capture_source_generation",
        "events",
        "duplicate_events",
        "coverage_complete",
        "coverage_transition",
        "elapsed_ms",
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        captured.events("opencode_v2_evidence_published")[0]
            .fields
            .keys()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        published_allowed
    );
    let gaps = captured.events("opencode_v2_capture_gap");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].fields["reason"], "\"other_capture_gap\"");
    let logs = captured.text();
    for private in [
        "PRIVATE_",
        "native-session",
        "/synthetic",
        "synthetic-database-identity",
        "private-gap-canary",
        "opencode.db",
        "record_id",
        "call_id",
    ] {
        assert!(!logs.contains(private), "{private}: {logs}");
    }
}

#[test]
fn capture_recovery_trace_does_not_repeat_after_leases_are_released() {
    let (_directory, store, _journal, start) = setup();
    started(&store, &start);
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        assert_eq!(store.recover_opencode_v2_capture_claims(200).unwrap(), 1);
        for _ in 0..100 {
            assert_eq!(store.recover_opencode_v2_capture_claims(200).unwrap(), 0);
        }
    });
    let events = captured.events("opencode_v2_capture_recovery");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].fields["recovered_claims"], "1");
    assert_eq!(events[0].fields["expired_claims"], "1");
    let transitions = captured.events("native_evidence_transition_committed");
    assert_eq!(transitions.len(), 1);
    assert_eq!(
        transitions[0].fields["publication_origin"],
        "\"capture_recovery\""
    );
    assert_eq!(transitions[0].fields["gaps"], "1");
    assert!(!captured.text().contains("epoch-a"));
}

#[test]
fn scope_retirement_uses_the_same_bounded_effect_summary_without_noop_repeats() {
    let (_directory, store, _journal, start) = setup();
    let claim = started(&store, &start);
    assert!(
        store
            .advance_opencode_v2_capture(&claim, 5, Some(&result(5, false)), 101, 60)
            .unwrap()
    );
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        store
            .mark_opencode_v2_capture_gap(std::slice::from_ref(&claim), "source-scope-changed", 102)
            .unwrap();
        for _ in 0..100 {
            assert_eq!(
                store
                    .continue_opencode_v2_scope_retirement(
                        std::slice::from_ref(&claim),
                        "source-scope-changed",
                        103
                    )
                    .unwrap(),
                OpenCodeV2ReconciliationPreparation::Ready
            );
        }
    });
    let events = captured.events("native_evidence_transition_committed");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].fields["publication_origin"], "\"capture_gap\"");
    assert_eq!(events[0].fields["reason"], "\"source_scope_changed\"");
    assert_eq!(events[0].fields["removals"], "1");
    assert_eq!(events[0].fields["gaps"], "1");
}

#[test]
fn capture_trace_does_not_claim_publication_before_the_transaction_commits() {
    let (_directory, store, _journal, start) = setup();
    let claim = started(&store, &start);
    store.lock().execute_batch("CREATE TRIGGER reject_trace_publication BEFORE INSERT ON native_tool_result_publication
        WHEN NEW.outcome = 'update' BEGIN SELECT RAISE(ABORT, 'PRIVATE_STORAGE_ERROR'); END;").unwrap();
    let captured = LogCapture::default();
    tracing::subscriber::with_default(captured.clone(), || {
        assert!(
            store
                .advance_opencode_v2_capture(&claim, 5, Some(&result(5, true)), 101, 60)
                .is_err()
        );
    });
    assert!(captured.events("opencode_v2_evidence_published").is_empty());
    assert!(
        captured
            .events("native_evidence_transition_committed")
            .is_empty()
    );
    assert!(!captured.text().contains("PRIVATE_STORAGE_ERROR"));
    assert_eq!(
        store
            .lock()
            .query_row("SELECT last_sequence FROM opencode_v2_capture", [], |row| {
                row.get::<_, u64>(0)
            })
            .unwrap(),
        4
    );
}

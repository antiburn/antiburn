use std::collections::BTreeSet;
use std::sync::Arc;

use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, EvidenceValue, MemoryTurnRowStore, PiSessionReader, RawSource,
    SessionEvidence, SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator,
    SessionReader, SourceCapabilities, SourceKind, TurnRowSink, TurnRowStore,
};
use antiburn_local::insights::{
    BadgeId, BadgeStatus, NotAssessedReason, ReportCatalogs, session_badges,
};
use serde_json::Value;

#[path = "support/pricing.rs"]
mod pricing;

const FIXTURE: &str = include_str!("fixtures/pi_characterization/official_subagent.jsonl");

fn analyze(source: RawSource) -> (SessionEvidence, SessionMetricsAccumulator) {
    pricing::install();
    let input = SessionInput {
        agent: "pi".to_owned(),
        session_id: "official-subagent".to_owned(),
        source,
        fork_parent_session_id: None,
    };
    let metrics = SessionMetricsAccumulator::new("pi", &input.session_id);
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: input.agent.clone(),
        session_id: input.session_id.clone(),
        kind: SourceKind::from(&input.source),
        capabilities: SourceCapabilities::pi(),
    });
    let store = MemoryTurnRowStore::new("pi", &input.session_id);
    let rows = TurnRowSink::new(
        store as Arc<dyn TurnRowStore>,
        input.session_id.clone(),
        None,
    );
    let mut sink = CompositeSink::with_turn_rows(metrics, evidence, rows);
    let outcome = PiSessionReader.visit(&input, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    let evidence = sink.evidence().unwrap();
    let (metrics, _) = sink.into_parts().unwrap();
    (evidence, metrics)
}

fn status(evidence: &SessionEvidence) -> BadgeStatus {
    session_badges(evidence, &ReportCatalogs::default())
        .into_iter()
        .find(|badge| badge.id == BadgeId::OverpoweredSubagents)
        .unwrap()
        .status
}

#[test]
fn official_extension_observations_find_premium_subagents_without_blanket_capabilities() {
    let (evidence, metrics) = analyze(RawSource::Jsonl(FIXTURE.to_owned()));
    assert!(!evidence.capabilities.subagent_relationships);
    assert!(!evidence.capabilities.subagent_models);
    let EvidenceValue::Partial { observed, .. } = &evidence.subagents else {
        panic!("direct Pi observations must publish partial subagent evidence");
    };
    assert_eq!(observed.spawn_count, 1);
    assert_eq!(observed.delegated_turns, 1);
    assert_eq!(
        observed.delegated_models,
        BTreeSet::from(["claude-opus-4-6".to_owned()])
    );
    assert_eq!(observed.children.len(), 1);
    assert_eq!(
        observed.children[0].parent_model.as_deref(),
        Some("claude-opus-4-6")
    );
    assert_eq!(status(&evidence), BadgeStatus::Finding);

    let metrics = metrics.metrics();
    assert_eq!(metrics.billable_input_tokens, 14);
    assert_eq!(metrics.billable_output_tokens, 18);
    assert_eq!(metrics.billable_cache_read_tokens, 24);
    assert_eq!(metrics.billable_cache_creation_tokens, 21);
    assert_eq!(
        metrics
            .buckets
            .iter()
            .map(|bucket| bucket.tokens_in)
            .sum::<u64>(),
        30
    );
    assert_eq!(
        metrics
            .buckets
            .iter()
            .map(|bucket| bucket.tokens_out)
            .sum::<u64>(),
        13
    );
    assert_eq!(
        metrics
            .buckets
            .iter()
            .map(|bucket| bucket.cache_read_tokens)
            .sum::<u64>(),
        17
    );
    assert_eq!(
        metrics
            .buckets
            .iter()
            .map(|bucket| bucket.cache_write_tokens)
            .sum::<u64>(),
        19
    );
    assert_eq!(metrics.peak_context_tokens, 47);
    assert_eq!(
        metrics
            .buckets
            .iter()
            .map(|bucket| bucket.subagent_tokens)
            .sum::<u64>(),
        10
    );
    let retained = serde_json::to_string(&evidence).unwrap();
    assert!(!retained.contains("synthetic private"));
}

#[test]
fn nonpremium_observed_worker_is_unavailable_not_false_clean_or_dispatch_alias_finding() {
    let mut rows: Vec<Value> = FIXTURE
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    rows[2]["message"]["details"]["results"][0]["messages"][1]["model"] =
        Value::String("claude-sonnet-4-6".to_owned());
    let source = rows
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let (evidence, _) = analyze(RawSource::Jsonl(source));
    let EvidenceValue::Partial { observed, .. } = &evidence.subagents else {
        panic!("observed worker models do not prove complete Pi coverage");
    };
    assert_eq!(
        observed.delegated_models,
        BTreeSet::from(["claude-sonnet-4-6".to_owned()])
    );
    assert_eq!(
        status(&evidence),
        BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
    );
}

#[test]
fn malformed_or_incomplete_worker_results_do_not_infer_premium_models() {
    for pointer in [
        "/message/toolCallId",
        "/message/details",
        "/message/details/mode",
        "/message/details/results/0/messages",
        "/message/details/results/0/messages/1/model",
    ] {
        let mut rows: Vec<Value> = FIXTURE
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        *rows[2].pointer_mut(pointer).unwrap() = Value::Null;
        let source = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let (evidence, _) = analyze(RawSource::Jsonl(source));
        assert!(
            matches!(status(&evidence), BadgeStatus::NotAssessed(_)),
            "{pointer}"
        );
    }

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("incomplete.jsonl");
    let result_start = FIXTURE.rfind("{\"type\":\"message\"").unwrap();
    for suffix in ["", "{malformed}\n", "{\"type\":\"message\",\"message\":"] {
        std::fs::write(&path, format!("{}{suffix}", &FIXTURE[..result_start])).unwrap();
        let (evidence, _) = analyze(RawSource::File(path.clone()));
        assert!(
            matches!(status(&evidence), BadgeStatus::NotAssessed(_)),
            "{suffix}"
        );
    }
}

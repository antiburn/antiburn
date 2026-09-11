use std::path::Path;
use std::sync::Arc;

use antiburn_local::analysis::RESUME_SNAPSHOT_REVISION;
use antiburn_local::analysis::{
    CompositeSink, EvidenceSnapshot, EvidenceSource, MemoryTurnRowStore, RawSource, ResumePoint,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SourceCapabilities,
    SourceClaim, SourceKind, StreamSnapshot, TurnRowSink, TurnRowStore, reader_for,
};
use antiburn_local::discovery::source_version::{FingerprintInputs, SourceStat, head_hash_of};

fn claim_for(path: &Path) -> SourceClaim {
    let file = std::fs::File::open(path).expect("open source");
    let stat = SourceStat::from_open_std_file(&file).expect("stat source");
    let bytes = std::fs::read(path).expect("read source");
    SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(head_hash_of(&bytes)),
    })
}

fn fresh_snapshot(
    agent: &str,
    session_id: &str,
    capabilities: SourceCapabilities,
) -> StreamSnapshot {
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: agent.to_owned(),
        session_id: session_id.to_owned(),
        kind: SourceKind::Jsonl,
        capabilities,
    });
    StreamSnapshot {
        revision: RESUME_SNAPSHOT_REVISION,
        resume: ResumePoint {
            offset: 0,
            prefix_hash: head_hash_of(&[]),
            tail_hash: head_hash_of(&[]),
            tail_len: 0,
        },
        adapter: reader_for(agent)
            .empty_resume_state()
            .expect("reader supports resume"),
        metrics: SessionMetricsAccumulator::new(agent, session_id),
        evidence: EvidenceSnapshot {
            record: evidence.coverage_record(),
            resume: Default::default(),
        },
        next_turn_index: 0,
    }
}

fn sink_for(agent: &str, session_id: &str, capabilities: SourceCapabilities) -> CompositeSink {
    CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(agent, session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::Jsonl,
            capabilities,
        }),
        TurnRowSink::new(
            MemoryTurnRowStore::new(agent, session_id) as Arc<dyn TurnRowStore>,
            session_id.to_owned(),
            None,
        ),
    )
}

fn assert_incomplete_tail_has_no_successor_snapshot(
    agent: &str,
    capabilities: SourceCapabilities,
    jsonl: &str,
) {
    let directory = tempfile::tempdir().expect("create tempdir");
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, jsonl).expect("write JSONL");
    let session_id = "incomplete-tail";
    let input = SessionInput {
        agent: agent.to_owned(),
        session_id: session_id.to_owned(),
        source: RawSource::File(path.clone()),
        fork_parent_session_id: None,
    };
    let claim = claim_for(&path);
    let snapshot = fresh_snapshot(agent, session_id, capabilities);
    let mut sink = sink_for(agent, session_id, capabilities);

    let visit = reader_for(agent)
        .visit_claimed_resumed(&input, &claim, &snapshot, &|| false, &mut sink)
        .expect("read incomplete JSONL tail");

    assert!(
        visit.resume.is_none(),
        "{agent} must not return a successor resume snapshot after an incomplete JSONL tail"
    );
}

#[test]
fn claude_incomplete_jsonl_tail_has_no_successor_resume_snapshot() {
    assert_incomplete_tail_has_no_successor_snapshot(
        "claude",
        SourceCapabilities::claude(),
        concat!(
            r#"{"type":"user","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"test"}}"#,
            "\n",
            r#"{"type":"assistant"#,
        ),
    );
}

#[test]
fn codex_incomplete_jsonl_tail_has_no_successor_resume_snapshot() {
    assert_incomplete_tail_has_no_successor_snapshot(
        "codex",
        SourceCapabilities::codex(),
        concat!(
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"incomplete-tail","timestamp":"2026-01-01T00:00:00Z","source":"cli"}}"#,
            "\n",
            r#"{"timestamp":"2026-01-01T00:00:01Z","type":"event_msg"#,
        ),
    );
}

#[test]
fn pi_incomplete_jsonl_tail_has_no_successor_resume_snapshot() {
    assert_incomplete_tail_has_no_successor_snapshot(
        "pi",
        SourceCapabilities::pi(),
        concat!(
            r#"{"type":"message","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"test"}}"#,
            "\n",
            r#"{"type":"message"#,
        ),
    );
}

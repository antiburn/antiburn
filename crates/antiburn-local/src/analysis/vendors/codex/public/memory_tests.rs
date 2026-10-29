use super::*;
use crate::analysis;
use crate::analysis::{
    AdapterSnapshot, CompositeSink, EvidenceSource, MemoryTurnRowStore, SessionEvidenceAccumulator,
    SessionInput, SessionMetricsAccumulator, SourceCapabilities, SourceClaim, SourceFormat,
    SourceKind, StreamSnapshot, TurnRowSink, TurnRowStore, VisitOutcome,
};
use crate::discovery::source_version::head_hash_of;
use crate::discovery::{FingerprintInputs, SourceStat};
use serde_json::json;

mod snapshot_memory {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/support/snapshot_memory.rs"
    ));
}

fn json_bytes(value: &impl Serialize) -> usize {
    serde_json::to_vec(value).unwrap().len()
}

fn public_json_bound() -> usize {
    // JSON can encode one byte as six bytes: \u0000.
    let identity = "\0".repeat(4096);
    let requester = NativeRequestAttribution {
        provider: Some(identity.clone()),
        model: Some(identity.clone()),
        fast_mode: Some(false),
    };
    let position = ToolResultPosition::FileBytes {
        record_start: u64::MAX,
        record_end: u64::MAX,
        item_index: u32::MAX,
    };
    let scalar_state = PublicState {
        active: Some(Cycle {
            turn: identity.clone(),
            start: position,
            started_at_ms: Some(i64::MIN),
            valid: false,
            inputs: usize::MAX,
            messages: Vec::new(),
            bytes: MAX_CYCLE_BYTES,
            requester: Some(requester.clone()),
            generation_observed: false,
            open_calls: BTreeSet::new(),
        }),
        projection: Some(Projection {
            id: identity.clone(),
            turn: identity.clone(),
            texts: Vec::new(),
            generator: requester,
        }),
        closed_bytes: MAX_CYCLE_BYTES,
        pending_input_bytes: MAX_CYCLE_BYTES,
        ..Default::default()
    };
    let hash = "f".repeat(crate::digest::sha256_hex(b"").len());
    let members = MAX_CYCLE_BYTES + MAX_CYCLE_MESSAGES;
    let open_calls = MAX_CYCLE_MESSAGES * (json_bytes(&hash) + 1);
    // A correction keeps a second hash and can widen the position, revision, and tag.
    let closed = MAX_CYCLE_BYTES
        + MAX_CYCLE_MESSAGES
            * (1 + json_bytes(&position) + json_bytes(&u64::MAX) + 1 + hash.len() + 1);
    let finished = MAX_CYCLE_MESSAGES * (json_bytes(&hash) + 1);
    let call_turns = MAX_CYCLE_MESSAGES * json_bytes(&(&hash, &hash));
    let pending = MAX_CYCLE_BYTES + MAX_PENDING_INPUTS * (json_bytes(&identity) + 4);
    // Text strings and array framing fit inside their admitted 8 MiB source row.
    let projection = analysis::MAX_RECORD_BYTES;
    json_bytes(&scalar_state)
        + members
        + open_calls
        + closed
        + finished
        + call_turns
        + pending
        + projection
}

fn assert_public_bound(state: &PublicState) -> usize {
    if let Some(active) = &state.active {
        assert!(active.messages.len() <= MAX_CYCLE_MESSAGES);
        assert!(active.open_calls.len() <= MAX_CYCLE_MESSAGES);
        assert_eq!(
            active.bytes,
            active.messages.iter().map(json_bytes).sum::<usize>()
        );
        assert!(active.bytes <= MAX_CYCLE_BYTES);
    }
    assert!(state.closed.len() <= MAX_CYCLE_MESSAGES);
    assert!(state.closed_bytes <= MAX_CYCLE_BYTES);
    assert!(
        state
            .closed
            .values()
            .all(|(_, cycle)| cycle.messages.capacity() == 0
                && cycle.context.user_messages.capacity() == 0)
    );
    assert!(state.finished.len() <= MAX_CYCLE_MESSAGES);
    assert!(state.call_turns.len() <= MAX_CYCLE_MESSAGES);
    assert!(state.pending_inputs.len() <= MAX_PENDING_INPUTS);
    assert_eq!(
        state.pending_input_bytes,
        state
            .pending_inputs
            .iter()
            .map(|(_, message)| json_bytes(message))
            .sum::<usize>()
    );
    assert!(state.pending_input_bytes <= MAX_CYCLE_BYTES);
    if let Some(projection) = &state.projection {
        assert!(projection.texts.iter().map(String::len).sum::<usize>() <= MAX_CYCLE_BYTES);
        assert!(json_bytes(&projection.texts) <= analysis::MAX_RECORD_BYTES);
    }
    let bound = public_json_bound();
    assert!(json_bytes(state) <= bound);
    bound + snapshot_memory::encoded_size(&bound)
}

#[test]
fn a_serialized_codex_collab_fixture_snapshot_keeps_its_legacy_bound() {
    let source = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/codex_characterization/collab_agent_records.jsonl"
    ));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.jsonl");
    std::fs::write(&path, source).unwrap();
    let input = SessionInput {
        agent: "codex".into(),
        session_id: "codex-snapshot-bound".into(),
        source: RawSource::File(path.clone()),
        fork_parent_session_id: None,
        source_format: Default::default(),
    };
    let file = std::fs::File::open(&path).unwrap();
    let claim = SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat: SourceStat::from_open_std_file(&file).unwrap(),
        head_hash: Some(head_hash_of(source.as_bytes())),
    });
    let mut sink = composite(&input.session_id);
    let visit = CodexSessionReader
        .visit_claimed_resumed(
            &input,
            &claim,
            &empty_snapshot(&input.session_id),
            &|| false,
            &mut sink,
        )
        .unwrap();
    assert_eq!(visit.outcome, VisitOutcome::AcceptedFull);
    sink.observe_source_outcome(visit.outcome);
    let snapshot = sink.snapshot(visit.resume.unwrap()).unwrap();
    let state: ReadState = postcard::from_bytes(&snapshot.adapter.0).unwrap();
    let public_bound = assert_public_bound(&state.public.0);
    // The old fixture ceiling still covers the parser and all old snapshot fields.
    snapshot_memory::assert_snapshot_bounds(
        &snapshot,
        AdapterSnapshot(postcard::to_allocvec(&state.parser).unwrap()),
        4 * 1024,
        snapshot_memory::encoded_size(&state.public),
        public_bound,
    );
}

fn composite(session: &str) -> CompositeSink {
    let store = MemoryTurnRowStore::new("codex", session);
    CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("codex", session),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "codex".into(),
            session_id: session.into(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::codex(),
        }),
        TurnRowSink::new(
            std::sync::Arc::clone(&store) as std::sync::Arc<dyn TurnRowStore>,
            session,
            None,
        ),
    )
}

fn empty_snapshot(session: &str) -> StreamSnapshot {
    let mut sink = composite(session);
    sink.observe_source_outcome(VisitOutcome::AcceptedFull);
    sink.finish(SessionSummary::default());
    sink.snapshot(crate::analysis::AdapterResume {
        point: crate::analysis::ResumePoint {
            offset: 0,
            tail_hash: head_hash_of(&[]),
            tail_len: 0,
        },
        adapter: CodexSessionReader.empty_resume_state().unwrap(),
    })
    .unwrap()
}

#[derive(Default)]
struct Capture(Vec<NormalizedRecord>);

impl RecordSink for Capture {
    fn record(&mut self, record: NormalizedRecord) {
        self.0.push(record);
    }
    fn finish(&mut self, _: SessionSummary) {}
}

fn user_message(text_bytes: usize) -> NativeMessageObservation {
    NativeMessageObservation::User(
        user_observation(UserMessage {
            source_format: SourceFormat::CodexRolloutJsonl,
            session_id: "facts".into(),
            message_id: "user".into(),
            position: crate::analysis::UserMessagePosition::Jsonl { record_index: 1 },
            origin: crate::analysis::UserMessageOrigin::CodexRetainedUserText,
            scope: crate::analysis::UserMessageScope::MainSession,
            change: crate::analysis::UserMessageChange::Present,
            completeness: crate::analysis::UserMessageCompleteness::Complete,
            parts: vec![UserMessagePart {
                item_index: 0,
                native_part_id: None,
                text: "x".repeat(text_bytes),
            }],
            proof_revision: 1,
        })
        .unwrap(),
    )
}

fn tool_call(index: usize, text_bytes: usize) -> NativeMessageObservation {
    NativeMessageObservation::Public(PublicMessageObservation {
        source_format: SourceFormat::CodexRolloutJsonl,
        session_id: "facts".into(),
        record_id: format!("record-{index}"),
        part_id: None,
        call_id: Some(format!("call-{index}")),
        position: ToolResultPosition::Jsonl {
            record_index: index as u64,
            item_index: 0,
        },
        revision: 1,
        change: ToolResultChange::Update,
        eligible: true,
        completeness: ToolResultCompleteness::Complete,
        context: None,
        payload: PublicMessagePayload::ToolCall {
            tool_name: "custom".into(),
            input: Some(crate::analysis::NativeToolInput::Text(
                "x".repeat(text_bytes),
            )),
            facts: None,
            provider_executed: None,
        },
    })
}

#[test]
fn invalid_codex_cycles_drop_payload_and_stop_collecting_call_history() {
    for text_bytes in [0, MAX_CYCLE_BYTES / 2] {
        let mut state = PublicState::default();
        let mut sink = Capture::default();
        state.begin(
            &json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"turn"}}),
            0,
            &None,
            &mut sink,
        );
        for index in 0..=MAX_CYCLE_MESSAGES {
            state.member(tool_call(index, text_bytes), Some("turn"), true);
            if !state.active.as_ref().unwrap().valid {
                break;
            }
        }
        let active = state.active.as_ref().unwrap();
        assert!(!active.valid);
        assert_eq!(active.turn, "turn");
        assert!(active.messages.is_empty() && active.open_calls.is_empty());
        assert_eq!(active.bytes, 0);
        assert_eq!(active.messages.capacity(), 0);
        let encoded = postcard::to_allocvec(&JsonState(state)).unwrap();
        let mut state: JsonState<PublicState> = postcard::from_bytes(&encoded).unwrap();
        let context = Some("turn".to_owned());
        for index in 0..MAX_CYCLE_MESSAGES * 3 {
            state.0.begin(&json!({"type":"response_item","payload":{"type":"function_call","call_id":format!("new-{index}")}}), index as u64, &context, &mut sink);
            state.0.member(tool_call(index, 0), Some("turn"), true);
            if index + 1 == MAX_CYCLE_MESSAGES * 2 {
                assert_eq!(state.0.call_turns.len(), MAX_CYCLE_MESSAGES);
            }
        }
        assert_eq!(state.0.call_turns.len(), MAX_CYCLE_MESSAGES);
        let before = postcard::to_allocvec(&state).unwrap();
        state
            .0
            .member(tool_call(usize::MAX, MAX_CYCLE_BYTES), Some("turn"), true);
        assert_eq!(postcard::to_allocvec(&state).unwrap(), before);
        assert_public_bound(&state.0);
    }
}

#[test]
fn codex_pending_inputs_stop_at_both_retention_limits() {
    for text_bytes in [1, MAX_CYCLE_BYTES / 2] {
        let mut state = PublicState::default();
        let message = user_message(text_bytes);
        let attempts = if text_bytes == 1 {
            MAX_PENDING_INPUTS * 2
        } else {
            8
        };
        for _ in 0..attempts {
            state.member(message.clone(), Some("turn"), true);
        }
        assert_public_bound(&state);
        assert!(!state.pending_inputs.is_empty());
        if text_bytes == 1 {
            assert_eq!(state.pending_inputs.len(), MAX_PENDING_INPUTS);
        }
        let before = postcard::to_allocvec(&JsonState(state.clone())).unwrap();
        for _ in 0..attempts {
            state.member(message.clone(), Some("turn"), true);
        }
        assert_eq!(postcard::to_allocvec(&JsonState(state)).unwrap(), before);
    }
}

#[test]
fn codex_closed_cycles_keep_only_bounded_corrections_and_finished_guards() {
    for reason in [String::new(), "\0".repeat(4096)] {
        let mut state = PublicState::default();
        let mut sink = Capture::default();
        for index in 0..MAX_CYCLE_MESSAGES * 2 {
            let turn = format!("turn-{index:08}");
            state.begin(
                &json!({"type":"event_msg","payload":{"type":"task_started","turn_id":turn}}),
                0,
                &None,
                &mut sink,
            );
            state.member(user_message(1), Some(&turn), true);
            state.end(&json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":turn,"reason":reason}}), 1, true, "facts", (&NativeRequestAttribution::default(), None), &mut sink);
            sink.0.clear();
        }
        assert_public_bound(&state);
        assert!(!state.closed.is_empty());
        assert_eq!(state.finished.len(), MAX_CYCLE_MESSAGES);
        assert!(state.cycles_exhausted);
        let prior_turn = state.closed.keys().next().unwrap().clone();
        state.end(&json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":prior_turn,"error":"changed"}}), u64::MAX, true, "facts", (&NativeRequestAttribution::default(), None), &mut sink);
        assert!(
            matches!(sink.0.last(), Some(NormalizedRecord::ResponseCycle(cycle)) if cycle.change == ToolResultChange::Removal)
        );
        sink.0.clear();
        assert_public_bound(&state);
        for turn in state.closed.keys().cloned().collect::<Vec<_>>() {
            state.end(&json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":turn,"error":"changed"}}), u64::MAX, true, "facts", (&NativeRequestAttribution::default(), None), &mut sink);
            sink.0.clear();
        }
        assert_public_bound(&state);
        assert!(state.closed.values().all(|(proof, _)| proof.len() == 129));
        let before = postcard::to_allocvec(&JsonState(state.clone())).unwrap();
        for index in MAX_CYCLE_MESSAGES * 2..MAX_CYCLE_MESSAGES * 3 {
            let turn = format!("turn-{index:08}");
            state.begin(
                &json!({"type":"event_msg","payload":{"type":"task_started","turn_id":turn}}),
                0,
                &None,
                &mut sink,
            );
            state.member(user_message(1), Some(&turn), true);
            state.end(
                &json!({"type":"event_msg","payload":{"type":"task_complete","turn_id":turn}}),
                1,
                true,
                "facts",
                (&NativeRequestAttribution::default(), None),
                &mut sink,
            );
        }
        assert_eq!(
            postcard::to_allocvec(&JsonState(state)).unwrap().len(),
            before.len()
        );
    }
}

#[test]
fn terminal_digest_proofs_keep_only_original_and_latest_without_growing() {
    let original = crate::digest::sha256_hex(b"original");
    let mut retained = original.clone();
    assert!(terminal_digest_matches(&retained, &original));
    let mut previous = original.clone();
    for index in 0..8192 {
        let current = crate::digest::sha256_hex(index.to_string().as_bytes());
        retain_terminal_digest(&mut retained, &current);
        assert_eq!(retained.len(), 129);
        assert!(terminal_digest_matches(&retained, &original));
        assert!(terminal_digest_matches(&retained, &current));
        if previous != original {
            assert!(!terminal_digest_matches(&retained, &previous));
        }
        previous = current;
    }
}

#[test]
fn an_invalid_codex_cycle_still_publishes_independently_completed_assistant_text() {
    let mut state = PublicState::default();
    let mut sink = Capture::default();
    state.begin(
        &json!({"type":"event_msg","payload":{"type":"task_started","turn_id":"turn"}}),
        0,
        &None,
        &mut sink,
    );
    state.invalidate();
    let request = NativeRequestAttribution::default();
    state.end(&json!({"type":"event_msg","payload":{"type":"item_completed","thread_id":"facts","turn_id":"turn","item":{"type":"AgentMessage","id":"assistant","content":[{"type":"Text","text":"complete"}]}}}), 1, true, "facts", (&request, Some("turn")), &mut sink);
    assert!(state.projection.is_some());
    state.end(&json!({"type":"response_item","payload":{"type":"message","role":"assistant","id":"assistant","content":[{"type":"output_text","text":"complete"}],"internal_chat_message_metadata_passthrough":{"turn_id":"turn"}}}), 2, true, "facts", (&request, Some("turn")), &mut sink);
    assert_eq!(
        sink.0
            .iter()
            .filter(|record| matches!(record, NormalizedRecord::PublicMessage(_)))
            .count(),
        1
    );
    assert!(state.active.as_ref().unwrap().messages.is_empty());
    assert_public_bound(&state);
}

#[test]
fn codex_assistant_projection_keeps_one_bounded_source_record() {
    let mut state = PublicState::default();
    let mut sink = Capture::default();
    let request = NativeRequestAttribution::default();
    for parts in [
        vec![json!({"type":"Text","text":""}); MAX_CYCLE_MESSAGES + 1],
        vec![json!({"type":"Text","text":"x".repeat(MAX_CYCLE_BYTES)})],
    ] {
        state.end(&json!({"type":"event_msg","payload":{"type":"item_completed","thread_id":"facts","turn_id":"turn","item":{"type":"AgentMessage","id":"assistant","content":parts}}}), 0, true, "facts", (&request, None), &mut sink);
        assert!(state.projection.is_some());
        assert_public_bound(&state);
        let before = postcard::to_allocvec(&JsonState(state.clone())).unwrap();
        state.end(&json!({"type":"event_msg","payload":{"type":"item_completed","thread_id":"facts","turn_id":"turn","item":{"type":"AgentMessage","id":"assistant","content":parts}}}), 1, true, "facts", (&request, None), &mut sink);
        assert_eq!(
            postcard::to_allocvec(&JsonState(state.clone()))
                .unwrap()
                .len(),
            before.len()
        );
    }
    let parts = vec![json!({"type":"Text","text":"x".repeat(MAX_CYCLE_BYTES + 1)})];
    state.end(&json!({"type":"event_msg","payload":{"type":"item_completed","thread_id":"facts","turn_id":"turn","item":{"type":"AgentMessage","id":"assistant","content":parts}}}), 0, true, "facts", (&request, None), &mut sink);
    assert!(state.projection.is_none());
}

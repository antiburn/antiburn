use std::collections::BTreeMap;

use antiburn_local::analysis::ignored_instructions::*;
use antiburn_local::analysis::jev::*;

pub fn native_fields(name: &str, input: &serde_json::Value) -> JevNormalizedFields {
    use antiburn_local::analysis::{
        CompositeSink, EvidenceSource, FenceScope, MemoryTurnRowStore, RawSource,
        SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SourceCapabilities,
        SourceKind, TurnRowSink, TurnRowStore, TurnSessionKey, query_turn_content, reader_for,
    };
    use std::sync::Arc;
    let session = "rich-native";
    let input = SessionInput {
        agent: "claude".to_owned(),
        session_id: session.to_owned(),
        source: RawSource::Jsonl(
            serde_json::json!({"type":"assistant","uuid":"native-record",
            "message":{"role":"assistant","content":[{"type":"tool_use","id":"native-call",
                "name":name,"input":input}]}})
            .to_string(),
        ),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    };
    let store = MemoryTurnRowStore::new("claude", session);
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", session),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session.to_owned(),
            kind: SourceKind::from(&input.source),
            capabilities: SourceCapabilities::claude(),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session.to_owned(),
            None,
        ),
    );
    let outcome = reader_for("claude").visit(&input, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    assert!(!sink.turn_row_write_failed());
    let published = store.with_connection(|connection| {
        query_turn_content(
            connection,
            &TurnSessionKey {
                environment_key: "native",
                agent: "claude",
                session_id: session,
            },
            &FenceScope::single(1),
        )
        .unwrap()
    });
    let prepared =
        prepare_session_content(session, SourceFormat::ClaudeJsonl, published, Vec::new());
    assert_eq!(prepared.actions.len(), 1);
    prepared.actions[0].normalized_fields.clone().unwrap()
}
use antiburn_local::analysis::SourceFormat;

pub fn message(id: &str, order: u64, text: &str) -> ContentAction {
    ContentAction {
        reference: ContentEventReference {
            id: id.to_owned(),
            source_key_digest: "synthetic-source".to_owned(),
            thread_digest: "main".to_owned(),
            turn_index: order,
            native_record_id: Some(id.to_owned()),
            part_index: 0,
            stable: true,
        },
        timestamp_ms: Some(100 - i64::try_from(order).unwrap()),
        turn_role: "assistant".to_owned(),
        turn_scope: "main".to_owned(),
        authority: "assistant".to_owned(),
        kind: "assistant_text".to_owned(),
        text: text.to_owned(),
        tool_name: None,
        tool_call_id: None,
        normalized_fields: None,
        metadata: Default::default(),
        truncated: false,
        context_only: false,
    }
}

pub fn tool(id: &str, order: u64, name: &str, text: &str) -> ContentAction {
    let mut action = message(id, order, text);
    action.kind = "tool_input".to_owned();
    action.tool_name = Some(name.to_owned());
    action.tool_call_id = Some(format!("call-{id}"));
    let input = serde_json::from_str(text).unwrap_or_else(|_| serde_json::json!({"command":text}));
    action.normalized_fields = Some(native_fields(name, &input));
    action
}

pub fn content(rule: &str, actions: Vec<ContentAction>) -> SessionContentEvidence {
    SessionContentEvidence {
        session_identity_digest: "synthetic-session".to_owned(),
        source_format: SourceFormat::ClaudeJsonl,
        publication_fence: 7,
        selected_input_digest: String::new(),
        actions,
        instructions: vec![
            snapshot_from_text(
                "AGENTS.md",
                rule.to_owned(),
                InstructionProvenance::RecordedInjection,
                InstructionScope::Project,
            )
            .unwrap(),
        ],
        complete: true,
        limitations: Vec::new(),
        excluded_thinking_parts: 0,
        field_availability: Vec::new(),
    }
}

pub fn context(content: SessionContentEvidence) -> JevSessionContext {
    build_jev_context(&AssessmentInput {
        content: select_session_content(&content, INPUT_SELECTION),
        prior_history_complete: true,
        activity_after_ms: None,
        boundary_positions: BTreeMap::new(),
        source_generation: 1,
        source_fingerprint: Some("synthetic-fingerprint".to_owned()),
        incarnation: 1,
        comparison_after: None,
    })
    .unwrap()
}

pub fn choice(question: &JevQuestion, selected: &str) -> JevAnswer {
    let JevQuestion::Choice { criteria, .. } = question else {
        panic!("expected Choice")
    };
    assert!(criteria.contains_key(selected), "missing option {selected}");
    JevAnswer::Choice {
        choice: selected.to_owned(),
        confidence: 1.0,
        probabilities: criteria
            .keys()
            .map(|key| (key.clone(), f64::from(key == selected)))
            .collect(),
    }
}

pub fn response(batch: &JevRequestBatch) -> JevResponse {
    JevResponse {
        model: batch.request.model.clone(),
        answers: batch
            .request
            .questions
            .iter()
            .map(|(id, question)| {
                let local = &batch.answer_owners[id].1;
                let JevQuestion::Choice { .. } = question else {
                    panic!("expected Choice")
                };
                assert_eq!(local.rsplit("::").next(), Some("decision"));
                (id.clone(), choice(question, "uncertain"))
            })
            .collect(),
        usage: JevUsage {
            input_tokens: 12,
            output_tokens: 2,
        },
    }
}

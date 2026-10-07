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

#[derive(Clone, Copy)]
pub struct Answers {
    pub relationship: &'static str,
    pub permission: &'static str,
    pub read: &'static str,
    pub obligation: &'static str,
    pub completion: &'static str,
    pub paths: &'static [&'static str],
}

impl Answers {
    pub const CONFLICT: Self = Self {
        relationship: "conflict",
        permission: "independent",
        read: "not_read_order",
        obligation: "action",
        completion: "not_completion_obligation",
        paths: &[],
    };
    pub const FOLLOWS: Self = Self {
        relationship: "follows",
        ..Self::CONFLICT
    };
    pub const MISSING: Self = Self {
        relationship: "insufficient_evidence",
        ..Self::CONFLICT
    };
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

pub fn results(items: &[JevWorkItem], answers: Answers) -> Vec<JevWorkItemResult> {
    let packing = pack_work_items(items);
    assert!(packing.skipped_item_ids.is_empty());
    let mut output = Vec::new();
    for batch in packing.batches {
        assert!(batch.serialized_bytes <= MAX_REQUEST_BYTES);
        let response = response(&batch, answers);
        output.extend(unpack_jev_response(&batch, &response).unwrap());
    }
    output
}

pub fn response(batch: &JevRequestBatch, answers: Answers) -> JevResponse {
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
                let selected = match local.rsplit("::").next().unwrap() {
                    "permission" => answers.permission,
                    "condition_evidence" => "selected",
                    "read_trigger" => {
                        if matches!(answers.read, "request_order" | "read_success") {
                            "edit_request"
                        } else {
                            "not_read_rule"
                        }
                    }
                    "path_change_policy" => "other_path",
                    _ if local.starts_with("literal_qualification_") => "qualified",
                    _ if local.starts_with("literal_policy_") => "literal_other",
                    "read_prerequisite" => answers.read,
                    "action_family" => "any",
                    "obligation" => answers.obligation,
                    _ if local.starts_with("read_path_") => {
                        let index: usize =
                            local.strip_prefix("read_path_").unwrap().parse().unwrap();
                        answers.paths.get(index).copied().unwrap_or("other_path")
                    }
                    "applicability" => "applies",
                    "relationship" => answers.relationship,
                    "evidence_basis" => "self_contained",
                    "completion" => answers.completion,
                    _ => panic!("unhandled question {local}"),
                };
                (id.clone(), choice(question, selected))
            })
            .collect(),
        usage: JevUsage {
            input_tokens: 12,
            output_tokens: 2,
        },
    }
}

pub fn assess(
    context: &JevSessionContext,
    answers: Answers,
) -> (JevCheckPlan<AssessmentPlan>, AssessmentResult) {
    let check = IgnoredInstructionsCheck;
    let mut plan = check.prepare(context).unwrap();
    let classifications = results(&check.classifications(context).unwrap(), answers)
        .into_iter()
        .map(|result| (result.work_item_id.clone(), result))
        .collect::<BTreeMap<_, _>>();
    check
        .apply_classifications(&mut plan, &classifications, context)
        .unwrap();
    let mut initial = results(&plan.work_items, answers);
    for result in &mut initial {
        let item = plan
            .work_items
            .iter()
            .find(|item| item.id == result.work_item_id)
            .unwrap();
        if let Some(followup) = check.reconcile(item, result, context).unwrap() {
            let reconciled = results(&[followup], answers).pop().unwrap();
            result.answers.extend(reconciled.answers);
        }
    }
    initial.extend(classifications.into_values());
    let reduced = check.reduce(&plan, &initial, true).unwrap();
    (plan, reduced)
}

pub fn assert_citations(
    plan: &JevCheckPlan<AssessmentPlan>,
    result: &AssessmentResult,
    raw: &SessionContentEvidence,
) {
    for comparison in &plan.prepared.comparisons {
        let action = raw
            .actions
            .iter()
            .find(|action| action.reference.id == comparison.reference.action_id)
            .unwrap();
        let selected = select_session_content(raw, INPUT_SELECTION);
        let projected = selected
            .actions
            .iter()
            .find(|selected| selected.reference.id == action.reference.id)
            .unwrap();
        assert_eq!(
            &projected.text[comparison.action_text_start..comparison.action_text_end],
            comparison.action.text
        );
        assert!(raw.instructions.iter().any(|snapshot| {
            snapshot.id == comparison.reference.instruction_id
                && snapshot.digest == comparison.reference.instruction_digest
                && snapshot.sections.iter().any(|section| {
                    section.id == comparison.reference.rule_id
                        && section.start_line == comparison.reference.start_line
                        && section.end_line == comparison.reference.end_line
                })
        }));
    }
    for finding in &result.findings {
        assert!(
            plan.prepared
                .comparisons
                .iter()
                .any(|comparison| comparison.reference == finding.reference)
        );
    }
}

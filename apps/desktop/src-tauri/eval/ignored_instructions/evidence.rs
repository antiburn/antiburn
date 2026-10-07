use std::collections::BTreeSet;

use antiburn_local::analysis::jev::JevCheck;
use antiburn_local::analysis::jev::JevInputField;
use antiburn_local::analysis::jev_evidence::{
    JevNativeFieldContainer, JevNativeFieldRange, UserTextHistoryProof,
};
use antiburn_local::checks::ignored_instructions::{
    AssessmentInput, AssessmentResult, ContentAction, ContentEventReference,
    IgnoredInstructionsCheck, InstructionProvenance, InstructionScope, RuleActionRef,
    SessionContentEvidence, content_action_digest, select_session_content, snapshot_from_text,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::fixtures::{Case, Event, Kind, Shape};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct EvidenceReference {
    pub(crate) source: String,
    pub(crate) start_line: u32,
    pub(crate) end_line: u32,
    pub(crate) action: String,
}

pub(crate) fn reference(value: &RuleActionRef) -> EvidenceReference {
    EvidenceReference {
        source: value.source.clone(),
        start_line: value.start_line,
        end_line: value.end_line,
        action: value.action_id.clone(),
    }
}

const FIELD_PREFIX: &str = "Synthetic retained field prefix: π\n";

fn bind(action: &mut ContentAction, field: JevInputField, name: &str, text_bytes: usize) {
    action.metadata.bindings.push(JevNativeFieldRange {
        native_record_id: action.reference.native_record_id.clone(),
        field,
        container: JevNativeFieldContainer::Record,
        pointer: format!("/synthetic/{name}"),
        start: FIELD_PREFIX.len(),
        end: FIELD_PREFIX.len() + text_bytes,
    });
}

fn action(index: usize, event: &Event, case: &Case) -> ContentAction {
    let text = event.text();
    let (role, authority, kind, tool, value) = match event.kind() {
        Kind::Assistant => (
            "assistant",
            "assistant",
            "assistant_text",
            None,
            text.to_owned(),
        ),
        Kind::User => ("user", "user", "user_text", None, text.to_owned()),
        Kind::Unknown => (
            "unknown",
            "unknown",
            "assistant_text",
            None,
            text.to_owned(),
        ),
        Kind::Bash => (
            "assistant",
            "assistant",
            "tool_input",
            Some("Bash"),
            json!({"command":text}).to_string(),
        ),
        Kind::BashOutput => ("tool", "tool", "tool_result", Some("Bash"), text.to_owned()),
        Kind::Read => (
            "assistant",
            "assistant",
            "tool_input",
            Some("Read"),
            json!({"filePath":text}).to_string(),
        ),
        Kind::ReadOutput => ("tool", "tool", "tool_result", Some("Read"), text.to_owned()),
        Kind::Search => (
            "assistant",
            "assistant",
            "tool_input",
            Some("Grep"),
            text.to_owned(),
        ),
        Kind::SearchOutput => ("tool", "tool", "tool_result", Some("Grep"), text.to_owned()),
        Kind::Other => (
            "assistant",
            "assistant",
            "tool_input",
            Some("issue_tool"),
            text.to_owned(),
        ),
        Kind::OtherOutput => (
            "tool",
            "tool",
            "tool_result",
            Some("issue_tool"),
            text.to_owned(),
        ),
        Kind::EditPath => (
            "assistant",
            "assistant",
            "tool_input",
            Some("Edit"),
            json!({"filePath":text}).to_string(),
        ),
        Kind::EditContent => (
            "assistant",
            "assistant",
            "tool_input",
            Some("Edit"),
            json!({"filePath":"src/App.tsx","newString":text}).to_string(),
        ),
        Kind::Patch => (
            "assistant",
            "assistant",
            "tool_input",
            Some("apply_patch"),
            json!({"patchText":text}).to_string(),
        ),
        Kind::Rename => {
            let (from, to) = text
                .split_once('\n')
                .expect("rename has source and destination");
            ("assistant","assistant","tool_input",Some("apply_patch"),json!({"patchText":format!("*** Begin Patch\n*** Update File: {from}\n*** Move to: {to}\n@@\n-old\n+new\n*** End Patch")}).to_string())
        }
    };
    let mut action = ContentAction {
        reference: ContentEventReference {
            id: format!("e{index}"),
            native_record_id: Some(format!("e{index}")),
            source_key_digest: "eval-source".to_owned(),
            thread_digest: "main".to_owned(),
            turn_index: index as u64,
            part_index: 0,
            stable: true,
        },
        timestamp_ms: Some(index as i64),
        turn_role: role.to_owned(),
        turn_scope: "main".to_owned(),
        authority: authority.to_owned(),
        kind: kind.to_owned(),
        text: value,
        tool_name: tool.map(str::to_owned),
        tool_call_id: tool.map(|_| format!("call{index}")),
        normalized_fields: None,
        metadata: Default::default(),
        truncated: false,
        context_only: false,
    };
    match event.kind() {
        Kind::Bash => bind(
            &mut action,
            JevInputField::BashCommandInput,
            "command",
            text.len(),
        ),
        Kind::User => {
            action.metadata.user_text_history = Some(UserTextHistoryProof {
                source_format: case.source_format,
                session_id: "eval-session".to_owned(),
                message_id: format!("e{index}"),
                revision: 1,
            });
            bind(
                &mut action,
                JevInputField::UserMessage,
                "human_text",
                text.len(),
            );
        }
        _ => {}
    }
    if let Event::Result((_, _, request, state)) = event {
        action.tool_call_id = Some(format!("call{request}"));
        action.metadata.state = *state;
        bind(
            &mut action,
            JevInputField::BashCommandOutput,
            "output",
            text.len(),
        );
    }
    action
}

pub(crate) fn raw_actions(case: &Case) -> Vec<ContentAction> {
    let mut actions = case
        .scenario
        .events
        .iter()
        .enumerate()
        .map(|(index, event)| action(index, event, case))
        .collect::<Vec<_>>();
    match case.scenario.shape {
        Shape::LongHistory => {
            let mut last = actions.pop().expect("long history has final action");
            for index in 0..32 {
                actions.push(action(
                    index + 100,
                    &Event::Text((Kind::Assistant, "I checked the documentation.".to_owned())),
                    case,
                ));
            }
            last.reference.turn_index = 200;
            actions.push(last);
        }
        Shape::LongText => {
            let first = actions.first_mut().expect("long text has an action");
            if first.tool_name.as_deref() == Some("Bash") {
                let mut command: Value = serde_json::from_str(&first.text).expect("command JSON");
                let text = format!(
                    "{}{}",
                    "# Unrelated Unicode context é🦀.\n".repeat(600),
                    command["command"].as_str().expect("command text")
                );
                command["command"] = json!(text);
                first.text = command.to_string();
                first.metadata.bindings.clear();
                bind(
                    first,
                    JevInputField::BashCommandInput,
                    "command",
                    text.len(),
                );
            } else {
                first.text = format!(
                    "{}{}",
                    "Unrelated Unicode context é🦀.\n".repeat(600),
                    first.text
                );
            }
        }
        Shape::Truncated => actions[0].truncated = true,
        Shape::SiblingHistory => actions[0].reference.thread_digest = "sibling".to_owned(),
        _ => {}
    }
    for action in &mut actions {
        if action.kind == "tool_input" {
            action.normalized_fields = antiburn_local::analysis::ContentPart::new(
                antiburn_local::analysis::ContentKind::ToolInput,
                action.text.clone(),
            )
            .with_tool_identity(action.tool_name.clone(), action.tool_call_id.clone())
            .normalized_fields;
        }
    }
    antiburn_local::analysis::jev_evidence::normalize_context(&mut actions, case.source_format);
    actions
}

pub(crate) fn input(case: &Case) -> AssessmentInput {
    let provenance = if case.scenario.shape == Shape::CurrentFile {
        InstructionProvenance::CurrentFileComparison
    } else {
        InstructionProvenance::RecordedInjection
    };
    let instruction = snapshot_from_text(
        "AGENTS.md",
        format!("# Evaluation rule\n\n{}", case.scenario.instruction),
        provenance,
        InstructionScope::Project,
    )
    .expect("authored instruction snapshot");
    let content = SessionContentEvidence {
        session_identity_digest: "eval-session".to_owned(),
        source_format: case.source_format,
        publication_fence: 1,
        selected_input_digest: "unselected".to_owned(),
        actions: raw_actions(case),
        instructions: vec![instruction],
        complete: true,
        limitations: Vec::new(),
        excluded_thinking_parts: 0,
        field_availability: Vec::new(),
    };
    AssessmentInput {
        content: select_session_content(&content, IgnoredInstructionsCheck.input_selection()),
        prior_history_complete: case.scenario.shape != Shape::MissingHistory,
        comparison_after: None,
        boundary_positions: Default::default(),
        activity_after_ms: None,
        source_generation: 1,
        source_fingerprint: None,
        incarnation: 1,
    }
}

pub(crate) fn expected_references(case: &Case) -> BTreeSet<EvidenceReference> {
    if case.scenario.expected.actions.is_empty() {
        return BTreeSet::new();
    }
    let input = input(case);
    let instruction = &input.content.instructions[0];
    let rule_sections = if case.scenario.expected.rule_sections.is_empty() {
        assert_eq!(
            instruction.sections.len(),
            1,
            "{}: multi-rule findings require expected.rule_sections",
            case.id
        );
        &[0][..]
    } else {
        case.scenario.expected.rule_sections.as_slice()
    };
    case.scenario
        .expected
        .actions
        .iter()
        .flat_map(|index| {
            rule_sections.iter().map(move |section_index| {
                let section = instruction
                    .sections
                    .get(*section_index)
                    .expect("expected rule section exists");
                EvidenceReference {
                    source: instruction.source.clone(),
                    start_line: section.start_line,
                    end_line: section.end_line,
                    action: format!("e{index}"),
                }
            })
        })
        .collect()
}

pub(crate) fn findings_valid(result: &AssessmentResult, input: &AssessmentInput) -> bool {
    result.findings.iter().all(|finding| {
        let Some(decision) = finding.decision_record() else {
            return false;
        };
        let reference = &finding.reference;
        decision.evaluator_revision
            == antiburn_local::checks::ignored_instructions::evaluator_revision()
            && decision.source_generation == input.source_generation
            && decision.source_fingerprint == input.source_fingerprint
            && decision.publication_fence == input.content.publication_fence
            && decision.model == result.model_version
            && input.content.instructions.iter().any(|snapshot| {
                snapshot.id == reference.instruction_id
                    && snapshot.digest == reference.instruction_digest
                    && snapshot.provenance == reference.provenance
                    && snapshot.scope == reference.scope
                    && snapshot.sections.iter().any(|section| {
                        section.id == reference.rule_id
                            && section.start_line == reference.start_line
                            && section.end_line == reference.end_line
                            && section
                                .text
                                .get(decision.rule_start_byte..decision.rule_end_byte)
                                .is_some_and(|text| !text.is_empty())
                    })
            })
            && input.content.actions.iter().any(|action| {
                action.reference == decision.action_anchor.source
                    && action.authority == decision.action_authority
                    && content_action_digest(action) == reference.action_digest
                    && action
                        .text
                        .get(decision.action_anchor.start_byte..decision.action_anchor.end_byte)
                        .is_some_and(|text| !text.is_empty())
            })
            && decision.selected_evidence.iter().all(|identity| {
                input.content.actions.iter().any(|action| {
                    action.reference == identity.source
                        && content_action_digest(action) == identity.content_digest
                        && action
                            .text
                            .get(identity.start_byte..identity.end_byte)
                            .is_some_and(|text| !text.is_empty())
                })
            })
    })
}

#[test]
fn sibling_rules_bind_exact_expected_action_references() {
    for mut case in crate::fixtures::select("context", Some("aislop-known-base-pr-missing-base"))
        .expect("four agent cases")
    {
        let input = input(&case);
        let instruction = &input.content.instructions[0];
        assert_eq!(instruction.sections.len(), 2);
        let expected = expected_references(&case);
        assert_eq!(expected.len(), 1);
        let conditional = &instruction.sections[1];
        assert!(expected.contains(&EvidenceReference {
            source: instruction.source.clone(),
            start_line: conditional.start_line,
            end_line: conditional.end_line,
            action: "e2".to_owned(),
        }));

        case.scenario.expected.rule_sections = vec![0, 1];
        let siblings = expected_references(&case);
        assert_eq!(siblings.len(), 2);
        assert!(expected.is_subset(&siblings));
        assert!(siblings.iter().all(|reference| reference.action == "e2"));
    }
}

#[test]
fn positive_references_resolve_through_production_preparation() {
    let capabilities = &crate::support::provider::configuration().capabilities;
    for case in crate::fixtures::select("all", None).expect("all cases") {
        let input = input(&case);
        let context =
            antiburn_local::checks::ignored_instructions::build_jev_context_with_capabilities(
                &input,
                &Default::default(),
                capabilities,
            )
            .expect("context");
        let plan = IgnoredInstructionsCheck
            .prepare_with_capabilities(&context, capabilities)
            .expect("plan");
        for expected in expected_references(&case) {
            assert!(
                plan.prepared
                    .comparisons
                    .iter()
                    .any(|comparison| reference(&comparison.reference) == expected),
                "{}: {expected:?}",
                case.id
            );
        }
    }
}

#[test]
fn request_result_joins_and_human_suffixes_are_exact() {
    for case in crate::fixtures::select("context", None).expect("context cases") {
        let actions = raw_actions(&case);
        for (index, event) in case.scenario.events.iter().enumerate() {
            if let Event::Result((_, text, request, state)) = event {
                assert_eq!(actions[index].tool_call_id, actions[*request].tool_call_id);
                assert_eq!(actions[index].metadata.state, *state);
                let binding = &actions[index].metadata.bindings[0];
                let native = format!("{FIELD_PREFIX}{text}");
                assert!(binding.start > 0);
                assert_eq!(&native[binding.start..binding.end], text);
                assert_eq!(
                    binding.native_record_id,
                    actions[index].reference.native_record_id
                );
            }
            if event.kind() == Kind::User {
                let proof = actions[index]
                    .metadata
                    .user_text_history
                    .as_ref()
                    .expect("human source proof");
                assert_eq!(proof.source_format, case.source_format);
                assert_eq!(
                    Some(&proof.message_id),
                    actions[index].reference.native_record_id.as_ref()
                );
            }
        }
    }
}

#[test]
fn projection_keeps_context_and_excludes_edit_body_and_read_output() {
    let case = crate::fixtures::select("limits", Some("excluded-edit-body:claude"))
        .expect("case")
        .remove(0);
    let selected = input(&case);
    assert!(
        selected
            .content
            .actions
            .iter()
            .all(|action| !action.text.contains("useEffect"))
    );
    let case = crate::fixtures::select("context", Some("failure-marker-missing:pi"))
        .expect("case")
        .remove(0);
    let selected = input(&case);
    assert!(
        selected
            .content
            .actions
            .iter()
            .any(|action| action.kind == "tool_result" && action.text.contains("FAILED"))
    );
    let case = crate::fixtures::select("context", Some("approval-exact-file:codex"))
        .expect("case")
        .remove(0);
    assert!(
        self::input(&case)
            .content
            .actions
            .iter()
            .any(|action| action.authority == "user")
    );
    let case = crate::fixtures::select("limits", Some("excluded-read-output:pi"))
        .expect("case")
        .remove(0);
    assert!(
        self::input(&case)
            .content
            .actions
            .iter()
            .all(|action| !action.text.contains("Retrieved text:"))
    );
}

#[test]
fn normalized_context_rejects_short_ranges_and_duplicate_calls() {
    let case = crate::fixtures::select("context", Some("failure-marker-missing:claude"))
        .expect("case")
        .remove(0);
    let mut actions = raw_actions(&case);
    assert!(actions[1].metadata.command_result.is_some());
    actions.push(actions[0].clone());
    antiburn_local::analysis::jev_evidence::normalize_context(&mut actions, case.source_format);
    assert!(actions[1].metadata.command_result.is_none());
    let case = crate::fixtures::select("context", Some("approval-exact-file:pi"))
        .expect("case")
        .remove(0);
    let mut actions = raw_actions(&case);
    assert!(actions[0].metadata.human_text.is_some());
    actions[0].metadata.bindings[0].end -= 1;
    antiburn_local::analysis::jev_evidence::normalize_context(&mut actions, case.source_format);
    assert!(actions[0].metadata.human_text.is_none());
}

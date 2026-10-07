use super::*;
use crate::analysis::jev::{JevInputField, JevNormalizedCategory, JevNormalizedFields};
use crate::analysis::jev_evidence::{
    JevNativeFieldContainer, JevNativeFieldRange, JevOperationState, UserTextHistoryProof,
    normalize_context,
};
use crate::checks::ignored_instructions::assessment::tests::{event, input};
use crate::checks::ignored_instructions::{
    INPUT_SELECTION, build_assessment_plan, select_session_content,
};

fn bind(action: &mut ContentAction, field: JevInputField) {
    action.metadata.bindings = vec![JevNativeFieldRange {
        native_record_id: action.reference.native_record_id.clone(),
        field,
        container: JevNativeFieldContainer::Record,
        pointer: if field == JevInputField::BashCommandInput {
            "/command"
        } else {
            "/content"
        }
        .to_owned(),
        start: 0,
        end: action.text.len(),
    }];
}

pub(crate) fn human() -> ContentAction {
    let mut action = event(
        "human",
        1,
        "user",
        "main",
        "Run the focused tests. Wait before publishing.",
    );
    action.kind = "user".to_owned();
    action.metadata.user_text_history = Some(UserTextHistoryProof {
        source_format: crate::analysis::SourceFormat::ClaudeJsonl,
        session_id: "session".to_owned(),
        message_id: "human".to_owned(),
        revision: 1,
    });
    bind(&mut action, JevInputField::UserMessage);
    normalize_context(
        std::slice::from_mut(&mut action),
        crate::analysis::SourceFormat::ClaudeJsonl,
    );
    action
}

pub(crate) fn test_pair() -> Vec<ContentAction> {
    let mut request = event("test-request", 2, "assistant", "main", "cargo test --lib");
    request.kind = "tool_input".to_owned();
    request.tool_name = Some("bash".to_owned());
    request.tool_call_id = Some("test-call".to_owned());
    request.normalized_fields = Some(JevNormalizedFields {
        category: Some(JevNormalizedCategory::BashCommand),
        values: [(JevInputField::BashCommandInput, request.text.clone())].into(),
        malformed: false,
    });
    bind(&mut request, JevInputField::BashCommandInput);
    let mut result = event(
        "test-result",
        3,
        "tool",
        "main",
        "test result: FAILED. 3 passed; 1 failed",
    );
    result.authority = "tool".to_owned();
    result.kind = "tool_result".to_owned();
    result.tool_name = request.tool_name.clone();
    result.tool_call_id = request.tool_call_id.clone();
    result.metadata.state = JevOperationState::Completed;
    bind(&mut result, JevInputField::BashCommandOutput);
    let mut actions = vec![request, result];
    normalize_context(&mut actions, crate::analysis::SourceFormat::ClaudeJsonl);
    actions
}

#[test]
fn command_results_require_one_exact_native_join() {
    let pair = test_pair();
    assert!(command_result(&pair[1], &pair));
    for variant in 0..8 {
        let mut changed = pair.clone();
        match variant {
            0 => changed[1].tool_call_id = Some("other-call".to_owned()),
            1 => changed[1].reference.thread_digest = "sibling".to_owned(),
            2 => changed[1].reference.turn_index = 0,
            3 => changed[1].truncated = true,
            4 => changed[1].metadata.state = JevOperationState::Running,
            5 => changed[1].metadata.bindings[0].native_record_id = Some("other-record".to_owned()),
            6 => changed.push(changed[0].clone()),
            7 => changed.push(changed[1].clone()),
            _ => unreachable!(),
        }
        normalize_context(&mut changed, crate::analysis::SourceFormat::ClaudeJsonl);
        assert!(!command_result(&changed[1], &changed), "variant {variant}");
    }
}

#[test]
fn native_ranges_preserve_human_suffixes_and_command_context() {
    let mut suffix = human();
    suffix.metadata.bindings[0].start = 120;
    suffix.metadata.bindings[0].end += 120;
    normalize_context(
        std::slice::from_mut(&mut suffix),
        crate::analysis::SourceFormat::ClaudeJsonl,
    );
    assert!(human_text(&suffix));

    let mut pair = test_pair();
    pair[0].text = serde_json::json!({"command": "cargo test --lib", "cwd": "/repo"}).to_string();
    let command = pair[0].text.clone();
    pair[0]
        .normalized_fields
        .as_mut()
        .unwrap()
        .values
        .insert(JevInputField::BashCommandInput, command);
    normalize_context(&mut pair, crate::analysis::SourceFormat::ClaudeJsonl);
    assert!(command_result(&pair[1], &pair));
    pair[0].metadata.bindings[0].end -= 1;
    assert!(!command_result(&pair[1], &pair));
    pair[0].metadata.bindings[0].end += 1;
    pair[0].metadata.bindings[0].pointer = "/native/nested/command_text".to_owned();
    normalize_context(&mut pair, crate::analysis::SourceFormat::ClaudeJsonl);
    assert!(command_result(&pair[1], &pair));
}

fn pi_human_suffix() -> (String, ContentAction) {
    let prefix =
        "<skill name=\"source-review\" location=\"/repo/SKILL.md\">\nReview café 🦀.\n</skill>\n";
    let suffix = "I approve the café update 🦀. Run its focused tests.";
    let native = format!("{prefix}{suffix}");
    let mut action = human();
    action.text = suffix.to_owned();
    action
        .metadata
        .user_text_history
        .as_mut()
        .unwrap()
        .source_format = crate::analysis::SourceFormat::PiV3Jsonl;
    action.metadata.bindings[0].pointer = "/message/content/0/text".to_owned();
    action.metadata.bindings[0].start = prefix.len();
    action.metadata.bindings[0].end = native.len();
    normalize_context(
        std::slice::from_mut(&mut action),
        crate::analysis::SourceFormat::PiV3Jsonl,
    );
    (native, action)
}

#[test]
fn utf8_pi_human_suffix_keeps_native_offsets_through_selection_and_episode_binding() {
    let (native, action) = pi_human_suffix();
    let range = action.metadata.bindings[0].clone();
    assert!(range.start > native[..range.start].chars().count());
    assert!(action.text.len() > action.text.chars().count());
    assert_eq!(
        native.get(range.start..range.end),
        Some(action.text.as_str())
    );
    assert!(human_text(&action));
    let mut source = input(vec![action], "Get human approval before publishing.");
    source.content.source_format = crate::analysis::SourceFormat::PiV3Jsonl;
    normalize_context(&mut source.content.actions, source.content.source_format);
    let selected = select_session_content(&source.content, INPUT_SELECTION);
    let restored: ContentAction =
        serde_json::from_value(serde_json::to_value(&selected.actions[0]).unwrap()).unwrap();
    assert_eq!(restored.metadata.bindings, [range]);
    assert_eq!(
        native.get(restored.metadata.bindings[0].start..restored.metadata.bindings[0].end),
        Some(restored.text.as_str())
    );
    assert!(human_text(&restored));
    assert_eq!(selected, select_session_content(&selected, INPUT_SELECTION));

    let trigger = event("publish", 4, "assistant", "main", "Published the update.");
    let mut actions = vec![restored, trigger];
    let plan = build_assessment_plan(input(
        actions.clone(),
        "Get human approval before publishing.",
    ));
    let comparison = plan
        .comparisons
        .iter()
        .find(|comparison| comparison.reference.action_id == "publish")
        .unwrap();
    let episode = crate::checks::ignored_instructions::decisions::episode(
        comparison,
        &actions,
        &crate::analysis::jev::capabilities::ModelCapabilities::jev_default(),
        true,
    );
    assert!(episode.authorization_available());
    assert!(episode.has_source_bindings(comparison.source_binding.as_ref().unwrap()));
    assert_eq!(
        episode.selected_actions[0].metadata.bindings[0].start,
        native.len() - actions[0].text.len()
    );

    // A same-length text change invalidates the saved selected-text digest.
    actions[0].text = actions[0].text.replacen("approve", "decline", 1);
    let mut stale = episode;
    stale.selected_actions[0] = actions[0].clone();
    assert!(!stale.has_source_bindings(comparison.source_binding.as_ref().unwrap()));
}

#[test]
fn utf8_pi_human_suffix_rejects_native_range_and_source_mismatches() {
    let (native, original) = pi_human_suffix();
    for variant in 0..6 {
        let mut action = original.clone();
        match variant {
            0 => {
                action.metadata.bindings[0].end =
                    action.metadata.bindings[0].start + action.text.chars().count()
            }
            1 => action.metadata.bindings[0].end -= 1,
            2 => action.metadata.bindings[0].start = action.metadata.bindings[0].end + 1,
            3 => {
                action.metadata.bindings[0].native_record_id =
                    Some("other-native-record".to_owned())
            }
            4 => action.metadata.bindings[0].field = JevInputField::BashCommandOutput,
            5 => action.text = native.clone(),
            _ => unreachable!(),
        }
        normalize_context(
            std::slice::from_mut(&mut action),
            crate::analysis::SourceFormat::PiV3Jsonl,
        );
        assert!(!human_text(&action), "variant {variant}");
    }
}

#[test]
fn human_authority_needs_native_proof_and_selected_text() {
    let original = human();
    assert!(human_text(&original));
    for variant in 0..4 {
        let mut changed = original.clone();
        match variant {
            0 => changed.metadata.user_text_history = None,
            1 => changed.authority = "tool".to_owned(),
            2 => changed.metadata.bindings[0].end -= 1,
            3 => changed.truncated = true,
            _ => unreachable!(),
        }
        normalize_context(
            std::slice::from_mut(&mut changed),
            crate::analysis::SourceFormat::ClaudeJsonl,
        );
        assert!(!human_text(&changed));
    }
    let source = input(vec![original], "Do not publish without approval.");
    let selected = select_session_content(&source.content, INPUT_SELECTION);
    assert!(human_text(&selected.actions[0]));
    assert_eq!(selected, select_session_content(&selected, INPUT_SELECTION));
}

#[test]
fn episodes_keep_observed_results_and_reject_injected_authority() {
    let mut actions = vec![human()];
    actions.extend(test_pair());
    actions.push(event(
        "report",
        4,
        "assistant",
        "main",
        "All focused tests passed.",
    ));
    let source = input(actions, "Report test results accurately.");
    let plan = build_assessment_plan(source.clone());
    let comparison = plan
        .comparisons
        .iter()
        .find(|comparison| comparison.reference.action_id == "report")
        .unwrap();
    let capabilities = crate::analysis::jev::capabilities::ModelCapabilities::jev_default();
    let episode = crate::checks::ignored_instructions::decisions::episode(
        comparison,
        &source.content.actions,
        &capabilities,
        true,
    );
    assert!(episode.complete_selected_history);
    assert!(episode.authorization_available());
    assert!(episode.results_available());
    assert!(episode.has_source_bindings(comparison.source_binding.as_ref().unwrap()));

    let mut injected = source.content.actions.clone();
    injected[0].metadata.user_text_history = None;
    normalize_context(&mut injected, crate::analysis::SourceFormat::ClaudeJsonl);
    let rejected = crate::checks::ignored_instructions::decisions::episode(
        comparison,
        &injected,
        &capabilities,
        true,
    );
    assert!(!rejected.complete_selected_history);
    assert!(!rejected.authorization_available());
    assert!(rejected.events.iter().all(|event| event.role != "user"));
    assert_ne!(episode.revision, rejected.revision);
}

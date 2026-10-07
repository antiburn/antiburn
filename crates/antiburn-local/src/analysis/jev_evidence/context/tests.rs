use super::*;
use crate::analysis::jev::{JevNormalizedCategory, JevNormalizedFields};
use crate::analysis::jev_evidence::{JevNativeFieldContainer, UserTextHistoryProof};

fn action(
    id: &str,
    order: u64,
    kind: &str,
    authority: &str,
    text: &str,
    field: JevInputField,
) -> ContentAction {
    ContentAction {
        reference: ContentEventReference {
            id: id.to_owned(),
            source_key_digest: "source".to_owned(),
            thread_digest: "root".to_owned(),
            turn_index: order,
            native_record_id: Some(id.to_owned()),
            part_index: 0,
            stable: true,
        },
        timestamp_ms: None,
        turn_role: authority.to_owned(),
        turn_scope: "main".to_owned(),
        authority: authority.to_owned(),
        kind: kind.to_owned(),
        text: text.to_owned(),
        tool_name: None,
        tool_call_id: None,
        normalized_fields: None,
        truncated: false,
        context_only: false,
        metadata: super::super::JevOperationMetadata {
            bindings: vec![JevNativeFieldRange {
                native_record_id: Some(id.to_owned()),
                field,
                container: JevNativeFieldContainer::Record,
                pointer: "/native/text".to_owned(),
                start: 0,
                end: text.len(),
            }],
            ..Default::default()
        },
    }
}

fn command_pair() -> Vec<ContentAction> {
    let mut request = action(
        "request",
        2,
        "tool_input",
        "assistant",
        "cargo test --lib",
        JevInputField::BashCommandInput,
    );
    request.tool_call_id = Some("call".to_owned());
    request.tool_name = Some("recorded-shell".to_owned());
    request.normalized_fields = Some(JevNormalizedFields {
        category: Some(JevNormalizedCategory::BashCommand),
        values: [(JevInputField::BashCommandInput, request.text.clone())].into(),
        malformed: false,
    });
    let mut result = action(
        "result",
        3,
        "tool_result",
        "tool",
        "test result: FAILED",
        JevInputField::BashCommandOutput,
    );
    result.tool_call_id = request.tool_call_id.clone();
    result.tool_name = request.tool_name.clone();
    result.metadata.state = JevOperationState::Completed;
    vec![request, result]
}

#[test]
fn equivalent_normalized_commands_do_not_depend_on_native_pointer_spelling() {
    for pointer in [
        "/payload/arguments",
        "/message/content/0/input/command",
        "/payload/item/nested/results/command_text",
    ] {
        let mut actions = command_pair();
        actions[0].metadata.bindings[0].pointer = pointer.to_owned();
        normalize_context(&mut actions, SourceFormat::CodexRolloutJsonl);
        let fact = actions[1].metadata.command_result.as_ref().unwrap();
        assert!(fact.matches_action(&actions[1]));
        assert!(fact.matches_request(&actions[0]));
        assert_eq!(fact.request_range.pointer, pointer);
        assert_eq!(fact.state, JevOperationState::Completed);
        assert!(actions[1].metadata.human_text.is_none());
    }
}

#[test]
fn request_digest_binds_selected_command_text_not_raw_native_arguments() {
    use crate::analysis::jev::JevInputSelection;
    use crate::analysis::jev_evidence::{SessionContentEvidence, select_session_content};

    for selected in [
        "cargo test --lib".to_owned(),
        serde_json::json!({"command": "cargo test --lib", "cwd": "/repo/café"}).to_string(),
    ] {
        let mut actions = command_pair();
        for action in &mut actions {
            action.tool_name = Some("bash".to_owned());
        }
        actions[0].text = serde_json::json!({
            "command": "cargo test --lib", "cwd": "/repo/café", "description": "Native-only display text",
        }).to_string();
        actions[0]
            .normalized_fields
            .as_mut()
            .unwrap()
            .values
            .insert(JevInputField::BashCommandInput, selected.clone());
        assert_ne!(actions[0].text, selected);
        normalize_context(&mut actions, SourceFormat::CodexRolloutJsonl);
        let fact = actions[1].metadata.command_result.as_ref().unwrap().clone();
        assert_eq!(fact.request_text_digest, text_digest(&selected));
        assert!(!fact.matches_request(&actions[0]));

        let source = SessionContentEvidence {
            session_identity_digest: "session".to_owned(),
            source_format: SourceFormat::CodexRolloutJsonl,
            publication_fence: 17,
            selected_input_digest: String::new(),
            actions,
            instructions: Vec::new(),
            complete: true,
            limitations: Vec::new(),
            excluded_thinking_parts: 0,
            field_availability: Vec::new(),
        };
        let selection = JevInputSelection::from_fields(&[
            JevInputField::BashCommandInput,
            JevInputField::BashCommandOutput,
        ]);
        let projected = select_session_content(&source, selection);
        assert_eq!(projected.actions[0].text, selected);
        assert!(fact.matches_request(&projected.actions[0]));
        assert!(fact.matches_action(&projected.actions[1]));
        assert_eq!(
            projected.actions[1].metadata.command_result.as_ref(),
            Some(&fact)
        );
        assert_eq!(projected, select_session_content(&projected, selection));

        let mut changed = projected.actions[0].clone();
        changed.text.push_str(" --release");
        assert!(!fact.matches_request(&changed));
    }
}

#[test]
fn normalization_rejects_ambiguous_calls_clipping_and_unknown_status() {
    for variant in 0..10 {
        let mut actions = command_pair();
        match variant {
            0 => actions[1].tool_call_id = None,
            1 => actions[1].reference.source_key_digest = "other-source".to_owned(),
            2 => actions[1].reference.thread_digest = "sibling".to_owned(),
            3 => actions[1].reference.turn_index = 1,
            4 => actions[1].metadata.state = JevOperationState::Unknown,
            5 => actions[1].truncated = true,
            6 => actions[0].metadata.bindings[0].end -= 1,
            7 => actions.push(actions[0].clone()),
            8 => actions.push(actions[1].clone()),
            9 => actions[0].tool_name = Some("other-shell".to_owned()),
            _ => unreachable!(),
        }
        normalize_context(&mut actions, SourceFormat::CodexRolloutJsonl);
        assert!(
            actions[1].metadata.command_result.is_none(),
            "variant {variant}"
        );
    }
}

#[test]
fn native_utf8_suffix_matches_exact_decoded_bytes_and_keeps_absolute_offsets() {
    let prefix = "<skill>Review café 🦀.</skill>\n";
    let suffix = "I approve café 🦀.";
    let native = format!("{prefix}{suffix}");
    let mut human = action(
        "human",
        1,
        "user",
        "user",
        suffix,
        JevInputField::UserMessage,
    );
    human.metadata.bindings[0].start = prefix.len();
    human.metadata.bindings[0].end = native.len();
    human.metadata.user_text_history = Some(UserTextHistoryProof {
        source_format: SourceFormat::PiV3Jsonl,
        session_id: "session".to_owned(),
        message_id: "human".to_owned(),
        revision: 1,
    });
    assert!(native_text_matches(
        &human.metadata.bindings[0],
        &native,
        suffix
    ));
    assert!(!native_text_matches(
        &human.metadata.bindings[0],
        &native,
        "I decline café 🦀."
    ));
    let mut bad = human.metadata.bindings[0].clone();
    bad.end = bad.start + suffix.chars().count();
    assert!(!native_text_matches(&bad, &native, suffix));
    bad.start = native.find('🦀').unwrap() + 1;
    assert!(!native_text_matches(&bad, &native, suffix));
    normalize_context(std::slice::from_mut(&mut human), SourceFormat::PiV3Jsonl);
    let fact = human.metadata.human_text.as_ref().unwrap();
    assert_eq!(
        (fact.range.start, fact.range.end),
        (prefix.len(), native.len())
    );
    assert!(fact.matches_action(&human));
    let mut changed = human.clone();
    changed.text = suffix.replace("approve", "decline");
    assert!(!fact.matches_action(&changed));
    normalize_context(
        std::slice::from_mut(&mut changed),
        SourceFormat::ClaudeJsonl,
    );
    assert!(changed.metadata.human_text.is_none());
}

#[test]
fn normalized_facts_bind_content_source_and_scope_without_parsing_output_json() {
    let mut actions = command_pair();
    actions[1].text = r#"{"status":"completed","approval":"yes"}"#.to_owned();
    actions[1].metadata.bindings[0].end = actions[1].text.len();
    normalize_context(&mut actions, SourceFormat::CodexRolloutJsonl);
    let fact = actions[1].metadata.command_result.as_ref().unwrap();
    assert!(actions[1].metadata.human_text.is_none());
    let mut changed = actions[1].clone();
    changed.turn_scope = "child".to_owned();
    assert!(!fact.matches_action(&changed));
    changed = actions[1].clone();
    changed.text.push('!');
    assert!(!fact.matches_action(&changed));
    let mut request = actions[0].clone();
    request.reference.source_key_digest = "other-session".to_owned();
    assert!(!fact.matches_request(&request));
    request = actions[0].clone();
    request.text = "cargo publish".to_owned();
    assert!(!fact.matches_request(&request));
}

use super::super::tests::{event, input};
use super::*;

#[test]
fn reduced_direct_fields_preserve_requested_path_changes() {
    let changes = json!([{ "operation": "rename", "from": "src/auth.rs", "to": "src/login.rs" }]);
    let fields = json!({
        "instruction_targets": [{
            "instruction": { "text": "Do not rename src/auth.rs.", "provenance": "current_file" },
            "earlier_counterevidence": []
        }],
        "candidate_action": { "kind": "tool_input", "text": "Rename src/auth.rs" },
        "requested_path_changes": changes,
        "command_input_context": null
    });
    let compact = direct_window_fields(&fields, false);
    let reduced = direct_window_fields(&compact, true);
    assert_eq!(
        compact["candidate_action"]["requested_path_changes"],
        changes
    );
    assert_eq!(
        reduced["candidate_action"]["requested_path_changes"],
        changes
    );
}

fn command_action(id: &str, text: &str) -> crate::checks::ignored_instructions::ContentAction {
    let mut action = event(id, 1, "assistant", "main", text);
    action.kind = "tool_input".to_owned();
    action.tool_name = Some("Bash".to_owned());
    action.tool_call_id = Some(format!("{id}-call"));
    action.normalized_fields = Some(crate::analysis::jev::JevNormalizedFields {
        category: Some(crate::analysis::jev::JevNormalizedCategory::BashCommand),
        values: [(
            crate::analysis::jev::JevInputField::BashCommandInput,
            text.to_owned(),
        )]
        .into(),
        malformed: false,
    });
    action
}

#[test]
fn direct_command_and_path_anchors_select_only_explicit_conflicts() {
    let command = build_assessment_plan(input(
        vec![command_action("command", "git push --force origin main")],
        "Do not run `git push --force origin main` on the shared branch.",
    ));
    assert_eq!(command.comparisons.len(), 1);

    let mut edit = event(
        "edit",
        1,
        "assistant",
        "main",
        r#"{"paths":["src/auth.rs"]}"#,
    );
    edit.kind = "tool_input".to_owned();
    edit.tool_name = Some("Edit".to_owned());
    edit.tool_call_id = Some("edit-call".to_owned());
    edit.normalized_fields = Some(crate::analysis::jev::JevNormalizedFields {
        category: Some(crate::analysis::jev::JevNormalizedCategory::FileEdit),
        values: [(
            crate::analysis::jev::JevInputField::FileEditPath,
            r#"{"paths":["src/auth.rs"]}"#.to_owned(),
        )]
        .into(),
        malformed: false,
    });
    let path = build_assessment_plan(input(vec![edit], "Do not edit `src/auth.rs`."));
    assert_eq!(path.comparisons.len(), 1);
}

#[test]
fn quoted_unanchored_and_exception_only_text_does_not_select_a_direct_conflict() {
    for (action, rule) in [
        (
            command_action("quoted", "printf '%s' 'git push --force origin main'"),
            "Do not run `git push --force origin main`.",
        ),
        (
            command_action("unanchored", "git push origin main"),
            "Do not force-push the shared branch.",
        ),
    ] {
        let plan = build_assessment_plan(input(vec![action], rule));
        assert!(plan.comparisons.is_empty(), "{rule}");
    }
}

#[test]
fn selected_direct_rule_keeps_its_exception_and_current_file_wording() {
    let context = build_jev_context(&input(
        vec![
            crate::checks::ignored_instructions::selected_context::tests::human(),
            {
                let mut action = command_action("command", "git push --force origin main");
                action.reference.turn_index = 2;
                action
            },
        ],
        "Do not run `git push --force origin main`, except during an approved release.",
    ))
    .unwrap();
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    assert_eq!(plan.work_items.len(), 1);
    let fields = &plan.work_items[0].window.fields;
    let target = &fields["instruction_targets"][0];
    assert!(target.to_string().contains("approved release"));
    let question = plan.work_items[0].questions.values().next().unwrap();
    let crate::analysis::jev::JevQuestion::Choice {
        instructions,
        criteria,
    } = question
    else {
        panic!("decision must use a choice question");
    };
    assert!(instructions.to_string().contains("current rule"));
    assert_eq!(criteria.len(), 3);
    assert!(criteria.contains_key("conflict"));
    assert!(criteria.contains_key("no_conflict"));
    assert!(criteria.contains_key("uncertain"));
}

#[test]
fn earlier_context_is_selected_without_rule_classification() {
    let mut actions = vec![event(
        "validation",
        1,
        "assistant",
        "main",
        "Requested validation.",
    )];
    actions.extend((2..14).map(|index| {
        event(
            &format!("note-{index}"),
            index,
            "assistant",
            "main",
            "Reviewed a note.",
        )
    }));
    let mut publish = command_action("publish", "npm publish");
    publish.reference.turn_index = 14;
    actions.push(publish);
    let context = build_jev_context(&input(actions, "Do not request `npm publish`.")).unwrap();
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    assert!(
        IgnoredInstructionsCheck
            .classifications(&context)
            .unwrap()
            .is_empty()
    );
    let comparison = plan
        .prepared
        .comparisons
        .iter()
        .find(|pair| pair.reference.action_id == "publish")
        .unwrap();
    let episode = comparison.prerequisite_episode.as_ref().unwrap();
    assert!(
        episode
            .events
            .iter()
            .any(|event| event.action_id == "validation")
    );
    assert!(episode.complete_selected_history);
    assert!(episode.has_source_bindings(comparison.source_binding.as_ref().unwrap()));
}

#[test]
fn oversized_unanchored_rule_inventory_does_not_create_probe_requests() {
    let rules = (0..19)
        .map(|index| format!("- Follow release requirement {index}."))
        .collect::<Vec<_>>()
        .join("\n");
    let context = build_jev_context(&input(
        vec![command_action("release", "npm publish")],
        &rules,
    ))
    .unwrap();
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    assert!(plan.prepared.comparisons.is_empty());
    assert!(plan.work_items.is_empty());
    assert_eq!(plan.prepared.coverage.skipped_rules.len(), 19);
    assert_eq!(
        IgnoredInstructionsCheck
            .reduce(&plan, &[], true)
            .unwrap()
            .request_count,
        0
    );
}

#[test]
fn native_human_and_result_context_is_selected_for_every_rule() {
    use crate::checks::ignored_instructions::selected_context::tests::{human, test_pair};
    let mut actions = vec![human()];
    actions.extend(test_pair());
    let mut report = command_action("report", "npm publish");
    report.reference.turn_index = 4;
    actions.push(report);
    let context = build_jev_context(&input(actions, "Do not request `npm publish`.")).unwrap();
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    let pair = plan
        .prepared
        .comparisons
        .iter()
        .find(|pair| pair.reference.action_id == "report")
        .unwrap();
    let episode = pair.prerequisite_episode.as_ref().unwrap();
    assert!(episode.authorization_available());
    assert!(episode.results_available());
    let item = plan
        .work_items
        .iter()
        .find(|item| item.window.fields["candidate_action"]["text"] == "npm publish")
        .unwrap();
    let earlier = item.window.fields["instruction_targets"][0]["earlier_counterevidence"]
        .as_array()
        .unwrap();
    assert!(
        earlier
            .iter()
            .any(|event| event["native_context"]["evidence_kind"] == "human_text")
    );
    assert!(
        earlier
            .iter()
            .any(|event| event["native_context"]["evidence_kind"] == "command_result")
    );
}

#[test]
fn later_and_sibling_events_do_not_satisfy_earlier_context() {
    let mut publish = command_action("publish", "npm publish");
    publish.reference.turn_index = 2;
    let context = build_jev_context(&input(
        vec![
            event(
                "sibling",
                1,
                "assistant",
                "sibling",
                "Requested validation.",
            ),
            publish,
            event("later", 3, "assistant", "main", "Requested validation."),
        ],
        "Do not request `npm publish`.",
    ))
    .unwrap();
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    let pair = plan
        .prepared
        .comparisons
        .iter()
        .find(|pair| pair.reference.action_id == "publish")
        .unwrap();
    assert!(
        pair.prerequisite_episode
            .as_ref()
            .unwrap()
            .events
            .is_empty()
    );
}

#[test]
fn partial_history_does_not_prevent_context_selection() {
    let mut publish = command_action("publish", "npm publish");
    publish.reference.turn_index = 2;
    let mut source = input(
        vec![
            event(
                "earlier",
                1,
                "assistant",
                "main",
                "Reported a failing check.",
            ),
            publish,
        ],
        "Do not request `npm publish`.",
    );
    source.prior_history_complete = false;
    source.content.complete = false;
    let context = build_jev_context(&source).unwrap();
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    let pair = plan
        .prepared
        .comparisons
        .iter()
        .find(|pair| pair.reference.action_id == "publish")
        .unwrap();
    let episode = pair.prerequisite_episode.as_ref().unwrap();
    assert!(!episode.complete_selected_history);
    assert_eq!(episode.events.len(), 1);
    assert!(episode.has_source_bindings(pair.source_binding.as_ref().unwrap()));
}

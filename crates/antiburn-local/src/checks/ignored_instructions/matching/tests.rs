use super::super::tests::{event, input};
use super::*;

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
    actions.push(event(
        "publish",
        14,
        "assistant",
        "main",
        "Requested publication.",
    ));
    let context =
        build_jev_context(&input(actions, "Request validation before publication.")).unwrap();
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
fn rule_target_inventory_survives_eight_pair_window_budget() {
    let rules = (0..19)
        .map(|index| format!("- Follow release requirement {index}."))
        .collect::<Vec<_>>()
        .join("\n");
    let context = build_jev_context(&input(
        vec![event(
            "release",
            1,
            "assistant",
            "main",
            "Requested release.",
        )],
        &rules,
    ))
    .unwrap();
    let plan = IgnoredInstructionsCheck.prepare(&context).unwrap();
    assert_eq!(plan.prepared.coverage.candidate_pairs, 19);
    assert_eq!(plan.prepared.comparisons.len(), 19);
    assert!(plan.work_items.len() >= 3);
    assert!(plan.work_items.iter().all(|item| item.questions.len() <= 8));
    let ids = plan
        .work_items
        .iter()
        .flat_map(|item| item.questions.keys())
        .collect::<BTreeSet<_>>();
    assert_eq!(ids.len(), 19);
    for comparison in &plan.prepared.comparisons {
        assert!(ids.contains(&target_question_key(&comparison.id, QUESTION_DECISION)));
    }
}

#[test]
fn native_human_and_result_context_is_selected_for_every_rule() {
    use crate::checks::ignored_instructions::selected_context::tests::{human, test_pair};
    let mut actions = vec![human()];
    actions.extend(test_pair());
    actions.push(event(
        "report",
        4,
        "assistant",
        "main",
        "Reported the test outcome.",
    ));
    let context = build_jev_context(&input(actions, "Report results accurately.")).unwrap();
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
        .find(|item| item.window.fields["candidate_action"]["text"] == "Reported the test outcome.")
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
    let context = build_jev_context(&input(
        vec![
            event(
                "sibling",
                1,
                "assistant",
                "sibling",
                "Requested validation.",
            ),
            event("publish", 2, "assistant", "main", "Requested publication."),
            event("later", 3, "assistant", "main", "Requested validation."),
        ],
        "Request validation before publication.",
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
    let mut source = input(
        vec![
            event(
                "earlier",
                1,
                "assistant",
                "main",
                "Reported a failing check.",
            ),
            event("publish", 2, "assistant", "main", "Requested publication."),
        ],
        "Request validation before publication.",
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

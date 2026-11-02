use std::sync::Arc;

use antiburn_checks::compiler::ToolCategory;
use antiburn_checks::context::{CurrentInventories, ObservedAttribution, uniform_attribution};
use antiburn_checks::definition_lifecycle::EffectiveActivation;
use antiburn_checks::discovery::DefinitionScope;
use antiburn_checks::domain::{
    ActivationId, ActivationScope, Generation, NativeOrderKey, Observation, ObservationKind,
    ObservationPosition,
};
use antiburn_checks::expression_eval::{EvaluationBudget, EvaluationValue};
use antiburn_checks::filters::{
    AttributionScope, FilterField, FilterReason, RepositoryGlob, evaluate_attribution_filters,
    evaluate_path_filters,
};
use antiburn_checks::preparation::evaluate_decision_input_with_budget;
use antiburn_checks::selection::{BranchSelection, SelectionOutcome, SelectionState};
use antiburn_checks::source::{
    EvidenceValidity, PublicationCursor, PublicationRead, PublishedContext, PublishedEvidenceBatch,
    PublishedEvidenceChange, PublishedEvidenceEvent, PublishedEvidenceReader,
    PublishedEvidenceSource, PublishedResultStatus, PublishedToolMetadata, PublishedToolResult,
    SourceError, SourceEvent,
};
use antiburn_checks::{CompiledCheck, DiagnosticCode, compile};
use serde_json::json;

fn definition(filters: &str, input: Option<&str>) -> String {
    let decisions = input.map_or_else(String::new, |input| format!(
        "decisions:\n  review:\n    input:\n{input}\n    output:\n      verdict:\n        type: choice\n        question: Is this useful?\n        criteria: {{yes: Useful., no: Not useful.}}\n"
    ));
    format!(
        "name: Native context\non:\n  tool_result: {filters}\n{decisions}actions:\n  - type: status\n    input: {{result: failure}}\n"
    )
}

fn activation(check: CompiledCheck) -> EffectiveActivation {
    EffectiveActivation {
        id: ActivationId {
            scope: ActivationScope::default(),
            definition_id: "native-context".into(),
            generation: Generation(1),
        },
        revision: [1; 32],
        definition_scope: DefinitionScope::Global,
        source_paths: vec![],
        project_root: Some("/repo".into()),
        compiled: Arc::new(check),
        bootstrap_observation_position: ObservationPosition(0),
        bootstrap_publication: PublicationCursor(0),
    }
}

struct Reader(PublishedEvidenceEvent);

impl PublishedEvidenceReader for Reader {
    fn read_batch(&self, _: PublicationRead) -> Result<PublishedEvidenceBatch, SourceError> {
        Ok(PublishedEvidenceBatch {
            events: vec![self.0.clone()],
            next: self.0.cursor,
            has_more: false,
        })
    }
}

fn publication(
    metadata: PublishedToolMetadata,
    inventories: CurrentInventories,
) -> PublishedEvidenceEvent {
    PublishedEvidenceEvent {
        source_evidence_version: None,
        cursor: PublicationCursor(1),
        branch: "main".into(),
        context: Some(PublishedContext {
            session_selection: None,
            session_id: "session".into(),
            agent_name: "opencode".into(),
            requests: vec![],
            project_root: Some("/repo".into()),
            version: "context-1".into(),
            main_session: true,
            complete_through_event: false,
            inventories,
        }),
        change: PublishedEvidenceChange::Upsert(PublishedToolResult {
            identity: "call/result".into(),
            native_order: NativeOrderKey("native:1".into()),
            first_terminal_publication: Some(PublicationCursor(1)),
            revision: 1,
            tool_name: Some("unclassified-native-tool".into()),
            category: Some("other".into()),
            metadata: Box::new(metadata),
            arguments: None,
            content: Some("public output".into()),
            outcome: None,
            provider_executed: None,
            status: PublishedResultStatus::Completed,
            finality: true,
            payload_complete: false,
            validity: EvidenceValidity::Valid,
        }),
    }
}

fn observation(event: PublishedEvidenceEvent) -> Observation {
    let page = PublishedEvidenceSource::new(Reader(event), 1)
        .unwrap()
        .read_batch(PublicationCursor(0))
        .unwrap();
    let SourceEvent::Observation { observation, .. } = page.events.into_iter().next().unwrap()
    else {
        panic!("expected a safely admitted observation");
    };
    observation.commit(ObservationPosition(1))
}

fn inventory() -> CurrentInventories {
    serde_json::from_value(json!({
        "global": {
            "instructions_complete": false, "instructions_limits": ["managed-policy-unavailable"],
            "skills": [{"name": "review", "description": "", "path": "/config/review/SKILL.md"}],
            "skills_complete": false, "skills_limits": ["unsupported-entry"], "snapshot_revision": "global-1"
        },
        "repo": {
            "instructions": [{"path": "/repo/AGENTS.md", "content": "Run focused tests.",
                "scope": {"root": "/repo", "globs": ["src/**"]}, "observed_at": "2026-10-23T12:00:00Z"}],
            "instructions_complete": true, "instructions_limits": [], "skills": [],
            "skills_complete": true, "skills_limits": [], "snapshot_revision": "repo-1"
        }
    })).unwrap()
}

#[test]
fn whole_segment_globs_match_entire_case_sensitive_relative_paths() {
    for (pattern, path, expected) in [
        ("**", "a", true),
        ("**", "a/b", true),
        ("**/file.rs", "file.rs", true),
        ("**/file.rs", "src/file.rs", true),
        ("src/**/file.rs", "src/file.rs", true),
        ("src/**/file.rs", "src/a/b/file.rs", true),
        ("src/**", "src", true),
        ("src/**", "src/a/b", true),
        ("src/*", "src/a", true),
        ("src/*", "src/a/b", false),
        ("src/*.rs", "src/a.rs", true),
        ("src/*.rs", "src/a.rs.bak", false),
        ("src/?", "src/é", true),
        ("src/?", "src/ab", false),
        ("*.rs", "a.rs", true),
        ("*.rs", "src/a.rs", false),
        ("A.rs", "a.rs", false),
        ("a(b)!+.rs", "a(b)!+.rs", true),
        ("**/a/**/b", "a/b", true),
        ("**/a/**/b", "x/a/y/z/b", true),
        ("**/a/**/b", "a/b/c", false),
        ("**/**/a", "a", true),
        ("**", "./a", false),
        ("**", "/a", false),
        ("**", "a/../b", false),
        ("**", "C:/a", false),
        ("**", "a\\b", false),
        ("**", "", false),
    ] {
        assert_eq!(
            RepositoryGlob::parse(pattern).unwrap().matches(path),
            expected,
            "{pattern} against {path}"
        );
    }
}

#[test]
fn rejects_invalid_filters_before_activation_even_behind_false_conditions() {
    for filters in [
        "{models: []}",
        "{models: [p/m, p/m]}",
        "{models: [model]}",
        "{models: ['/m']}",
        "{models: ['p/']}",
        "{models: ['p//m']}",
        "{models: ['p/m name']}",
        "{models: [false]}",
        "{fast_mode: 'false'}",
        "{fast_mode: null}",
        "{fast_mode: '${{ false }}'}",
        "{paths: []}",
        "{paths: [a, a]}",
        "{paths: ['/a']}",
        "{paths: ['./a']}",
        "{paths: ['a/../b']}",
        "{paths: ['C:/a']}",
        "{paths: ['a\\b']}",
        "{paths: ['a//b']}",
        "{paths: ['a/**b']}",
        "{paths: ['a/[bc]']}",
        "{paths: ['a/{b,c}']}",
        "{paths: ['@(a)']}",
        "{paths-ignore: [false]}",
    ] {
        let source = definition(filters, None).replace("actions:", "if: false\nactions:");
        let errors = compile(&source).unwrap_err();
        assert!(
            errors.iter().all(|error| error.span.is_some()
                && error
                    .config_path
                    .as_deref()
                    .is_some_and(|path| path.starts_with("on.tool_result."))),
            "{filters}: {errors:?}"
        );
    }
    let check = compile(&definition("{models: [Provider/namespace/model], fast_mode: false, paths: ['src/**/?.rs'], paths-ignore: ['**/generated/**']}", None)).unwrap();
    assert_eq!(check.trigger.models.unwrap(), ["Provider/namespace/model"]);
    assert_eq!(check.trigger.fast_mode, Some(false));
}

#[test]
fn model_filters_reject_interpolation_at_the_authored_entry() {
    for event in [
        "user_message",
        "assistant_message",
        "tool_call",
        "tool_result",
        "turn_end",
        "session_start",
        "session_idle",
    ] {
        for model in [
            "provider/${{session.model}}",
            "${{session.model}}/model",
            "provider/${{session.model",
        ] {
            let source = format!(
                "name: Fixed model filter\non:\n  {event}:\n    models:\n      - provider/model\n      - '{model}'\nif: false\nactions: [{{type: status, input: {{result: failure}}}}]\n"
            );
            let errors = compile(&source).unwrap_err();
            assert_eq!(errors.len(), 1, "{event}, {model}: {errors:?}");
            let error = &errors[0];
            assert_eq!(error.code, DiagnosticCode::ConfigInvalidValue);
            assert_eq!(
                error.config_path.as_deref(),
                Some(format!("on.{event}.models").as_str())
            );
            let span = error.span.unwrap();
            assert_eq!(span.line, 6);
            assert_eq!(span.column, 9);
        }
    }
}

#[test]
fn recorded_model_identifiers_do_not_apply_authored_interpolation_rules() {
    assert!(antiburn_checks::filters::is_model_identifier(
        "provider/${{session.model}}"
    ));
}

#[test]
fn common_filters_never_fall_back_to_other_scopes_or_current_settings() {
    let models = vec!["p/m".into()];
    for scope in [
        AttributionScope::Message,
        AttributionScope::Turn,
        AttributionScope::Session,
    ] {
        for value in [json!({}), json!({"model": null}), json!({"model": "P/m"})] {
            assert!(evaluate_attribution_filters(Some(&models), None, scope, &value).is_err());
        }
        for expected in [true, false] {
            assert_eq!(
                evaluate_attribution_filters(None, Some(expected), scope, &json!({}))
                    .unwrap_err()
                    .reason,
                FilterReason::Unavailable
            );
            assert!(
                evaluate_attribution_filters(
                    None,
                    Some(expected),
                    scope,
                    &json!({"fast_mode": expected})
                )
                .is_ok()
            );
            assert!(
                evaluate_attribution_filters(
                    None,
                    Some(expected),
                    scope,
                    &json!({"fast_mode": !expected})
                )
                .is_err()
            );
        }
    }
    let activation =
        activation(compile(&definition("{models: [p/m], fast_mode: false}", None)).unwrap());
    let mut observation = observation(publication(
        PublishedToolMetadata::default(),
        CurrentInventories::default(),
    ));
    observation.context.value["session"]["model"] = json!("p/m");
    observation.context.value["session"]["fast_mode"] = json!(false);
    let mut state = SelectionState::default();
    let SelectionOutcome::FilterRejected(diagnostic) =
        state.apply_published(&activation, "call/result", 1, &observation)
    else {
        panic!("missing requester attribution must reject");
    };
    assert_eq!(
        (diagnostic.scope, diagnostic.field, diagnostic.reason),
        (
            AttributionScope::Message,
            FilterField::Model,
            FilterReason::Unavailable
        )
    );
    let branch = &state.branches[&observation.branch];
    assert!(branch.results.is_empty());
    let restored: BranchSelection =
        serde_json::from_str(&serde_json::to_string(branch).unwrap()).unwrap();
    assert_eq!(restored.last_filter_diagnostic, Some(diagnostic));
}

#[test]
fn path_filters_distinguish_missing_partial_empty_and_vetoed_sets() {
    let includes = vec![RepositoryGlob::parse("src/**").unwrap()];
    let ignores = vec![RepositoryGlob::parse("**/generated/**").unwrap()];
    let message = |paths, complete| json!({"tool": {"paths": paths}, "quality": {"paths_complete": complete, "paths_limits": ["unsupported-operation"]}});
    for (value, expected) in [
        (json!({}), FilterReason::Unavailable),
        (message(json!([]), false), FilterReason::Incomplete),
        (message(json!([]), true), FilterReason::KnownEmpty),
        (message(json!(["docs/a"]), true), FilterReason::NoMatch),
        (message(json!(["/outside"]), true), FilterReason::Invalid),
    ] {
        assert_eq!(
            evaluate_path_filters(Some(&includes), None, &value)
                .unwrap_err()
                .reason,
            expected
        );
    }
    assert!(
        evaluate_path_filters(Some(&includes), None, &message(json!(["src/a"]), false)).is_ok()
    );
    let error = evaluate_path_filters(
        Some(&includes),
        Some(&ignores),
        &message(json!(["src/a"]), false),
    )
    .unwrap_err();
    assert_eq!(error.reason, FilterReason::Incomplete);
    assert_eq!(error.limits, ["unsupported-operation"]);
    for complete in [true, false] {
        assert_eq!(
            evaluate_path_filters(
                Some(&includes),
                Some(&ignores),
                &message(json!(["src/a", "src/generated/b"]), complete)
            )
            .unwrap_err()
            .reason,
            FilterReason::IgnoredPath
        );
    }
    assert!(evaluate_path_filters(None, Some(&ignores), &message(json!([]), true)).is_ok());
    assert!(
        evaluate_path_filters(
            Some(&includes),
            Some(&ignores),
            &message(json!(["src/a"]), true)
        )
        .is_ok()
    );
    assert!(evaluate_path_filters(None, None, &json!({})).is_ok());
}

#[test]
fn reviewed_categories_and_recorded_metadata_flow_through_publication_and_selection() {
    for category in [
        ToolCategory::Shell,
        ToolCategory::Read,
        ToolCategory::Search,
        ToolCategory::Edit,
        ToolCategory::Write,
        ToolCategory::Delegate,
        ToolCategory::Skill,
        ToolCategory::Mcp,
        ToolCategory::Other,
    ] {
        let metadata = PublishedToolMetadata {
            attribution: ObservedAttribution {
                model: Some("p/m".into()),
                fast_mode: Some(false),
            },
            check_category: Some(category),
            paths: Some(vec!["src/a.rs".into()]),
            paths_complete: true,
            ..Default::default()
        };
        let activation = activation(compile(&definition(&format!("{{tools: [{}], models: [p/m], fast_mode: false, paths: ['src/**'], paths-ignore: ['**/generated/**']}}", category.as_str()), None)).unwrap());
        let observation = observation(publication(metadata, inventory()));
        let SelectionOutcome::Candidate(candidate) =
            SelectionState::default().apply(&activation, &observation)
        else {
            panic!("reviewed category must select: {category:?}");
        };
        assert_eq!(
            candidate.messages[0].value["tool"]["category"],
            category.as_str()
        );
        assert_eq!(candidate.messages[0].value["model"], "p/m");
        assert_eq!(candidate.messages[0].value["fast_mode"], false);
        assert_eq!(
            candidate.context.value["repo"]["instructions"][0]["scope"]["globs"],
            json!(["src/**"])
        );
    }
    for category in ["write", "delegate", "skill", "mcp"] {
        let activation =
            activation(compile(&definition(&format!("{{tools: [{category}]}}"), None)).unwrap());
        let mut event = publication(
            PublishedToolMetadata::default(),
            CurrentInventories::default(),
        );
        let PublishedEvidenceChange::Upsert(result) = &mut event.change else {
            unreachable!()
        };
        result.tool_name = Some(category.into());
        assert_eq!(
            SelectionState::default().apply(&activation, &observation(event)),
            SelectionOutcome::Filtered
        );
    }
}

#[test]
fn known_partial_inventory_entries_remain_typed_and_frozen() {
    let source = definition(
        "{}",
        Some(
            r#"      rules:
        map: "${{ repo.instructions }}"
        as: rule
        value:
          path: "${{ rule.path }}"
          text: "${{ rule.content }}"
          scope: "${{ rule['scope'].globs }}"
          observed: "${{ rule.observed_at }}"
      descriptions: "${{ global.skills.*.description }}"
      complete: "${{ global.skills_complete }}"
      limits: "${{ global.skills_limits }}"
      revision: "${{ repo.snapshot_revision }}"
      empty: "${{ repo.skills }}""#,
        ),
    );
    let activation = activation(compile(&source).unwrap());
    let mut event = publication(PublishedToolMetadata::default(), inventory());
    let original = observation(event.clone());
    let SelectionOutcome::Candidate(candidate) =
        SelectionState::default().apply(&activation, &original)
    else {
        panic!("partial inventory must not block unrelated usable evidence");
    };
    let prepared = evaluate_decision_input_with_budget(
        &activation.compiled.decisions[0],
        &candidate.context.value,
        &mut EvaluationBudget::default(),
    )
    .unwrap();
    assert_eq!(
        prepared.value,
        EvaluationValue::Available(json!({
            "rules": [{"path": "/repo/AGENTS.md", "text": "Run focused tests.", "scope": ["src/**"], "observed": "2026-10-23T12:00:00Z"}],
            "descriptions": [""], "complete": false, "limits": ["unsupported-entry"], "revision": "repo-1", "empty": []
        }))
    );
    assert!(
        candidate.context.value["global"]
            .get("instructions")
            .is_none()
    );
    let context = event.context.as_mut().unwrap();
    context.inventories.repo.instructions.as_mut().unwrap()[0].content = "New current rule.".into();
    context.inventories.repo.snapshot_revision = Some("repo-2".into());
    let changed = observation(event);
    assert_ne!(original.version, changed.version);
    assert_eq!(
        candidate.context.value["repo"]["instructions"][0]["content"],
        "Run focused tests."
    );
    let selected_unavailable = compile(&definition(
        "{}",
        Some("      rules: \"${{ global.instructions }}\""),
    ))
    .unwrap();
    assert_eq!(
        evaluate_decision_input_with_budget(
            &selected_unavailable.decisions[0],
            &candidate.context.value,
            &mut EvaluationBudget::default()
        )
        .unwrap()
        .value,
        EvaluationValue::Unavailable
    );
}

#[test]
fn new_closed_records_reject_misspellings_and_unimplemented_roots_in_all_access_forms() {
    for expression in [
        "global.instructions[0].body",
        "repo.skills[0]['content']",
        "repo.instructions[0].scope.invalid",
        "global.skills_complete.invalid",
        "global['missing']",
        "agent.skills",
        "turn.messages",
        "session.last_activity_period",
    ] {
        assert!(
            compile(&definition(
                "{}",
                Some(&format!("      value: \"${{{{ {expression} }}}}\""))
            ))
            .is_err(),
            "{expression}"
        );
    }
    for event in [
        "user_message",
        "assistant_message",
        "tool_call",
        "turn_end",
        "session_start",
        "session_idle",
    ] {
        let compiled = compile(&definition("{}", None).replace("tool_result", event)).unwrap();
        assert!(match compiled.trigger.family {
            antiburn_checks::compiler::TriggerFamily::Messages { .. } =>
                matches!(event, "user_message" | "assistant_message" | "tool_call"),
            antiburn_checks::compiler::TriggerFamily::ResponseCycle(_) => event == "turn_end",
            antiburn_checks::compiler::TriggerFamily::Session { .. } =>
                matches!(event, "session_start" | "session_idle"),
        });
    }
    assert!(compile(&definition("{}", None).replace("type: status", "type: message")).is_err());
}

#[test]
fn uniform_attribution_requires_complete_nonempty_evidence_and_independent_uniform_facts() {
    let first = ObservedAttribution {
        model: Some("p/m".into()),
        fast_mode: Some(true),
    };
    assert_eq!(
        uniform_attribution(&[], true),
        ObservedAttribution::default()
    );
    assert_eq!(
        uniform_attribution(std::slice::from_ref(&first), false),
        ObservedAttribution::default()
    );
    assert_eq!(
        uniform_attribution(std::slice::from_ref(&first), true),
        first
    );
    let changed_mode = ObservedAttribution {
        fast_mode: Some(false),
        ..first.clone()
    };
    assert_eq!(
        uniform_attribution(&[first.clone(), changed_mode], true),
        ObservedAttribution {
            model: first.model.clone(),
            fast_mode: None
        }
    );
    let changed_model = ObservedAttribution {
        model: Some("p/other".into()),
        ..first.clone()
    };
    assert_eq!(
        uniform_attribution(&[first.clone(), changed_model], true),
        ObservedAttribution {
            model: None,
            fast_mode: Some(true)
        }
    );
    assert_eq!(
        uniform_attribution(&[first, ObservedAttribution::default()], true),
        ObservedAttribution::default()
    );
}

#[test]
fn optional_publication_facts_stay_absent_instead_of_becoming_false_or_empty() {
    let observation = observation(publication(
        PublishedToolMetadata {
            model_limit: Some("request-model-unavailable".into()),
            fast_mode_limit: Some("effective-tier-unavailable".into()),
            ..Default::default()
        },
        CurrentInventories::default(),
    ));
    let ObservationKind::ToolResult { message, .. } = observation.kind else {
        unreachable!()
    };
    for field in ["model", "fast_mode"] {
        assert!(message.value.get(field).is_none());
    }
    assert!(message.value["tool"].get("paths").is_none());
    assert_eq!(message.value["quality"]["paths_available"], false);
    assert_eq!(
        message.value["quality"]["model_limit"],
        "request-model-unavailable"
    );
    let error =
        evaluate_attribution_filters(None, Some(false), AttributionScope::Message, &message.value)
            .unwrap_err();
    assert_eq!(error.limits, ["effective-tier-unavailable"]);
}

#[test]
fn path_work_exhaustion_cannot_turn_an_unchecked_veto_into_a_match() {
    let path = "a".repeat(1_000);
    let includes = vec![RepositoryGlob::parse(&path).unwrap()];
    let ignores = vec![RepositoryGlob::parse(&"b".repeat(1_000)).unwrap()];
    let message = json!({"tool": {"paths": [path]}, "quality": {"paths_complete": true}});
    assert!(evaluate_path_filters(Some(&includes), None, &message).is_ok());
    let diagnostic = evaluate_path_filters(Some(&includes), Some(&ignores), &message).unwrap_err();
    assert_eq!(
        (diagnostic.field, diagnostic.reason),
        (FilterField::Paths, FilterReason::LimitExceeded)
    );
    let oversized_ignore = vec![RepositoryGlob::parse(&"b".repeat(1_001)).unwrap()];
    let diagnostic = evaluate_path_filters(None, Some(&oversized_ignore), &message).unwrap_err();
    assert_eq!(
        (diagnostic.field, diagnostic.reason),
        (FilterField::PathsIgnore, FilterReason::LimitExceeded)
    );
}

#[test]
fn unknown_project_cannot_promote_a_repo_inventory_into_known_empty_or_known_entries() {
    let mut event = publication(PublishedToolMetadata::default(), inventory());
    event.context.as_mut().unwrap().project_root = None;
    let observation = observation(event);
    let repo = &observation.context.value["repo"];
    assert!(repo.get("instructions").is_none());
    assert!(repo.get("skills").is_none());
    assert_eq!(repo["instructions_complete"], false);
    assert_eq!(
        repo["skills_limits"],
        json!(["repository-root-unavailable"])
    );
    assert_eq!(
        observation.context.value["global"]["skills"][0]["name"],
        "review"
    );
}

#[test]
fn filters_precede_window_counting_and_corrections_never_create_another_trigger() {
    let source = definition("{models: [p/m], fast_mode: false, paths: ['src/**']}", None)
        .replace("actions:", "  window: {size: 2, min: 2}\nactions:");
    let activation = activation(compile(&source).unwrap());
    let result = |position, mode| {
        let mut event = publication(
            PublishedToolMetadata {
                attribution: ObservedAttribution {
                    model: Some("p/m".into()),
                    fast_mode: mode,
                },
                paths: Some(vec!["src/a.rs".into()]),
                paths_complete: true,
                ..Default::default()
            },
            CurrentInventories::default(),
        );
        let PublishedEvidenceChange::Upsert(value) = &mut event.change else {
            unreachable!()
        };
        value.identity = format!("call-{position}/result");
        value.native_order = NativeOrderKey(format!("native:{position}"));
        let mut observed = observation(event);
        observed.position = ObservationPosition(position);
        observed
    };
    let mut state = SelectionState::default();
    let missing = result(1, None);
    assert!(matches!(
        state.apply_published(&activation, "call-1/result", 1, &missing),
        SelectionOutcome::FilterRejected(_)
    ));
    let selected = result(2, Some(false));
    assert_eq!(
        state.apply_published(&activation, "call-2/result", 1, &selected),
        SelectionOutcome::Updated
    );
    let trigger = result(3, Some(false));
    let SelectionOutcome::Candidate(candidate) =
        state.apply_published(&activation, "call-3/result", 1, &trigger)
    else {
        panic!("only the two matching results must count");
    };
    assert_eq!(
        candidate
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["call-2/result", "call-3/result"]
    );
    let mut correction = result(2, None);
    correction.position = ObservationPosition(4);
    assert!(matches!(
        state.apply_published(&activation, "call-2/result", 2, &correction),
        SelectionOutcome::FilterRejected(_)
    ));
    assert_eq!(state.branches[&correction.branch].results.len(), 1);
    assert_eq!(
        state.branches[&correction.branch]
            .last_filter_diagnostic
            .as_ref()
            .unwrap()
            .field,
        FilterField::FastMode
    );
    assert_eq!(candidate.messages[0].value["fast_mode"], false);
    let corrected = result(2, Some(false));
    assert_eq!(
        state.apply_published(&activation, "call-2/result", 3, &corrected),
        SelectionOutcome::WindowInvalidated
    );
    let false_activation = activation_with_false_condition(&source);
    assert!(matches!(
        SelectionState::default().apply(&false_activation, &missing),
        SelectionOutcome::FilterRejected(_)
    ));
}

fn activation_with_false_condition(source: &str) -> EffectiveActivation {
    activation(compile(&source.replace("actions:", "if: false\nactions:")).unwrap())
}

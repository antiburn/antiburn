use super::*;
use antiburn_local::insights::{BadgeId, BadgeStatus, ReportCatalogs, session_badges};
use serde_json::json;

fn fixture(
    tool: &str,
    parent_model: &str,
    child_model: &str,
) -> (tempfile::TempDir, Vec<SessionInput>) {
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("parent.jsonl");
    let child_dir = directory.path().join("parent/subagents");
    std::fs::create_dir_all(&child_dir).unwrap();
    let child = child_dir.join("agent-worker.jsonl");
    let record = json!({
        "type": "assistant", "timestamp": "2026-09-01T10:00:00Z",
        "message": {"id": "parent-message", "role": "assistant", "model": parent_model,
            "usage": {"input_tokens": 2, "output_tokens": 3},
            "content": [{"type": "tool_use", "id": "call-worker", "name": tool,
                "input": {"model": "haiku", "description": "Synthetic task"}}]}
    });
    std::fs::write(&parent, record.to_string() + "\n").unwrap();
    std::fs::write(
        &child,
        claude_record_with("worker", 1_788_256_801, child_model, Some("fast")),
    )
    .unwrap();
    std::fs::write(
        child.with_extension("meta.json"),
        r#"{"toolUseId":"call-worker"}"#,
    )
    .unwrap();
    (
        directory,
        vec![
            file_input(&parent, "parent"),
            file_input(&child, "agent-worker"),
        ],
    )
}

fn read(inputs: &[SessionInput], store: Arc<dyn TurnRowStore>) -> StreamedSession {
    let StreamOutcome::Published { session, .. } =
        stream_vendor_with_hooks(inputs, &|| false, &no_after_claim, None, Some(store))
    else {
        panic!("synthetic files must publish")
    };
    *session
}

fn status(evidence: &SessionEvidence) -> BadgeStatus {
    session_badges(evidence, &ReportCatalogs::default())
        .into_iter()
        .find(|badge| badge.id == BadgeId::OverpoweredSubagents)
        .unwrap()
        .status
}

#[test]
fn claude_native_task_and_agent_join_both_models_and_preserve_child_routes_on_replay() {
    for tool in ["Task", "Agent"] {
        let (_directory, inputs) = fixture(tool, "claude-opus-4-6", "claude-opus-4-6");
        let store = MemoryTurnRowStore::new("claude", "parent");
        let pass = read(&inputs, store.clone());
        assert_eq!(pass.parent.billable_input_tokens, 2);
        assert_eq!(pass.merged.billable_input_tokens, 4);
        assert_eq!(pass.merged.billable_output_tokens, 6);
        assert_eq!(pass.subagents.len(), 1);
        let evidence = pass.evidence.unwrap();
        let subagents = observed(&evidence.subagents);
        assert_eq!(subagents.spawn_count, 1);
        assert_eq!(subagents.delegated_turns, 1);
        assert_eq!(
            subagents.children[0].parent_call_id.as_deref(),
            Some("call-worker")
        );
        assert_eq!(
            subagents.children[0].parent_model.as_deref(),
            Some("claude-opus-4-6")
        );
        assert_eq!(
            subagents.children[0].observed_child_models,
            BTreeSet::from(["claude-opus-4-6".to_owned()])
        );
        assert_eq!(status(&evidence), BadgeStatus::Finding);
        store.with_connection(|connection| {
            let route: (String, Option<String>, Option<String>, String) = connection.query_row(
                "SELECT scope, provider, api, model FROM turn WHERE source_key = 'agent-worker'",
                [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            ).unwrap();
            assert_eq!(
                route,
                (
                    "delegated".to_owned(),
                    None,
                    None,
                    "claude-opus-4-6".to_owned()
                )
            );
        });
        let record = store.query_coverage_record().unwrap().unwrap();
        let decoded = serde_json::from_str(&serde_json::to_string(&record).unwrap()).unwrap();
        let replayed = evidence_from_facts(&store.query_turn_facts().unwrap(), &decoded);
        assert_eq!(replayed, evidence);
        for outcome in pass.source_outcomes {
            let resume = outcome.resume.unwrap();
            StreamSnapshot::decode(&resume.snapshot)
                .expect("native call evidence must round-trip in a resume snapshot");
            store.write_resume(&outcome.source_key, resume).unwrap();
        }
        let resumed = read(&inputs, store.clone());
        assert!(
            resumed
                .source_outcomes
                .iter()
                .all(|outcome| outcome.mode == SourcePublishMode::Resumed)
        );
        assert_eq!(resumed.evidence.unwrap(), evidence);
        // A new sidecar claim must not reuse the previous pass's joined models.
        let RawSource::File(child) = &inputs[1].source else {
            unreachable!()
        };
        std::fs::write(
            child.with_extension("meta.json"),
            r#"{"toolUseId":"wrong-call"}"#,
        )
        .unwrap();
        let changed = read(&inputs, store.clone());
        let evidence = changed.evidence.unwrap();
        assert!(
            observed(&evidence.subagents).children[0]
                .observed_child_models
                .is_empty()
        );
        assert!(matches!(status(&evidence), BadgeStatus::NotAssessed(_)));
    }
}

#[test]
fn claude_unproved_sidecar_claims_remain_partial_without_losing_metrics() {
    for (tool, meta, duplicate) in [
        ("Task", None, false),
        ("Task", Some("{"), false),
        ("Task", Some(r#"{"toolUseId":"wrong-call"}"#), false),
        ("Task", Some(r#"{"toolUseId":""}"#), false),
        ("Read", Some(r#"{"toolUseId":"call-worker"}"#), false),
        (
            "Task",
            Some(r#"{"toolUseId":"call-worker","parentAgentId":"other"}"#),
            false,
        ),
        ("Task", Some(r#"{"toolUseId":"call-worker"}"#), true),
    ] {
        let (_directory, mut inputs) = fixture(tool, "claude-opus-4-6", "claude-opus-4-6");
        let RawSource::File(child) = &inputs[1].source else {
            unreachable!()
        };
        match meta {
            Some(meta) => std::fs::write(child.with_extension("meta.json"), meta).unwrap(),
            None => std::fs::remove_file(child.with_extension("meta.json")).unwrap(),
        }
        if duplicate {
            let second = child.with_file_name("agent-second.jsonl");
            std::fs::write(
                &second,
                claude_record_with("second", 1_788_256_802, "claude-opus-4-6", None),
            )
            .unwrap();
            std::fs::write(second.with_extension("meta.json"), meta.unwrap()).unwrap();
            inputs.push(file_input(&second, "agent-second"));
        }
        let pass = read(&inputs, turn_row_store("claude", "parent"));
        assert_eq!(
            pass.merged.billable_input_tokens,
            if duplicate { 6 } else { 4 }
        );
        let evidence = pass.evidence.unwrap();
        assert!(
            matches!(
                evidence.subagents,
                EvidenceValue::Partial {
                    reason: CoverageReason::AttributionIncomplete,
                    ..
                }
            ),
            "{tool} {meta:?}"
        );
        let subagents = observed(&evidence.subagents);
        assert_eq!(subagents.spawn_count, u64::from(tool == "Task"));
        assert!(
            subagents
                .children
                .iter()
                .all(|child| child.observed_child_models.is_empty())
        );
        assert!(
            matches!(status(&evidence), BadgeStatus::NotAssessed(_)),
            "{tool} {meta:?}"
        );
    }
}

#[test]
fn claude_sidecar_correlation_uses_the_call_model_not_the_dominant_model_or_requested_alias() {
    let (_directory, inputs) = fixture("Task", "claude-sonnet-4-6", "claude-opus-4-6");
    let RawSource::File(parent) = &inputs[0].source else {
        unreachable!()
    };
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(parent)
        .unwrap();
    for index in 0..3 {
        file.write_all(
            claude_record_with(
                &format!("main-{index}"),
                1_788_256_803 + index,
                "claude-opus-4-6",
                None,
            )
            .as_bytes(),
        )
        .unwrap();
    }
    let pass = read(&inputs, turn_row_store("claude", "parent"));
    let evidence = pass.evidence.unwrap();
    assert_eq!(
        observed(&evidence.models).dominant_main_model.as_deref(),
        Some("claude-opus-4-6")
    );
    assert_ne!(status(&evidence), BadgeStatus::Finding);
    assert_eq!(
        observed(&evidence.subagents).children[0]
            .parent_model
            .as_deref(),
        Some("claude-sonnet-4-6")
    );
}

#[test]
fn claude_sidecars_do_not_cross_pair_models_when_roster_order_differs_from_call_order() {
    let (_directory, mut inputs) = fixture("Task", "claude-opus-4-6", "claude-sonnet-4-6");
    let RawSource::File(parent) = &inputs[0].source else {
        unreachable!()
    };
    let second_call = json!({
        "type": "assistant", "timestamp": "2026-09-01T10:00:01Z",
        "message": {"id": "second-call", "role": "assistant", "model": "claude-sonnet-4-6",
            "usage": {"input_tokens": 2, "output_tokens": 1},
            "content": [{"type": "tool_use", "id": "call-second", "name": "Agent", "input": {}}]}
    });
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(parent)
        .unwrap()
        .write_all((second_call.to_string() + "\n").as_bytes())
        .unwrap();
    let RawSource::File(child) = &inputs[1].source else {
        unreachable!()
    };
    let second = child.with_file_name("agent-second.jsonl");
    std::fs::write(
        &second,
        claude_record_with("second-child", 1_788_256_802, "claude-opus-4-6", None),
    )
    .unwrap();
    std::fs::write(
        second.with_extension("meta.json"),
        r#"{"toolUseId":"call-second"}"#,
    )
    .unwrap();
    inputs.insert(1, file_input(&second, "agent-second"));
    let pass = read(&inputs, turn_row_store("claude", "parent"));
    let evidence = pass.evidence.unwrap();
    assert_eq!(
        observed(&evidence.models).dominant_main_model.as_deref(),
        Some("claude-opus-4-6")
    );
    let children = observed(&evidence.subagents).children;
    assert_eq!(children[0].parent_model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(
        children[0].observed_child_models,
        BTreeSet::from(["claude-sonnet-4-6".to_owned()])
    );
    assert_eq!(
        children[1].parent_model.as_deref(),
        Some("claude-sonnet-4-6")
    );
    assert_eq!(
        children[1].observed_child_models,
        BTreeSet::from(["claude-opus-4-6".to_owned()])
    );
    assert_ne!(status(&evidence), BadgeStatus::Finding);
}

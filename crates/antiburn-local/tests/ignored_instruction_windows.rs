use std::collections::BTreeMap;
use std::sync::Arc;

use antiburn_local::analysis::ignored_instructions::{
    AssessmentInput, IgnoredInstructionsCheck, InstructionProvenance, InstructionScope,
    build_jev_context, prepare_session_content, snapshot_from_text,
};
use antiburn_local::analysis::jev::{JevCheck, JevInputField};
use antiburn_local::analysis::jev_evidence::{
    JevNativeFieldContainer, JevNativeFieldRange, UserTextHistoryProof, normalize_context,
};
use antiburn_local::analysis::{
    CompositeSink, EvidenceSource, FenceScope, MemoryTurnRowStore, RawSource,
    SessionEvidenceAccumulator, SessionInput, SessionMetricsAccumulator, SourceCapabilities,
    SourceFormat, SourceKind, TurnRowSink, TurnRowStore, TurnSessionKey, query_turn_content,
    reader_for,
};
use serde_json::{Value, json};

fn request_windows(rule: &str, task: &str, command: &str) -> Vec<Value> {
    let session = "instruction-window-context";
    let transcript = [
        json!({"type": "user", "uuid": "user-task", "message": {"role": "user", "content": [{"type": "text", "text": task}]}}),
        json!({"type": "assistant", "uuid": "command-request", "parentUuid": "user-task", "message": {
            "role": "assistant", "content": [{"type": "tool_use", "id": "command-call", "name": "Bash", "input": {"command": command}}]
        }}),
    ].into_iter().map(|record| record.to_string()).collect::<Vec<_>>().join("\n");
    let source = SessionInput {
        agent: "claude".to_owned(),
        session_id: session.to_owned(),
        source: RawSource::Jsonl(transcript),
        source_format: SourceFormat::ClaudeJsonl,
        fork_parent_session_id: None,
    };
    let store = MemoryTurnRowStore::new("claude", session);
    let mut sink = CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new("claude", session),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session.to_owned(),
            kind: SourceKind::from(&source.source),
            capabilities: SourceCapabilities::claude(),
        }),
        TurnRowSink::new(
            Arc::clone(&store) as Arc<dyn TurnRowStore>,
            session.to_owned(),
            None,
        ),
    );
    let outcome = reader_for("claude").visit(&source, &mut sink).unwrap();
    sink.observe_source_outcome(outcome);
    assert!(!sink.turn_row_write_failed());
    let parts = store.with_connection(|connection| {
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
    let instruction = snapshot_from_text(
        "AGENTS.md",
        rule.to_owned(),
        InstructionProvenance::RecordedInjection,
        InstructionScope::Project,
    )
    .unwrap();
    let mut content =
        prepare_session_content(session, SourceFormat::ClaudeJsonl, parts, vec![instruction]);
    for action in &mut content.actions {
        if action.turn_role == "user" && matches!(action.kind.as_str(), "user" | "user_text") {
            // Bind the complete synthetic user record to its native field.
            action.metadata.user_text_history = Some(UserTextHistoryProof {
                source_format: SourceFormat::ClaudeJsonl,
                session_id: session.to_owned(),
                message_id: "user-task".to_owned(),
                revision: 1,
            });
            action.metadata.bindings = vec![JevNativeFieldRange {
                native_record_id: action.reference.native_record_id.clone(),
                field: JevInputField::UserMessage,
                container: JevNativeFieldContainer::Record,
                pointer: "/message/content/0/text".to_owned(),
                start: 0,
                end: action.text.len(),
            }];
        }
    }
    normalize_context(&mut content.actions, SourceFormat::ClaudeJsonl);
    assert!(
        content
            .actions
            .iter()
            .any(|action| action.metadata.human_text.is_some()),
        "{:#?}",
        content.actions
    );
    let context = build_jev_context(&AssessmentInput {
        content,
        prior_history_complete: true,
        activity_after_ms: None,
        boundary_positions: BTreeMap::new(),
        source_generation: 1,
        source_fingerprint: None,
        incarnation: 1,
        comparison_after: None,
    })
    .unwrap();
    IgnoredInstructionsCheck
        .prepare(&context)
        .unwrap()
        .work_items
        .into_iter()
        .map(|item| item.window.fields)
        .collect()
}

#[test]
fn six_frozen_commit_and_scan_cases_reach_production_windows_with_context() {
    let commits = "Use Conventional Commits for commit messages: feat:, fix:, chore:, docs:, refactor:, or test:. An optional scope can follow the type, as in feat(metrics): add action counts. Every commit must include a DCO sign-off. Use git commit -s.";
    let scans = "## Quality review\n\n- After a coherent batch of supported-language code changes, and before final project validation, run aislop scan --changes once.\n- For branch or PR work with a known base, run aislop scan --changes --base origin/main or the actual target branch.";
    for (rule, task, command) in [
        (
            commits,
            "Commit the action counts.",
            "git commit -s -m \"feat(metrics): add action counts\"",
        ),
        (
            commits,
            "Commit the action counts.",
            "git commit -m \"feat(metrics): add action counts\" -s",
        ),
        (
            commits,
            "Commit the action counts.",
            "git commit -s -m \"add action counts\"",
        ),
        (
            scans,
            "This is ordinary local change work. There is no branch or PR review and no established base.",
            "aislop scan --changes",
        ),
        (
            scans,
            "This is PR work. The PR target and known review base are origin/main.",
            "aislop scan --changes",
        ),
        (
            scans,
            "This is PR work. The PR target and known review base are origin/main.",
            "aislop scan --changes --base origin/main",
        ),
    ] {
        let windows = request_windows(rule, task, command);
        assert!(!windows.is_empty(), "{command}");
        for fields in windows {
            assert_eq!(fields["candidate_action"]["text"], command);
            assert!(fields.to_string().contains(task), "{fields}");
            for target in fields["instruction_targets"].as_array().unwrap() {
                let instruction = &target["instruction"];
                let context = instruction["surrounding_context"].to_string();
                if rule == scans {
                    assert!(context.contains("run aislop scan --changes once"));
                    assert!(context.contains("For branch or PR work with a known base"));
                    assert!(context.contains("--base origin/main"));
                    assert_eq!(
                        instruction["context_limits"]["surrounding_context_clipped"],
                        false
                    );
                } else {
                    assert!(context.contains("An optional scope can follow the type"));
                    assert!(context.contains("DCO sign-off"));
                }
            }
        }
    }
}

#[test]
fn long_scoped_commit_message_reaches_production_as_one_atomic_value() {
    let command = format!(
        "git commit -s -m \"feat(metabase): {}\"",
        "add action counts ".repeat(100)
    );
    let windows = request_windows(
        "# Commits\nUse Conventional Commits for commit messages. An optional scope can follow the type. Every commit must include a DCO sign-off.",
        "Commit the action counts.",
        &command,
    );
    assert!(!windows.is_empty());
    for fields in windows {
        assert_eq!(fields["candidate_action"]["text"], command);
        assert_eq!(fields["candidate_action_range_bytes"]["start"], 0);
        assert_eq!(fields["candidate_action_range_bytes"]["end"], command.len());
    }
}

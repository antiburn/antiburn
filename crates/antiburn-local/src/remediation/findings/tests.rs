use super::*;

#[test]
fn detector_keys_are_stable_and_unique() {
    let keys: BTreeSet<_> = DetectorId::ALL.into_iter().map(DetectorId::key).collect();
    assert_eq!(keys.len(), DetectorId::ALL.len());
    assert_eq!(DetectorId::SessionsOverDepth.key(), "sessions_over_depth");
    assert_eq!(DetectorId::CacheChurn.key(), "cache_churn");
}

#[test]
fn display_labels_stop_at_a_utf8_boundary() {
    let label = "界".repeat(200);
    let sanitized = sanitize_display_value(&label).unwrap();
    assert!(sanitized.len() <= MAX_DISPLAY_LABEL_BYTES);
    assert!(sanitized.is_char_boundary(sanitized.len()));
}

#[test]
fn exact_backend_identities_do_not_enter_display_data() {
    let mut evidence =
        crate::insights::detectors::test_support::claude_evidence("session-private-identity");
    evidence.capabilities.source_format = SourceFormat::ClaudeJsonl;
    let cause = FindingCause::OverpoweredSubagents {
        parent_model: "claude-opus-parent".to_owned(),
        worker_model: "claude-opus-worker".to_owned(),
        worker_ordinal: 4,
        parent_call_id: Some("call-private-identity".to_owned()),
    };
    let finding = finding(&evidence, cause.clone());
    assert_eq!(finding.session_id(), "session-private-identity");
    assert_eq!(finding.cause(), &cause);
    let display = finding.display().unwrap();
    let rendered = format!("{display:?}");
    assert!(!rendered.contains("session-private-identity"));
    assert!(!rendered.contains("call-private-identity"));
    assert_eq!(
        display.facts.labels,
        ["claude-opus-parent", "claude-opus-worker"]
    );
}

#[test]
fn canonical_identity_includes_source_format_and_exact_resource_scope() {
    let mut evidence = crate::insights::detectors::test_support::claude_evidence("session-a");
    let cause = FindingCause::UnusedSkill {
        skill: "review".to_owned(),
    };
    let first = finding(&evidence, cause.clone()).canonical_identity("project-a");
    evidence.capabilities.source_format = SourceFormat::OpenCodeJsonl;
    evidence.identity.agent = "opencode".to_owned();
    let second = finding(&evidence, cause).canonical_identity("project-a");
    assert_ne!(first, second);
    assert!(first.contains("claude_jsonl"));
    assert!(first.contains("project-a"));
    assert!(first.contains("review"));
}

#[test]
fn all_checks_group_only_matching_target_policies() {
    let cases = [
        (
            FindingCause::SessionsOverDepth {
                maximum_tokens: 500,
                limit_tokens: 400,
                requests: Vec::new(),
                omitted_requests: Some(0),
            },
            FindingCause::SessionsOverDepth {
                maximum_tokens: 900,
                limit_tokens: 400,
                requests: vec![RequestFact {
                    model: Some("model-a".to_owned()),
                    timestamp_ms: Some(2),
                    value: 900,
                }],
                omitted_requests: Some(1),
            },
            FindingCause::SessionsOverDepth {
                maximum_tokens: 500,
                limit_tokens: 401,
                requests: Vec::new(),
                omitted_requests: Some(0),
            },
        ),
        (
            FindingCause::ModelOverthinking {
                provider: Some("provider-a".to_owned()),
                api: Some("api-a".to_owned()),
                model: "model-a".to_owned(),
                reasoning: "high".to_owned(),
                turns: 1,
            },
            FindingCause::ModelOverthinking {
                provider: Some("provider-a".to_owned()),
                api: Some("api-a".to_owned()),
                model: "model-a".to_owned(),
                reasoning: "high".to_owned(),
                turns: 8,
            },
            FindingCause::ModelOverthinking {
                provider: Some("provider-b".to_owned()),
                api: Some("api-a".to_owned()),
                model: "model-a".to_owned(),
                reasoning: "high".to_owned(),
                turns: 1,
            },
        ),
        (
            FindingCause::OverpoweredSubagents {
                parent_model: "parent-a".to_owned(),
                worker_model: "worker-a".to_owned(),
                worker_ordinal: 1,
                parent_call_id: Some("call-a".to_owned()),
            },
            FindingCause::OverpoweredSubagents {
                parent_model: "parent-a".to_owned(),
                worker_model: "worker-a".to_owned(),
                worker_ordinal: 9,
                parent_call_id: Some("call-b".to_owned()),
            },
            FindingCause::OverpoweredSubagents {
                parent_model: "parent-a".to_owned(),
                worker_model: "worker-b".to_owned(),
                worker_ordinal: 1,
                parent_call_id: Some("call-a".to_owned()),
            },
        ),
        (
            FindingCause::UnusedMcpServer {
                server: "server-a".to_owned(),
            },
            FindingCause::UnusedMcpServer {
                server: "server-a".to_owned(),
            },
            FindingCause::UnusedMcpServer {
                server: "server-b".to_owned(),
            },
        ),
        (
            FindingCause::UnusedBuiltInTool {
                tool: "tool-a".to_owned(),
                tokens: BuiltInToolTokens::Definition(10),
            },
            FindingCause::UnusedBuiltInTool {
                tool: "tool-a".to_owned(),
                tokens: BuiltInToolTokens::Replicated(90),
            },
            FindingCause::UnusedBuiltInTool {
                tool: "tool-b".to_owned(),
                tokens: BuiltInToolTokens::Definition(10),
            },
        ),
        (
            FindingCause::UnusedSkill {
                skill: "skill-a".to_owned(),
            },
            FindingCause::UnusedSkill {
                skill: "skill-a".to_owned(),
            },
            FindingCause::UnusedSkill {
                skill: "skill-b".to_owned(),
            },
        ),
        (
            FindingCause::OldModelUsage {
                provider: Some("provider-a".to_owned()),
                api: Some("api-a".to_owned()),
                model: "old-a".to_owned(),
                replacement: "new-a".to_owned(),
                turns: 1,
            },
            FindingCause::OldModelUsage {
                provider: Some("provider-a".to_owned()),
                api: Some("api-a".to_owned()),
                model: "old-a".to_owned(),
                replacement: "new-a".to_owned(),
                turns: 7,
            },
            FindingCause::OldModelUsage {
                provider: Some("provider-a".to_owned()),
                api: Some("api-a".to_owned()),
                model: "old-a".to_owned(),
                replacement: "new-b".to_owned(),
                turns: 1,
            },
        ),
        (
            FindingCause::OveruseOfFastMode {
                provider: Some("provider-a".to_owned()),
                api: Some("api-a".to_owned()),
                model: "model-a".to_owned(),
                delegated_turns: 1,
            },
            FindingCause::OveruseOfFastMode {
                provider: Some("provider-a".to_owned()),
                api: Some("api-a".to_owned()),
                model: "model-a".to_owned(),
                delegated_turns: 6,
            },
            FindingCause::OveruseOfFastMode {
                provider: Some("provider-a".to_owned()),
                api: Some("api-b".to_owned()),
                model: "model-a".to_owned(),
                delegated_turns: 1,
            },
        ),
        (
            FindingCause::CacheChurn {
                model: "model-a".to_owned(),
                repeated_tokens: 10,
                paid_tokens: 20,
                threshold_basis_points: 20_000,
            },
            FindingCause::CacheChurn {
                model: "model-a".to_owned(),
                repeated_tokens: 90,
                paid_tokens: 100,
                threshold_basis_points: 20_000,
            },
            FindingCause::CacheChurn {
                model: "model-a".to_owned(),
                repeated_tokens: 10,
                paid_tokens: 20,
                threshold_basis_points: 30_000,
            },
        ),
    ];

    for (same_a, same_b, different) in cases {
        let first_evidence = crate::insights::detectors::test_support::claude_evidence("session-a");
        let second_evidence =
            crate::insights::detectors::test_support::claude_evidence("session-b");
        let scope_variant = same_a.clone();
        let first = finding(&first_evidence, same_a).canonical_identity("scope-a");
        let second = finding(&second_evidence, same_b).canonical_identity("scope-a");
        let changed = finding(&second_evidence, different).canonical_identity("scope-a");
        let changed_scope = finding(&second_evidence, scope_variant).canonical_identity("scope-b");

        assert_eq!(first, second);
        assert_ne!(first, changed);
        assert_ne!(first, changed_scope);
    }
}

#[test]
fn sensitive_labels_are_redacted_for_display() {
    for sensitive in [
        "Authorization: Bearer private",
        "API_KEY =private",
        "C:\\Users\\private\\config.json",
        "file:///Users/private/config.json",
    ] {
        let cause = FindingCause::UnusedMcpServer {
            server: sensitive.to_owned(),
        };
        assert_eq!(display_facts(&cause).labels, ["[private value]"]);
    }
}

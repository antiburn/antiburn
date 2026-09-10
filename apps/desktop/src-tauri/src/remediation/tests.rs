use super::recovery::recovery_target_matches;
use super::watch::{
    positive_control_resolution, scope_identity_matches, session_starts_after_boundary,
};
use super::*;
use crate::store::{PublishedEvidence, SessionKey, SessionRecord};
use antiburn_local::analysis::{
    EvidenceSource, EvidenceValue, ModelControlObservation, SessionEvidenceAccumulator,
    SourceCapabilities, SourceKind, TurnCounts, TurnFacts,
};

const SOURCE_FORMATS: [SourceFormat; 26] = [
    SourceFormat::ClaudeJsonl,
    SourceFormat::CodexRolloutJsonl,
    SourceFormat::OpenCodeJsonl,
    SourceFormat::OpenCodeSqliteV2,
    SourceFormat::PiV3Jsonl,
    SourceFormat::CursorJsonl,
    SourceFormat::CursorCliAgentJsonl,
    SourceFormat::CursorCliStoreDb,
    SourceFormat::CursorIdeComposer,
    SourceFormat::CursorLegacyChatJson,
    SourceFormat::AntigravityJson,
    SourceFormat::AntigravityBrainJsonl,
    SourceFormat::AntigravityCascadeJson,
    SourceFormat::AntigravityWorkspaceChatJson,
    SourceFormat::AntigravitySqlite,
    SourceFormat::CopilotCliJsonl,
    SourceFormat::CopilotIdeChatJson,
    SourceFormat::ClineSessionJson,
    SourceFormat::KiroSessionJson,
    SourceFormat::KiroChat,
    SourceFormat::AmpThreadJson,
    SourceFormat::AmpFileChanges,
    SourceFormat::WindsurfWorkspaceJson,
    SourceFormat::WindsurfMirrorJson,
    SourceFormat::WindsurfCascadeProtobuf,
    SourceFormat::Uncharacterized,
];

#[test]
fn publication_attribution_covers_supported_vendor_sources_and_settings() {
    struct Case {
        agent: AgentKind,
        source: SourceFormat,
        config_path: &'static str,
        config: &'static str,
        provider: &'static str,
        api: &'static str,
        model: &'static str,
        effective_model: &'static str,
        reasoning: Option<&'static str>,
    }
    let cases = [
        Case {
            agent: AgentKind::Claude,
            source: SourceFormat::ClaudeJsonl,
            config_path: ".claude/settings.json",
            config: r#"{"model":"claude-opus-5","effortLevel":"max"}"#,
            provider: "anthropic",
            api: "messages",
            model: "claude-opus-5",
            effective_model: "claude-opus-5",
            reasoning: Some("max"),
        },
        Case {
            agent: AgentKind::Codex,
            source: SourceFormat::CodexRolloutJsonl,
            config_path: ".codex/config.toml",
            config: "model = \"gpt-5.6-sol\"\nmodel_reasoning_effort = \"xhigh\"\n",
            provider: "openai",
            api: "responses",
            model: "gpt-5.6-sol",
            effective_model: "gpt-5.6-sol",
            reasoning: Some("xhigh"),
        },
        Case {
            agent: AgentKind::OpenCode,
            source: SourceFormat::OpenCodeJsonl,
            config_path: ".config/opencode/opencode.json",
            config: r#"{"model":"openai/gpt-5.6-sol"}"#,
            provider: "openai",
            api: "responses",
            model: "gpt-5.6-sol",
            effective_model: "openai/gpt-5.6-sol",
            reasoning: None,
        },
        Case {
            agent: AgentKind::OpenCode,
            source: SourceFormat::OpenCodeSqliteV2,
            config_path: ".config/opencode/opencode.json",
            config: r#"{"model":"openai/gpt-5.6-sol"}"#,
            provider: "openai",
            api: "responses",
            model: "gpt-5.6-sol",
            effective_model: "openai/gpt-5.6-sol",
            reasoning: None,
        },
        Case {
            agent: AgentKind::Pi,
            source: SourceFormat::PiV3Jsonl,
            config_path: ".pi/agent/settings.json",
            config: r#"{"defaultProvider":"openai","defaultModel":"gpt-5.6","defaultThinkingLevel":"max"}"#,
            provider: "openai",
            api: "openai-responses",
            model: "gpt-5.6",
            effective_model: "openai/gpt-5.6",
            reasoning: Some("max"),
        },
    ];
    for case in cases {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let config_path = home.join(case.config_path);
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        std::fs::write(config_path, case.config).unwrap();
        let store = Store::open(&directory.path().join("store")).unwrap();
        let key = SessionKey::new("native", case.agent.slug(), "session");
        store
            .upsert_sessions(
                &[SessionRecord {
                    key: key.clone(),
                    source_kind: "file".into(),
                    source_label: "synthetic".into(),
                    wsl_distro: None,
                    title: None,
                    title_source: None,
                    cwd: None,
                    surface: "cli".into(),
                    updated_at_epoch: Some(1),
                    activity_cursor: "cursor".into(),
                    activity_source: "event".into(),
                    subagent_count: 0,
                    fork_parent_session_id: None,
                    source_fingerprint: Some("fingerprint".into()),
                }],
                &[case.agent.slug()],
            )
            .unwrap();
        let mut capabilities = match case.agent {
            AgentKind::Claude => SourceCapabilities::claude(),
            AgentKind::Codex => SourceCapabilities::codex(),
            AgentKind::OpenCode => SourceCapabilities::opencode(),
            AgentKind::Pi => SourceCapabilities::pi(),
            _ => unreachable!(),
        };
        capabilities.source_format = case.source;
        let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: case.agent.slug().into(),
            session_id: "session".into(),
            kind: SourceKind::File,
            capabilities,
        })
        .evidence(&TurnFacts::default());
        let EvidenceValue::Complete(models) = &mut evidence.models else {
            panic!("synthetic model evidence must be complete");
        };
        models.control_observations = vec![ModelControlObservation {
            provider: Some(case.provider.into()),
            api: Some(case.api.into()),
            model: case.model.into(),
            effort: case.reasoning.map(str::to_owned),
            speed: None,
            last_ts_ms: 1_000,
            turns: TurnCounts {
                main_loop: 1,
                delegated: 0,
            },
        }];
        let attribution = publication_config_attribution_with_home(
            &store,
            &key,
            PublishedEvidence::Ready,
            &serde_json::to_string(&evidence).unwrap(),
            &home,
        )
        .unwrap();
        assert_eq!(
            attribution.model.as_ref().map(|value| value.2.as_str()),
            Some(case.effective_model),
            "{:?} {:?}",
            case.agent,
            case.source
        );
        assert_eq!(
            attribution.reasoning.as_ref().map(|value| value.2.as_str()),
            case.reasoning,
            "{:?} {:?}",
            case.agent,
            case.source
        );
    }
}

#[test]
fn automatic_editor_matrix_covers_all_agents_scopes_sources_and_platforms() {
    let scopes = ["global", "project", "session", "worker"];
    let platforms = ["macos", "linux", "windows"];
    let settings = [ConfigSetting::Model, ConfigSetting::Reasoning];
    let mut supported = Vec::new();
    for &agent in AgentKind::ALL {
        for &source in &SOURCE_FORMATS {
            for setting in settings {
                for scope in scopes {
                    for platform in platforms {
                        if automatic_editor_supported(
                            agent, setting, source, scope, "native", platform,
                        ) {
                            supported.push((agent, setting, source, scope, platform));
                        }
                        assert!(!automatic_editor_supported(
                            agent,
                            setting,
                            source,
                            scope,
                            "wsl:ubuntu",
                            platform,
                        ));
                    }
                }
            }
        }
    }
    assert_eq!(supported.len(), 32);
    assert!(
        supported
            .iter()
            .all(|(agent, setting, source, scope, platform)| {
                matches!(
                    (agent, setting, source),
                    (AgentKind::Claude, _, SourceFormat::ClaudeJsonl)
                        | (AgentKind::Codex, _, SourceFormat::CodexRolloutJsonl)
                        | (
                            AgentKind::OpenCode,
                            ConfigSetting::Model,
                            SourceFormat::OpenCodeJsonl | SourceFormat::OpenCodeSqliteV2
                        )
                        | (AgentKind::Pi, _, SourceFormat::PiV3Jsonl)
                ) && matches!(*scope, "global" | "project")
                    && matches!(*platform, "macos" | "linux")
            })
    );
}

#[test]
fn current_platform_matches_the_compile_target() {
    #[cfg(target_os = "macos")]
    assert_eq!(current_editor_platform(), "macos");
    #[cfg(target_os = "linux")]
    assert_eq!(current_editor_platform(), "linux");
    #[cfg(target_os = "windows")]
    assert_eq!(current_editor_platform(), "windows");
}

#[test]
fn model_specific_reasoning_selectors_have_distinct_physical_keys() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let path = directory.path().join("settings.json");
    let first = physical_key(
        &store,
        AgentKind::Claude,
        (&path, "modelSettings.effortLevel"),
        Some("claude-opus-5"),
    )
    .unwrap();
    let second = physical_key(
        &store,
        AgentKind::Claude,
        (&path, "modelSettings.effortLevel"),
        Some("claude-sonnet-5"),
    )
    .unwrap();
    assert_ne!(first, second);
}

#[test]
fn remediation_pricing_requires_the_reviewed_provider_route() {
    assert!(
        reviewed_pricing(
            AgentKind::Claude,
            Some("anthropic"),
            Some("messages"),
            "claude-opus-5",
        )
        .is_some()
    );
    assert!(
        reviewed_pricing(
            AgentKind::Claude,
            Some("openai"),
            Some("responses"),
            "claude-opus-5",
        )
        .is_none()
    );
}

#[test]
fn automatic_replacement_requires_a_reviewed_exact_route() {
    let cause = FindingCause::OldModelUsage {
        provider: Some("anthropic".into()),
        api: Some("messages".into()),
        model: "claude-opus-4-8".into(),
        replacement: "claude-opus-5".into(),
        turns: 1,
    };
    assert!(reviewed_replacement(AgentKind::Claude, &cause));
    let mut gateway = cause.clone();
    let FindingCause::OldModelUsage { provider, .. } = &mut gateway else {
        unreachable!()
    };
    *provider = Some("gateway".into());
    assert!(!reviewed_replacement(AgentKind::Claude, &gateway));

    let routed = FindingCause::OldModelUsage {
        provider: Some("openai".into()),
        api: Some("responses".into()),
        model: "gpt-5.5".into(),
        replacement: "gpt-5.6-sol".into(),
        turns: 1,
    };
    for agent in [AgentKind::OpenCode, AgentKind::Pi] {
        assert_eq!(
            reviewed_config_operation(agent, &routed),
            Some(ConfigOperation {
                setting: ConfigSetting::Model,
                expected_value: "openai/gpt-5.5".into(),
                proposed_value: "openai/gpt-5.6-sol".into(),
            })
        );
    }
}

#[test]
fn reasoning_auto_fix_requires_an_above_cap_reviewed_route() {
    let cause = |reasoning: &str| FindingCause::ModelOverthinking {
        provider: Some("openai".into()),
        api: Some("responses".into()),
        model: "gpt-5.6-sol".into(),
        reasoning: reasoning.into(),
        turns: 2,
    };
    assert_eq!(
        reviewed_config_operation(AgentKind::Codex, &cause("xhigh")),
        Some(ConfigOperation {
            setting: ConfigSetting::Reasoning,
            expected_value: "xhigh".into(),
            proposed_value: "medium".into(),
        })
    );
    assert!(reviewed_config_operation(AgentKind::Codex, &cause("high")).is_none());
    assert!(reviewed_config_operation(AgentKind::Claude, &cause("xhigh")).is_none());
}

#[test]
fn recovery_rejects_a_changed_physical_target_or_scope() {
    assert!(recovery_target_matches(
        Some("target-a"),
        "global",
        "target-a",
        ConfigScope::Global,
    ));
    assert!(!recovery_target_matches(
        Some("target-a"),
        "global",
        "target-b",
        ConfigScope::Global,
    ));
    assert!(!recovery_target_matches(
        Some("target-a"),
        "global",
        "target-a",
        ConfigScope::Project,
    ));
}

#[test]
fn nested_workspace_context_round_trips_for_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let root = directory.path().join("project");
    let cwd = root.join("packages/app");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    let context = config_context(AgentKind::OpenCode, &home, Some(&cwd), Some(&root)).unwrap();
    assert_eq!(
        workspace_relative_cwd(&context).as_deref(),
        Some("packages/app")
    );
    assert_eq!(
        recovery_workspace_cwd(
            context.trusted_workspace_root.as_deref().unwrap(),
            workspace_relative_cwd(&context).as_deref(),
        )
        .unwrap(),
        context.workspace_cwd.unwrap()
    );
}

#[test]
fn workspace_context_rejects_an_untrusted_cwd() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let root = directory.path().join("project");
    let outside = directory.path().join("outside");
    for path in [&home, &root, &outside] {
        std::fs::create_dir(path).unwrap();
    }
    assert!(config_context(AgentKind::OpenCode, &home, Some(&outside), Some(&root),).is_none());
}

#[test]
fn generic_watch_scope_rejects_other_projects_and_sessions() {
    assert!(scope_identity_matches(
        "project",
        "project-a",
        Some("project-a"),
        Some("session-b")
    ));
    assert!(!scope_identity_matches(
        "project",
        "project-a",
        Some("project-b"),
        Some("session-a")
    ));
    assert!(scope_identity_matches(
        "session",
        "session-a",
        Some("project-b"),
        Some("session-a")
    ));
    assert!(!scope_identity_matches(
        "session",
        "session-a",
        Some("project-a"),
        Some("session-b")
    ));
}

#[test]
fn session_scope_identity_round_trips_to_the_verifier_hash() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let secret = store.provider_account_secret().unwrap();
    let target_scope = session_scope_key(&secret, "claude-code", "session-a");
    let verifier_scope = hashed_parts(&store, b"session", &["claude-code", "session-a"]).unwrap();

    assert_eq!(target_scope, verifier_scope);
    assert!(scope_identity_matches(
        "session",
        &target_scope,
        None,
        Some(&verifier_scope)
    ));
}

#[test]
fn verification_availability_matches_all_documented_source_cells() {
    let definition = |detector: DetectorId, source_format: SourceFormat| WatchDefinition {
        version: 1,
        detector: detector.key().into(),
        canonical_identity: "target".into(),
        source_format: source_format.into(),
        workspace_key: Some("workspace".into()),
        workspace_relative_cwd: None,
        provider: Some("provider".into()),
        api: Some("api".into()),
        old_model: (detector == DetectorId::OldModelUsage).then(|| "old".into()),
        replacement: (detector == DetectorId::OldModelUsage).then(|| "new".into()),
        resource: None,
        physical_target_key: (detector == DetectorId::OldModelUsage).then(|| "physical".into()),
        config_setting: (detector == DetectorId::OldModelUsage).then(|| "model".into()),
        config_expected_value: None,
        config_proposed_value: None,
        verification_method_revision: VERIFICATION_METHOD_REVISION,
        remediation_policy_revision: Some(REMEDIATION_POLICY_REVISION),
        savings_method_revision: SAVINGS_METHOD_REVISION,
        pricing_revision: None,
        old_pricing: None,
        replacement_pricing: None,
        catalog_revision: Some(antiburn_local::insights::ReportCatalogs::default().revision),
        target_model: matches!(
            detector,
            DetectorId::ModelOverthinking | DetectorId::OveruseOfFastMode
        )
        .then(|| "model".into()),
        target_control: matches!(
            detector,
            DetectorId::ModelOverthinking | DetectorId::OveruseOfFastMode
        )
        .then(|| "control".into()),
    };
    for detector in DetectorId::ALL {
        for source_format in SOURCE_FORMATS {
            let agent = match source_format {
                SourceFormat::ClaudeJsonl => AgentKind::Claude,
                SourceFormat::CodexRolloutJsonl => AgentKind::Codex,
                SourceFormat::OpenCodeJsonl | SourceFormat::OpenCodeSqliteV2 => AgentKind::OpenCode,
                SourceFormat::PiV3Jsonl => AgentKind::Pi,
                _ => AgentKind::Claude,
            };
            let expected = verification_evidence_supported(detector, source_format);
            assert_eq!(
                watch_verification_available(
                    &definition(detector, source_format),
                    "project",
                    agent.slug(),
                    detector
                ),
                expected,
                "{detector:?} {source_format:?}"
            );
            assert!(!watch_verification_available(
                &definition(detector, source_format),
                "session",
                agent.slug(),
                detector
            ));
        }
    }
}

#[test]
fn verification_rejects_mismatched_agents_and_named_resources() {
    let mut definition = WatchDefinition {
        version: 1,
        detector: DetectorId::OldModelUsage.key().into(),
        canonical_identity: "target".into(),
        source_format: SourceFormat::ClaudeJsonl.into(),
        workspace_key: Some("workspace".into()),
        workspace_relative_cwd: None,
        provider: Some("anthropic".into()),
        api: Some("messages".into()),
        old_model: Some("old".into()),
        replacement: Some("new".into()),
        resource: None,
        physical_target_key: Some("physical".into()),
        config_setting: Some("model".into()),
        config_expected_value: Some("old".into()),
        config_proposed_value: Some("new".into()),
        verification_method_revision: VERIFICATION_METHOD_REVISION,
        remediation_policy_revision: Some(REMEDIATION_POLICY_REVISION),
        savings_method_revision: SAVINGS_METHOD_REVISION,
        pricing_revision: None,
        old_pricing: None,
        replacement_pricing: None,
        catalog_revision: Some(antiburn_local::insights::ReportCatalogs::default().revision),
        target_model: None,
        target_control: None,
    };
    assert!(watch_verification_available(
        &definition,
        "project",
        AgentKind::Claude.slug(),
        DetectorId::OldModelUsage,
    ));
    assert!(!watch_verification_available(
        &definition,
        "project",
        AgentKind::Codex.slug(),
        DetectorId::OldModelUsage,
    ));
    definition.resource = Some("server-a".into());
    assert!(!watch_verification_available(
        &definition,
        "project",
        AgentKind::Claude.slug(),
        DetectorId::OldModelUsage,
    ));
}

#[test]
fn session_fallback_scope_includes_the_agent() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let secret = store.provider_account_secret().unwrap();

    assert_ne!(
        session_scope_key(&secret, "claude-code", "shared-session"),
        session_scope_key(&secret, "codex", "shared-session"),
    );
}

#[test]
fn a_truncated_assessment_cannot_prove_a_fix() {
    let result = verify_prompt_watch(
        DetectorId::ModelOverthinking,
        SourceFormat::ClaudeJsonl,
        "target",
        VerificationStage::Watching,
        100,
        &[TargetAssessment {
            observed_at_ms: 101,
            identity: "target".into(),
            target_present: false,
            assessment: FindingAssessment::Unavailable(
                FindingUnavailableReason::IncompleteEvidence,
            ),
        }],
    );
    assert!(matches!(result.outcome, VerificationOutcome::Unknown(_)));
}

#[test]
fn an_observed_resource_subset_cannot_prove_an_absent_target_fixed() {
    let result = verify_prompt_watch(
        DetectorId::ModelOverthinking,
        SourceFormat::ClaudeJsonl,
        "target",
        VerificationStage::Watching,
        100,
        &[TargetAssessment {
            observed_at_ms: 101,
            identity: "target".into(),
            target_present: false,
            assessment: FindingAssessment::Unavailable(FindingUnavailableReason::SignalMissing),
        }],
    );
    assert!(matches!(result.outcome, VerificationOutcome::Unknown(_)));
}

#[test]
fn prompt_references_are_bounded_and_stable() {
    let first = prompt_with_evidence_paths("Fix this.", &[], Some("attempt-1")).unwrap();
    let second = prompt_with_evidence_paths("Fix this.", &[], Some("attempt-1")).unwrap();
    assert_eq!(first, second);
    assert!(first.ends_with("Remediation reference: ABR-attempt-1"));
    assert!(
        prompt_with_evidence_paths(
            &"x".repeat(antiburn_local::remediation::MAX_PROMPT_BYTES),
            &[],
            Some("attempt-1")
        )
        .is_err()
    );
}

#[test]
fn representative_paths_are_quoted_bounded_and_optional() {
    let paths = vec![
        "/sessions/one.jsonl".to_owned(),
        "/sessions/two\"quoted.jsonl".to_owned(),
        "/sessions/three.jsonl".to_owned(),
        "/sessions/four.jsonl".to_owned(),
    ];
    let prompt = prompt_with_evidence_paths("Fix this.", &paths, None).unwrap();
    assert!(prompt.contains("Representative session evidence"));
    assert!(prompt.contains(r#"- "/sessions/one.jsonl""#));
    assert!(prompt.contains(r#"- "/sessions/two\"quoted.jsonl""#));
    assert!(prompt.contains(r#"- "/sessions/three.jsonl""#));
    assert!(!prompt.contains("four.jsonl"));
    assert!(prompt.len() <= antiburn_local::remediation::MAX_PROMPT_BYTES);

    let oversized = vec!["/".repeat(antiburn_local::remediation::MAX_PROMPT_BYTES)];
    assert_eq!(
        prompt_with_evidence_paths("Fix this.", &oversized, None).unwrap(),
        "Fix this."
    );
}

#[test]
fn representative_path_resolution_omits_non_file_and_relative_sources() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let file = SessionRecord {
        key: SessionKey::new("native", "claude-code", "file"),
        source_kind: "file".into(),
        source_label: "/sessions/file.jsonl".into(),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "cli".into(),
        updated_at_epoch: Some(1),
        activity_cursor: String::new(),
        activity_source: "event".into(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: Some("file".into()),
    };
    let mut inline = file.clone();
    inline.key.session_id = "inline".into();
    inline.source_kind = "inline".into();
    inline.source_label = "/private/inline-label".into();
    let mut relative = file.clone();
    relative.key.session_id = "relative".into();
    relative.source_label = "private/relative.jsonl".into();
    store
        .upsert_sessions(
            &[file.clone(), inline.clone(), relative.clone()],
            &["claude-code"],
        )
        .unwrap();

    let paths = representative_paths(
        &store,
        [file.key.clone(), inline.key.clone(), relative.key.clone()],
    )
    .unwrap();

    assert_eq!(paths, vec![file.source_label]);
    assert!(!paths.iter().any(|path| path.contains("inline-label")));
    assert!(!paths.iter().any(|path| path.contains("relative")));
}

#[test]
fn typed_display_values_hide_private_paths_and_secrets() {
    assert_eq!(
        safe_display_value("/private/work/config.json").as_deref(),
        Some("[private value]")
    );
    assert_eq!(
        safe_display_value("api_key: secret-value").as_deref(),
        Some("[private value]")
    );
    let bounded = safe_display_value(&"é".repeat(300)).unwrap();
    assert!(bounded.len() <= 256);
    assert!(!bounded.contains(char::REPLACEMENT_CHARACTER));
}

#[test]
fn display_opportunities_keep_known_values_and_omit_incomplete_evidence() {
    let depth = FindingCause::SessionsOverDepth {
        maximum_tokens: 13_000,
        limit_tokens: 10_000,
        requests: vec![
            antiburn_local::remediation::RequestFact {
                model: None,
                timestamp_ms: Some(1),
                value: 12_000,
            },
            antiburn_local::remediation::RequestFact {
                model: None,
                timestamp_ms: Some(2),
                value: 13_000,
            },
        ],
        omitted_requests: Some(0),
    };
    assert_eq!(
        display_cause_opportunity(&depth, 100),
        Some(SavingsValue {
            unit: antiburn_local::remediation::SavingsUnit::LiteralInputTokens,
            value: 5_000.0,
        })
    );
    let zero = FindingCause::SessionsOverDepth {
        maximum_tokens: 10_000,
        limit_tokens: 10_000,
        requests: vec![antiburn_local::remediation::RequestFact {
            model: None,
            timestamp_ms: Some(1),
            value: 10_000,
        }],
        omitted_requests: Some(0),
    };
    assert_eq!(display_cause_opportunity(&zero, 100).unwrap().value, 0.0);

    let incomplete = FindingCause::SessionsOverDepth {
        maximum_tokens: 12_000,
        limit_tokens: 10_000,
        requests: vec![antiburn_local::remediation::RequestFact {
            model: None,
            timestamp_ms: Some(1),
            value: 12_000,
        }],
        omitted_requests: None,
    };
    assert_eq!(display_cause_opportunity(&incomplete, 100), None);

    let replicated = FindingCause::UnusedBuiltInTool {
        tool: "Read".into(),
        tokens: antiburn_local::remediation::BuiltInToolTokens::Replicated(750),
    };
    assert_eq!(
        display_cause_opportunity(&replicated, 100).unwrap().value,
        750.0
    );
    let unavailable = FindingCause::UnusedBuiltInTool {
        tool: "Read".into(),
        tokens: antiburn_local::remediation::BuiltInToolTokens::Definition(50),
    };
    assert_eq!(display_cause_opportunity(&unavailable, 100), None);
}

#[test]
fn display_limits_match_the_available_verification_evidence() {
    assert_eq!(
        verification_limit(DetectorId::ModelOverthinking),
        BurnCheckVerificationLimit::ExactPositiveControlRequired
    );
    assert_eq!(
        verification_limit(DetectorId::OveruseOfFastMode),
        BurnCheckVerificationLimit::ExactPositiveControlRequired
    );
    assert_eq!(
        verification_limit(DetectorId::OverpoweredSubagents),
        BurnCheckVerificationLimit::CurrentEvidenceCannotProveFix
    );
    assert_eq!(
        verification_limit(DetectorId::OldModelUsage),
        BurnCheckVerificationLimit::FreshEvidenceFromSameSourceAndTarget
    );
}

#[test]
fn missing_or_changed_policy_cannot_verify_an_attempt() {
    let target = WatchDefinition {
        version: 1,
        detector: DetectorId::OldModelUsage.key().into(),
        canonical_identity: "target".into(),
        source_format: SourceFormat::ClaudeJsonl.into(),
        workspace_key: None,
        workspace_relative_cwd: None,
        provider: Some("anthropic".into()),
        api: Some("messages".into()),
        old_model: Some("old".into()),
        replacement: Some("new".into()),
        resource: None,
        physical_target_key: Some("physical".into()),
        config_setting: Some("model".into()),
        config_expected_value: Some("old".into()),
        config_proposed_value: Some("new".into()),
        verification_method_revision: VERIFICATION_METHOD_REVISION,
        remediation_policy_revision: Some(REMEDIATION_POLICY_REVISION),
        savings_method_revision: SAVINGS_METHOD_REVISION,
        pricing_revision: None,
        old_pricing: None,
        replacement_pricing: None,
        catalog_revision: Some(antiburn_local::insights::ReportCatalogs::default().revision),
        target_model: None,
        target_control: None,
    };
    assert!(remediation_policy_is_current(&target));
    let mut missing = target.clone();
    missing.remediation_policy_revision = None;
    assert!(!remediation_policy_is_current(&missing));
    let mut changed = target.clone();
    changed.verification_method_revision += 1;
    assert!(!remediation_policy_is_current(&changed));
    let mut changed = target;
    changed.savings_method_revision += 1;
    assert!(!remediation_policy_is_current(&changed));
}

#[test]
fn a_session_spanning_the_boundary_cannot_verify_a_fix() {
    assert!(!session_starts_after_boundary(100, 100));
    assert!(!session_starts_after_boundary(99, 100));
    assert!(session_starts_after_boundary(101, 100));
}

#[test]
fn only_explicit_same_route_controls_prove_generic_transitions() {
    let definition = WatchDefinition {
        version: 1,
        detector: DetectorId::ModelOverthinking.key().into(),
        canonical_identity: "target".into(),
        source_format: SourceFormat::ClaudeJsonl.into(),
        workspace_key: None,
        workspace_relative_cwd: None,
        provider: Some("anthropic".into()),
        api: Some("messages".into()),
        old_model: None,
        replacement: None,
        resource: None,
        physical_target_key: None,
        config_setting: None,
        config_expected_value: None,
        config_proposed_value: None,
        verification_method_revision: VERIFICATION_METHOD_REVISION,
        remediation_policy_revision: Some(REMEDIATION_POLICY_REVISION),
        savings_method_revision: SAVINGS_METHOD_REVISION,
        pricing_revision: None,
        old_pricing: None,
        replacement_pricing: None,
        catalog_revision: Some(antiburn_local::insights::ReportCatalogs::default().revision),
        target_model: Some("claude-opus-5".into()),
        target_control: Some("max".into()),
    };
    let assessment = insights_report::CurrentDetectorAssessment {
        assessment: FindingAssessment::Clean,
        observed_at_ms: 200,
        finding_observed_at_ms: Vec::new(),
        started_at_ms: 150,
        workspace_candidate: None,
        source_format: antiburn_local::analysis::SourceFormat::ClaudeJsonl,
        session_id: "later".into(),
        control_observations: vec![antiburn_local::analysis::ModelControlObservation {
            provider: Some("anthropic".into()),
            api: Some("messages".into()),
            model: "claude-opus-5".into(),
            effort: Some("high".into()),
            speed: None,
            last_ts_ms: 190,
            turns: antiburn_local::analysis::TurnCounts {
                main_loop: 1,
                delegated: 0,
            },
        }],
        effective_reasoning_target_hash: None,
        effective_reasoning_scope: None,
    };
    assert_eq!(
        positive_control_resolution(DetectorId::ModelOverthinking, &definition, &assessment,),
        Some(190)
    );
    let mut wrong_route = definition.clone();
    wrong_route.provider = Some("gateway".into());
    assert_eq!(
        positive_control_resolution(DetectorId::ModelOverthinking, &wrong_route, &assessment,),
        None
    );
    assert_eq!(
        positive_control_resolution(DetectorId::SessionsOverDepth, &definition, &assessment),
        None
    );
}

#[test]
fn aggregate_wins_decode_only_typed_safe_documents() {
    let directory = tempfile::TempDir::new().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO remediation (
                    remediation_id, target_key, environment_key, agent, scope_kind, scope_key,
                    state, definition_json, result_json, created_at_epoch, updated_at_epoch,
                    effective_boundary_ms, verified_at_epoch)
                 VALUES ('attempt', 'target', 'native', 'claude-code', 'project', 'scope',
                         'fixed', '{\"version\":1}', '{\"version\":1}', 1, 2, 1000, 2)",
            [],
        )
        .unwrap();
    let display = BurnCheckDisplayFacts {
        resource_kind: BurnCheckResourceKind::Model,
        resource_identity: Some("old-model".into()),
        current_value: Some("old-model".into()),
        replacement_value: Some("new-model".into()),
        scope_kind: BurnCheckScopeKind::Project,
        quantity: Some(2),
        quantity_unit: Some(BurnCheckQuantityUnit::Turns),
        observation_count: 1,
        first_observed_at_ms: 1_000,
        last_observed_at_ms: 1_000,
        estimate_method: Some(BurnCheckEstimateMethod::OldModelPriceDifference),
        estimated_opportunity: None,
        verification_limit: BurnCheckVerificationLimit::FreshEvidenceFromSameSourceAndTarget,
    };
    let legacy = store.remediation("attempt").unwrap().unwrap();
    assert_eq!(public_watch(&store, &legacy).unwrap(), None);
    let snapshot = serde_json::to_string(&StoredDisplaySnapshot {
        version: 1,
        finding_id: "stable-finding".into(),
        display,
    })
    .unwrap();
    let savings = serde_json::to_string(&AggregateSavings {
        version: 1,
        token_savings: None,
        api_equivalent_cost_avoided_usd: Some(0.25),
        improvement_count: None,
        method: Some(BurnCheckEstimateMethod::OldModelPriceDifference),
    })
    .unwrap();
    store
        .lock()
        .execute(
            "INSERT INTO remediation_contribution (
                    owner_key, remediation_id, detector_id, origin, display_snapshot_json,
                    facts_json, starts_at_ms, ends_at_ms, updated_at_ms)
                 VALUES ('owner', 'attempt', ?1, 'action', ?2, ?3, 1000, 2000, 2000)",
            rusqlite::params![DetectorId::OldModelUsage.key(), snapshot, savings],
        )
        .unwrap();
    for index in 1..1_000 {
        store
            .upsert_remediation_contribution(&RemediationContribution {
                owner_key: format!("owner-{index:03}"),
                remediation_id: "attempt".into(),
                detector_id: DetectorId::OldModelUsage.key().into(),
                origin: "action".into(),
                display_snapshot_json: snapshot.clone(),
                facts_json: savings.clone(),
                starts_at_ms: 1_000,
                ends_at_ms: 2_000 + index,
                updated_at_ms: 2_000 + index,
            })
            .unwrap();
    }
    let controller = RemediationController::new(directory.path().to_owned());
    let aggregate = controller.aggregate_wins(&store).unwrap();
    assert_eq!(aggregate.wins.len(), 1_000);
    assert!(
        aggregate
            .wins
            .iter()
            .all(|win| win.finding_id == "stable-finding")
    );

    store
        .lock()
        .execute(
            "UPDATE remediation_contribution
                    SET facts_json = '{\"version\":1,\"path\":\"private\"}'
                  WHERE owner_key = 'owner'",
            [],
        )
        .unwrap();
    assert!(matches!(
        controller.aggregate_wins(&store),
        Err(ControllerError::Internal)
    ));
}

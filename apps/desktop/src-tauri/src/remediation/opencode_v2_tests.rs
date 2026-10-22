use antiburn_local::analysis::{
    EvidenceSource, EvidenceValue, METRICS_SCHEMA_REVISION, ModelControlObservation,
    PARSER_REVISION, SessionEvidence, SessionEvidenceAccumulator, SourceCapabilities, SourceKind,
    TurnCounts, TurnFacts,
};
use antiburn_local::insights::ReportWindow;

use super::*;
use crate::store::{AnalysisRecord, EvidenceCompletion, PublishedEvidence, SessionRecord};

const V2_SOURCES: [SourceFormat; 1] = [SourceFormat::OpenCodeV2SessionExport];
const MODEL_USAGE_AT_MS: i64 = 2_000_000_000_000;
const SESSION_STARTED_AT_EPOCH: i64 = MODEL_USAGE_AT_MS / 1_000;

pub(super) fn model_evidence(source: SourceFormat) -> SessionEvidence {
    let mut capabilities = if source == SourceFormat::OpenCodeSqliteV2 {
        SourceCapabilities::opencode_v2()
    } else {
        SourceCapabilities::opencode()
    };
    capabilities.source_format = source;
    let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "opencode".into(),
        session_id: "session".into(),
        kind: SourceKind::File,
        capabilities,
    })
    .evidence(&TurnFacts::default());
    let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
        panic!("the fixture needs complete eligibility");
    };
    eligibility.assistant_turns = 1;
    let EvidenceValue::Complete(models) = &mut evidence.models else {
        panic!("the fixture needs complete model evidence");
    };
    models.by_model.insert(
        "gpt-5.5".into(),
        antiburn_local::analysis::ModelTokens {
            input: 10,
            turns: 1,
            first_ts_ms: MODEL_USAGE_AT_MS,
            last_ts_ms: MODEL_USAGE_AT_MS,
            ..Default::default()
        },
    );
    models.control_observations = vec![ModelControlObservation {
        provider: Some("openai".into()),
        api: Some("responses".into()),
        model: "gpt-5.5".into(),
        effort: None,
        speed: None,
        last_ts_ms: MODEL_USAGE_AT_MS,
        turns: TurnCounts {
            main_loop: 1,
            delegated: 0,
        },
    }];
    evidence
}

fn publish_v1_model(store: &Store) -> SessionKey {
    publish_model(store, SourceFormat::OpenCodeSqliteV1)
}

fn publish_model(store: &Store, source: SourceFormat) -> SessionKey {
    publish_evidence(store, &model_evidence(source))
}

pub(super) fn publish_evidence(store: &Store, evidence: &SessionEvidence) -> SessionKey {
    let key = SessionKey::new("native", "opencode", "session");
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
                updated_at_epoch: Some(SESSION_STARTED_AT_EPOCH),
                activity_cursor: "cursor".into(),
                activity_source: "event".into(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some("fingerprint".into()),
            }],
            &["opencode"],
        )
        .unwrap();
    let claim = store
        .claim_next_evidence(&["opencode"], 10, 60)
        .unwrap()
        .unwrap();
    assert!(
        store
            .publish_projections(
                &AnalysisRecord {
                    key: key.clone(),
                    model_breakdown_json: "{}".into(),
                    pricing_breakdown_json: "{}".into(),
                    inclusive_models_json: "[]".into(),
                    initial_context_json: None,
                    source_summaries_json: None,
                    provider_hints_json: None,
                    source_fingerprint: "fingerprint".into(),
                    pricing_generation: 1,
                    analyzed_generation: claim.source_generation,
                    parser_revision: PARSER_REVISION,
                    analyzer_revision: ANALYZER_REVISION,
                    metrics_schema_revision: METRICS_SCHEMA_REVISION,
                },
                Some(SESSION_STARTED_AT_EPOCH),
                &EvidenceCompletion {
                    claim_fence: claim.claim_fence,
                    status: PublishedEvidence::Ready,
                    evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
                    evidence_json: serde_json::to_string(evidence).unwrap(),
                },
                &[],
                &[]
            )
            .unwrap()
    );
    key
}

#[test]
fn native_v2_publication_attribution_and_prepare_require_the_same_current_consumer_target() {
    use crate::agent_config::opencode_config_authority::CurrentConfigAuthority;
    for source in [
        SourceFormat::OpenCodeSqliteV1,
        SourceFormat::OpenCodeSqliteV2,
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let home = root.join("home");
        let cwd = root.join("repo");
        let path = home.join(".config/opencode/opencode.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(&path, r#"{"model":"openai/gpt-5.5"}"#).unwrap();
        let store = Store::open(temporary.path()).unwrap();
        let key = publish_model(&store, source);
        store
            .replace_repositories(&[crate::store::RepositoryRecord {
                key: "repo".into(),
                repo_name: "repo".into(),
                full_name: "synthetic/repo".into(),
                status: "accessible".into(),
                repo_root: Some(cwd.to_string_lossy().into_owned()),
                suspected_path: None,
                worktree_count: 1,
                session_count: 1,
                wsl_distro: None,
                enabled: true,
            }])
            .unwrap();
        store
            .lock()
            .execute(
                "UPDATE session SET cwd = ?1",
                [cwd.to_string_lossy().as_ref()],
            )
            .unwrap();
        let authority = CurrentConfigAuthority::fixture(
            &cwd,
            json!([{"type":"document","path":path,"info":{"model":{"providerID":"openai","model":"gpt-5.5"}}}]),
        );
        let attribution = config::publication_config_attribution_with_authority(
            &store,
            &key,
            PublishedEvidence::Ready,
            &serde_json::to_string(&model_evidence(source)).unwrap(),
            &home,
            Some(&authority),
        )
        .unwrap();
        let (physical, scope, value) = attribution.model.unwrap();
        assert_eq!(scope, "global");
        assert_eq!(value, "openai/gpt-5.5");
        assert_eq!(attribution.records.len(), 1);
        store.lock().execute("UPDATE session_evidence SET effective_model_target_hash=?1, effective_model_scope=?2, effective_model=?3", rusqlite::params![physical, scope, value]).unwrap();
        let finding = insights_report::list_current_findings(
            temporary.path(),
            CurrentFindingsRequest {
                environment_key: "native".into(),
                window: ReportWindow {
                    start_epoch: SESSION_STARTED_AT_EPOCH - 1,
                    end_epoch: SESSION_STARTED_AT_EPOCH + 10,
                },
                detector: DetectorId::OldModelUsage,
            },
        )
        .unwrap()
        .findings
        .into_iter()
        .next()
        .unwrap();
        let controller = RemediationController::new(temporary.path().into());
        let (_, resolved) = controller
            .resolve_opencode_target(&store, finding.clone(), Some(&home), Some(&authority))
            .unwrap();
        assert_eq!(resolved.config.is_some(), cfg!(not(windows)));
        let (_, unbound) = controller
            .resolve_opencode_target(&store, finding, Some(&home), None)
            .unwrap();
        assert!(unbound.config.is_none());
        let revision = store.check_preferences_revision().unwrap();
        controller
            .state
            .lock()
            .unwrap()
            .targets
            .entry(DetectorId::OldModelUsage)
            .or_default()
            .push_back(TimedTarget {
                id: "native-current".into(),
                value: resolved,
                check_preferences_revision: revision,
                smart_master_generation: None,
                created_at_epoch: 10,
            });
        let review = controller.prepare_auto_fix_with_receipt_at(
            &store,
            "native-current",
            10,
            Some(&authority),
        );
        if cfg!(windows) {
            assert_eq!(
                review.unwrap_err(),
                ControllerError::AutoFixUnavailable(
                    AutoFixUnavailableReason::UnsupportedOrUnprovenTarget
                )
            );
        } else {
            let review = review.unwrap();
            assert_eq!(review.effect, AutoFixEffect::FutureModelDefault);
            assert_eq!(
                review.side_effect,
                AutoFixSideEffect::ActiveSessionsKeepTheirModel
            );
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"model":"openai/gpt-5.5"}"#
        );
        let overridden = CurrentConfigAuthority::fixture(
            &cwd,
            json!([{"type":"document","path":path,"info":{"model":{"providerID":"openai","model":"gpt-5.5"}}},{"type":"document","info":{"model":{"providerID":"openai","model":"gpt-5.5"}}}]),
        );
        assert!(
            config::publication_config_attribution_with_authority(
                &store,
                &key,
                PublishedEvidence::Ready,
                &serde_json::to_string(&model_evidence(source)).unwrap(),
                &home,
                Some(&overridden)
            )
            .unwrap()
            .model
            .is_none()
        );
    }
}

#[test]
fn current_v2_resource_actions_require_exact_consumer_bindings_and_keep_web_tools_prompt_only() {
    use crate::agent_config::{ResourceKind, opencode_config_authority::CurrentConfigAuthority};
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let home = root.join("home");
    let cwd = root.join("repo");
    let config_root = home.join(".config/opencode");
    let path = config_root.join("opencode.json");
    let skill = config_root.join("skills/review/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(
        &path,
        r#"{"mcp":{"servers":{"docs":{"type":"local","command":["docs"]}}}}"#,
    )
    .unwrap();
    std::fs::write(
        &skill,
        "---\ndescription: Review changes\n---\nReview the changes.\n",
    )
    .unwrap();
    let authority = CurrentConfigAuthority::fixture(
        &cwd,
        json!([{"type":"document","path":path,"info":{"mcp":{"servers":{"docs":{"type":"local","command":["docs"]}}}}},{"type":"directory","path":config_root}]),
    );
    let store = Store::open(temporary.path()).unwrap();
    let key = publish_model(&store, SourceFormat::OpenCodeSqliteV2);
    store
        .lock()
        .execute(
            "UPDATE session SET cwd=?1",
            [cwd.to_string_lossy().as_ref()],
        )
        .unwrap();
    store
        .replace_repositories(&[crate::store::RepositoryRecord {
            key: "repo".into(),
            repo_name: "repo".into(),
            full_name: "synthetic/repo".into(),
            status: "accessible".into(),
            repo_root: Some(cwd.to_string_lossy().into_owned()),
            suspected_path: None,
            worktree_count: 1,
            session_count: 1,
            wsl_distro: None,
            enabled: true,
        }])
        .unwrap();
    let controller = RemediationController::new(temporary.path().into());
    for (kind, name, editable) in [
        (ResourceKind::McpServer, "docs", true),
        (ResourceKind::Skill, "review", true),
        (ResourceKind::BuiltInTool, "webfetch", false),
    ] {
        let editable = editable && cfg!(not(windows));
        let resource = insights_report::UnusedResourceTarget {
            agent: AgentKind::OpenCode,
            source_format: SourceFormat::OpenCodeSqliteV2,
            kind,
            canonical_name: name.into(),
            scope: insights_report::ResourceAssessmentScope::Global,
            observations: 1,
            indexed: true,
            replicated_tokens: None,
            estimated_token_burn_basis_points: None,
            supporting_sessions: vec![insights_report::ResourceSupportingSession {
                environment_key: key.environment_key.clone(),
                agent: key.agent.clone(),
                session_id: key.session_id.clone(),
                observed_at_ms: MODEL_USAGE_AT_MS,
            }],
        };
        let context = BurnCheckTargetContext {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: SESSION_STARTED_AT_EPOCH - 1,
                end_epoch: SESSION_STARTED_AT_EPOCH + 10,
            },
        };
        let target = controller
            .resolve_resource_target_with_authority(
                &store,
                &resource,
                context.clone(),
                Some(&home),
                Some(&authority),
            )
            .unwrap();
        assert_eq!(target.config.is_some(), editable, "{kind:?}");
        if editable {
            let definition = watch_definition(&target);
            assert_eq!(target.scope_kind, "global");
            assert!(definition.workspace_key.is_some());
            assert!(definition.workspace_relative_cwd.is_some());
        }
        let inventory_only = insights_report::UnusedResourceTarget {
            indexed: false,
            ..resource.clone()
        };
        let inventory_target = controller
            .resolve_resource_target_with_authority(
                &store,
                &inventory_only,
                BurnCheckTargetContext {
                    environment_key: "native".into(),
                    window: ReportWindow {
                        start_epoch: SESSION_STARTED_AT_EPOCH - 1,
                        end_epoch: SESSION_STARTED_AT_EPOCH + 10,
                    },
                },
                Some(&home),
                Some(&authority),
            )
            .unwrap();
        assert!(inventory_target.config.is_none(), "{kind:?}");
        assert_eq!(
            target.finding().source_format,
            SourceFormat::OpenCodeSqliteV2
        );
        assert!(remediation_prompt(target.finding()).is_ok());
        if kind == ResourceKind::BuiltInTool {
            assert!(matches!(
                target.finding().cause(),
                FindingCause::UnusedBuiltInTool {
                    tokens: antiburn_local::remediation::BuiltInToolTokens::Unavailable,
                    ..
                }
            ));
            assert_eq!(finding_quantity(target.finding().cause()), (None, None));
            assert!(display_cause_opportunity(target.finding().cause(), 1).is_none());
        }
        assert!(
            burn_check_display_facts(&target, None)
                .estimated_opportunity
                .is_none()
        );
        let unbound = controller
            .resolve_resource_target_with_authority(&store, &resource, context, Some(&home), None)
            .unwrap();
        assert!(unbound.config.is_none());
        let exported = insights_report::UnusedResourceTarget {
            source_format: SourceFormat::OpenCodeV2SessionExport,
            ..resource
        };
        let exported_target = controller
            .resolve_resource_target_with_authority(
                &store,
                &exported,
                BurnCheckTargetContext {
                    environment_key: "native".into(),
                    window: ReportWindow {
                        start_epoch: SESSION_STARTED_AT_EPOCH - 1,
                        end_epoch: SESSION_STARTED_AT_EPOCH + 10,
                    },
                },
                Some(&home),
                Some(&authority),
            )
            .unwrap();
        assert!(exported_target.config.is_none());
        assert!(remediation_prompt(exported_target.finding()).is_err());
    }
}

#[test]
fn v2_matching_model_use_does_not_prove_current_config_attribution() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let config = home.join(".config/opencode/opencode.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, r#"{"model":"openai/gpt-5.5"}"#).unwrap();
    let store = Store::open(&temporary.path().join("store")).unwrap();
    let key = publish_v1_model(&store);
    for source in [
        SourceFormat::OpenCodeSqliteV1,
        SourceFormat::OpenCodeSqliteV2,
        V2_SOURCES[0],
    ] {
        let attribution = publication_config_attribution_with_home(
            &store,
            &key,
            PublishedEvidence::Ready,
            &serde_json::to_string(&model_evidence(source)).unwrap(),
            &home,
        )
        .unwrap();
        assert!(attribution.model.is_none());
        assert!(attribution.records.is_empty());
        assert!(attribution.reasoning.is_none());
    }
}

#[test]
fn retained_v2_findings_cannot_reuse_v1_physical_targets_or_watches() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let config = home.join(".config/opencode/opencode.json");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, r#"{"model":"openai/gpt-5.5"}"#).unwrap();
    let store = Store::open(temporary.path()).unwrap();
    publish_v1_model(&store);
    let attribution = (
        "saved-v1-physical-target".to_owned(),
        "global".to_owned(),
        "openai/gpt-5.5".to_owned(),
    );
    store.lock().execute(
        "UPDATE session_evidence SET effective_model_target_hash = ?1, effective_model_scope = ?2, effective_model = ?3",
        rusqlite::params![attribution.0, attribution.1, attribution.2],
    ).unwrap();
    let current = insights_report::list_current_findings(
        temporary.path(),
        CurrentFindingsRequest {
            environment_key: "native".into(),
            window: ReportWindow {
                start_epoch: SESSION_STARTED_AT_EPOCH - 1,
                end_epoch: SESSION_STARTED_AT_EPOCH + 10,
            },
            detector: DetectorId::OldModelUsage,
        },
    )
    .unwrap()
    .findings
    .into_iter()
    .next()
    .expect("the V1 fixture must produce a finding");
    assert_eq!(
        current.finding.source_format,
        SourceFormat::OpenCodeSqliteV1
    );
    let secret = store.provider_account_secret().unwrap();
    let v1_identity = target_identity(&secret, &current, None);
    assert_eq!(
        v1_identity.physical_target_key.as_deref(),
        Some(attribution.0.as_str())
    );
    let controller = RemediationController::new(temporary.path().into());
    for source in V2_SOURCES {
        let mut retained = current.clone();
        retained.finding.source_format = source;
        assert!(remediation_prompt(&retained.finding).is_err());
        let identity = target_identity(&secret, &retained, None);
        assert!(identity.physical_target_key.is_none());
        assert_ne!(identity.target_key, v1_identity.target_key);
        let (_, resolved) = controller
            .resolve_target(&store, retained.clone(), AgentKind::OpenCode, Some(&home))
            .unwrap();
        assert!(resolved.config.is_none());
        let definition = watch_definition(&resolved);
        assert_eq!(definition.source_format.value(), source);
        assert!(!watch_verification_available(
            &definition,
            "global",
            "opencode",
            DetectorId::OldModelUsage
        ));
        assert!(
            passive_remediations(
                &store.lock(),
                &secret,
                vec![retained],
                MODEL_USAGE_AT_MS + 1_000
            )
            .unwrap()
            .is_empty()
        );
    }
}

#[test]
fn v2_static_checks_do_not_turn_missing_support_into_clean_results() {
    for source in V2_SOURCES {
        let evidence = model_evidence(source);
        for detector in [
            DetectorId::SessionsOverDepth,
            DetectorId::ModelOverthinking,
            DetectorId::OverpoweredSubagents,
            DetectorId::UnusedMcpServers,
            DetectorId::UnusedBuiltInTools,
            DetectorId::UnusedSkills,
            DetectorId::OldModelUsage,
            DetectorId::OveruseOfFastMode,
            DetectorId::CacheChurn,
        ] {
            assert!(
                matches!(
                    antiburn_local::remediation::assess_detector(
                        detector,
                        &evidence,
                        &antiburn_local::insights::ReportCatalogs::default()
                    ),
                    FindingAssessment::NotApplicable | FindingAssessment::Unavailable(_)
                ),
                "{source:?} {detector:?}"
            );
        }
    }
}

#[test]
fn v2_actions_stay_unavailable_on_every_edit_platform_and_environment() {
    for source in V2_SOURCES {
        for setting in [
            ConfigSetting::Model,
            ConfigSetting::Reasoning,
            ConfigSetting::Compaction,
            ConfigSetting::SubagentModel,
            ConfigSetting::McpServer,
            ConfigSetting::BuiltInTool,
            ConfigSetting::Skill,
            ConfigSetting::FastMode,
        ] {
            for action in [
                RemediationAction::AutomaticEdit(setting),
                RemediationAction::RecoverUncertainWrite(setting),
                RemediationAction::PublicationAttribution(setting),
            ] {
                assert_eq!(
                    vendor_policy(AgentKind::OpenCode)
                        .unwrap()
                        .action_support(action, source),
                    ActionSupport::Unsupported
                );
            }
            for platform in ["macos", "linux", "windows"] {
                for environment in ["native", "wsl:ubuntu", "ssh:host"] {
                    assert!(!automatic_editor_supported(
                        AgentKind::OpenCode,
                        setting,
                        source,
                        "global",
                        environment,
                        platform
                    ));
                }
            }
        }
    }
}

#[test]
fn v2_uncertain_writes_stay_blocked_instead_of_starting_verification() {
    for source in V2_SOURCES {
        let temporary = tempfile::tempdir().unwrap();
        let store = Store::open(temporary.path()).unwrap();
        let definition = json!({
            "version": 1,
            "detector": "unused_mcp_servers",
            "canonicalIdentity": "docs",
            "sourceFormat": source,
            "workspaceKey": null,
            "provider": null,
            "api": null,
            "oldModel": null,
            "replacement": null,
            "resource": "docs",
            "physicalTargetKey": "physical",
            "configSetting": "mcpServer",
            "configExpectedValue": "docs=true",
            "configProposedValue": "docs=false",
            "verificationMethodRevision": VERIFICATION_METHOD_REVISION,
            "remediationPolicyRevision": REMEDIATION_POLICY_REVISION,
            "savingsMethodRevision": SAVINGS_METHOD_REVISION,
            "pricingRevision": null,
            "oldPricing": null,
            "replacementPricing": null
        })
        .to_string();
        let record = store.create_or_reuse_remediation(&Remediation {
            remediation_id: "recovery".into(), target_key: "target".into(),
            environment_key: "native".into(), agent: "opencode".into(),
            scope_kind: "global".into(), scope_key: "physical".into(),
            state: RemediationState::Reserved, origin: "action".into(), prompt_group_id: None,
            definition_json: definition.clone(),
            result_json: json!({"version": 1, "verification": {"status": "watching"}, "savings": {"status": "unavailable"}}).to_string(),
            created_at_epoch: 1, effective_boundary_ms: None,
        }, &[]).unwrap().unwrap();
        assert!(
            store
                .begin_remediation_write(&record.remediation_id, 2)
                .unwrap()
        );
        assert!(
            store
                .mark_remediation_recovery_needed(&record.remediation_id, "readbackFailed", 2)
                .unwrap()
        );
        let record = store.remediation(&record.remediation_id).unwrap().unwrap();
        assert!(recover_uncertain_write(&store, &record, 3).unwrap());
        let after = store.remediation(&record.remediation_id).unwrap().unwrap();
        assert_eq!(after.definition_json, definition);
        assert_eq!(after.state, RemediationState::RecoveryNeeded);
        assert!(after.effective_boundary_ms.is_none());
        let result: serde_json::Value = serde_json::from_str(&after.result_json).unwrap();
        assert_eq!(result["verification"]["reason"], "verificationUnavailable");
    }
}

#[cfg(feature = "analytics")]
#[test]
fn retired_setup_interactions_decode_but_cannot_emit() {
    for value in [
        serde_json::json!({"kind":"firstRunStepReached", "step":"started"}),
        serde_json::json!({"kind":"firstRunAction", "action":"live_usage_started"}),
        serde_json::json!({"kind":"firstRunFinished"}),
        serde_json::json!({"kind":"stepSettingsViewed", "label":"agents", "detail":"first_run"}),
    ] {
        let interaction: super::Interaction = serde_json::from_value(value).unwrap();
        assert!(interaction.is_retired());
    }
    assert!(!super::Interaction::AppSearchOpened {}.is_retired());
}

use super::super::resources::schema::{
    CoverageBand, CpuBand, IoRateBand, MemoryBand, ResourceUsageSummary,
};
use super::*;

fn assert_closed_payload(
    resolved: (EventName, Facts),
    name: &str,
    label: Option<&str>,
    detail: Option<&str>,
) {
    let (event_name, facts) = resolved;
    let mut event = sample();
    event.event = event_name.as_str().to_owned();
    event.properties = Properties {
        arch: "aarch64",
        bucket: facts.bucket,
        label: facts.label,
        detail: facts.detail,
        origin: facts.origin,
        usage_band: facts.usage_band,
        response_shape: facts.response_shape,
        eligibility: facts.eligibility,
        ineligible_reason: facts.ineligible_reason,
        experiment: facts.experiment,
        reset_arm: facts.reset_arm,
        reset_availability: facts.reset_availability,
        resets_per_week: facts.resets_per_week,
        next_reset_available: facts.next_reset_available,
        plan: facts.plan,
        factor_band: facts.factor_band,
        residual_band: facts.residual_band,
        estimate_bias: facts.estimate_bias,
        unexplained_band: facts.unexplained_band,
        reading_coverage: facts.reading_coverage,
        resource_usage: facts.resource_usage,
        unrecognized_types: facts.unrecognized_types,
    };
    let payload = serde_json::to_value(event).expect("Typed event serializes");
    assert_eq!(payload["event"], name);
    let mut expected = serde_json::json!({"arch": "aarch64"});
    if let Some(label) = label {
        expected["label"] = label.into();
    }
    if let Some(detail) = detail {
        expected["detail"] = detail.into();
    }
    assert_eq!(payload["properties"], expected);
}

#[test]
fn onboarding_interactions_serialize_only_closed_properties() {
    for (screen, label) in [
        ("welcome", "welcome"),
        ("sessions", "sessions"),
        ("limits", "limits"),
        ("hud", "hud"),
        ("checks", "checks"),
        ("cloud", "cloud"),
        ("cloudUnavailable", "cloud_unavailable"),
        ("cloudHandoff", "cloud_handoff"),
        ("cloudIdentity", "cloud_identity"),
        ("cloudFolders", "cloud_folders"),
        ("cloudSync", "cloud_sync"),
        ("cloudComplete", "cloud_complete"),
    ] {
        let interaction: Interaction = serde_json::from_value(serde_json::json!({
            "kind": "onboardingScreenViewed",
            "screen": screen,
        }))
        .unwrap();
        assert!(!interaction.is_retired());
        let resolved = interaction.resolve();
        assert!(!resolved.0.is_user_oriented(&resolved.1));
        assert_closed_payload(
            resolved,
            "antiburn.onboarding_screen_viewed",
            Some(label),
            None,
        );
    }

    for (screen, label, action) in [
        ("welcome", "welcome", "get_started"),
        ("sessions", "sessions", "continue"),
        ("cloudUnavailable", "cloud_unavailable", "back"),
        ("cloud", "cloud", "use_locally"),
        ("cloud", "cloud", "set_up_cloud"),
        ("cloudHandoff", "cloud_handoff", "learn_more"),
        ("cloudHandoff", "cloud_handoff", "sign_in"),
        ("cloudHandoff", "cloud_handoff", "cancel"),
        ("cloudHandoff", "cloud_handoff", "retry"),
        ("cloudFolders", "cloud_folders", "add_folder"),
        ("cloudFolders", "cloud_folders", "select_folder"),
        ("cloudFolders", "cloud_folders", "start_sync"),
        ("cloudSync", "cloud_sync", "check_status"),
        ("cloudSync", "cloud_sync", "cancel_sync"),
        ("cloudComplete", "cloud_complete", "done"),
        ("cloudIdentity", "cloud_identity", "sign_out"),
    ] {
        let interaction: Interaction = serde_json::from_value(serde_json::json!({
            "kind": "onboardingActionTaken",
            "screen": screen,
            "action": action,
        }))
        .unwrap();
        assert!(!interaction.is_retired());
        let resolved = interaction.resolve();
        assert!(resolved.0.is_user_oriented(&resolved.1));
        assert_closed_payload(
            resolved,
            "antiburn.onboarding_action_taken",
            Some(label),
            Some(action),
        );
    }

    let interaction: Interaction = serde_json::from_value(serde_json::json!({
        "kind": "onboardingCloudUnavailable",
    }))
    .unwrap();
    assert!(!interaction.is_retired());
    let resolved = interaction.resolve();
    assert!(!resolved.0.is_user_oriented(&resolved.1));
    assert_closed_payload(
        resolved,
        "antiburn.onboarding_cloud_outcome",
        None,
        Some("unavailable"),
    );
    let completion = (EventName::OnboardingCompleted, Facts::default());
    assert!(!completion.0.is_user_oriented(&completion.1));
    assert_closed_payload(completion, "antiburn.onboarding_completed", None, None);
}

#[test]
fn cloud_statuses_remain_closed_and_do_not_refresh_activity() {
    for status in [
        "opening",
        "waiting",
        "returned",
        "cancelled",
        "expired",
        "browser_failed",
        "offline",
        "credential_failed",
        "signed_in",
        "signed_out",
        "unavailable",
        "accepted",
        "partial",
        "failed",
    ] {
        let native_status: OnboardingCloudStatus =
            serde_json::from_value(serde_json::json!(status)).unwrap();
        let resolved = (
            EventName::OnboardingCloudOutcome,
            Facts {
                detail: Some(native_status.as_str()),
                ..Facts::default()
            },
        );
        assert!(
            serde_json::from_value::<Interaction>(serde_json::json!({
                "kind": "onboardingCloudStatusObserved", "status": status,
            }))
            .is_err()
        );
        assert!(!resolved.0.is_user_oriented(&resolved.1));
        assert_closed_payload(
            resolved,
            "antiburn.onboarding_cloud_outcome",
            None,
            Some(status),
        );
    }
}

#[test]
fn onboarding_dismissal_uses_the_saved_draft_and_excludes_completion() {
    use crate::store::OnboardingState;

    let facts = onboarding_dismissed_facts(OnboardingState::CloudUnavailable).unwrap();
    let event = EventName::OnboardingActionTaken;
    assert!(event.is_user_oriented(&facts));
    assert_closed_payload(
        (event, facts),
        "antiburn.onboarding_action_taken",
        Some("cloud_unavailable"),
        Some("dismissed"),
    );
    assert!(onboarding_dismissed_facts(OnboardingState::Completed).is_none());
}

#[test]
fn onboarding_ipc_rejects_unlisted_values_and_renderer_completion() {
    for value in [
        serde_json::json!({"kind":"onboardingScreenViewed", "screen":"/private"}),
        serde_json::json!({"kind":"onboardingScreenViewed", "screen":"sessions", "sessionId":"private"}),
        serde_json::json!({"kind":"onboardingActionTaken", "screen":"cloud", "action":"signed_in"}),
        serde_json::json!({"kind":"onboardingActionTaken", "screen":"cloud", "action":"dismissed"}),
        serde_json::json!({"kind":"onboardingActionTaken", "screen":"cloud", "action":"use_locally", "folder":"/private"}),
        serde_json::json!({"kind":"onboardingActionTaken", "screen":"cloudFolders", "action":"remove_folder"}),
        serde_json::json!({"kind":"onboardingActionTaken", "screen":"cloudFolders", "action":"not_now"}),
        serde_json::json!({"kind":"onboardingCloudUnavailable", "credential":"private"}),
        serde_json::json!({"kind":"onboardingCloudOutcome", "outcome":"uploaded"}),
        serde_json::json!({"kind":"onboardingCompleted"}),
        serde_json::json!({"kind":"onboardingCloudStatusObserved", "status":"uploaded"}),
        serde_json::json!({"kind":"onboardingCloudStatusObserved", "status":"signed_in", "account":"private"}),
        serde_json::json!({"kind":"onboardingCloudStatusObserved", "status":"failed", "callback":"private"}),
    ] {
        assert!(
            serde_json::from_value::<Interaction>(value.clone()).is_err(),
            "{value}"
        );
    }
}

const RETIRED_CHECKS: &[&str] = &[
    "ignored_instructions",
    "scope_creep",
    "over_exploring",
    "skill_opportunities",
];

const PREPARATION_OUTCOMES: &[(BurnCheckPreparationOutcome, &str)] = &[
    (BurnCheckPreparationOutcome::Unchanged, "unchanged"),
    (BurnCheckPreparationOutcome::Shortened, "shortened"),
    (
        BurnCheckPreparationOutcome::InputTooLarge,
        "input_too_large",
    ),
    (BurnCheckPreparationOutcome::InvalidFit, "invalid_fit"),
    (
        BurnCheckPreparationOutcome::InputUnavailable,
        "input_unavailable",
    ),
    (BurnCheckPreparationOutcome::ResourceLimit, "resource_limit"),
    (BurnCheckPreparationOutcome::InvalidInput, "invalid_input"),
    (
        BurnCheckPreparationOutcome::InvalidRequest,
        "invalid_request",
    ),
];

const DEFINITION_OPEN_OUTCOMES: &[(BurnCheckDefinitionOpenOutcome, &str)] = &[
    (BurnCheckDefinitionOpenOutcome::Succeeded, "succeeded"),
    (BurnCheckDefinitionOpenOutcome::Missing, "missing"),
    (BurnCheckDefinitionOpenOutcome::Unavailable, "unavailable"),
    (BurnCheckDefinitionOpenOutcome::OpenFailed, "open_failed"),
];

const DIRECTORY_OPEN_OUTCOMES: &[(BurnCheckDirectoryOpenOutcome, &str)] = &[
    (BurnCheckDirectoryOpenOutcome::Succeeded, "succeeded"),
    (BurnCheckDirectoryOpenOutcome::Missing, "missing"),
    (BurnCheckDirectoryOpenOutcome::Unavailable, "unavailable"),
    (BurnCheckDirectoryOpenOutcome::OpenFailed, "open_failed"),
];

#[test]
fn installed_check_payloads_have_only_closed_outcomes_and_correct_session_classification() {
    for &(outcome, detail) in PREPARATION_OUTCOMES {
        let name = EventName::BurnCheckInputPrepared;
        assert!(!name.is_user_oriented(&outcome.facts()));
        assert_closed_payload((name, outcome.facts()), name.as_str(), None, Some(detail));
    }
    for &(outcome, detail) in DEFINITION_OPEN_OUTCOMES {
        let name = EventName::BurnCheckDefinitionOpened;
        assert!(name.is_user_oriented(&outcome.facts()));
        assert_closed_payload((name, outcome.facts()), name.as_str(), None, Some(detail));
    }
    for &(outcome, detail) in DIRECTORY_OPEN_OUTCOMES {
        let name = EventName::BurnCheckDirectoryOpened;
        assert!(name.is_user_oriented(&outcome.facts()));
        assert_closed_payload((name, outcome.facts()), name.as_str(), None, Some(detail));
    }
}

#[test]
fn installed_check_preparation_excludes_content_paths_counts_and_false_conditions() {
    use antiburn_checks::fit::{
        FitOmission, FitReport, FitStrategy, PreparationFailure, PreparationFailureCode,
        PreparationRecord,
    };
    use antiburn_checks::prepared_assessment::{PreparedAssessment, PreparedBlock};
    let mut prepared = PreparedAssessment::new(
        "private-provider-profile".into(),
        Default::default(),
        0,
        ["private_first_decision", "private_second_decision"]
            .into_iter()
            .map(|decision| PreparedBlock {
                decision: decision.into(),
                original_input: serde_json::json!({"private_input_name": "private transcript"}),
                input: serde_json::json!({"private_input_name": "private transcript"}),
                request: serde_json::json!({"private_yaml_value": "private request"}),
                report: FitReport::default(),
            })
            .collect(),
    )
    .unwrap();
    assert_eq!(
        BurnCheckPreparationOutcome::from_record(&PreparationRecord::Local(
            antiburn_checks::fit::LocalPreparation {
                version: 1,
                started_at_unix_ms: 1001,
                evaluation_budget: Default::default(),
            },
        )),
        None,
    );
    assert_eq!(
        BurnCheckPreparationOutcome::from_record(&PreparationRecord::Ready(prepared.clone())),
        Some(BurnCheckPreparationOutcome::Unchanged)
    );
    prepared.decisions[1].report.omissions.push(FitOmission {
        path: "private_input_name[*].private_path".into(),
        strategy: FitStrategy::Truncate,
        original_count: 982_451_653,
        retained_count: 256,
    });
    assert_eq!(
        BurnCheckPreparationOutcome::from_record(&PreparationRecord::Ready(prepared.clone())),
        Some(BurnCheckPreparationOutcome::Shortened)
    );
    for (code, expected) in [
        (PreparationFailureCode::ConditionFalse, None),
        (
            PreparationFailureCode::InputUnavailable,
            Some(BurnCheckPreparationOutcome::InputUnavailable),
        ),
        (
            PreparationFailureCode::InvalidInput,
            Some(BurnCheckPreparationOutcome::InvalidInput),
        ),
        (
            PreparationFailureCode::InvalidRequest,
            Some(BurnCheckPreparationOutcome::InvalidRequest),
        ),
        (
            PreparationFailureCode::NoFit,
            Some(BurnCheckPreparationOutcome::InputTooLarge),
        ),
        (
            PreparationFailureCode::PreparationResourceLimit,
            Some(BurnCheckPreparationOutcome::ResourceLimit),
        ),
        (
            PreparationFailureCode::InvalidFit,
            Some(BurnCheckPreparationOutcome::InvalidFit),
        ),
        (
            PreparationFailureCode::ProviderUnavailable,
            Some(BurnCheckPreparationOutcome::InputUnavailable),
        ),
        (
            PreparationFailureCode::IncompatibleProfile,
            Some(BurnCheckPreparationOutcome::InputUnavailable),
        ),
    ] {
        let record = PreparationRecord::Rejected(PreparationFailure {
            code,
            detail: "private error /private/check.yml".into(),
            report: Some(prepared.decisions[1].report.clone()),
            context: Box::new(antiburn_checks::fit::PreparationFailureContext {
                decision: Some("private decision name".into()),
                input_path: Some("private_input_name".into()),
                ..Default::default()
            }),
        });
        let outcome = BurnCheckPreparationOutcome::from_record(&record);
        assert_eq!(outcome, expected);
        if let Some(outcome) = outcome {
            assert_closed_payload(
                (EventName::BurnCheckInputPrepared, outcome.facts()),
                "antiburn.burn_check_input_prepared",
                None,
                Some(outcome.as_str()),
            );
        }
    }
}

#[test]
fn installed_check_events_are_native_only_and_the_catalog_lists_every_outcome() {
    for (kind, outcome) in [
        ("burnCheckInputPrepared", "shortened"),
        ("burnCheckDefinitionOpened", "succeeded"),
        ("burnCheckDirectoryOpened", "succeeded"),
    ] {
        assert!(
            serde_json::from_value::<Interaction>(serde_json::json!({
                "kind": kind, "outcome": outcome,
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<Interaction>(serde_json::json!({
                "kind": kind, "outcome": "arbitrary-code", "sourceReference": "private-path"
            }))
            .is_err()
        );
    }
    let catalog = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/analytics.md"
    ))
    .replace("\r\n", "\n");
    let section = catalog
        .split("### YAML input fitting and definition opens\n")
        .nth(1)
        .expect("Document the installed-check contract")
        .split("\n### ")
        .next()
        .unwrap();
    for detail in PREPARATION_OUTCOMES
        .iter()
        .map(|(_, detail)| detail)
        .chain(DEFINITION_OPEN_OUTCOMES.iter().map(|(_, detail)| detail))
        .chain(DIRECTORY_OPEN_OUTCOMES.iter().map(|(_, detail)| detail))
    {
        assert!(
            section.contains(&format!("`{detail}`")),
            "Missing outcome: {detail}"
        );
    }
    assert!(section.contains("antiburn.burn_check_input_prepared"));
    assert!(section.contains("antiburn.burn_check_definition_opened"));
    assert!(section.contains("antiburn.burn_check_directory_opened"));
}

#[test]
fn model_check_enablement_preserves_the_existing_global_contract() {
    for enabled in [false, true] {
        let lifecycle = ModelCheckLifecycle::Enablement { enabled };
        assert_closed_payload(
            lifecycle.resolve(),
            "antiburn.ignored_instruction_lifecycle",
            Some("enablement"),
            Some(if enabled { "enabled" } else { "disabled" }),
        );
    }
}

#[test]
fn decision_provider_setup_and_test_payloads_are_closed() {
    use crate::jev::config::SystemOneProvider;
    for (source, label) in [
        (SystemOneProvider::Jev, "jev"),
        (SystemOneProvider::Cloudflare, "cloudflare"),
        (SystemOneProvider::Ollama, "ollama"),
        (SystemOneProvider::Custom, "custom"),
    ] {
        let provider = source.into();
        for (outcome, detail) in [
            (ProviderSetupOutcome::Saved, "provider_saved"),
            (ProviderSetupOutcome::Switched, "provider_switched"),
            (
                ProviderSetupOutcome::ConnectionRemoved,
                "connection_removed",
            ),
            (
                ProviderSetupOutcome::CredentialRemoved,
                "credential_removed",
            ),
        ] {
            let lifecycle = ModelCheckLifecycle::ProviderSetup { provider, outcome };
            assert_closed_payload(
                lifecycle.resolve(),
                "antiburn.ignored_instruction_lifecycle",
                Some(label),
                Some(detail),
            );
        }
        for (outcome, detail) in [
            (ProviderTestOutcome::Succeeded, "test_succeeded"),
            (ProviderTestOutcome::Failed, "test_failed"),
        ] {
            assert_closed_payload(
                ModelCheckLifecycle::ProviderTest { provider, outcome }.resolve(),
                "antiburn.ignored_instruction_lifecycle",
                Some(label),
                Some(detail),
            );
        }
    }
}

#[test]
fn prompt_payloads_carry_only_the_generic_action_outcome() {
    for outcome in ["ready", "stale", "expired", "unavailable", "failed"] {
        let interaction: Interaction = serde_json::from_value(serde_json::json!({
            "kind":"burnCheckPromptPrepared", "outcome":outcome,
        }))
        .expect("Prompt preparation uses a closed outcome");
        assert_closed_payload(
            interaction.resolve(),
            "antiburn.burn_check_prompt_prepared",
            None,
            Some(outcome),
        );
    }
    let interaction: Interaction =
        serde_json::from_value(serde_json::json!({"kind":"burnCheckPromptCopied"}))
            .expect("Prompt copy carries no properties");
    assert_closed_payload(
        interaction.resolve(),
        "antiburn.burn_check_prompt_copied",
        None,
        None,
    );
}

#[test]
fn prompt_contracts_reject_check_labels_private_fields_and_unknown_values() {
    let interactions = [
        serde_json::json!({"kind":"burnCheckPromptPrepared", "outcome":"ready"}),
        serde_json::json!({"kind":"burnCheckPromptCopied"}),
    ];
    for input in &interactions {
        for check in RETIRED_CHECKS
            .iter()
            .copied()
            .chain(["arbitrary_yaml_check"])
        {
            let mut invalid = input.clone();
            invalid["check"] = check.into();
            assert!(serde_json::from_value::<Interaction>(invalid).is_err());
        }
        let mut invalid = input.clone();
        invalid["check"] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Interaction>(invalid).is_err());
    }
    for field in [
        "unknownField",
        "label",
        "detail",
        "observation",
        "stage",
        "outcome",
        "content",
        "prompt",
        "excerpt",
        "path",
        "url",
        "endpoint",
        "model",
        "key",
        "apiKey",
        "accountId",
        "sessionId",
        "findingId",
        "approval",
        "error",
    ] {
        for input in &interactions {
            let mut invalid = input.clone();
            invalid[field] = "private".into();
            assert!(
                serde_json::from_value::<Interaction>(invalid).is_err(),
                "{field}"
            );
        }
    }
    for outcome in [
        "private_unknown_value",
        "execution_clean",
        "backfill_failed",
    ] {
        assert!(
            serde_json::from_value::<Interaction>(serde_json::json!({
                "kind":"burnCheckPromptPrepared", "outcome":outcome,
            }))
            .is_err()
        );
    }
}

#[test]
fn retired_reader_interactions_and_native_lifecycle_payloads_are_rejected() {
    for check in RETIRED_CHECKS {
        for observation in [
            "finding_visible",
            "evidence_available",
            "evidence_unavailable",
            "evidence_failed",
        ] {
            assert!(
                serde_json::from_value::<Interaction>(serde_json::json!({
                    "kind":"smartCheckObserved", "check":check, "observation":observation,
                }))
                .is_err()
            );
        }
    }
    for stage in ["finding", "evidence", "prompt"] {
        for outcome in ["visible", "available", "unavailable", "failed", "copied"] {
            assert!(
                serde_json::from_value::<Interaction>(serde_json::json!({
                    "kind":"ignoredInstructionObserved", "stage":stage, "outcome":outcome,
                }))
                .is_err()
            );
        }
    }
    for kind in [
        "ignoredInstructionLifecycle",
        "smartCheckLifecycle",
        "modelCheckLifecycle",
    ] {
        for (label, detail) in [
            ("enablement", "enabled"),
            ("jev", "provider_saved"),
            ("jev", "connection_removed"),
            ("history_window", "future"),
            ("backfill", "requested"),
            ("execution", "completed"),
            ("scope_creep", "execution_clean"),
        ] {
            assert!(
                serde_json::from_value::<Interaction>(serde_json::json!({
                    "kind":kind, "label":label, "detail":detail,
                }))
                .is_err()
            );
        }
    }
    assert!(
        !EVERY_EVENT
            .iter()
            .any(|name| name.as_str() == "antiburn.ignored_instruction_observed")
    );
}

#[test]
fn memory_action_resolves_to_closed_wire_labels() {
    let cases = [
        ("reveal", "succeeded"),
        ("archive", "changed_on_disk"),
        ("restore", "failed"),
        ("remove_index_line", "unsupported"),
        ("open_from_session", "succeeded"),
    ];
    for (action, outcome) in cases {
        let interaction: Interaction = serde_json::from_value(serde_json::json!({
            "kind": "memoryAction",
            "action": action,
            "outcome": outcome,
        }))
        .unwrap();
        let (name, facts) = interaction.resolve();
        assert_eq!(name.as_str(), "antiburn.memory_action");
        assert_eq!(facts.label, Some(action));
        assert_eq!(facts.detail, Some(outcome));
    }
    for bad in [
        serde_json::json!({"kind":"memoryAction","action":"delete","outcome":"succeeded"}),
        serde_json::json!({"kind":"memoryAction","action":"archive","outcome":"ok"}),
        serde_json::json!({"kind":"memoryAction","action":"archive","outcome":"failed","path":"/x"}),
    ] {
        assert!(serde_json::from_value::<Interaction>(bad).is_err());
    }
}

#[test]
fn usage_limits_interactions_resolve_to_closed_wire_labels() {
    let actions = [
        "opened",
        "closed",
        "pin_added",
        "pin_removed",
        "appearance_dials",
        "appearance_bars",
        "refresh_succeeded",
        "refresh_failed",
        "enable_succeeded",
        "enable_failed",
        "settings_opened",
        "settings_open_failed",
    ];
    for action in actions {
        let interaction: Interaction = serde_json::from_value(serde_json::json!({
            "kind": "usageLimitsInteraction",
            "action": action,
        }))
        .unwrap();
        let (name, facts) = interaction.resolve();
        assert_eq!(name.as_str(), "antiburn.usage_limits_interaction");
        assert_eq!(facts.label, Some(action));
        assert!(facts.detail.is_none());
        assert!(name.is_user_oriented(&facts));
    }
    for invalid in [
        serde_json::json!({"kind":"usageLimitsInteraction","action":"pin_account"}),
        serde_json::json!({"kind":"usageLimitsInteraction","action":"opened","path":"/private"}),
        serde_json::json!({"kind":"usageLimitsInteraction","action":"opened","account":"private"}),
    ] {
        assert!(serde_json::from_value::<Interaction>(invalid).is_err());
    }
}

#[test]
fn memories_surface_resolves_to_its_wire_label() {
    let viewed: Interaction =
        serde_json::from_str(r#"{"kind":"surfaceViewed","surface":"memories","origin":"user"}"#)
            .unwrap();
    let (name, facts) = viewed.resolve();
    assert_eq!(name.as_str(), "antiburn.surface_viewed");
    assert_eq!(facts.label, Some("memories"));
    assert_eq!(facts.detail, Some("user"));

    let observed: Interaction = serde_json::from_str(
        r#"{"kind":"surfaceStateObserved","surface":"memories","state":"empty","origin":"user"}"#,
    )
    .unwrap();
    let (name, facts) = observed.resolve();
    assert_eq!(name.as_str(), "antiburn.surface_state_observed");
    assert_eq!(facts.label, Some("memories"));
}
#[test]
fn agents_surface_resolves_to_its_wire_label() {
    let viewed: Interaction =
        serde_json::from_str(r#"{"kind":"surfaceViewed","surface":"agents","origin":"user"}"#)
            .unwrap();
    let (name, facts) = viewed.resolve();
    assert_eq!(name.as_str(), "antiburn.surface_viewed");
    assert_eq!(facts.label, Some("agents"));
    assert_eq!(facts.detail, Some("user"));

    let observed: Interaction = serde_json::from_str(
        r#"{"kind":"surfaceStateObserved","surface":"agents","state":"empty","origin":"user"}"#,
    )
    .unwrap();
    let (name, facts) = observed.resolve();
    assert_eq!(name.as_str(), "antiburn.surface_state_observed");
    assert_eq!(facts.label, Some("agents"));
}

fn resource_summary() -> ResourceUsageSummary {
    ResourceUsageSummary {
        memory_mean: MemoryBand::From100ToUnder250Mib,
        memory_max: MemoryBand::From250ToUnder500Mib,
        memory_coverage: CoverageBand::Full,
        cpu_average: CpuBand::From10ToUnder25Percent,
        cpu_coverage: CoverageBand::Partial,
        read_rate_average: IoRateBand::Zero,
        read_coverage: CoverageBand::Full,
        write_rate_average: IoRateBand::Unavailable,
        write_coverage: CoverageBand::None,
        database_size: MemoryBand::From50ToUnder100Mib,
        database_coverage: CoverageBand::Full,
        wal_size: MemoryBand::Under50Mib,
        wal_coverage: CoverageBand::Partial,
    }
}

fn sample() -> Event {
    Event {
        platform: PLATFORM,
        message_id: "11111111-1111-4111-8111-111111111111".into(),
        anonymous_id: "22222222-2222-4222-8222-222222222222".into(),
        session_id: "33333333-3333-4333-8333-333333333333".into(),
        event: EventName::ScanCompleted.as_str().into(),
        original_timestamp: "2026-08-18T00:00:00Z".into(),
        properties: Properties {
            arch: "aarch64",
            bucket: Some("10-49"),
            label: Some("claude-code"),
            detail: Some("native"),
            origin: Some("user"),
            usage_band: Some("80_to_under_100"),
            response_shape: Some("object"),
            eligibility: Some("eligible"),
            ineligible_reason: Some("null"),
            experiment: Some("in_experiment"),
            reset_arm: Some("reset"),
            reset_availability: Some("available"),
            resets_per_week: Some("1"),
            next_reset_available: Some("present"),
            plan: Some("max"),
            factor_band: Some("2_to_under_4"),
            residual_band: Some("within_5"),
            estimate_bias: Some("within_5"),
            unexplained_band: Some("none"),
            reading_coverage: Some("covered"),
            resource_usage: Some(resource_summary()),
            unrecognized_types: Some(vec!["custom_event".to_string()]),
        },
        context: Context {
            app_version: "antiburn:1.2.3".into(),
            os: "macos",
        },
    }
}

#[test]
fn buckets_never_reveal_an_exact_count() {
    assert_eq!(bucket(0), "0");
    assert_eq!(bucket(1), "1-9");
    assert_eq!(bucket(9), "1-9");
    assert_eq!(bucket(10), "10-49");
    assert_eq!(bucket(4_000), "1000+");
}

/// A recognized plan name maps case- and whitespace-insensitively; an
/// absent plan is `unknown`; anything else, including an empty string, is
/// `other` rather than the raw text.
#[test]
fn an_unlisted_plan_name_maps_to_other_rather_than_leaking_its_text() {
    let provider = LiveUsageProvider::Anthropic;
    assert_eq!(map_plan(provider, None, None), "unknown");
    assert_eq!(map_plan(provider, Some(""), None), "unknown");
    assert_eq!(map_plan(provider, Some("   "), None), "unknown");
    assert_eq!(map_plan(provider, Some("Max"), None), "max");
    assert_eq!(map_plan(provider, Some(" pro "), None), "pro");
    assert_eq!(map_plan(provider, Some("FREE"), None), "free");
    assert_eq!(map_plan(provider, Some("team"), None), "team");
    assert_eq!(map_plan(provider, Some("enterprise"), None), "enterprise");
    assert_eq!(map_plan(provider, Some("plus"), None), "plus");
    assert_eq!(map_plan(provider, Some("business"), None), "business");
    assert_eq!(map_plan(provider, Some("edu"), None), "edu");
    assert_eq!(map_plan(provider, Some("some-future-plan"), None), "other");
}

#[test]
fn provider_specific_plan_values_stay_in_their_provider_vocabulary() {
    assert_eq!(
        map_plan(
            LiveUsageProvider::Anthropic,
            Some("max"),
            Some("default_claude_max_5x")
        ),
        "max_5x"
    );
    assert_eq!(
        map_plan(
            LiveUsageProvider::Anthropic,
            Some("MAX"),
            Some("default_claude_max_20x")
        ),
        "max_20x"
    );
    assert_eq!(
        map_plan(
            LiveUsageProvider::Anthropic,
            Some("max"),
            Some("new_claude_max_tier")
        ),
        "max"
    );
    assert_eq!(
        map_plan(LiveUsageProvider::Openai, Some("prolite"), None),
        "prolite"
    );
    assert_eq!(
        map_plan(LiveUsageProvider::Anthropic, Some("prolite"), None),
        "other"
    );
    assert_eq!(
        map_plan(LiveUsageProvider::Google, Some("prolite"), None),
        "other"
    );
}

#[test]
fn codex_plan_values_keep_the_existing_closed_mappings() {
    for (raw, mapped) in [
        ("pro", "pro"),
        ("plus", "plus"),
        ("team", "team"),
        ("enterprise", "enterprise"),
        ("prolite", "prolite"),
    ] {
        assert_eq!(map_plan(LiveUsageProvider::Openai, Some(raw), None), mapped);
    }
    assert_eq!(
        map_plan(LiveUsageProvider::Openai, Some("future_codex_plan"), None),
        "other"
    );
}

#[test]
fn the_factor_band_boundaries_step_by_powers_of_two() {
    assert_eq!(factor_band(f64::NAN), "under_1");
    assert_eq!(factor_band(f64::NEG_INFINITY), "under_1");
    assert_eq!(factor_band(-1.0), "under_1");
    assert_eq!(factor_band(0.0), "under_1");
    assert_eq!(factor_band(0.99), "under_1");
    assert_eq!(factor_band(1.0), "1_to_under_2");
    assert_eq!(factor_band(1.99), "1_to_under_2");
    assert_eq!(factor_band(2.0), "2_to_under_4");
    assert_eq!(factor_band(3.99), "2_to_under_4");
    assert_eq!(factor_band(4.0), "4_to_under_8");
    assert_eq!(factor_band(7.99), "4_to_under_8");
    assert_eq!(factor_band(8.0), "8_to_under_16");
    assert_eq!(factor_band(15.99), "8_to_under_16");
    assert_eq!(factor_band(16.0), "16_to_under_32");
    assert_eq!(factor_band(31.99), "16_to_under_32");
    assert_eq!(factor_band(32.0), "32_to_under_64");
    assert_eq!(factor_band(63.99), "32_to_under_64");
    assert_eq!(factor_band(64.0), "64_to_under_128");
    assert_eq!(factor_band(127.99), "64_to_under_128");
    assert_eq!(factor_band(128.0), "128_and_over");
    assert_eq!(factor_band(1_000.0), "128_and_over");
    assert_eq!(factor_band(f64::INFINITY), "128_and_over");
}

#[test]
fn the_residual_band_boundaries_match_the_absolute_difference() {
    assert_eq!(residual_band(None), "unknown");
    assert_eq!(residual_band(Some((50.0, 50.0))), "within_5");
    assert_eq!(residual_band(Some((50.0, 45.0))), "within_5");
    assert_eq!(residual_band(Some((50.0, 44.9))), "within_20");
    assert_eq!(residual_band(Some((50.0, 30.0))), "within_20");
    assert_eq!(residual_band(Some((50.0, 29.9))), "over_20");
    // Order does not matter: a factor that under- or over-estimates by
    // the same amount lands in the same band.
    assert_eq!(residual_band(Some((29.9, 50.0))), "over_20");
}

#[test]
fn the_estimate_bias_bands_are_signed_and_symmetric_around_the_meter() {
    assert_eq!(estimate_bias(None), "unknown");
    // Estimate matches the meter, and the two boundary cases either side.
    assert_eq!(estimate_bias(Some((50.0, 50.0))), "within_5");
    assert_eq!(estimate_bias(Some((50.0, 55.0))), "within_5");
    assert_eq!(estimate_bias(Some((50.0, 45.0))), "within_5");
    // The estimate under-shoots the meter: "low".
    assert_eq!(estimate_bias(Some((50.0, 44.9))), "low_5_to_20");
    assert_eq!(estimate_bias(Some((50.0, 30.0))), "low_5_to_20");
    assert_eq!(estimate_bias(Some((50.0, 29.9))), "low_over_20");
    // The estimate over-shoots the meter: "high".
    assert_eq!(estimate_bias(Some((50.0, 55.1))), "high_5_to_20");
    assert_eq!(estimate_bias(Some((50.0, 70.0))), "high_5_to_20");
    assert_eq!(estimate_bias(Some((50.0, 70.1))), "high_over_20");
}

#[test]
fn the_unexplained_band_is_a_share_of_the_last_readings_percent() {
    assert_eq!(unexplained_band(5.0, None), "unknown");
    // A reading of exactly zero cannot have a positive share.
    assert_eq!(unexplained_band(0.0, Some(0.0)), "none");
    // Boundaries, expressed as a percent of a 50-point reading.
    assert_eq!(unexplained_band(0.0, Some(50.0)), "none");
    assert_eq!(unexplained_band(0.49, Some(50.0)), "none"); // 0.98%
    assert_eq!(unexplained_band(0.5, Some(50.0)), "under_10"); // 1.0%
    assert_eq!(unexplained_band(4.9, Some(50.0)), "under_10"); // 9.8%
    assert_eq!(unexplained_band(5.0, Some(50.0)), "10_to_under_30"); // 10.0%
    assert_eq!(unexplained_band(14.9, Some(50.0)), "10_to_under_30"); // 29.8%
    assert_eq!(unexplained_band(15.0, Some(50.0)), "30_and_over"); // 30.0%
    assert_eq!(unexplained_band(50.0, Some(50.0)), "30_and_over");
}

#[test]
fn reading_coverage_is_covered_within_an_hour_of_the_windows_end() {
    assert_eq!(reading_coverage(None, 10_000), "none");
    assert_eq!(reading_coverage(Some(10_000 - 3_600), 10_000), "covered");
    assert_eq!(reading_coverage(Some(10_000), 10_000), "covered");
    assert_eq!(reading_coverage(Some(10_000 - 3_601), 10_000), "stale");
}

/// The complete wire surface, pinned.
///
/// This test cannot read the Privacy pane, so it does not pretend to: it
/// pins the exact set of keys that go out, and the pane's "Exactly what is
/// sent" row has to be updated in the same change as any diff here. An
/// earlier version of this test was named as though it checked the copy,
/// which is worse than not checking it — the name asserted a guarantee
/// nothing enforced, and five fields went unnamed in the pane for exactly
/// that reason. Adding a field below without touching
/// `apps/desktop/src/views/settings/PrivacyPane.tsx` is the bug this
/// comment exists to prevent.
#[test]
fn the_wire_payload_is_exactly_these_thirty_one_fields() {
    let json = serde_json::to_value(sample()).expect("serializes");
    let object = json.as_object().expect("an object");
    let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "anonymousId",
            "context",
            "event",
            "messageId",
            "originalTimestamp",
            "platform",
            "properties",
            "sessionId",
        ]
    );

    let mut property_keys: Vec<_> = object["properties"]
        .as_object()
        .expect("properties object")
        .keys()
        .map(String::as_str)
        .collect();
    property_keys.sort_unstable();
    assert_eq!(
        property_keys,
        [
            "arch",
            "bucket",
            "detail",
            "eligibility",
            "estimateBias",
            "experiment",
            "factorBand",
            "ineligibleReason",
            "label",
            "nextResetAvailable",
            "origin",
            "plan",
            "readingCoverage",
            "resetArm",
            "resetAvailability",
            "resetsPerWeek",
            "residualBand",
            "resourceUsage",
            "responseShape",
            "unexplainedBand",
            "unrecognizedTypes",
            "usageBand",
        ]
    );

    let mut context_keys: Vec<_> = object["context"]
        .as_object()
        .expect("context object")
        .keys()
        .map(String::as_str)
        .collect();
    context_keys.sort_unstable();
    assert_eq!(context_keys, ["appVersion", "os"]);
}

/// `sentAt` belongs to delivery, not to capture. A queued event that
/// waited an hour must not claim it was sent when it was recorded, so the
/// stamp is spliced in at the moment of the request instead.
#[test]
fn a_captured_event_carries_no_sent_at() {
    let json = serde_json::to_string(&sample()).expect("serializes");
    assert!(!json.contains("sentAt"), "{json}");
}

/// Nothing about the reader's account, because there is no account. The
/// contract's identify half is deliberately unimplemented.
#[test]
fn no_user_or_organisation_identity_is_ever_carried() {
    let json = serde_json::to_string(&sample()).expect("serializes");
    // `sessionId` is deliberately absent from this list. It identifies a
    // run of the process rather than a person.
    for forbidden in ["userId", "orgId", "email", "locale"] {
        assert!(!json.contains(forbidden), "{forbidden} in {json}");
    }
}

#[test]
fn absent_optional_fields_are_omitted_rather_than_null() {
    let mut event = sample();
    event.properties.bucket = None;
    event.properties.label = None;
    event.properties.detail = None;
    event.properties.origin = None;
    event.properties.usage_band = None;
    event.properties.response_shape = None;
    event.properties.eligibility = None;
    event.properties.ineligible_reason = None;
    event.properties.experiment = None;
    event.properties.reset_arm = None;
    event.properties.reset_availability = None;
    event.properties.resets_per_week = None;
    event.properties.next_reset_available = None;
    event.properties.plan = None;
    event.properties.factor_band = None;
    event.properties.residual_band = None;
    event.properties.estimate_bias = None;
    event.properties.unexplained_band = None;
    event.properties.reading_coverage = None;
    event.properties.resource_usage = None;
    event.properties.unrecognized_types = None;
    let json = serde_json::to_string(&event).expect("serializes");
    assert!(!json.contains("bucket"), "{json}");
    assert!(!json.contains("label"), "{json}");
    assert!(!json.contains("detail"), "{json}");
    assert!(!json.contains("\"origin\""), "{json}");
    assert!(!json.contains("\"plan\""), "{json}");
    assert!(!json.contains("factorBand"), "{json}");
    assert!(!json.contains("residualBand"), "{json}");
    assert!(!json.contains("estimateBias"), "{json}");
    assert!(!json.contains("unexplainedBand"), "{json}");
    assert!(!json.contains("readingCoverage"), "{json}");
    assert!(!json.contains("resourceUsage"), "{json}");
    assert!(!json.contains("unrecognizedTypes"), "{json}");
}

/// `unrecognizedTypes` appears as a JSON array only on the event it
/// belongs to. Every other event carries `None` for this field, so the
/// key is absent from its wire payload.
#[test]
fn unrecognized_types_appears_only_on_its_own_event() {
    let mut carrying = sample();
    carrying.event = EventName::UnrecognizedRecordsObserved.as_str().into();
    carrying.properties.unrecognized_types = Some(vec!["custom_event".to_string()]);
    let json = serde_json::to_value(&carrying).expect("serializes");
    let types = json["properties"]["unrecognizedTypes"]
        .as_array()
        .expect("unrecognizedTypes is an array");
    assert_eq!(types, &[serde_json::json!("custom_event")]);

    let mut other = sample();
    other.properties.unrecognized_types = None;
    let json = serde_json::to_string(&other).expect("serializes");
    assert!(!json.contains("unrecognizedTypes"), "{json}");
}

#[test]
fn resource_usage_has_only_the_typed_nested_allowlist() {
    let mut event = sample();
    event.event = EventName::ResourceUsageObserved.as_str().into();
    event.properties.resource_usage = Some(resource_summary());

    let json = serde_json::to_value(event).expect("serializes");
    let resource = json["properties"]["resourceUsage"]
        .as_object()
        .expect("resourceUsage object");
    let mut keys: Vec<_> = resource.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "cpuAverage",
            "cpuCoverage",
            "databaseCoverage",
            "databaseSize",
            "memoryCoverage",
            "memoryMax",
            "memoryMean",
            "readCoverage",
            "readRateAverage",
            "walCoverage",
            "walSize",
            "writeCoverage",
            "writeRateAverage",
        ]
    );
    assert_eq!(resource["cpuAverage"], "from10_to_under25_percent");
    assert_eq!(resource["readRateAverage"], "zero");
    assert_eq!(resource["writeRateAverage"], "unavailable");
}

/// User-origin events refresh the timeout; automatic and background
/// telemetry does not.
#[test]
fn cli_outcomes_have_only_closed_dimensions() {
    assert!(
        serde_json::from_value::<Interaction>(serde_json::json!({
            "kind": "cliOperationCompleted", "label": "startup", "detail": "succeeded"
        }))
        .is_err()
    );
    for (operation, label) in [
        (CliOperation::Startup, "startup"),
        (CliOperation::Test, "test"),
        (CliOperation::Repair, "repair"),
    ] {
        for succeeded in [false, true] {
            let facts = cli_operation_facts(operation, succeeded);
            assert_eq!(facts.label, Some(label));
            assert_eq!(
                facts.detail,
                Some(if succeeded { "succeeded" } else { "failed" })
            );
            assert_eq!(
                EventName::CliOperationCompleted.is_user_oriented(&facts),
                label != "startup"
            );
        }
    }
}

#[test]
fn user_oriented_classification_excludes_automatic_events() {
    assert!(EventName::SessionOpened.is_user_oriented(&Facts::default()));
    assert!(EventName::SettingToggled.is_user_oriented(&Facts::default()));
    assert!(EventName::SurfaceViewed.is_user_oriented(&Facts {
        detail: Some("user"),
        ..Facts::default()
    }));
    assert!(!EventName::SurfaceViewed.is_user_oriented(&Facts {
        detail: Some("automatic"),
        ..Facts::default()
    }));
    assert!(!EventName::SurfaceStateObserved.is_user_oriented(&Facts {
        origin: Some("user"),
        ..Facts::default()
    }));
    assert!(!EventName::ResourceUsageObserved.is_user_oriented(&Facts::default()));
    assert!(!EventName::AppLaunched.is_user_oriented(&Facts::default()));
    assert!(EventName::RemoteSyncCompleted.is_user_oriented(&Facts {
        detail: Some("manual"),
        ..Facts::default()
    }));
    assert!(!EventName::RemoteSyncCompleted.is_user_oriented(&Facts {
        detail: Some("automatic"),
        ..Facts::default()
    }));
    assert!(EventName::MemoryAction.is_user_oriented(&Facts::default()));
}

/// Only the step a Next press reveals is user activity; the others report
/// first-run work completing on its own.
#[test]
fn only_the_first_run_result_step_is_user_oriented() {
    for (step, user) in [
        (FirstRunStep::Started, false),
        (FirstRunStep::Found, false),
        (FirstRunStep::Read, false),
        (FirstRunStep::Checked, false),
        (FirstRunStep::Result, true),
    ] {
        let (name, facts) = Interaction::FirstRunStepReached {
            step,
            sessions: (step == FirstRunStep::Found).then_some(12),
            result: (step == FirstRunStep::Result).then_some(FirstRunResult::Clean),
        }
        .resolve();
        assert_eq!(name.is_user_oriented(&facts), user, "{step:?}");
    }
}

#[test]
fn model_check_settings_outcomes_refresh_the_session() {
    let mut lifecycles = vec![
        ModelCheckLifecycle::Enablement { enabled: true },
        ModelCheckLifecycle::Enablement { enabled: false },
    ];
    for provider in [
        DecisionProvider::Jev,
        DecisionProvider::Cloudflare,
        DecisionProvider::Ollama,
        DecisionProvider::Custom,
    ] {
        for outcome in [
            ProviderSetupOutcome::Saved,
            ProviderSetupOutcome::Switched,
            ProviderSetupOutcome::ConnectionRemoved,
            ProviderSetupOutcome::CredentialRemoved,
        ] {
            lifecycles.push(ModelCheckLifecycle::ProviderSetup { provider, outcome });
        }
        for outcome in [ProviderTestOutcome::Succeeded, ProviderTestOutcome::Failed] {
            lifecycles.push(ModelCheckLifecycle::ProviderTest { provider, outcome });
        }
    }
    for lifecycle in lifecycles {
        let (name, facts) = lifecycle.resolve();
        assert!(name.is_user_oriented(&facts), "{lifecycle:?}");
    }
}

/// The compiler, not a reviewer, keeps [`EVERY_EVENT`] complete.
///
/// A new variant makes this match non-exhaustive, which fails the build at
/// this line — the one place that then points at the catalog and at the
/// document the event has to appear in.
#[test]
fn no_variant_escapes_the_catalog() {
    fn listed(event: EventName) -> bool {
        match event {
            EventName::CliOperationCompleted
            | EventName::AppLaunched
            | EventName::ScanCompleted
            | EventName::SettingToggled
            | EventName::AnalyticsOptedOut
            | EventName::SessionOpened
            | EventName::ErrorOccurred
            | EventName::UnrecognizedRecordsObserved
            | EventName::SurfaceViewed
            | EventName::SettingsPaneViewed
            | EventName::SurfaceStateObserved
            | EventName::LiveUsageStateObserved
            | EventName::ClaudeLimitResetObserved
            | EventName::ClaudeLoginObserved
            | EventName::UsageObserved
            | EventName::LimitFactorObserved
            | EventName::ResourceUsageObserved
            | EventName::BurnCheckAutoFixReviewed
            | EventName::BurnCheckAutoFixConfirmed
            | EventName::BurnCheckAutoFixCompleted
            | EventName::BurnCheckPromptPrepared
            | EventName::BurnCheckPromptCopied
            | EventName::BurnCheckOutcomeObserved
            | EventName::BurnCheckInputPrepared
            | EventName::BurnCheckDefinitionOpened
            | EventName::BurnCheckDirectoryOpened
            | EventName::ProjectFolderAction
            | EventName::MemoryAction
            | EventName::SessionFilterSelected
            | EventName::SessionFiltersChanged
            | EventName::RemoteHostConnectionChecked
            | EventName::RemoteHostChanged
            | EventName::RemoteSyncCompleted
            | EventName::QuotaIncidentsObserved
            | EventName::ProviderIncidentsObserved
            | EventName::ProviderIncidentsIngested
            | EventName::QuotaWindowClosed
            | EventName::NavigationHistoryMoved
            | EventName::AppSearchOpened
            | EventName::AppSearchResultOpened
            | EventName::PromotionInteraction
            | EventName::UsageLimitsInteraction
            | EventName::InterfaceScaleChanged
            | EventName::OnboardingScreenViewed
            | EventName::OnboardingActionTaken
            | EventName::OnboardingCloudOutcome
            | EventName::OnboardingCompleted
            | EventName::FirstRunStepReached
            | EventName::FirstRunAction
            | EventName::FirstRunFinished
            | EventName::ModelCheckLifecycle
            | EventName::CheckEnablementSaved
            | EventName::StepSettingsViewed => true,
        }
    }
    assert_eq!(
        EVERY_EVENT.len(),
        53,
        "a variant was added to the match above but not to EVERY_EVENT"
    );
    assert!(EVERY_EVENT.iter().copied().all(listed));
}

#[test]
fn check_enablement_ids_cover_all_nine_detectors_with_stable_values() {
    use antiburn_local::checks::DetectorId;

    let values: Vec<_> = DetectorId::ALL
        .into_iter()
        .map(|detector| CheckEnablementId::from(detector).as_str())
        .collect();

    assert_eq!(
        values,
        [
            "sessions_over_depth",
            "model_overthinking",
            "overpowered_subagents",
            "unused_mcp_servers",
            "unused_built_in_tools",
            "unused_skills",
            "old_model_usage",
            "overuse_of_fast_mode",
            "cache_churn",
        ]
    );
    for key in RETIRED_CHECKS {
        assert!(DetectorId::from_key(key).is_none(), "{key}");
    }
}

#[test]
fn check_enablement_facts_carry_only_check_and_saved_state() {
    use antiburn_local::checks::DetectorId;

    for detector in DetectorId::ALL {
        for enabled in [false, true] {
            let facts = check_enablement_facts(detector, enabled);
            let name = EventName::CheckEnablementSaved;
            assert!(name.is_user_oriented(&facts));
            assert_closed_payload(
                (name, facts),
                "antiburn.check_enablement_saved",
                Some(detector.key()),
                Some(if enabled { "enabled" } else { "disabled" }),
            );
        }
    }
}

/// The public catalog in `docs/analytics.md` is a promise to a
/// reader who cannot read this file, so it cannot be allowed to drift.
/// Adding a variant above without documenting it fails here rather than
/// shipping a document that quietly under-reports what is sent.
#[test]
fn the_documented_catalog_matches_the_code() {
    let doc = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../docs/analytics.md"
    ));
    for name in EVERY_EVENT {
        assert!(
            doc.contains(name.as_str()),
            "{} is sent but is not in docs/analytics.md",
            name.as_str()
        );
    }
}

/// Every document that counts the fields counts the same number.
///
/// Three of them do, in the reader's own words, and none is generated
/// from this struct — so the count is a hand-copied number in four
/// places. It has already drifted once: `detail` was added, three
/// documents were updated to thirteen, and the privacy policy went on
/// saying twelve. Counting from the payload here is the only way that
/// stays true without somebody remembering.
#[test]
fn every_document_that_counts_the_fields_counts_the_same_number() {
    let json = serde_json::to_value(sample()).expect("serializes");
    let object = json.as_object().expect("an object");
    let counted = object
        .values()
        .map(|value| match value.as_object() {
            // `properties` and `context` are containers; their leaves are
            // what a reader counts, not the container itself.
            Some(nested) => nested.len(),
            None => 1,
        })
        .sum::<usize>()
        // `sentAt` is stamped at delivery rather than at capture, so it
        // is absent from the struct and present on the wire. A reader
        // counting what arrives counts it.
        + 1;

    let word = match counted {
        12 => "twelve",
        13 => "thirteen",
        14 => "fourteen",
        22 => "twenty-two",
        23 => "twenty-three",
        27 => "twenty-seven",
        28 => "twenty-eight",
        31 => "thirty-one",
        other => panic!("no word for {other} fields; add one and update the documents"),
    };

    for path in [
        "/../../../docs/analytics.md",
        "/../../../docs/privacy-policy.md",
        "/../../../docs/support.md",
        "/../src/views/settings/PrivacyPane.tsx",
    ] {
        let doc = std::fs::read_to_string(env!("CARGO_MANIFEST_DIR").to_owned() + path)
            .unwrap_or_else(|error| panic!("{path}: {error}"));
        assert!(
            doc.to_lowercase().contains(&format!("{word} fields")),
            "{path} does not say \"{word} fields\"; the payload now has {counted}"
        );
    }
}

/// An interaction resolves to constants this file owns, never to
/// anything the renderer supplied verbatim.
#[test]
fn an_interaction_resolves_to_this_files_own_constants() {
    let (name, facts) = Interaction::SessionOpened {
        agent: AgentKind::Claude,
        environment: Environment::Wsl,
    }
    .resolve();
    assert_eq!(name, EventName::SessionOpened);
    assert_eq!(facts.label, Some("claude-code"));
    assert_eq!(facts.detail, Some("wsl"));
    assert_eq!(facts.bucket, None);

    let (name, facts) = Interaction::SurfaceStateObserved {
        surface: StateSurface::ProviderPreview,
        state: SurfaceState::LoadingTimeout,
        origin: Origin::User,
    }
    .resolve();
    assert_eq!(name, EventName::SurfaceStateObserved);
    assert_eq!(facts.label, Some("provider_preview"));
    assert_eq!(facts.detail, Some("loading_timeout"));
    assert_eq!(facts.origin, Some("user"));

    let (name, facts) = Interaction::LiveUsageStateObserved {
        provider: LiveUsageProvider::Google,
        state: LiveUsageState::RateLimited,
        origin: Origin::User,
    }
    .resolve();
    assert_eq!(name, EventName::LiveUsageStateObserved);
    assert_eq!(facts.label, Some("google"));
    assert_eq!(facts.detail, Some("rate_limited"));
    assert_eq!(facts.origin, None);

    let (_, facts) = Interaction::LiveUsageStateObserved {
        provider: LiveUsageProvider::Anthropic,
        state: LiveUsageState::LoginRecovering,
        origin: Origin::User,
    }
    .resolve();
    assert_eq!(facts.label, Some("anthropic"));
    assert_eq!(facts.detail, Some("login_recovering"));

    let (name, facts) = Interaction::BurnCheckAutoFixCompleted {
        outcome: AutoFixOutcome::RecoveryNeeded,
    }
    .resolve();
    assert_eq!(name, EventName::BurnCheckAutoFixCompleted);
    assert_eq!(facts.detail, Some("recovery_needed"));
    assert_eq!(facts.label, None);

    let (name, facts) = Interaction::BurnCheckOutcomeObserved {
        outcome: BurnCheckOutcome::Recurred,
        origin: BurnCheckOrigin::Passive,
    }
    .resolve();
    assert_eq!(name, EventName::BurnCheckOutcomeObserved);
    assert_eq!(facts.detail, Some("recurred"));
    assert_eq!(facts.origin, Some("passive"));

    let (name, facts) = Interaction::SessionFilterSelected {
        filter: SessionFilterKind::Agent,
        agent: Some(AgentKind::Codex),
    }
    .resolve();
    assert_eq!(name, EventName::SessionFilterSelected);
    assert_eq!(facts.label, Some("agent"));
    assert_eq!(facts.detail, Some("codex"));

    let (name, facts) = Interaction::SessionFilterSelected {
        filter: SessionFilterKind::Notable,
        agent: None,
    }
    .resolve();
    assert_eq!(name, EventName::SessionFilterSelected);
    assert_eq!(facts.label, Some("notable"));
    assert_eq!(facts.detail, None);
}

#[test]
fn burn_check_interactions_only_resolve_closed_safe_facts() {
    fn assert_safe(facts: Facts, detail: Option<&'static str>, origin: Option<&'static str>) {
        assert_eq!(facts.detail, detail);
        assert_eq!(facts.origin, origin);
        assert_eq!(facts.bucket, None);
        assert_eq!(facts.label, None);
        assert_eq!(facts.usage_band, None);
        assert_eq!(facts.response_shape, None);
        assert_eq!(facts.eligibility, None);
        assert_eq!(facts.ineligible_reason, None);
        assert_eq!(facts.experiment, None);
        assert_eq!(facts.reset_arm, None);
        assert_eq!(facts.reset_availability, None);
        assert_eq!(facts.resets_per_week, None);
        assert_eq!(facts.next_reset_available, None);
        assert_eq!(facts.plan, None);
        assert_eq!(facts.factor_band, None);
        assert_eq!(facts.residual_band, None);
        assert_eq!(facts.resource_usage, None);
        assert_eq!(facts.unrecognized_types, None);
    }

    for outcome in [
        AutoFixReviewOutcome::Ready,
        AutoFixReviewOutcome::Stale,
        AutoFixReviewOutcome::Expired,
        AutoFixReviewOutcome::Conflict,
        AutoFixReviewOutcome::Unavailable,
        AutoFixReviewOutcome::Failed,
    ] {
        let (_, facts) = Interaction::BurnCheckAutoFixReviewed { outcome }.resolve();
        assert_safe(facts, Some(outcome.as_str()), None);
    }
    let (_, facts) = Interaction::BurnCheckAutoFixConfirmed.resolve();
    assert_safe(facts, None, None);
    for outcome in [
        AutoFixOutcome::AppliedAwaitingVerification,
        AutoFixOutcome::AppliedVerificationUnavailable,
        AutoFixOutcome::RecoveryNeeded,
        AutoFixOutcome::Stale,
        AutoFixOutcome::Expired,
        AutoFixOutcome::Conflict,
        AutoFixOutcome::Unavailable,
        AutoFixOutcome::Failed,
    ] {
        let (_, facts) = Interaction::BurnCheckAutoFixCompleted { outcome }.resolve();
        assert_safe(facts, Some(outcome.as_str()), None);
    }
    for (outcome, origin) in [
        (BurnCheckOutcome::Verified, BurnCheckOrigin::Passive),
        (BurnCheckOutcome::Verified, BurnCheckOrigin::Action),
        (BurnCheckOutcome::Recurred, BurnCheckOrigin::Passive),
        (BurnCheckOutcome::Recurred, BurnCheckOrigin::Action),
    ] {
        let (_, facts) = Interaction::BurnCheckOutcomeObserved { outcome, origin }.resolve();
        assert_safe(facts, Some(outcome.as_str()), Some(origin.as_str()));
    }
}

/// An agent filter with no recognized harness reports the filter kind
/// alone. The renderer never sends a slug this enum rejects; `None` is
/// what a genuinely unrecognized harness (or a non-agent filter) looks
/// like here.
#[test]
fn an_agent_filter_with_no_recognized_harness_has_no_detail() {
    let (name, facts) = Interaction::SessionFilterSelected {
        filter: SessionFilterKind::Agent,
        agent: None,
    }
    .resolve();
    assert_eq!(name, EventName::SessionFilterSelected);
    assert_eq!(facts.label, Some("agent"));
    assert_eq!(facts.detail, None);
}

#[test]
fn session_filters_changed_uses_closed_actions_and_scopes_agent_detail() {
    for action in [
        SessionFilterAction::AgentAdded,
        SessionFilterAction::AgentRemoved,
    ] {
        let (name, facts) = Interaction::SessionFiltersChanged {
            action,
            agent: Some(AgentKind::Codex),
        }
        .resolve();
        assert_eq!(name, EventName::SessionFiltersChanged);
        assert_eq!(facts.label, Some(action.as_str()));
        assert_eq!(facts.detail, Some("codex"));
    }
    for action in [
        SessionFilterAction::AgentsAll,
        SessionFilterAction::ResultFailing,
        SessionFilterAction::ResultPassing,
        SessionFilterAction::ResultAll,
        SessionFilterAction::SpendNotable,
        SessionFilterAction::SpendMaterial,
        SessionFilterAction::SpendAll,
        SessionFilterAction::ClearedAll,
    ] {
        let (_, facts) = Interaction::SessionFiltersChanged {
            action,
            agent: Some(AgentKind::Codex),
        }
        .resolve();
        assert_eq!(facts.label, Some(action.as_str()));
        assert_eq!(facts.detail, None);
    }
}

/// `usage_observed` reads the same closed vocabulary
/// `live_usage_state_observed` does, rather than trusting a new source's
/// provider id outright. A provider this build does not recognize maps to
/// `None`, so the caller skips it instead of inventing a fourth label.
#[test]
fn an_unrecognised_provider_id_has_no_usage_observed_label() {
    assert_eq!(
        LiveUsageProvider::from_provider_id(crate::provider_usage::providers::ANTHROPIC),
        Some(LiveUsageProvider::Anthropic)
    );
    assert_eq!(
        LiveUsageProvider::from_provider_id(crate::provider_usage::providers::OPENAI),
        Some(LiveUsageProvider::Openai)
    );
    assert_eq!(
        LiveUsageProvider::from_provider_id(crate::provider_usage::providers::GOOGLE),
        Some(LiveUsageProvider::Google)
    );
    assert_eq!(
        LiveUsageProvider::from_provider_id("some-future-provider"),
        None
    );
}

/// The renderer cannot invent a value. This is the whole reason the IPC
/// edge takes a closed enum rather than a name and a map: a call site in
/// TypeScript that passed a repository name would not compile into
/// anything the collector could receive — it fails at the command
/// boundary instead.
#[test]
fn an_unrecognised_interaction_is_refused_at_the_boundary() {
    let unknown_agent = serde_json::json!({
        "kind": "sessionOpened",
        "agent": "some-repository-name",
        "environment": "native",
    });
    assert!(serde_json::from_value::<Interaction>(unknown_agent).is_err());

    let unknown_shape = serde_json::json!({
        "kind": "somethingElse",
        "path": "/Users/someone/work",
    });
    assert!(serde_json::from_value::<Interaction>(unknown_shape).is_err());

    let unknown_environment = serde_json::json!({
        "kind": "sessionOpened",
        "agent": "claude-code",
        "environment": "Ubuntu-24.04",
    });
    assert!(serde_json::from_value::<Interaction>(unknown_environment).is_err());

    let extra_property = serde_json::json!({
        "kind": "surfaceViewed",
        "surface": "activity",
        "origin": "user",
        "repository": "private-name",
    });
    assert!(serde_json::from_value::<Interaction>(extra_property).is_err());

    let unknown_origin = serde_json::json!({
        "kind": "surfaceViewed",
        "surface": "activity",
        "origin": "background_poll",
    });
    assert!(serde_json::from_value::<Interaction>(unknown_origin).is_err());

    let private_action_fields = serde_json::json!({
        "kind": "burnCheckAutoFixCompleted",
        "outcome": "applied_awaiting_verification",
        "path": "/Users/someone/work",
        "watchId": "private-watch",
    });
    assert!(serde_json::from_value::<Interaction>(private_action_fields).is_err());

    let unknown_result = serde_json::json!({
        "kind": "burnCheckOutcomeObserved",
        "outcome": "fixed_with_1234_tokens",
        "origin": "action",
    });
    assert!(serde_json::from_value::<Interaction>(unknown_result).is_err());

    let unknown_filter_action = serde_json::json!({
        "kind": "sessionFiltersChanged",
        "action": "future_action",
        "agent": "codex",
    });
    assert!(serde_json::from_value::<Interaction>(unknown_filter_action).is_err());

    let unknown_filter_agent = serde_json::json!({
        "kind": "sessionFiltersChanged",
        "action": "agent_added",
        "agent": "private-harness",
    });
    assert!(serde_json::from_value::<Interaction>(unknown_filter_agent).is_err());

    let extra_filter_property = serde_json::json!({
        "kind": "sessionFiltersChanged",
        "action": "result_all",
        "repository": "private-name",
    });
    assert!(serde_json::from_value::<Interaction>(extra_filter_property).is_err());
}

#[test]
fn every_event_name_sits_in_antiburns_own_namespace() {
    for name in EVERY_EVENT {
        assert!(name.as_str().starts_with("antiburn."), "{}", name.as_str());
    }
}

#[test]
fn navigation_events_have_closed_facts_and_enums() {
    let cases = [
        (
            Interaction::NavigationHistoryMoved {
                direction: HistoryDirection::Back,
            },
            EventName::NavigationHistoryMoved,
            Some("back"),
        ),
        (
            Interaction::AppSearchOpened {},
            EventName::AppSearchOpened,
            None,
        ),
        (
            Interaction::AppSearchResultOpened {
                category: SearchCategory::Check,
            },
            EventName::AppSearchResultOpened,
            Some("check"),
        ),
    ];
    for (interaction, expected_name, expected_label) in cases {
        let (name, facts) = interaction.resolve();
        assert_eq!(name, expected_name);
        assert_eq!(facts.label, expected_label);
    }
    for value in [
        serde_json::json!({"kind":"navigationHistoryMoved","direction":"sideways"}),
        serde_json::json!({"kind":"appSearchOpened","extra":true}),
        serde_json::json!({"kind":"appSearchResultOpened","category":"session"}),
        serde_json::json!({"kind":"appSearchResultOpened","category":"step_setting"}),
    ] {
        assert!(serde_json::from_value::<Interaction>(value).is_err());
    }
}

/// Each fixed funnel step carries only the facts the catalog documents
/// for it: `found` buckets its raw session count rather than sending it
/// exact, and `result` carries the closed result vocabulary. The other
/// steps carry neither.
#[test]
fn first_run_step_reached_carries_only_the_facts_its_own_step_defines() {
    let (name, facts) = Interaction::FirstRunStepReached {
        step: FirstRunStep::Started,
        sessions: None,
        result: None,
    }
    .resolve();
    assert_eq!(name, EventName::FirstRunStepReached);
    assert_eq!(facts.label, Some("started"));
    assert_eq!(facts.bucket, None);
    assert_eq!(facts.detail, None);

    let (_, facts) = Interaction::FirstRunStepReached {
        step: FirstRunStep::Found,
        sessions: Some(49),
        result: None,
    }
    .resolve();
    assert_eq!(facts.label, Some("found"));
    assert_eq!(facts.bucket, Some(bucket(49)));
    assert_eq!(facts.bucket, Some("10-49"));
    assert_eq!(facts.detail, None);

    let (_, facts) = Interaction::FirstRunStepReached {
        step: FirstRunStep::Read,
        sessions: None,
        result: None,
    }
    .resolve();
    assert_eq!(facts.label, Some("read"));
    assert_eq!(facts.bucket, None);

    let (_, facts) = Interaction::FirstRunStepReached {
        step: FirstRunStep::Checked,
        sessions: None,
        result: None,
    }
    .resolve();
    assert_eq!(facts.label, Some("checked"));
    assert_eq!(facts.bucket, None);

    for (result, expected) in [
        (FirstRunResult::Empty, "empty"),
        (FirstRunResult::ChecksDisabled, "checks_disabled"),
        (FirstRunResult::NotAssessed, "not_assessed"),
        (FirstRunResult::Clean, "clean"),
        (FirstRunResult::FixesFound, "fixes_found"),
    ] {
        let (_, facts) = Interaction::FirstRunStepReached {
            step: FirstRunStep::Result,
            sessions: None,
            result: Some(result),
        }
        .resolve();
        assert_eq!(facts.label, Some("result"));
        assert_eq!(facts.bucket, None);
        assert_eq!(facts.detail, Some(expected));
        let accepted: Interaction = serde_json::from_value(serde_json::json!({
            "kind": "firstRunStepReached", "step": "result", "result": expected
        }))
        .expect("a documented first-run result is accepted");
        assert_eq!(accepted.resolve().1.detail, Some(expected));
    }
}

#[test]
fn first_run_action_uses_closed_vocabulary() {
    for (action, expected) in [
        (
            FirstRunActionKind::FolderAccessRequested,
            "folder_access_requested",
        ),
        (
            FirstRunActionKind::FolderAccessGranted,
            "folder_access_granted",
        ),
        (
            FirstRunActionKind::IncludeNonRepoFolders,
            "include_non_repo_folders",
        ),
        (FirstRunActionKind::LiveUsageStarted, "live_usage_started"),
        (FirstRunActionKind::LiveUsageSkipped, "live_usage_skipped"),
        (FirstRunActionKind::EnhanceOpened, "enhance_opened"),
    ] {
        let (name, facts) = Interaction::FirstRunAction { action }.resolve();
        assert_eq!(name, EventName::FirstRunAction);
        assert_eq!(facts.label, Some(expected));
    }
}

#[test]
fn step_settings_viewed_uses_closed_vocabulary() {
    for (label, expected_label) in [
        (StepSettingsLabel::Agents, "agents"),
        (StepSettingsLabel::Limits, "limits"),
        (StepSettingsLabel::Sessions, "sessions"),
        (StepSettingsLabel::Checks, "checks"),
    ] {
        let (name, facts) = Interaction::StepSettingsViewed {
            label,
            detail: StepSettingsDetail::FirstRun,
        }
        .resolve();
        assert_eq!(name, EventName::StepSettingsViewed);
        assert_eq!(facts.label, Some(expected_label));
        assert_eq!(facts.detail, Some("first_run"));
    }
    for value in [
        serde_json::json!({"kind":"stepSettingsViewed","label":"usage","detail":"first_run"}),
        serde_json::json!({"kind":"stepSettingsViewed","label":"agents","detail":"settings"}),
        serde_json::json!({"kind":"stepSettingsViewed","label":"agents","detail":"modal"}),
        serde_json::json!({"kind":"stepSettingsViewed","label":"agents","detail":"first_run","extra":true}),
    ] {
        assert!(serde_json::from_value::<Interaction>(value).is_err());
    }
}

#[test]
fn settings_pane_viewed_accepts_every_settings_pane() {
    for (pane, expected) in [
        ("general", "general"),
        ("agents", "agents"),
        ("sessions", "sessions"),
        ("checks", "checks"),
        ("notifications", "notifications"),
        ("usage", "usage"),
        ("appearance", "appearance"),
        ("privacy", "privacy"),
        ("about", "about"),
    ] {
        let interaction: Interaction =
            serde_json::from_value(serde_json::json!({"kind":"settingsPaneViewed","pane":pane}))
                .expect(pane);
        let (name, facts) = interaction.resolve();
        assert_eq!(name, EventName::SettingsPaneViewed);
        assert_eq!(facts.label, Some(expected));
    }
    assert!(
        serde_json::from_value::<Interaction>(
            serde_json::json!({"kind":"settingsPaneViewed","pane":"sources"})
        )
        .is_err()
    );
}

#[test]
fn first_run_finished_carries_no_properties() {
    let (name, facts) = Interaction::FirstRunFinished {}.resolve();
    assert_eq!(name, EventName::FirstRunFinished);
    assert_eq!(facts.label, None);
    assert_eq!(facts.bucket, None);
    assert_eq!(facts.detail, None);
}

/// The renderer cannot send an unlisted step, action, or result, and
/// cannot attach a property the schema does not define for that shape.
#[test]
fn first_run_interactions_are_refused_at_the_boundary_for_unknown_values() {
    for value in [
        serde_json::json!({"kind":"firstRunStepReached","step":"welcome"}),
        serde_json::json!({"kind":"firstRunStepReached","step":"result","result":"broken"}),
        serde_json::json!({"kind":"firstRunStepReached","step":"started","path":"/private"}),
        serde_json::json!({"kind":"firstRunAction","action":"folder_opened"}),
        serde_json::json!({"kind":"firstRunAction","action":"live_usage_stopped"}),
        serde_json::json!({"kind":"firstRunAction"}),
        serde_json::json!({"kind":"firstRunFinished","result":"clean"}),
    ] {
        assert!(
            serde_json::from_value::<Interaction>(value.clone()).is_err(),
            "{value}"
        );
    }
}

#[test]
fn claude_login_observations_use_closed_vocabularies() {
    use crate::provider_usage::live::LoginObservation;

    let facts = claude_login_facts(LoginObservation {
        label: "sign_in_required",
        detail: Some("cannot_refresh"),
    })
    .expect("a closed value");
    assert_eq!(facts.label, Some("sign_in_required"));
    assert_eq!(facts.detail, Some("cannot_refresh"));
    assert_eq!(facts.usage_band, None);

    let facts = claude_login_facts(LoginObservation {
        label: "keychain_metadata_failed",
        detail: None,
    })
    .expect("a closed value");
    assert_eq!(facts.detail, None);

    for observation in [
        LoginObservation {
            label: "synthetic-token",
            detail: None,
        },
        LoginObservation {
            label: "refreshed",
            detail: Some("/fixture/path"),
        },
    ] {
        assert!(claude_login_facts(observation).is_none());
    }
}

#[test]
fn sign_in_states_have_closed_wire_values() {
    for (state, wire) in [
        (LiveUsageState::SignInRequired, "sign_in_required"),
        (LiveUsageState::NotSignedIn, "not_signed_in"),
    ] {
        assert_eq!(
            serde_json::from_value::<LiveUsageState>(serde_json::json!(wire)).unwrap(),
            state
        );
        let (_, facts) = Interaction::LiveUsageStateObserved {
            provider: LiveUsageProvider::Anthropic,
            state,
            origin: Origin::User,
        }
        .resolve();
        assert_eq!(facts.detail, Some(wire));
    }
}

#[test]
fn promotion_views_do_not_extend_user_activity() {
    for action in [
        PromotionAction::AnnouncementViewed,
        PromotionAction::SetupViewed,
        PromotionAction::PrimaryActivated,
        PromotionAction::LearnMoreActivated,
        PromotionAction::CommandCopied,
        PromotionAction::Dismissed,
    ] {
        let (name, facts) = Interaction::PromotionUsed {
            campaign: PromotionCampaign::ClaudeMod,
            action,
            origin: Origin::Automatic,
        }
        .resolve();
        assert_eq!(
            name.is_user_oriented(&facts),
            !matches!(
                action,
                PromotionAction::AnnouncementViewed | PromotionAction::SetupViewed
            )
        );
    }
}

#[test]
fn promotion_interactions_resolve_only_closed_actions_and_origins() {
    let cases = [
        (PromotionAction::AnnouncementViewed, "announcement_viewed"),
        (PromotionAction::SetupViewed, "setup_viewed"),
        (PromotionAction::PrimaryActivated, "primary_activated"),
        (PromotionAction::LearnMoreActivated, "learn_more_activated"),
        (PromotionAction::CommandCopied, "command_copied"),
        (PromotionAction::Dismissed, "dismissed"),
    ];

    for (action, label) in cases {
        for origin in [Origin::User, Origin::Automatic] {
            let (name, facts) = Interaction::PromotionUsed {
                campaign: PromotionCampaign::ClaudeMod,
                action,
                origin,
            }
            .resolve();
            assert_eq!(name, EventName::PromotionInteraction);
            assert_eq!(name.as_str(), "antiburn.promotion_interaction");
            assert_eq!(facts.label, Some(label));
            assert_eq!(facts.origin, Some(origin.as_str()));
            assert_eq!(facts.detail, Some("claude_mod"));
        }
    }

    let valid = serde_json::json!({
        "kind": "promotionInteraction",
        "campaign": "claude_mod",
        "action": "command_copied",
        "origin": "user"
    });
    let decoded = serde_json::from_value::<Interaction>(valid.clone()).unwrap();
    assert_eq!(decoded.resolve().1.detail, Some("claude_mod"));
    for campaign in [
        serde_json::json!("unregistered"),
        serde_json::json!(null),
        serde_json::json!(12),
    ] {
        let mut invalid = valid.clone();
        invalid["campaign"] = campaign;
        assert!(serde_json::from_value::<Interaction>(invalid).is_err());
    }
    let mut missing_campaign = valid;
    missing_campaign.as_object_mut().unwrap().remove("campaign");
    assert!(serde_json::from_value::<Interaction>(missing_campaign).is_err());

    for value in [
        serde_json::json!({
            "kind": "promotionInteraction",
            "campaign": "claude_mod",
            "action": "installed",
            "origin": "user"
        }),
        serde_json::json!({
            "kind": "promotionInteraction",
            "campaign": "claude_mod",
            "action": "command_copied",
            "origin": "launch_background"
        }),
        serde_json::json!({
            "kind": "promotionInteraction",
            "campaign": "claude_mod",
            "action": "command_copied",
            "origin": "user",
            "command": "private command text"
        }),
    ] {
        assert!(serde_json::from_value::<Interaction>(value).is_err());
    }
}

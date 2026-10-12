use super::*;

#[test]
fn check_progress_serializes_optional_notices_and_unknown_total() {
    let category = ChecksCategoryPayload {
        id: BurnCheckDetectorId::ScopeCreep,
        sampled: true,
        checking: Some(true),
        checking_count: Some(2),
        partial_context: Some(true),
        review_coverage: Some(ChecksReviewCoveragePayload {
            reviewed: 5,
            total: None,
            uncertain: Some(2),
            pending: Some(3),
            pending_completion: None,
            continuing: true,
        }),
        lifecycle: None,
        finding: 0,
        agents: Vec::new(),
        clean: 0,
        unavailable: 1,
        estimated_token_burn_basis_points: None,
    };
    let value = serde_json::to_value(&category).unwrap();
    assert_eq!(value["checking"], true);
    assert_eq!(value["checkingCount"], 2);
    assert_eq!(value["partialContext"], true);
    assert_eq!(
        value["reviewCoverage"],
        serde_json::json!({
            "reviewed": 5, "total": null, "uncertain": 2, "pending": 3, "pendingCompletion": null, "continuing": true,
        })
    );
    assert_eq!(value["lifecycle"], serde_json::Value::Null);
    assert_eq!(value["clean"], 0);
}

#[test]
fn review_coverage_serializes_terminal_uncertainty_and_known_total() {
    let coverage = ChecksReviewCoveragePayload {
        reviewed: 2,
        total: Some(3),
        uncertain: Some(1),
        pending: Some(1),
        pending_completion: Some(0),
        continuing: false,
    };
    assert_eq!(
        serde_json::to_value(coverage).unwrap(),
        serde_json::json!({
            "reviewed": 2, "total": 3, "uncertain": 1, "pending": 1, "pendingCompletion": 0, "continuing": false,
        })
    );
}

#[test]
fn quota_usage_payload_serializes_camel_case_fields_and_boundary_source_strings() {
    let payload = QuotaUsagePayload {
        provider: "anthropic".to_string(),
        account_key: "a".repeat(64),
        lane: "fiveHour".to_string(),
        lane_label: "5-hour".to_string(),
        range_start_epoch: 0,
        range_end_epoch: 1_000,
        periods: vec![QuotaPeriodPayload {
            period_id: None,
            starts_at_epoch: 0,
            resets_at_epoch: 1_000,
            start_source: "turnGap".to_string(),
            reset_source: "cadence".to_string(),
            samples: vec![QuotaSamplePayload {
                observed_at_epoch: 500,
                used_percent: Some(10.0),
                fresh: true,
                authoritative: true,
            }],
            contributions: vec![QuotaContributionPayload {
                agent: "claude-code".to_string(),
                session_id: "s1".to_string(),
                wsl_distro: None,
                remote_host_id: None,
                bucket_start_epoch: 0,
                usd: 1.0,
                percent: Some(2.0),
            }],
            sessions: vec![QuotaSessionTotalPayload {
                agent: "claude-code".to_string(),
                session_id: "s1".to_string(),
                wsl_distro: None,
                remote_host_id: None,
                title: Some("Fix the bug".to_string()),
                usd: 1.0,
                percent: Some(2.0),
            }],
            unattributed: QuotaUnattributedPayload {
                usd: 0.5,
                percent: Some(1.0),
                session_count: 1,
            },
            unattributed_buckets: vec![QuotaBucketTotalPayload {
                bucket_start_epoch: 0,
                usd: 0.5,
                percent: Some(1.0),
            }],
            estimated_percent: Some(2.0),
            unexplained_buckets: vec![QuotaBucketTotalPayload {
                bucket_start_epoch: 0,
                usd: 0.0,
                percent: Some(0.5),
            }],
            unexplained_percent: Some(0.5),
        }],
        generated_at: "2026-09-16T00:00:00Z".to_string(),
    };
    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["accountKey"], "a".repeat(64));
    assert_eq!(json["laneLabel"], "5-hour");
    assert_eq!(json["rangeStartEpoch"], 0);
    assert_eq!(json["rangeEndEpoch"], 1_000);
    let period = &json["periods"][0];
    assert_eq!(period["periodId"], serde_json::Value::Null);
    assert_eq!(period["startsAtEpoch"], 0);
    assert_eq!(period["resetsAtEpoch"], 1_000);
    assert_eq!(period["startSource"], "turnGap");
    assert_eq!(period["resetSource"], "cadence");
    assert_eq!(period["contributions"][0]["bucketStartEpoch"], 0);
    assert_eq!(period["sessions"][0]["sessionId"], "s1");
    assert_eq!(period["unattributed"]["sessionCount"], 1);
    assert_eq!(period["unattributedBuckets"][0]["bucketStartEpoch"], 0);
    assert_eq!(period["unattributedBuckets"][0]["usd"], 0.5);
    assert_eq!(period["unattributedBuckets"][0]["percent"], 1.0);
    assert_eq!(period["estimatedPercent"], 2.0);
    assert_eq!(period["unexplainedBuckets"][0]["usd"], 0.0);
    assert_eq!(period["unexplainedBuckets"][0]["percent"], 0.5);
    assert_eq!(period["unexplainedPercent"], 0.5);
}

#[test]
fn session_quota_payload_serializes_camel_case_fields_and_confidence_strings() {
    let payload = SessionQuotaPayload {
        entries: vec![
            SessionQuotaEntryPayload {
                provider: "anthropic".to_string(),
                display_name: "Claude".to_string(),
                account_key: Some("a".repeat(64)),
                lane: Some("weekly".to_string()),
                lane_label: Some("Weekly".to_string()),
                period: Some(SessionQuotaPeriodPayload {
                    period_id: Some(7),
                    starts_at_epoch: 0,
                    resets_at_epoch: 604_800,
                    start_source: "reported".to_string(),
                    reset_source: "derived".to_string(),
                }),
                usd: 1.0,
                percent: Some(2.0),
                confidence: "learned".to_string(),
                plan: Some(LiveProviderPlan {
                    name: "max".to_string(),
                    tier: Some("max_20x".to_string()),
                }),
            },
            SessionQuotaEntryPayload {
                provider: "openai".to_string(),
                display_name: "Codex".to_string(),
                account_key: None,
                lane: None,
                lane_label: None,
                period: None,
                usd: 0.5,
                percent: None,
                confidence: "unbound".to_string(),
                plan: None,
            },
        ],
        generated_at: "2026-09-16T00:00:00Z".to_string(),
    };
    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["entries"][0]["displayName"], "Claude");
    assert_eq!(json["entries"][0]["accountKey"], "a".repeat(64));
    assert_eq!(json["entries"][0]["laneLabel"], "Weekly");
    assert_eq!(json["entries"][0]["period"]["periodId"], 7);
    assert_eq!(json["entries"][0]["period"]["startSource"], "reported");
    assert_eq!(json["entries"][0]["period"]["resetSource"], "derived");
    assert_eq!(json["entries"][0]["confidence"], "learned");
    assert_eq!(json["entries"][0]["plan"]["name"], "max");
    assert_eq!(json["entries"][0]["plan"]["tier"], "max_20x");
    assert_eq!(json["entries"][1]["accountKey"], serde_json::Value::Null);
    assert_eq!(json["entries"][1]["plan"], serde_json::Value::Null);
    assert_eq!(json["entries"][1]["lane"], serde_json::Value::Null);
    assert_eq!(json["entries"][1]["laneLabel"], serde_json::Value::Null);
    assert_eq!(json["entries"][1]["period"], serde_json::Value::Null);
    assert_eq!(json["entries"][1]["confidence"], "unbound");
}

#[test]
fn legacy_live_errors_round_trip_without_a_detail_field() {
    let json = serde_json::json!({
        "source": "fixture", "provider": "anthropic", "displayName": "Claude", "category": "unavailable"
    });
    let error: LiveUsageSourceError = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(error.detail, None);
    assert_eq!(serde_json::to_value(error).unwrap(), json);
}

#[test]
fn live_error_details_use_closed_camel_case_values() {
    for (detail, wire, provider, category) in [
        (
            SourceErrorDetail::KeychainUnreadable,
            "keychainUnreadable",
            "anthropic",
            "unavailable",
        ),
        (
            SourceErrorDetail::RefreshUnsupported,
            "refreshUnsupported",
            "google",
            "authentication",
        ),
        (
            SourceErrorDetail::CredentialExpired,
            "credentialExpired",
            "anthropic",
            "authentication",
        ),
    ] {
        let json = serde_json::json!({
            "source": "fixture", "provider": provider, "displayName": "Fixture",
            "category": category, "detail": wire
        });
        let error: LiveUsageSourceError = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(error.detail, Some(detail));
        assert_eq!(serde_json::to_value(error).unwrap(), json);
    }
}

#[test]
fn a_legacy_live_meter_round_trips_with_unknown_detection() {
    let meter: LiveUsageMeter = serde_json::from_value(serde_json::json!({
        "provider": "anthropic", "displayName": "Claude", "shown": true
    }))
    .unwrap();
    assert_eq!(meter.detection, Detection::Unknown);
    let json = serde_json::to_value(&meter).unwrap();
    assert_eq!(json["detection"], "unknown");
    assert_eq!(
        serde_json::from_value::<LiveUsageMeter>(json).unwrap(),
        meter
    );
}

#[test]
fn live_detection_uses_camel_case_wire_values() {
    for (detection, wire) in [
        (Detection::Unknown, "unknown"),
        (Detection::NotInstalled, "notInstalled"),
        (Detection::InstalledNotSignedIn, "installedNotSignedIn"),
        (Detection::SignedIn, "signedIn"),
    ] {
        let json = serde_json::to_value(detection).unwrap();
        assert_eq!(json, wire);
        assert_eq!(
            serde_json::from_value::<Detection>(json).unwrap(),
            detection
        );
    }
}

mod insights {
    use antiburn_local::analysis::{
        ContextEvidence, EvidenceSource, LoadedSource, ModelControlObservation, ModelTokens,
        RelationConfidence, RelationProvenance, RepeatedContext, SessionEvidenceAccumulator,
        SourceCapabilities, SourceKind, SubagentChild, ToolDefinition, TurnCounts, TurnFacts,
    };
    use antiburn_local::insights::{
        CoverageCounts, DetectorCounts, DetectorFindings, DetectorStatus,
        EfficiencyReportAccumulator, ReportContext, ReportWindow, SessionExample, session_badges,
    };

    use super::*;

    fn report() -> EfficiencyReport {
        EfficiencyReportAccumulator::new().finish(ReportContext {
            environment_key: "native".to_owned(),
            window: ReportWindow {
                start_epoch: 100,
                end_epoch: 200,
            },
            computed_at_epoch: 200,
            parser_revision: 1,
            analyzer_revision: 1,
            evidence_schema_revision: 1,
            coverage: CoverageCounts::default(),
        })
    }

    #[test]
    fn checks_report_serializes_only_display_fields() {
        let mut report = report();
        report.context.coverage.discovered = 42;
        report.finding_agents[0].extend(["codex".to_owned(), "claude-code".to_owned()]);
        report.clean_agents[0].insert("cursor".to_owned());
        report.clean_agents[1].insert("opencode".to_owned());
        report.estimated_token_burn_basis_points = Some(1_625);
        report.detector_estimated_token_burn_basis_points[0] = Some(500);
        report.assessed_sessions = 2;
        report.detector_statuses[0] = DetectorStatus::Findings(DetectorFindings {
            finding_sessions: 1,
            examples: vec![SessionExample {
                agent: "claude-code".to_owned(),
                session_id: "session-1".to_owned(),
            }],
        });
        report.detector_statuses[1] = DetectorStatus::Findings(DetectorFindings {
            finding_sessions: 1,
            examples: Vec::new(),
        });
        report.detectors[0] = DetectorCounts {
            eligible: 4,
            assessed: 3,
            finding: 2,
            clean: 1,
            unavailable: 1,
            not_applicable: 0,
        };

        let value =
            serde_json::to_value(ChecksReportPayload::from_report(&report, true, 0, 0)).unwrap();
        assert_eq!(
            value["categories"][0]["agents"],
            serde_json::json!(["claude-code", "codex"])
        );
        assert_eq!(
            value["categories"][1]["agents"],
            serde_json::json!(["opencode"])
        );
        assert!(value["categories"][0].get("examples").is_none());
        assert!(value.get("coverage").is_none());
        assert!(value.get("quotaPressure").is_none());
        assert!(value.get("providerIncidents").is_none());
        assert_eq!(value["evidenceSettled"], true);
        assert_eq!(value["windowSessions"], 42);
        assert_eq!(value["pendingEvidence"], 0);
        assert_eq!(value["estimatedTokenBurnBasisPoints"], 1_000);
        let aggregates = value["estimatedTokenBurnBasisPointsByDetectorMask"]
            .as_array()
            .unwrap();
        assert_eq!(aggregates.len(), 1 << DetectorId::COUNT);
        assert_eq!(aggregates[0], serde_json::Value::Null);
        assert_eq!(aggregates[1], 500);
        assert_eq!(aggregates[2], 1_000);
        assert_eq!(aggregates[3], 1_000);
        assert_eq!(value["categories"][0]["estimatedTokenBurnBasisPoints"], 500);
        assert!(value["categories"][0]["lifecycle"].is_null());
        assert_eq!(
            value["categories"][1]["estimatedTokenBurnBasisPoints"],
            serde_json::Value::Null
        );

        let top_keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            top_keys,
            [
                "categories",
                "deferredEvidence",
                "estimatedTokenBurnBasisPoints",
                "estimatedTokenBurnBasisPointsByDetectorMask",
                "evidenceSettled",
                "pendingEvidence",
                "smartChecksAvailable",
                "windowSessions"
            ]
        );
        let category_keys: Vec<&str> = value["categories"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            category_keys,
            [
                "agents",
                "clean",
                "estimatedTokenBurnBasisPoints",
                "finding",
                "id",
                "lifecycle",
                "sampled",
                "unavailable",
            ]
        );
        assert_eq!(value["categories"][0]["finding"], 2);
        assert_eq!(value["categories"][0]["clean"], 1);
        assert_eq!(value["categories"][0]["unavailable"], 1);
        assert_eq!(value["categories"][0]["sampled"], false);

        let value =
            serde_json::to_value(ChecksReportPayload::from_report(&report, false, 4, 0)).unwrap();
        assert_eq!(value["evidenceSettled"], false);
        assert_eq!(value["pendingEvidence"], 4);
        assert_eq!(value["estimatedTokenBurnBasisPoints"], 1_000);
    }

    #[test]
    fn sampled_notice_serializes_only_for_the_ignored_instructions_category() {
        let mut payload = ChecksReportPayload::from_report(&report(), true, 0, 0);
        payload.categories[DetectorId::IgnoredInstructions.index()].sampled = true;

        let value = serde_json::to_value(payload).unwrap();
        let categories = value["categories"].as_array().unwrap();
        assert_eq!(categories.len(), DetectorId::COUNT);
        for (index, category) in categories.iter().enumerate() {
            assert_eq!(
                category["sampled"],
                index == DetectorId::IgnoredInstructions.index()
            );
        }
    }

    #[test]
    fn burn_check_contract_serializes_tagged_states_and_decimal_savings() {
        let payload = BurnCheckWatchPayload {
            watch_id: "opaque-watch".into(),
            origin: AggregateWinOrigin::Action,
            lifecycle: BurnCheckWatchLifecycle::Fixed,
            verification: BurnCheckVerificationPayload::Fixed {
                method_revision: 3,
                evidence_revision: "source-7".into(),
            },
            savings: BurnCheckSavingsPayload::Known {
                method: BurnCheckSavingsMethod::OldModelPriceDifference,
                method_revision: 4,
                pricing_revision: "pricing-9".into(),
                api_equivalent_cost_avoided_usd: -1.25,
                measured_through_ms: 500,
                recurrence_ms: Some(450),
            },
        };

        let value = serde_json::to_value(payload).unwrap();
        assert_eq!(value["watchId"], "opaque-watch");
        assert_eq!(value["origin"], "action");
        assert_eq!(value["lifecycle"], "fixed");
        assert_eq!(value["verification"]["status"], "fixed");
        assert_eq!(value["verification"]["methodRevision"], 3);
        assert_eq!(value["savings"]["status"], "known");
        assert_eq!(value["savings"]["method"], "oldModelPriceDifference");
        assert!(value["savings"].get("tokenEquivalentSavings").is_none());
        assert_eq!(value["savings"]["apiEquivalentCostAvoidedUsd"], -1.25);
        assert_eq!(value["savings"]["measuredThroughMs"], 500);
        assert_eq!(value["savings"]["recurrenceMs"], 450);

        let outcome = serde_json::to_value(
            ApplyPreparedBurnCheckOperationOutcome::AppliedAwaitingVerification {
                watch_id: "opaque-watch".into(),
            },
        )
        .unwrap();
        assert_eq!(
            outcome,
            serde_json::json!({
                "outcome": "appliedAwaitingVerification",
                "watchId": "opaque-watch"
            })
        );
    }

    #[test]
    fn burn_check_review_and_display_dtos_expose_only_semantic_facts() {
        let display = BurnCheckDisplayFactsPayload {
            resource_kind: BurnCheckResourceKind::Model,
            resource_identity: Some("old-model".into()),
            instruction_title: None,
            current_value: Some("old-model".into()),
            replacement_value: Some("new-model".into()),
            scope_kind: BurnCheckScopeKind::Project,
            quantity: Some(3),
            quantity_unit: Some(BurnCheckQuantityUnit::Turns),
            observation_count: 2,
            first_observed_at_ms: 100,
            last_observed_at_ms: 200,
            estimate_method: Some(BurnCheckEstimateMethod::OldModelPriceDifference),
            estimated_opportunity: Some(BurnCheckEstimatedValuePayload {
                value: -1.25,
                unit: BurnCheckSavingsUnit::ApiEquivalentUsd,
            }),
            estimated_token_burn_basis_points: Some(1_250),
            verification_limit: BurnCheckVerificationLimit::FreshEvidenceFromSameSourceAndTarget,
        };
        let value = serde_json::to_value(display).unwrap();
        let keys = value
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            keys,
            [
                "currentValue",
                "estimateMethod",
                "estimatedOpportunity",
                "estimatedTokenBurnBasisPoints",
                "firstObservedAtMs",
                "instructionTitle",
                "lastObservedAtMs",
                "observationCount",
                "quantity",
                "quantityUnit",
                "replacementValue",
                "resourceIdentity",
                "resourceKind",
                "scopeKind",
                "verificationLimit",
            ]
        );
        assert_eq!(value["estimatedOpportunity"]["value"], -1.25);
        assert_eq!(value["estimatedOpportunity"]["unit"], "apiEquivalentUsd");
        assert_eq!(value["estimatedTokenBurnBasisPoints"], 1_250);
        let serialized = value.to_string();
        for private_name in [
            "path",
            "sessionId",
            "callId",
            "evidence",
            "config",
            "prompt",
        ] {
            assert!(!serialized.contains(private_name));
        }

        let review = serde_json::to_value(AutoFixReviewPayload {
            prepared_operation_id: "prepared".into(),
            expires_at_epoch: 300,
            agent: "claude-code".into(),
            scope: BurnCheckScopeKind::Project,
            setting: AutoFixSetting::Model,
            config_file: "~/.claude/settings.json".into(),
            selector_label: "model".into(),
            current_value: "old-model".into(),
            proposed_value: "new-model".into(),
            behavior_override_warning: false,
            effect: AutoFixEffect::ModelSelection,
            side_effect: AutoFixSideEffect::ModelBehaviorMayChange,
        })
        .unwrap();
        assert!(review.get("preparedOperationId").is_some());
        assert_eq!(review["configFile"], "~/.claude/settings.json");
        assert!(review.get("path").is_none());
        assert!(review.get("originalBytes").is_none());
        assert!(review.get("config").is_none());

        assert_eq!(
            serde_json::to_value(AutoFixSetting::Reasoning).unwrap(),
            "reasoning"
        );
        assert_eq!(
            serde_json::to_value(AutoFixEffect::ReasoningEffort).unwrap(),
            "reasoningEffort"
        );
        assert_eq!(
            serde_json::to_value(AutoFixSideEffect::ResponsesMayUseLessReasoning).unwrap(),
            "responsesMayUseLessReasoning"
        );
    }

    #[test]
    fn auto_fix_review_vocabulary_is_exhaustive_and_serialized() {
        let settings = [
            AutoFixSetting::Model,
            AutoFixSetting::Reasoning,
            AutoFixSetting::Compaction,
            AutoFixSetting::SubagentModel,
            AutoFixSetting::McpServer,
            AutoFixSetting::BuiltInTool,
            AutoFixSetting::Skill,
            AutoFixSetting::FastMode,
        ];
        let effects = [
            AutoFixEffect::ModelSelection,
            AutoFixEffect::ReasoningEffort,
            AutoFixEffect::SessionCompaction,
            AutoFixEffect::WorkerModelSelection,
            AutoFixEffect::McpAvailability,
            AutoFixEffect::ToolAvailability,
            AutoFixEffect::SkillAvailability,
            AutoFixEffect::ServiceTierSelection,
        ];
        let side_effects = [
            AutoFixSideEffect::ModelBehaviorMayChange,
            AutoFixSideEffect::ResponsesMayUseLessReasoning,
            AutoFixSideEffect::EarlierSessionSummarization,
            AutoFixSideEffect::WorkerBehaviorMayChange,
            AutoFixSideEffect::ServerWillNotBeAvailable,
            AutoFixSideEffect::ToolWillNotBeAvailable,
            AutoFixSideEffect::SkillWillNotBeAvailable,
            AutoFixSideEffect::ResponsesMayTakeLonger,
        ];
        assert_eq!(settings.len(), effects.len());
        assert_eq!(settings.len(), side_effects.len());
        for value in settings {
            assert!(serde_json::to_value(value).unwrap().is_string());
        }
        for value in effects {
            assert!(serde_json::to_value(value).unwrap().is_string());
        }
        for value in side_effects {
            assert!(serde_json::to_value(value).unwrap().is_string());
        }
    }

    #[test]
    fn burn_check_detector_request_rejects_unknown_values() {
        assert_eq!(
            serde_json::from_str::<BurnCheckDetectorId>("\"oldModelUsage\"").unwrap(),
            BurnCheckDetectorId::OldModelUsage
        );
        assert!(serde_json::from_str::<BurnCheckDetectorId>("\"futureDetector\"").is_err());
    }

    /// The badge wire shape carries identifiers only.
    #[test]
    fn the_session_hygiene_payload_contains_no_free_text() {
        let payload = SessionHygienePayload::from_badges(
            [
                SessionBadge {
                    id: BadgeId::SessionOverdepth,
                    status: BadgeStatus::Finding,
                },
                SessionBadge {
                    id: BadgeId::ModelOverthinking,
                    status: BadgeStatus::Clean,
                },
                SessionBadge {
                    id: BadgeId::OverpoweredSubagents,
                    status: BadgeStatus::NotAssessed(NotAssessedReason::IncompleteEvidence),
                },
                SessionBadge {
                    id: BadgeId::ObsoleteModel,
                    status: BadgeStatus::Clean,
                },
                SessionBadge {
                    id: BadgeId::FastModeOveruse,
                    status: BadgeStatus::Clean,
                },
                SessionBadge {
                    id: BadgeId::ExcessCacheRehydration,
                    status: BadgeStatus::Clean,
                },
            ],
            None,
            "ready",
        );

        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            serde_json::json!({
                "badges": [
                    {"id": "sessionOverdepth", "status": "finding", "notAssessedReason": null},
                    {"id": "modelOverthinking", "status": "clean", "notAssessedReason": null},
                    {
                        "id": "overpoweredSubagents",
                        "status": "notAssessed",
                        "notAssessedReason": "incompleteEvidence"
                    },
                    {"id": "obsoleteModel", "status": "clean", "notAssessedReason": null},
                    {"id": "fastModeOveruse", "status": "clean", "notAssessedReason": null},
                    {"id": "excessCacheRehydration", "status": "clean", "notAssessedReason": null}
                ],
                "evidenceState": "ready",
                "unusedResources": null
            })
        );
    }

    #[test]
    fn the_session_hygiene_payload_serializes_finding_evidence() {
        let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude-code".to_owned(),
            session_id: "finding-details".to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        let catalogs = ReportCatalogs::default();

        evidence.context = EvidenceValue::Complete(ContextEvidence {
            max_request_context_tokens: catalogs.depth_cap_tokens + 50_000,
            top_depth_examples: Vec::new(),
        });
        let EvidenceValue::Complete(models) = &mut evidence.models else {
            panic!("synthetic model evidence must be complete");
        };
        models.dominant_main_model = Some("claude-opus-4-6".to_owned());
        models.by_model.insert(
            "claude-opus-4-6".to_owned(),
            ModelTokens {
                turns: 2,
                last_ts_ms: i64::MAX,
                ..ModelTokens::default()
            },
        );
        models.effort_tiers.insert(
            "max".to_owned(),
            TurnCounts {
                main_loop: 2,
                delegated: 0,
            },
        );
        models.control_observations.push(ModelControlObservation {
            provider: None,
            api: None,
            model: "claude-opus-4-6".to_owned(),
            effort: Some("max".to_owned()),
            speed: None,
            last_ts_ms: i64::MAX,
            turns: TurnCounts {
                main_loop: 2,
                delegated: 0,
            },
        });
        models.fast_modes.insert(
            FAST_SPEED_KEY.to_owned(),
            TurnCounts {
                main_loop: 0,
                delegated: 2,
            },
        );
        models.control_observations.push(ModelControlObservation {
            provider: None,
            api: None,
            model: "claude-opus-4-6".to_owned(),
            effort: None,
            speed: Some(FAST_SPEED_KEY.to_owned()),
            last_ts_ms: i64::MAX,
            turns: TurnCounts {
                main_loop: 0,
                delegated: 2,
            },
        });

        let EvidenceValue::Complete(subagents) = &mut evidence.subagents else {
            panic!("synthetic subagent evidence must be complete");
        };
        subagents.spawn_count = 1;
        subagents.delegated_turns = 2;
        subagents
            .delegated_models
            .insert("claude-opus-4-6".to_owned());
        subagents.children.push(SubagentChild {
            ordinal: 1,
            parent_model: Some("claude-opus-4-6".to_owned()),
            parent_call_id: None,
            observed_child_models: subagents.delegated_models.clone(),
            child_model: EvidenceValue::Unsupported,
            confidence: RelationConfidence::Observed,
            provenance: RelationProvenance::TaskToolUse,
        });

        let EvidenceValue::Complete(cache) = &mut evidence.cache else {
            panic!("synthetic cache evidence must be complete");
        };
        cache.repeated_context = EvidenceValue::Complete(RepeatedContext {
            accounting: RepeatedContextAccounting::CacheWrite,
            repeated_tokens: 135,
            paid_tokens: 235,
            pairs_considered: 1,
            pairs_skipped: 0,
            transient_miss_episodes: 0,
            possible_rehydration_episodes: 1,
        });

        let payload = SessionHygienePayload::for_evidence(
            session_badges(&evidence, &catalogs),
            &evidence,
            &catalogs,
            "ready",
        );
        let value = serde_json::to_value(payload).unwrap();

        assert_eq!(
            value["badges"][0]["findingEvidence"],
            serde_json::json!({
                "kind": "sessionOverdepth",
                "maxRequestContextTokens": 450_000,
                "depthCapTokens": 400_000
            })
        );
        assert_eq!(
            value["badges"][1]["findingEvidence"]["kind"],
            "modelOverthinking"
        );
        assert_eq!(
            value["badges"][1]["findingEvidence"]["tiers"][0]["tier"],
            "max"
        );
        assert_eq!(
            value["badges"][2]["findingEvidence"]["kind"],
            "overpoweredSubagents"
        );
        assert_eq!(
            value["badges"][3]["findingEvidence"]["kind"],
            "obsoleteModel"
        );
        assert_eq!(
            value["badges"][3]["findingEvidence"]["models"][0]["replacement"],
            "claude-opus-5"
        );
        assert_eq!(value["badges"][4]["findingEvidence"]["delegatedTurns"], 2);
        assert_eq!(
            value["badges"][5]["findingEvidence"],
            serde_json::json!({
                "kind": "excessCacheRehydration",
                "repeatedTokens": 135,
                "paidTokens": 235,
                "thresholdMultiple": 2.35
            })
        );
    }

    /// One unused MCP server, built-in tool, and skill each report a
    /// name and a cost summed across every priced observed model; a
    /// used resource of each kind is absent from the payload.
    #[test]
    fn for_evidence_prices_unused_resources_and_omits_used_ones() {
        let mut evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude-code".to_owned(),
            session_id: "unused-resources".to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        let catalogs = ReportCatalogs::default();

        let EvidenceValue::Complete(models) = &mut evidence.models else {
            panic!("synthetic model evidence must be complete");
        };
        models.by_model.insert(
            "claude-sonnet-5".to_owned(),
            ModelTokens {
                turns: 2,
                ..ModelTokens::default()
            },
        );
        models.by_model.insert(
            "claude-opus-5".to_owned(),
            ModelTokens {
                turns: 3,
                ..ModelTokens::default()
            },
        );

        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            panic!("synthetic context source evidence must be complete");
        };
        sources.mcp_servers.insert(
            "unused-server".to_owned(),
            LoadedSource {
                description: None,
                configured: true,
                available: true,
                injected: true,
                invoked: false,
                token_count: Some(100),
                origin: EvidenceValue::Unsupported,
            },
        );
        sources.mcp_servers.insert(
            "used-server".to_owned(),
            LoadedSource {
                description: None,
                configured: true,
                available: true,
                injected: true,
                invoked: true,
                token_count: Some(100),
                origin: EvidenceValue::Unsupported,
            },
        );
        sources.skills.insert(
            "unused-skill".to_owned(),
            LoadedSource {
                description: None,
                configured: true,
                available: true,
                injected: true,
                invoked: false,
                token_count: Some(80),
                origin: EvidenceValue::Unsupported,
            },
        );
        sources.skills.insert(
            "used-skill".to_owned(),
            LoadedSource {
                description: None,
                configured: true,
                available: true,
                injected: true,
                invoked: true,
                token_count: Some(80),
                origin: EvidenceValue::Unsupported,
            },
        );
        let mut definitions = BTreeMap::new();
        definitions.insert(
            "unused-tool".to_owned(),
            ToolDefinition {
                tokens: 50,
                invoked: false,
                deferred: false,
            },
        );
        definitions.insert(
            "used-tool".to_owned(),
            ToolDefinition {
                tokens: 50,
                invoked: true,
                deferred: false,
            },
        );
        sources.tool_definitions = EvidenceValue::Complete(definitions);

        let payload = SessionHygienePayload::for_evidence(
            session_badges(&evidence, &catalogs),
            &evidence,
            &catalogs,
            "ready",
        );
        let expected_cost = |tokens: f64| tokens * (2.0 * 0.3e-6 + 3.0 * 0.4e-6);
        assert_eq!(
            payload.unused_resources,
            Some(SessionUnusedResourcesPayload {
                mcp_servers: vec![UnusedResourcePayload {
                    name: "unused-server".to_owned(),
                    cost_usd: Some(expected_cost(100.0)),
                }],
                built_in_tools: vec![UnusedResourcePayload {
                    name: "unused-tool".to_owned(),
                    cost_usd: Some(expected_cost(50.0)),
                }],
                skills: vec![UnusedResourcePayload {
                    name: "unused-skill".to_owned(),
                    cost_usd: Some(expected_cost(80.0)),
                }],
            })
        );
    }
}

/// The webview's `SubagentMemberPayload` contract names these exact
/// camelCase keys. A rename here would silently break that contract, so
/// this test pins the wire shape rather than the Rust field names.
#[test]
fn subagent_member_serializes_with_camel_case_cost_tokens_and_model_runs() {
    let member = SubagentMember {
        agent: "claude-code".to_string(),
        subagent_id: "sub-1".to_string(),
        label: "Reviewer".to_string(),
        cost: Some(SessionCost {
            total_usd: 1.5,
            input_usd: 0.5,
            output_usd: 1.0,
            cache_read_usd: 0.0,
            cache_write_usd: 0.0,
        }),
        tokens: Some(BillableTokens {
            input_tokens: 10,
            output_tokens: 20,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }),
        model_runs: vec![ModelRun {
            model: "claude-3-5-haiku-20241022".to_string(),
            thinking_mode: None,
        }],
        started_at_epoch: Some(1_760_000_000),
    };

    let value = serde_json::to_value(&member).expect("serialize");
    assert_eq!(value["agent"], "claude-code");
    assert_eq!(value["subagentId"], "sub-1");
    assert_eq!(value["label"], "Reviewer");
    assert_eq!(value["cost"]["totalUsd"], 1.5);
    assert_eq!(value["tokens"]["inputTokens"], 10);
    assert_eq!(value["modelRuns"][0]["model"], "claude-3-5-haiku-20241022");
    assert_eq!(value["startedAtEpoch"], 1_760_000_000);
}

/// A sub-agent with no metrics reports `null`, never a partial or zeroed
/// figure — the same rule [`SessionAnalysis::cost`] follows.
#[test]
fn subagent_member_with_no_metrics_serializes_cost_and_tokens_as_null() {
    let member = SubagentMember {
        agent: "claude-code".to_string(),
        subagent_id: "sub-2".to_string(),
        label: "Sub-agent".to_string(),
        cost: None,
        tokens: None,
        model_runs: Vec::new(),
        started_at_epoch: None,
    };

    let value = serde_json::to_value(&member).expect("serialize");
    assert!(value["cost"].is_null());
    assert!(value["tokens"].is_null());
    assert_eq!(value["modelRuns"], serde_json::json!([]));
    assert!(value["startedAtEpoch"].is_null());
}

#[test]
fn remediation_progress_preserves_outcome_origin_and_boundaries() {
    let payload = BurnCheckRemediationProgressPayload::from(
        crate::remediation::BurnCheckRemediationProgress {
            attempts: vec![crate::remediation::BurnCheckRemediationAttempt {
                detector: DetectorId::OldModelUsage,
                finding_id: "finding".into(),
                watch_id: "attempt".into(),
                remediation_cycle_id: "attempt".into(),
                display: crate::remediation::BurnCheckDisplayFacts {
                    resource_kind: crate::remediation::BurnCheckResourceKind::Model,
                    resource_identity: Some("old".into()),
                    instruction_title: None,
                    current_value: Some("old".into()),
                    replacement_value: Some("new".into()),
                    scope_kind: crate::remediation::BurnCheckScopeKind::Project,
                    quantity: None,
                    quantity_unit: None,
                    observation_count: 1,
                    first_observed_at_ms: 10,
                    last_observed_at_ms: 20,
                    estimate_method: None,
                    estimated_opportunity: None,
                    estimated_token_burn_basis_points: None,
                    verification_limit: crate::remediation::BurnCheckVerificationLimit::FreshEvidenceFromSameSourceAndTarget,
                },
                origin: crate::remediation::RemediationOrigin::Action,
                lifecycle: crate::store::RemediationState::WaitingForPromptUse,
                outcome: crate::remediation::BurnCheckRemediationOutcome::Failed,
                verification: crate::remediation::VerificationStatus::Reserved,
                savings: crate::remediation::SavingsStatus::Pending {
                    method_revision: None,
                },
                effective_boundary_ms: None,
                verified_boundary_ms: None,
                recurred_boundary_ms: None,
                environment_key: "native".into(),
                agent: "claude-code".into(),
                scope_kind: "project".into(),
                scope_key: "scope".into(),
                target_key: "target".into(),
                created_at_epoch: 1,
                prompt_action: false,
            }],
        },
    );

    let value = serde_json::to_value(payload).unwrap();
    assert_eq!(value["attempts"][0]["lifecycle"], "waitingForPromptUse");
    assert_eq!(value["attempts"][0]["outcome"], "failed");
    assert_eq!(value["attempts"][0]["origin"], "action");
    assert_eq!(value["attempts"][0]["findingId"], "finding");
    assert_eq!(value["attempts"][0]["remediationCycleId"], "attempt");
    assert!(value["attempts"][0]["effectiveBoundaryMs"].is_null());
}

#[test]
fn burn_check_sample_payload_exposes_no_session_identity() {
    let value = serde_json::to_value(BurnCheckSamplePayload {
        navigation_handle: "opaque-handle".to_owned(),
        title: "Sample session".to_owned(),
        agent: "codex".to_owned(),
        surface: BurnCheckSampleSurface::Cli,
        observed_at_ms: 1_760_000_000_000,
        repo: "demo".to_owned(),
        timestamp: "2026-09-14T12:00:00Z".to_owned(),
        is_active: false,
        has_fork_parent: false,
        fork_child_count: 0,
        cost: None,
        models: Vec::new(),
        model_runs: Vec::new(),
        hygiene: SessionHygienePayload {
            evidence_state: "pending",
            badges: Vec::new(),
            unused_resources: None,
        },
    })
    .expect("serialize");
    let encoded = value.to_string();

    assert_eq!(value["navigationHandle"], "opaque-handle");
    assert_eq!(value["surface"], "cli");
    assert!(!encoded.contains("sessionId"));
    assert!(!encoded.contains("environmentKey"));
    assert!(!encoded.contains("wslDistro"));
}

#[test]
fn agent_memories_report_serializes_camel_case_fields() {
    let report = AgentMemoriesReport {
        generated_at_ms: 9,
        writes_supported: true,
        projects: vec![MemoryProjectDto {
            agent: "claude-code".to_string(),
            slug: "-work-app".to_string(),
            display_path: "/work/app".to_string(),
            folder_exists: true,
            memory_dir: "/h/.claude/projects/-work-app/memory".to_string(),
            index_path: None,
            session_count: 2,
            last_session_ms: Some(5),
            dangling: vec![DanglingIndexEntryDto {
                title: "Gone".to_string(),
                target: "gone.md".to_string(),
                line_number: 4,
            }],
            memories: vec![MemoryEntryDto {
                path: "/h/a.md".to_string(),
                file_name: "a.md".to_string(),
                title: "A".to_string(),
                kind: Some("user".to_string()),
                hook: None,
                hook_source: "body".to_string(),
                index_entry: Some(MemoryIndexEntryDto {
                    title: "A".to_string(),
                    hook: None,
                    line_number: 3,
                }),
                frontmatter: Some("name: A".to_string()),
                body: "text".to_string(),
                truncated: false,
                size_bytes: 4,
                modified_ms: Some(3),
                in_index: true,
                facts: MemoryFactsDto {
                    last_referenced_ms: None,
                    last_written_ms: Some(1),
                    reference_count: 0,
                    write_count: 1,
                    sessions_since_written: Some(2),
                    has_history: true,
                },
            }],
        }],
    };
    assert_eq!(
        serde_json::to_value(&report).unwrap(),
        serde_json::json!({
            "generatedAtMs": 9,
            "writesSupported": true,
            "projects": [{
                "agent": "claude-code",
                "slug": "-work-app",
                "displayPath": "/work/app",
                "folderExists": true,
                "memoryDir": "/h/.claude/projects/-work-app/memory",
                "indexPath": null,
                "sessionCount": 2,
                "lastSessionMs": 5,
                "dangling": [{"title": "Gone", "target": "gone.md", "lineNumber": 4}],
                "memories": [{
                    "path": "/h/a.md",
                    "fileName": "a.md",
                    "title": "A",
                    "kind": "user",
                    "hook": null,
                    "hookSource": "body",
                    "indexEntry": {"title": "A", "hook": null, "lineNumber": 3},
                    "frontmatter": "name: A",
                    "body": "text",
                    "truncated": false,
                    "sizeBytes": 4,
                    "modifiedMs": 3,
                    "inIndex": true,
                    "facts": {
                        "lastReferencedMs": null,
                        "lastWrittenMs": 1,
                        "referenceCount": 0,
                        "writeCount": 1,
                        "sessionsSinceWritten": 2,
                        "hasHistory": true
                    }
                }]
            }]
        })
    );
}

#[cfg(not(windows))]
#[test]
fn memory_edit_outcomes_serialize_as_tagged_camel_case() {
    let cases = [
        (
            MemoryEditOutcome::Archived {
                archive_id: "1-a.md".to_string(),
                index_line_removed: true,
            },
            serde_json::json!({"outcome": "archived", "archiveId": "1-a.md", "indexLineRemoved": true}),
        ),
        (
            MemoryEditOutcome::Restored {
                index_line_restored: false,
            },
            serde_json::json!({"outcome": "restored", "indexLineRestored": false}),
        ),
        (
            MemoryEditOutcome::IndexLineRemoved,
            serde_json::json!({"outcome": "indexLineRemoved"}),
        ),
        (
            MemoryEditOutcome::ChangedOnDisk,
            serde_json::json!({"outcome": "changedOnDisk"}),
        ),
        (
            MemoryEditOutcome::Missing,
            serde_json::json!({"outcome": "missing"}),
        ),
        (
            MemoryEditOutcome::AlreadyExists,
            serde_json::json!({"outcome": "alreadyExists"}),
        ),
        (
            MemoryEditOutcome::Unavailable {
                reason: "unsafepath".to_string(),
            },
            serde_json::json!({"outcome": "unavailable", "reason": "unsafepath"}),
        ),
    ];
    for (outcome, expected) in cases {
        assert_eq!(serde_json::to_value(&outcome).unwrap(), expected);
    }
}

#[test]
fn session_memories_dtos_use_camel_case() {
    let request: SessionMemoriesRequest = serde_json::from_value(serde_json::json!({
        "agent": "claude-code",
        "sessionId": "s1",
        "wslDistro": null,
        "remoteHostId": "h",
    }))
    .unwrap();
    assert_eq!(request.session_id, "s1");
    assert_eq!(request.remote_host_id.as_deref(), Some("h"));
    let payload = SessionMemoriesPayload {
        entries: vec![SessionMemoryTouchDto {
            slug: "-work-app".to_string(),
            path: "/h/.claude/projects/-work-app/memory/a.md".to_string(),
            file_name: "a.md".to_string(),
            title: "A".to_string(),
            action: "written".to_string(),
            count: 2,
            last_ms: None,
            exists: false,
        }],
    };
    let json = serde_json::to_value(&payload).unwrap();
    assert_eq!(json["entries"][0]["fileName"], "a.md");
    assert_eq!(json["entries"][0]["action"], "written");
    assert_eq!(json["entries"][0]["lastMs"], serde_json::Value::Null);
    assert_eq!(json["entries"][0]["exists"], false);
}

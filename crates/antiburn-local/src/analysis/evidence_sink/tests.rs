use std::sync::Arc;

use super::*;
use crate::analysis::evidence::ModelTokens;
use crate::analysis::model::{Role, ToolCall};
use crate::analysis::rows::{MemoryTurnRowStore, TurnRowStore};
use crate::analysis::{EVIDENCE_STRING_CAP, PartialReason, RawSource, SessionReader};

fn accumulator(request_context_tokens: bool) -> SessionEvidenceAccumulator {
    let mut capabilities = SourceCapabilities::claude();
    capabilities.request_context_tokens = request_context_tokens;
    SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: "s1".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities,
    })
}

/// A `CompositeSink` with a fresh in-memory row store attached, so
/// `composite.evidence()` has facts to build from.
fn composite_with_rows(agent: &str, session_id: &str) -> CompositeSink {
    let store = MemoryTurnRowStore::new(agent, session_id);
    let turn_rows = crate::analysis::rows::TurnRowSink::new(
        Arc::clone(&store) as Arc<dyn TurnRowStore>,
        session_id.to_owned(),
        None,
    );
    CompositeSink::with_turn_rows(
        SessionMetricsAccumulator::new(agent, session_id),
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: agent.to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        }),
        turn_rows,
    )
}

#[test]
fn quota_incidents_stay_unsupported_when_the_source_capability_is_false() {
    use crate::analysis::evidence::{QuotaConfidence, QuotaHitSeverity, QuotaLimitKind};

    // OpenCode does not claim `quota_incidents`, so a pushed incident
    // must not leak into a supported group.
    let mut accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "opencode".to_owned(),
        session_id: "s1".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::opencode(),
    });
    accumulator.observe_observation(&EvidenceObservation::QuotaIncident(QuotaIncident {
        ts_ms: 100,
        limit_kind: QuotaLimitKind::RateLimit,
        severity: QuotaHitSeverity::HardHit,
        model: None,
        reset_ts_ms: None,
        reset_clock: None,
        utilization_pct: None,
        confidence: QuotaConfidence::Observed,
    }));
    let evidence = accumulator.evidence(&TurnFacts::default());
    assert!(matches!(
        evidence.quota_incidents,
        EvidenceValue::Unsupported
    ));
}

#[test]
fn provider_incidents_stay_unsupported_when_the_source_capability_is_false() {
    use crate::analysis::evidence::ProviderIncidentKind;

    // OpenCode does not claim `provider_incidents`, so a pushed incident
    // must not leak into a supported group.
    let mut accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "opencode".to_owned(),
        session_id: "s1".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::opencode(),
    });
    accumulator.observe_observation(&EvidenceObservation::ProviderIncident(ProviderIncident {
        ts_ms: 100,
        kind: ProviderIncidentKind::Capacity,
        model: None,
    }));
    let evidence = accumulator.evidence(&TurnFacts::default());
    assert!(matches!(
        evidence.provider_incidents,
        EvidenceValue::Unsupported
    ));
}

#[test]
fn a_recognized_eventless_record_keeps_coverage_complete() {
    let input = crate::analysis::SessionInput {
        agent: "claude".to_owned(),
        session_id: "attachment".to_owned(),
        source: RawSource::Jsonl(
            r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- orbit: Synthetic source."}}"#.to_owned(),
        ),
        source_format: Default::default(),
        fork_parent_session_id: None,
    };
    let mut composite = composite_with_rows("claude", "attachment");
    let outcome = crate::analysis::ClaudeSessionReader
        .visit(&input, &mut composite)
        .expect("attachment must parse");
    composite.observe_source_outcome(outcome);
    assert_eq!(
        composite.evidence().expect("evidence").coverage,
        EvidenceCoverage::Complete
    );
}

#[test]
fn an_inert_unmodelled_type_keeps_coverage_and_records_its_discriminator() {
    let input = crate::analysis::SessionInput {
        agent: "claude".to_owned(),
        session_id: "unknown".to_owned(),
        source: RawSource::Jsonl(r#"{"type":"telemetry_ping","payload":"private"}"#.to_owned()),
        source_format: Default::default(),
        fork_parent_session_id: None,
    };
    let mut composite = composite_with_rows("claude", "unknown");
    let outcome = crate::analysis::ClaudeSessionReader
        .visit(&input, &mut composite)
        .expect("unknown record must be skipped");
    composite.observe_source_outcome(outcome);
    let evidence = composite.evidence().expect("evidence");
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert_eq!(evidence.diagnostics.records_unrecognized_inert, 1);
    assert_eq!(
        evidence.diagnostics.unrecognized_types,
        BTreeSet::from(["telemetry_ping".to_owned()])
    );
}

fn identity_string_overflow(long_agent: bool) -> SessionEvidence {
    let source = EvidenceSource {
        agent: if long_agent {
            long_string()
        } else {
            "claude".to_owned()
        },
        session_id: if long_agent {
            "s1".to_owned()
        } else {
            long_string()
        },
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    };
    SessionEvidenceAccumulator::new(source).evidence(&TurnFacts::default())
}

#[test]
fn identity_agent_overflows_to_partial() {
    let evidence = identity_string_overflow(true);
    assert_eq!(evidence.identity.agent.len(), EVIDENCE_STRING_CAP);
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );
    assert_truncated_string(&evidence, "identity.agent");
}

#[test]
fn a_truncated_identity_string_degrades_session_coverage() {
    let evidence = identity_string_overflow(false);
    assert_eq!(evidence.identity.session_id.len(), EVIDENCE_STRING_CAP);
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );
    assert_truncated_string(&evidence, "identity.session_id");
}

#[test]
fn a_record_loss_reason_outranks_a_cap_reason_in_coverage() {
    let source = EvidenceSource {
        agent: "a".repeat(512),
        session_id: "s1".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    };
    let mut accumulator = SessionEvidenceAccumulator::new(source);
    accumulator.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
    assert_eq!(
        accumulator.evidence(&TurnFacts::default()).coverage,
        EvidenceCoverage::Partial(CoverageReason::MalformedRecord)
    );
}

fn assistant_event(index: usize) -> NormalizedEvent {
    let mut event = NormalizedEvent::new(Role::Assistant);
    event.ts_ms = Some(i64::try_from(index).unwrap());
    event.model = Some(format!("model-{index}"));
    event.usage.input_tokens = u64::try_from(index + 1).unwrap();
    event
}

fn assert_cap_partial<T>(value: EvidenceValue<T>) -> T {
    let EvidenceValue::Partial {
        observed,
        reason: CoverageReason::CapExceeded,
    } = value
    else {
        panic!("cap owner must be partial");
    };
    observed
}

fn assert_capped_collection(evidence: &SessionEvidence, field: &str) {
    assert!(
        evidence.diagnostics.capped_collections.contains(field),
        "missing capped collection diagnostic for {field}"
    );
}

fn assert_truncated_string(evidence: &SessionEvidence, field: &str) {
    assert!(
        evidence.diagnostics.truncated_strings.contains(field),
        "missing truncated string diagnostic for {field}"
    );
}

/// The row query bounds `top_depth_examples`, not the sink — the sink's
/// own rule is that no cap can make `context` partial. `models.byModel`,
/// `models.effortTiers`, `models.fastModes`, `cache.modelTransitions`,
/// `compactions.boundaries`, and `subagents.delegatedModels` caps have
/// moved with their fields to `query_turn_facts`'s own tests in
/// `evidence_query.rs`.
#[test]
fn context_top_depth_examples_cap_keeps_the_group_complete() {
    let accumulator = accumulator(true);
    let facts = TurnFacts::default();
    let evidence = accumulator.evidence(&facts);
    assert!(
        matches!(evidence.context, EvidenceValue::Complete(_)),
        "an example cap must not make the context group partial"
    );
}

#[test]
fn models_cap_from_facts_overflows_models_to_partial() {
    let accumulator = accumulator(true);
    let facts = TurnFacts {
        models_capped: true,
        ..TurnFacts::default()
    };
    let evidence = accumulator.evidence(&facts);
    assert_cap_partial(evidence.models);
}

#[test]
fn duplicate_turn_identities_degrade_models_and_subagents_to_attribution_incomplete() {
    let accumulator = accumulator(true);
    let facts = TurnFacts {
        duplicate_turn_identities: 1,
        ..TurnFacts::default()
    };
    let evidence = accumulator.evidence(&facts);
    assert!(matches!(
        evidence.models,
        EvidenceValue::Partial {
            reason: CoverageReason::AttributionIncomplete,
            ..
        }
    ));
    assert!(matches!(
        evidence.subagents,
        EvidenceValue::Partial {
            reason: CoverageReason::AttributionIncomplete,
            ..
        }
    ));
    assert_eq!(evidence.diagnostics.duplicate_turn_identities, 1);
}

#[test]
fn child_model_controls_reach_detectors_through_reader_rows_and_replay() {
    use crate::analysis::{ClaudeSessionReader, SessionInput, TurnScope};
    use crate::insights::{
        DetectorId, DetectorStatus, EfficiencyReportAccumulator, ReportContext, ReportWindow,
    };

    for (parent_speed, child_effort, child_speed, finding) in [
        ("standard", "max", "fast", true),
        ("fast", "low", "standard", false),
    ] {
        let store = MemoryTurnRowStore::new("claude", "parent");
        let mut residuals = Vec::new();
        for (id, effort, speed, scope) in [
            ("parent", "low", parent_speed, None),
            (
                "child",
                child_effort,
                child_speed,
                Some(TurnScope::Delegated),
            ),
        ] {
            let input = SessionInput {
                agent: "claude".to_owned(),
                session_id: id.to_owned(),
                source: RawSource::Jsonl(
                    serde_json::json!({
                        "type": "assistant", "uuid": id,
                        "timestamp": "2026-09-01T12:00:00Z", "effort": effort,
                        "message": {"role": "assistant", "model": "claude-opus-4-6",
                            "usage": {"input_tokens": 10, "output_tokens": 5, "speed": speed},
                            "content": [{"type": "text", "text": "Synthetic response."}]}
                    })
                    .to_string(),
                ),
                fork_parent_session_id: None,
                source_format: Default::default(),
            };
            let mut sink = CompositeSink::with_turn_rows(
                SessionMetricsAccumulator::new("claude", id),
                accumulator(true),
                TurnRowSink::new(
                    Arc::clone(&store) as Arc<dyn TurnRowStore>,
                    id.to_owned(),
                    scope,
                ),
            );
            let outcome = ClaudeSessionReader
                .visit(&input, &mut sink)
                .expect("parse transcript");
            sink.observe_source_outcome(outcome);
            residuals.push(sink.into_parts().expect("published residual").1);
        }
        let child = residuals.pop().unwrap();
        let mut parent = residuals.pop().unwrap();
        parent.observe_child_coverage(&child);
        let facts = store
            .query_turn_facts()
            .expect("query parent and child rows");
        let evidence = parent.evidence(&facts);
        let EvidenceValue::Complete(models) = &evidence.models else {
            panic!("model evidence must be complete");
        };
        assert_eq!(models.control_observations.len(), 2);
        assert_eq!(models.control_observations[0].turns.main_loop, 1);
        assert_eq!(models.control_observations[0].turns.delegated, 0);
        assert_eq!(models.control_observations[1].turns.main_loop, 0);
        assert_eq!(models.control_observations[1].turns.delegated, 1);
        let encoded = serde_json::to_vec(&parent.coverage_record()).unwrap();
        let restored = SessionEvidenceAccumulator::from_coverage_record(
            serde_json::from_slice(&encoded).unwrap(),
        );
        assert_eq!(evidence, restored.evidence(&facts));

        let mut report = EfficiencyReportAccumulator::new();
        report.observe_session(restored.evidence(&facts));
        let report = report.finish(ReportContext {
            environment_key: "native".to_owned(),
            window: ReportWindow {
                start_epoch: 0,
                end_epoch: i64::MAX,
            },
            computed_at_epoch: 0,
            parser_revision: PARSER_REVISION,
            analyzer_revision: ANALYZER_REVISION,
            evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
            coverage: Default::default(),
        });
        for detector in [DetectorId::ModelOverthinking, DetectorId::OveruseOfFastMode] {
            let status = &report.detector_statuses[detector.index()];
            if finding {
                assert!(
                    matches!(status, DetectorStatus::Findings(_)),
                    "{detector:?}: {status:?}"
                );
            } else {
                assert_eq!(status, &DetectorStatus::Clean, "{detector:?}");
            }
        }
    }
}

#[test]
fn child_model_controls_merge_at_capacity_with_saturating_delegated_counts() {
    let mut parent = accumulator(true);
    for index in 0..MAX_MODEL_CONTROL_OBSERVATIONS {
        let mut event = assistant_event(index);
        event.thinking_mode = Some("low".to_owned());
        parent.observe_event(&event);
    }
    let mut child = accumulator(true);
    let mut event = assistant_event(MAX_MODEL_CONTROL_OBSERVATIONS);
    event.thinking_mode = Some("low".to_owned());
    child.observe_event(&event);
    event.model = Some("model-0".to_owned());
    child.observe_event(&event);
    child.model_control_observations[1].turns.main_loop = u64::MAX;
    child.model_control_observations[1].turns.delegated = 1;
    parent.model_control_observations[0].turns.delegated = 1;
    parent.observe_child_coverage(&child);
    let evidence = parent.evidence(&TurnFacts::default());
    assert_capped_collection(&evidence, "models.control_observations");
    let models = assert_cap_partial(evidence.models);
    assert_eq!(
        models.control_observations.len(),
        MAX_MODEL_CONTROL_OBSERVATIONS
    );
    assert_eq!(models.control_observations[0].turns.main_loop, 1);
    assert_eq!(models.control_observations[0].turns.delegated, u64::MAX);
    assert!(matches!(evidence.tools, EvidenceValue::Complete(_)));
}

#[test]
fn child_model_control_caps_and_truncation_keep_parent_models_partial() {
    for truncate in [false, true] {
        let mut child = accumulator(true);
        for index in 0..=MAX_MODEL_CONTROL_OBSERVATIONS {
            let mut event = assistant_event(index);
            event.thinking_mode = Some("low".to_owned());
            if truncate {
                event.model = Some(long_string());
            }
            child.observe_event(&event);
        }
        let mut parent = accumulator(true);
        parent.observe_child_coverage(&child);
        let evidence = parent.evidence(&TurnFacts::default());
        if truncate {
            assert_truncated_string(&evidence, "models.control_observations.model");
        } else {
            assert_capped_collection(&evidence, "models.control_observations");
        }
        assert_cap_partial(evidence.models);
        assert!(matches!(evidence.tools, EvidenceValue::Complete(_)));
        assert!(matches!(evidence.context, EvidenceValue::Complete(_)));
    }
}

#[test]
fn observe_child_coverage_with_a_lossy_child_degrades_child_dependent_groups_but_not_tools() {
    let mut parent = accumulator(true);
    let mut child = accumulator(true);
    child.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));

    parent.observe_child_coverage(&child);
    let evidence = parent.evidence(&TurnFacts::default());

    for reason in [
        evidence_reason(&evidence.models),
        evidence_reason(&evidence.subagents),
        evidence_reason(&evidence.cache),
        evidence_reason(&evidence.context),
        evidence_reason(&evidence.eligibility),
        evidence_reason(&evidence.time_range),
        evidence_reason(&evidence.compactions),
    ] {
        assert_eq!(reason, Some(CoverageReason::MalformedRecord));
    }
    // Tools stay parent-only: a child's loss does not reach them.
    assert!(matches!(evidence.tools, EvidenceValue::Complete(_)));
    // A child's loss does not change the session-level coverage either.
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
}

#[test]
fn a_childs_tool_call_never_marks_a_parent_definition_invoked() {
    let mut parent = accumulator(true);
    parent.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::HarnessVersion {
            version: "2.1.233".to_owned(),
        },
    )));
    let mut child = accumulator(true);
    let mut bash_call = assistant_event(0);
    bash_call.tools.push(ToolCall::new("Bash"));
    child.record(NormalizedRecord::MetricsEvent(Box::new(bash_call)));

    // Tool definitions are parent-only, the same as `tools` and
    // `context_sources`: folding a child's coverage never reaches
    // them (see `Self::observe_child_coverage`'s doc comment).
    parent.observe_child_coverage(&child);

    let facts = TurnFacts {
        by_model: BTreeMap::from([(
            "claude-opus-4-6".to_owned(),
            ModelTokens {
                turns: 1,
                ..ModelTokens::default()
            },
        )]),
        ..TurnFacts::default()
    };
    let evidence = parent.evidence(&facts);
    let EvidenceValue::Complete(sources) = evidence.context_sources else {
        panic!("context sources must be complete");
    };
    let EvidenceValue::Complete(definitions) = sources.tool_definitions else {
        panic!("tool definitions must resolve with a known version and model");
    };
    assert!(
        definitions
            .get("Bash")
            .is_some_and(|definition| !definition.invoked),
        "a child's own Bash call must never mark the parent's Bash definition invoked"
    );
}

#[test]
fn observe_child_unreadable_degrades_child_dependent_groups_with_read_failed() {
    let mut parent = accumulator(true);
    parent.observe_child_unreadable();
    let evidence = parent.evidence(&TurnFacts::default());

    assert_eq!(
        evidence_reason(&evidence.models),
        Some(CoverageReason::ReadFailed)
    );
    assert_eq!(
        evidence_reason(&evidence.subagents),
        Some(CoverageReason::ReadFailed)
    );
    assert!(matches!(evidence.tools, EvidenceValue::Complete(_)));
    assert_eq!(evidence.diagnostics.children_discovered, 1);
    assert_eq!(evidence.diagnostics.children_unreadable, 1);
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
}

/// The `CoverageReason` an `EvidenceValue` carries, or `None` for
/// `Complete`. Panics on `Unsupported` — every group this helper checks
/// is supported by the Claude capability set `accumulator` uses.
fn evidence_reason<T>(value: &EvidenceValue<T>) -> Option<CoverageReason> {
    match value {
        EvidenceValue::Complete(_) => None,
        EvidenceValue::Partial { reason, .. } => Some(*reason),
        EvidenceValue::Unsupported => panic!("group must be supported"),
    }
}

#[test]
fn tools_by_name_overflows_to_partial() {
    let mut accumulator = accumulator(true);
    for index in 0..(MAX_TOOL_NAMES * 2) {
        let mut event = assistant_event(index);
        event.model = Some("model".to_owned());
        event
            .tools
            .push(crate::analysis::ToolCall::new(format!("tool-{index}")));
        accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    let evidence = accumulator.evidence(&TurnFacts::default());
    assert_capped_collection(&evidence, "tools.by_name");
    assert_eq!(
        assert_cap_partial(evidence.tools).by_name.len(),
        MAX_TOOL_NAMES
    );
}

fn context_sources_overflow(kind: ContextSourceKind) -> SessionEvidence {
    let mut accumulator = accumulator(true);
    for index in 0..(MAX_CONTEXT_SOURCES * 2) {
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ContextSource {
                kind,
                name: format!("source-{index}"),
                description: Some("Synthetic description.".to_owned()),
            },
        )));
    }
    accumulator.evidence(&TurnFacts::default())
}

#[test]
fn context_sources_skills_overflows_to_partial() {
    let evidence = context_sources_overflow(ContextSourceKind::Skill);
    assert_capped_collection(&evidence, "context_sources.skills");
    assert_eq!(
        assert_cap_partial(evidence.context_sources).skills.len(),
        MAX_CONTEXT_SOURCES
    );
}

#[test]
fn context_sources_mcp_servers_overflows_to_partial() {
    let evidence = context_sources_overflow(ContextSourceKind::McpServer);
    assert_capped_collection(&evidence, "context_sources.mcp_servers");
    assert_eq!(
        assert_cap_partial(evidence.context_sources)
            .mcp_servers
            .len(),
        MAX_CONTEXT_SOURCES
    );
}

#[test]
fn resource_coverage_is_independent_and_survives_replay() {
    for capped_kind in [ContextSourceKind::Skill, ContextSourceKind::McpServer] {
        let mut sink = accumulator(true);
        sink.observe_context_source(ContextSourceKind::Skill, "skill", None, true, false);
        sink.observe_context_source(ContextSourceKind::McpServer, "mcp", None, true, false);
        for index in 0..MAX_CONTEXT_SOURCES {
            sink.observe_context_source(capped_kind, &format!("source-{index}"), None, true, false);
        }
        let replay = SessionEvidenceAccumulator::from_coverage_record(sink.coverage_record());
        let evidence = replay.evidence(&TurnFacts::default());
        let EvidenceValue::Partial {
            observed: sources, ..
        } = evidence.context_sources
        else {
            panic!("the coarse group must retain its cap");
        };
        let (capped, complete) = match capped_kind {
            ContextSourceKind::Skill => (sources.skill_coverage, sources.mcp_coverage),
            ContextSourceKind::McpServer => (sources.mcp_coverage, sources.skill_coverage),
        };
        assert_eq!(complete, EvidenceValue::Complete(()));
        assert_eq!(
            capped,
            EvidenceValue::Partial {
                observed: (),
                reason: CoverageReason::CapExceeded
            }
        );
    }
}

#[test]
fn missing_resource_lifecycle_does_not_prove_empty_inventory() {
    let sink = accumulator(true);
    let EvidenceValue::Complete(sources) = sink.evidence(&TurnFacts::default()).context_sources
    else {
        panic!("the wrapper remains available for built-in definitions");
    };
    assert_eq!(sources.skill_coverage, EvidenceValue::Unsupported);
    assert_eq!(sources.mcp_coverage, EvidenceValue::Unsupported);
}

#[test]
fn record_loss_degrades_each_observed_resource() {
    let mut sink = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "codex".to_owned(),
        session_id: "resource-record-loss".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::codex(),
    });
    sink.observe_context_source(ContextSourceKind::Skill, "skill", None, true, false);
    sink.observe_context_source(ContextSourceKind::McpServer, "mcp", None, true, false);
    sink.set_record_loss_reason(CoverageReason::MalformedRecord);
    let sink = SessionEvidenceAccumulator::from_coverage_record(sink.coverage_record());
    let EvidenceValue::Complete(sources) = sink.evidence(&TurnFacts::default()).context_sources
    else {
        unreachable!()
    };
    for coverage in [sources.skill_coverage, sources.mcp_coverage] {
        assert_eq!(
            coverage,
            EvidenceValue::Partial {
                observed: (),
                reason: CoverageReason::MalformedRecord
            }
        );
    }
}

#[test]
fn a_skill_suffix_does_not_match_a_namespaced_identity() {
    let mut accumulator = accumulator(true);
    accumulator.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::ContextSource {
            kind: ContextSourceKind::Skill,
            name: "plugin:deploy".to_owned(),
            description: None,
        },
    )));
    let mut event = assistant_event(0);
    event.tools.push(crate::analysis::ToolCall::new("deploy"));
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));

    let evidence = accumulator.evidence(&TurnFacts::default());

    let EvidenceValue::Complete(sources) = &evidence.context_sources else {
        panic!("context_sources must be complete");
    };
    let skill = sources
        .skills
        .get("plugin:deploy")
        .expect("plugin:deploy must be recorded as a loaded skill");
    assert!(!skill.invoked);

    let EvidenceValue::Complete(tools) = &evidence.tools else {
        panic!("tools must be complete");
    };
    assert_eq!(
        tools.by_name.get("deploy").map(|tool| tool.class),
        Some(ToolClass::Unclassified)
    );
}

#[test]
fn an_explicit_skill_alias_matches_one_namespaced_identity() {
    let mut accumulator = accumulator(true);
    accumulator.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::ContextSource {
            kind: ContextSourceKind::Skill,
            name: "plugin:deploy".to_owned(),
            description: None,
        },
    )));
    let mut event = assistant_event(0);
    let mut skill = crate::analysis::ToolCall::new("Skill");
    skill.detail = Some("deploy".to_owned());
    event.tools.push(skill);
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));

    let evidence = accumulator.evidence(&TurnFacts::default());

    let EvidenceValue::Complete(sources) = &evidence.context_sources else {
        panic!("context_sources must be complete");
    };
    assert!(sources.skills["plugin:deploy"].invoked);
}

#[test]
fn an_ambiguous_skill_alias_makes_attribution_partial() {
    let mut accumulator = accumulator(true);
    for name in ["a:deploy", "b:deploy"] {
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ContextSource {
                kind: ContextSourceKind::Skill,
                name: name.to_owned(),
                description: None,
            },
        )));
    }
    let mut event = assistant_event(0);
    let mut skill = crate::analysis::ToolCall::new("Skill");
    skill.detail = Some("deploy".to_owned());
    event.tools.push(skill);
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));

    let evidence = accumulator.evidence(&TurnFacts::default());

    assert!(matches!(
        evidence.context_sources,
        EvidenceValue::Partial {
            reason: CoverageReason::AttributionIncomplete,
            ..
        }
    ));
}

fn subagents_overflow() -> SessionEvidence {
    let mut accumulator = accumulator(true);
    for index in 0..(MAX_SUBAGENT_CHILDREN * 2) {
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::SubagentSpawn {
                ts_ms: Some(i64::try_from(index).unwrap()),
                parent_model: Some("model".to_owned()),
                parent_call_id: None,
                child_model: None,
                provenance: crate::analysis::RelationProvenance::TaskToolUse,
                config_binding: None,
            },
        )));
    }
    accumulator.evidence(&TurnFacts::default())
}

#[test]
fn subagents_children_overflows_to_partial() {
    let evidence = subagents_overflow();
    assert_capped_collection(&evidence, "subagents.children");
    assert_eq!(
        assert_cap_partial(evidence.subagents).children.len(),
        MAX_SUBAGENT_CHILDREN
    );
}

#[test]
fn subagents_examples_overflows_to_partial() {
    let evidence = subagents_overflow();
    assert_capped_collection(&evidence, "subagents.examples");
    assert_eq!(
        assert_cap_partial(evidence.subagents).examples.len(),
        MAX_EVIDENCE_EXAMPLES
    );
}

#[test]
fn an_inert_unrecognized_record_keeps_complete_coverage() {
    let mut accumulator = accumulator(true);
    accumulator.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::UnrecognizedType {
            discriminator: "telemetry_ping".to_owned(),
            inert: true,
        },
    )));

    let evidence = accumulator.evidence(&TurnFacts::default());
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert_eq!(evidence.diagnostics.records_observed, 1);
    assert_eq!(evidence.diagnostics.records_unrecognized_inert, 1);
    assert_eq!(evidence.diagnostics.records_unusable, 0);
    assert!(evidence.diagnostics.unusable_reasons.is_empty());
    assert!(
        evidence
            .diagnostics
            .unrecognized_types
            .contains("telemetry_ping")
    );
}

#[test]
fn an_evidence_bearing_unrecognized_record_still_fails_closed() {
    let mut accumulator = accumulator(true);
    accumulator.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::UnrecognizedType {
            discriminator: "telemetry_ping".to_owned(),
            inert: false,
        },
    )));
    accumulator.record(NormalizedRecord::Unusable(
        crate::analysis::framing::PartialReason::UnrecognizedRecordType,
    ));

    let evidence = accumulator.evidence(&TurnFacts::default());
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
    );
    assert_eq!(evidence.diagnostics.records_observed, 1);
    assert_eq!(evidence.diagnostics.records_unrecognized_inert, 0);
    assert_eq!(evidence.diagnostics.records_unusable, 1);
    assert!(matches!(
        evidence.context,
        EvidenceValue::Partial {
            reason: CoverageReason::UnrecognizedRecordType,
            ..
        }
    ));
    assert!(matches!(
        evidence.time_range,
        EvidenceValue::Partial {
            reason: CoverageReason::UnrecognizedRecordType,
            ..
        }
    ));
    assert!(matches!(
        evidence.eligibility,
        EvidenceValue::Partial {
            reason: CoverageReason::UnrecognizedRecordType,
            ..
        }
    ));
}

#[test]
fn diagnostics_unrecognized_types_overflows_to_partial() {
    let mut accumulator = accumulator(true);
    for index in 0..(MAX_UNRECOGNIZED_TYPES * 2) {
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::UnrecognizedType {
                discriminator: format!("type-{index}"),
                inert: true,
            },
        )));
    }
    let evidence = accumulator.evidence(&TurnFacts::default());
    assert_eq!(
        evidence.diagnostics.unrecognized_types.len(),
        MAX_UNRECOGNIZED_TYPES
    );
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );
    assert_capped_collection(&evidence, "diagnostics.unrecognized_types");
    assert_eq!(
        evidence.diagnostics.records_unrecognized_inert,
        (MAX_UNRECOGNIZED_TYPES * 2) as u64
    );
}

#[test]
fn a_capped_inert_session_and_an_evidence_bearing_record_report_the_loss_reason() {
    let mut accumulator = accumulator(true);
    for index in 0..=MAX_UNRECOGNIZED_TYPES {
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::UnrecognizedType {
                discriminator: format!("type-{index}"),
                inert: true,
            },
        )));
    }
    accumulator.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::UnrecognizedType {
            discriminator: "bearing".to_owned(),
            inert: false,
        },
    )));
    accumulator.record(NormalizedRecord::Unusable(
        crate::analysis::framing::PartialReason::UnrecognizedRecordType,
    ));

    assert_eq!(
        accumulator.evidence(&TurnFacts::default()).coverage,
        EvidenceCoverage::Partial(CoverageReason::UnrecognizedRecordType)
    );
}

#[test]
fn diagnostics_capped_collections_overflows_to_partial() {
    let mut accumulator = accumulator(true);
    for field in [
        "f00", "f01", "f02", "f03", "f04", "f05", "f06", "f07", "f08", "f09", "f10", "f11", "f12",
        "f13", "f14", "f15", "f16",
    ] {
        accumulator.note_collection_cap(field);
    }
    let evidence = accumulator.evidence(&TurnFacts::default());
    assert_eq!(
        evidence.diagnostics.capped_collections.len(),
        crate::analysis::evidence::MAX_DIAGNOSTIC_FIELDS
    );
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );
    assert_capped_collection(&evidence, "diagnostics.capped_collections");
}

#[test]
fn diagnostics_truncated_strings_overflows_to_partial() {
    let mut accumulator = accumulator(true);
    let long = "x".repeat(EVIDENCE_STRING_CAP * 2);
    for field in [
        "f00", "f01", "f02", "f03", "f04", "f05", "f06", "f07", "f08", "f09", "f10", "f11", "f12",
        "f13", "f14", "f15", "f16",
    ] {
        cap_string(field, &long, &mut accumulator.diagnostics);
    }
    let evidence = accumulator.evidence(&TurnFacts::default());
    assert_eq!(
        evidence.diagnostics.truncated_strings.len(),
        crate::analysis::evidence::MAX_DIAGNOSTIC_FIELDS
    );
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );
    assert_capped_collection(&evidence, "diagnostics.truncated_strings");
}

fn long_string() -> String {
    "x".repeat(EVIDENCE_STRING_CAP * 2)
}

#[test]
fn tools_by_name_key_overflows_to_partial() {
    let mut accumulator = accumulator(true);
    let mut event = assistant_event(1);
    event.model = Some("model".to_owned());
    event
        .tools
        .push(crate::analysis::ToolCall::new(long_string()));
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    let evidence = accumulator.evidence(&TurnFacts::default());
    assert_truncated_string(&evidence, "tools.by_name");
    let tools = assert_cap_partial(evidence.tools);
    assert_eq!(
        tools.by_name.keys().next().unwrap().len(),
        EVIDENCE_STRING_CAP
    );
}

fn source_string_overflow(kind: ContextSourceKind, long_description: bool) -> SessionEvidence {
    let mut accumulator = accumulator(true);
    accumulator.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::ContextSource {
            kind,
            name: if long_description {
                "source".to_owned()
            } else {
                long_string()
            },
            description: Some(if long_description {
                long_string()
            } else {
                "Synthetic description.".to_owned()
            }),
        },
    )));
    accumulator.evidence(&TurnFacts::default())
}

#[test]
fn context_sources_skill_name_overflows_to_partial() {
    let evidence = source_string_overflow(ContextSourceKind::Skill, false);
    assert_truncated_string(&evidence, "context_sources.skills");
    let sources = assert_cap_partial(evidence.context_sources);
    assert_eq!(
        sources.skills.keys().next().unwrap().len(),
        EVIDENCE_STRING_CAP
    );
}

#[test]
fn context_sources_skill_description_overflow_stays_complete() {
    let evidence = source_string_overflow(ContextSourceKind::Skill, true);
    assert_truncated_string(&evidence, "context_sources.skills.description");
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    let EvidenceValue::Complete(sources) = evidence.context_sources else {
        panic!("a truncated description must not degrade context_sources");
    };
    assert_eq!(
        sources
            .skills
            .values()
            .next()
            .unwrap()
            .description
            .as_ref()
            .unwrap()
            .len(),
        EVIDENCE_STRING_CAP
    );
}

#[test]
fn context_sources_mcp_server_name_overflows_to_partial() {
    let evidence = source_string_overflow(ContextSourceKind::McpServer, false);
    assert_truncated_string(&evidence, "context_sources.mcp_servers");
    let sources = assert_cap_partial(evidence.context_sources);
    assert_eq!(
        sources.mcp_servers.keys().next().unwrap().len(),
        EVIDENCE_STRING_CAP
    );
}

#[test]
fn context_sources_mcp_server_description_is_dropped_not_capped() {
    let evidence = source_string_overflow(ContextSourceKind::McpServer, true);
    assert!(
        !evidence
            .diagnostics
            .truncated_strings
            .contains("context_sources.mcp_servers.description"),
        "a dropped description must not record a truncation diagnostic"
    );
    let EvidenceValue::Complete(sources) = evidence.context_sources else {
        panic!("dropping the MCP description must not degrade coverage");
    };
    let source = sources
        .mcp_servers
        .get("source")
        .expect("name must persist");
    assert_eq!(source.description, None);
}

#[test]
fn native_child_models_require_one_exact_call_and_reject_truncated_identities() {
    for (ids, joined_id) in [
        (vec!["call-1".to_owned()], None),
        (vec!["call-1".to_owned()], Some("other".to_owned())),
        (
            vec!["call-1".to_owned(), "call-1".to_owned()],
            Some("call-1".to_owned()),
        ),
        (vec![long_string()], Some(long_string())),
    ] {
        let mut accumulator = accumulator(true);
        for id in ids {
            accumulator.observe_subagent_spawn(
                None,
                Some("claude-opus-4-6"),
                Some(&id),
                None,
                crate::analysis::RelationProvenance::TaskToolUse,
                None,
            );
        }
        accumulator.observe_child_models(joined_id.as_deref(), std::iter::once("claude-opus-4-6"));
        let record = accumulator.coverage_record();
        assert!(
            record
                .subagent_children
                .iter()
                .all(|child| child.observed_child_models.is_empty())
        );
        assert!(matches!(
            accumulator.evidence(&TurnFacts::default()).subagents,
            EvidenceValue::Partial { .. }
        ));
    }
}

#[test]
fn native_child_model_pairs_are_bounded_and_survive_json_and_binary_replay() {
    let mut accumulator = accumulator(true);
    accumulator.observe_subagent_spawn(
        None,
        Some("claude-opus-4-6"),
        Some("call-1"),
        None,
        crate::analysis::RelationProvenance::TaskToolUse,
        None,
    );
    accumulator.observe_child_models(Some("call-1"), std::iter::once(long_string().as_str()));
    assert!(
        accumulator.coverage_record().subagent_children[0]
            .observed_child_models
            .is_empty()
    );
    for index in 0..=crate::analysis::evidence::MAX_SUBAGENT_MODELS {
        accumulator.observe_child_models(
            Some("call-1"),
            std::iter::once(format!("model-{index}").as_str()),
        );
    }
    let record = accumulator.coverage_record();
    assert_eq!(
        record.subagent_children[0].observed_child_models.len(),
        crate::analysis::evidence::MAX_SUBAGENT_MODELS
    );
    assert!(record.subagents_cap_exceeded);
    assert!(accumulator.retained_bytes() < RETAINED_EVIDENCE_BYTES_BOUND);
    let decoded: SessionCoverageRecord =
        postcard::from_bytes(&postcard::to_allocvec(&record).unwrap()).unwrap();
    assert_eq!(decoded, record);
    assert_eq!(
        SessionEvidenceAccumulator::from_coverage_record(decoded).evidence(&TurnFacts::default()),
        accumulator.evidence(&TurnFacts::default())
    );
    let mut legacy = serde_json::to_value(&record).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("subagentLinkageIncomplete");
    for child in legacy["subagentChildren"].as_array_mut().unwrap() {
        child.as_object_mut().unwrap().remove("parentCallId");
        child.as_object_mut().unwrap().remove("observedChildModels");
    }
    let decoded: SessionCoverageRecord = serde_json::from_value(legacy).unwrap();
    assert!(decoded.subagent_children[0].parent_call_id.is_none());
    assert!(
        decoded.subagent_children[0]
            .observed_child_models
            .is_empty()
    );
}

fn subagent_string_overflow() -> SessionEvidence {
    let mut accumulator = accumulator(true);
    accumulator.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::SubagentSpawn {
            ts_ms: Some(1),
            parent_model: Some(long_string()),
            parent_call_id: None,
            child_model: None,
            provenance: crate::analysis::RelationProvenance::TaskToolUse,
            config_binding: None,
        },
    )));
    accumulator.evidence(&TurnFacts::default())
}

#[test]
fn subagents_child_parent_model_overflows_to_partial() {
    let evidence = subagent_string_overflow();
    assert_truncated_string(&evidence, "subagents.children.parent_model");
    let subagents = assert_cap_partial(evidence.subagents);
    assert!(subagents.children[0].parent_model.is_none());
}

#[test]
fn subagents_example_parent_model_overflows_to_partial() {
    let evidence = subagent_string_overflow();
    assert_truncated_string(&evidence, "subagents.examples.parent_model");
    let subagents = assert_cap_partial(evidence.subagents);
    assert_eq!(
        subagents.examples[0].parent_model.as_ref().unwrap().len(),
        EVIDENCE_STRING_CAP
    );
}

#[test]
fn diagnostics_unrecognized_type_string_overflows_to_partial() {
    let mut accumulator = accumulator(true);
    accumulator.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::UnrecognizedType {
            discriminator: long_string(),
            inert: true,
        },
    )));
    let evidence = accumulator.evidence(&TurnFacts::default());
    assert_eq!(
        evidence
            .diagnostics
            .unrecognized_types
            .iter()
            .next()
            .unwrap()
            .len(),
        EVIDENCE_STRING_CAP
    );
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );
    assert_truncated_string(&evidence, "diagnostics.unrecognized_types");
}

fn thread_record(
    uuid: Option<&str>,
    parent_uuid: Option<&str>,
    index: usize,
) -> Vec<NormalizedRecord> {
    let mut records = Vec::new();
    if uuid.is_some() || parent_uuid.is_some() {
        records.push(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ThreadLink {
                uuid: uuid.map(str::to_owned),
                parent_uuid: parent_uuid.map(str::to_owned),
            },
        )));
    }
    let mut event = assistant_event(index);
    event.model = Some("model".to_owned());
    event.uuid = uuid.map(str::to_owned);
    event.parent_uuid = parent_uuid.map(str::to_owned);
    records.push(NormalizedRecord::MetricsEvent(Box::new(event)));
    records
}

/// `thread_identity_missing` is row-derived now (`TurnFacts`), not
/// something the accumulator sees turn by turn. This mirrors what the
/// row query would report for the same chain: missing whenever any
/// counted turn in it carries no `uuid`.
fn previous_turn_for(chain: &[(Option<&str>, Option<&str>)]) -> EvidenceValue<()> {
    let mut accumulator = accumulator(true);
    let mut thread_identity_missing = false;
    for (index, (uuid, parent_uuid)) in chain.iter().enumerate() {
        if uuid.is_none() {
            thread_identity_missing = true;
        }
        for record in thread_record(*uuid, *parent_uuid, index) {
            accumulator.record(record);
        }
    }
    let facts = TurnFacts {
        thread_identity_missing,
        ..TurnFacts::default()
    };
    match accumulator.evidence(&facts).cache {
        EvidenceValue::Complete(cache)
        | EvidenceValue::Partial {
            observed: cache, ..
        } => cache.previous_turn,
        EvidenceValue::Unsupported => panic!("Claude cache evidence must be supported"),
    }
}

#[test]
fn a_resolved_uuid_chain_completes_previous_turn() {
    assert_eq!(
        previous_turn_for(&[
            (Some("u-1"), None),
            (Some("u-2"), Some("u-1")),
            (Some("u-3"), Some("u-2")),
        ]),
        EvidenceValue::Complete(())
    );
}

#[test]
fn an_unresolved_parent_link_degrades_previous_turn() {
    // A parent pointing outside this source (a resumed session) is
    // unresolved, not fabricated: the linkage claim degrades.
    assert_eq!(
        previous_turn_for(&[(Some("u-1"), Some("u-absent")), (Some("u-2"), Some("u-1")),]),
        EvidenceValue::Partial {
            observed: (),
            reason: CoverageReason::AttributionIncomplete,
        }
    );
}

#[test]
fn long_thread_identity_tracking_preserves_complete_coverage() {
    let mut accumulator = accumulator(true);
    for index in 0..MAX_TRACKED_THREAD_UUIDS {
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ThreadLink {
                uuid: Some(format!("{index:0256}")),
                parent_uuid: (index > 0).then(|| format!("{:0256}", index - 1)),
            },
        )));
    }
    let encoded = serde_json::to_vec(&accumulator.resume_state()).unwrap();
    let resume = serde_json::from_slice(&encoded).unwrap();
    let restored = SessionEvidenceAccumulator::from_coverage_record_with_resume(
        accumulator.coverage_record(),
        resume,
    );
    let evidence = restored.evidence(&TurnFacts::default());
    assert_eq!(evidence.coverage, EvidenceCoverage::Complete);
    assert!(
        !evidence
            .diagnostics
            .capped_collections
            .contains(THREAD_UUIDS_DIAGNOSTIC)
    );
    assert!(restored.retained_bytes() < RETAINED_EVIDENCE_BYTES_BOUND);
}

#[test]
fn thread_identity_tracking_is_bounded_and_degrades_coverage() {
    let mut accumulator = accumulator(true);
    for index in 0..=MAX_TRACKED_THREAD_UUIDS {
        accumulator.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::ThreadLink {
                uuid: Some(format!("u-{index}")),
                parent_uuid: None,
            },
        )));
    }

    assert_eq!(
        accumulator.resume_state().seen_thread_uuids.len(),
        MAX_TRACKED_THREAD_UUIDS
    );
    let evidence = accumulator.evidence(&TurnFacts::default());
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );
    assert_eq!(
        evidence_reason(&evidence.cache),
        Some(CoverageReason::AttributionIncomplete)
    );
    assert_capped_collection(&evidence, THREAD_UUIDS_DIAGNOSTIC);
    assert!(accumulator.retained_bytes() < RETAINED_EVIDENCE_BYTES_BOUND);
}

#[test]
fn oversized_resume_thread_state_is_bounded_and_degrades_coverage() {
    let accumulator = accumulator(true);
    let record = accumulator.coverage_record();
    let resume = EvidenceResumeState {
        last_ts_ms: Some(10),
        seen_thread_uuids: (0..=MAX_TRACKED_THREAD_UUIDS)
            .map(|index| format!("u-{index}"))
            .collect(),
    };

    let restored = SessionEvidenceAccumulator::from_coverage_record_with_resume(record, resume);
    assert_eq!(
        restored.resume_state().seen_thread_uuids.len(),
        MAX_TRACKED_THREAD_UUIDS
    );
    let evidence = restored.evidence(&TurnFacts::default());
    assert_eq!(
        evidence.coverage,
        EvidenceCoverage::Partial(CoverageReason::CapExceeded)
    );
    assert_eq!(
        evidence_reason(&evidence.cache),
        Some(CoverageReason::AttributionIncomplete)
    );
}

#[test]
fn serialized_resume_thread_state_rejects_overflow() {
    let resume = EvidenceResumeState {
        last_ts_ms: None,
        seen_thread_uuids: (0..=MAX_TRACKED_THREAD_UUIDS)
            .map(|index| format!("u-{index}"))
            .collect(),
    };
    let encoded = serde_json::to_vec(&resume).unwrap();

    assert!(serde_json::from_slice::<EvidenceResumeState>(&encoded).is_err());
}

#[test]
fn a_compaction_boundarys_null_parent_uuid_keeps_the_link_verified() {
    // `records::evidence_observations` reads a compaction boundary's
    // `ThreadLink.parent_uuid` from `parentUuid` only, with no fallback
    // to `logicalParentUuid`. The boundary's real `parentUuid` is
    // always null, so its `ThreadLink` carries no parent to verify: the
    // boundary's own `uuid` still registers, and the next turn's link
    // to it resolves normally.
    let mut accumulator = accumulator(true);
    for record in thread_record(Some("u-1"), None, 0) {
        accumulator.record(record);
    }
    accumulator.record(NormalizedRecord::Observation(Box::new(
        EvidenceObservation::ThreadLink {
            uuid: Some("boundary".to_owned()),
            parent_uuid: None,
        },
    )));
    for record in thread_record(Some("u-2"), Some("boundary"), 1) {
        accumulator.record(record);
    }
    let facts = TurnFacts::default();
    let EvidenceValue::Complete(cache) = accumulator.evidence(&facts).cache else {
        panic!("a null parentUuid on a boundary record must keep the cache group complete");
    };
    assert_eq!(cache.previous_turn, EvidenceValue::Complete(()));
}

#[test]
fn a_counted_turn_without_a_uuid_degrades_previous_turn() {
    assert_eq!(
        previous_turn_for(&[(Some("u-1"), None), (None, None)]),
        EvidenceValue::Partial {
            observed: (),
            reason: CoverageReason::AttributionIncomplete,
        }
    );
}

/// A source with thread identity but no record identity (Codex's
/// shape) never claimed per-record linkage, so a counted turn without
/// a `uuid` is not a gap: `previous_turn` stays unsupported and the
/// cache group stays complete.
#[test]
fn a_source_without_record_identity_keeps_previous_turn_unsupported() {
    let mut capabilities = SourceCapabilities::claude();
    capabilities.thread_identity = true;
    capabilities.record_identity = false;
    let mut accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "claude".to_owned(),
        session_id: "s1".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities,
    });
    for record in thread_record(None, None, 1) {
        accumulator.record(record);
    }
    let facts = TurnFacts {
        thread_identity_missing: true,
        ..TurnFacts::default()
    };
    let EvidenceValue::Complete(cache) = accumulator.evidence(&facts).cache else {
        panic!("cache must stay complete: this source never claimed record identity");
    };
    assert_eq!(cache.previous_turn, EvidenceValue::Unsupported);
}

/// A source with `linear_record_order` but no `record_identity`
/// (Codex's shape) attests linkage from line order alone: no counted
/// record was lost, so every turn's predecessor is the counted record
/// immediately before it.
#[test]
fn a_linear_order_source_with_no_loss_completes_previous_turn() {
    let mut accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "codex".to_owned(),
        session_id: "s1".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::codex(),
    });
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(assistant_event(1))));
    let EvidenceValue::Complete(cache) = accumulator.evidence(&TurnFacts::default()).cache else {
        panic!("cache must be complete: no record loss and no child");
    };
    assert_eq!(cache.previous_turn, EvidenceValue::Complete(()));
}

/// The same linear-order source, but a record was lost: the order
/// claim can no longer prove line N-1 is the predecessor of line N, so
/// linkage degrades to the same reason the loss itself carries.
#[test]
fn a_linear_order_source_with_a_record_loss_degrades_previous_turn() {
    let mut accumulator = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: "codex".to_owned(),
        session_id: "s1".to_owned(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::codex(),
    });
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(assistant_event(1))));
    accumulator.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
    let cache = match accumulator.evidence(&TurnFacts::default()).cache {
        EvidenceValue::Complete(cache)
        | EvidenceValue::Partial {
            observed: cache, ..
        } => cache,
        EvidenceValue::Unsupported => panic!("cache group must stay supported"),
    };
    assert_eq!(
        cache.previous_turn,
        EvidenceValue::Partial {
            observed: (),
            reason: CoverageReason::MalformedRecord,
        }
    );
}

#[test]
fn an_unclassified_tool_is_not_called_built_in() {
    let mut accumulator = accumulator(true);
    let mut event = assistant_event(1);
    event.tools.push(crate::analysis::ToolCall::new("Mystery"));
    accumulator.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    let EvidenceValue::Complete(tools) = accumulator.evidence(&TurnFacts::default()).tools else {
        panic!("tools must be complete");
    };
    assert_eq!(tools.by_name["Mystery"].class, ToolClass::Unclassified);
    assert!(!serde_json::to_string(&tools).unwrap().contains("built_in"));
}

fn native_result(result_id: &str, content: &str) -> ToolResultObservation {
    use crate::analysis::ToolResultChange;

    ToolResultObservation {
        facts: None,
        outcome: None,
        provider_executed: None,
        context: None,
        source_format: crate::analysis::SourceFormat::ClaudeJsonl,
        session_id: "s1".to_owned(),
        record_id: Some("record-1".to_owned()),
        call_id: Some("call-1".to_owned()),
        result_id: Some(result_id.to_owned()),
        position: crate::analysis::ToolResultPosition::Jsonl {
            record_index: 1,
            item_index: 0,
        },
        revision: 1,
        change: ToolResultChange::Update,
        tool_name: Some("Bash".to_owned()),
        tool_category: None,
        arguments: Some(serde_json::json!({"command": "true"})),
        content: Some(content.to_owned()),
        status: crate::analysis::ToolResultStatus::Success,
        finality: true,
        completeness: crate::analysis::ToolResultCompleteness::Complete,
    }
}

#[test]
fn identical_native_tool_result_replays_are_deduplicated() {
    let mut sink = composite_with_rows("claude", "s1");
    let result = native_result("result-1", "ok");
    sink.record(NormalizedRecord::ToolResult(Box::new(result.clone())));
    sink.record(NormalizedRecord::ToolResult(Box::new(result)));

    assert_eq!(sink.tool_result_observations().len(), 1);
    assert!(sink.tool_result_observations_complete());
}

#[test]
fn restored_generic_results_receive_every_unsafe_admission_with_exact_byte_accounting() {
    use crate::analysis::{
        NativeEvidenceFact, NativeMessageObservation, ToolResultAdmission, ToolResultContext,
    };
    for admission in [
        ToolResultAdmission::UnsafeIdentity,
        ToolResultAdmission::UnsafeOrder,
        ToolResultAdmission::UnsafeOwnership,
    ] {
        let mut sink = composite_with_rows("claude", "s1");
        let mut result = native_result("result-1", "retained output");
        result.context = Some(ToolResultContext::pending(result.position, true));
        sink.record(NormalizedRecord::ToolResult(Box::new(result)));
        sink.observe_source_outcome(VisitOutcome::AcceptedFull);
        sink.finish(SessionSummary::default());
        let snapshot = sink
            .snapshot(AdapterResume {
                point: crate::analysis::ResumePoint {
                    offset: 0,
                    tail_hash: 0,
                    tail_len: 0,
                },
                adapter: crate::analysis::AdapterSnapshot(Vec::new()),
            })
            .unwrap();
        let snapshot = StreamSnapshot::decode(&snapshot.encode()).unwrap();
        let mut restored = composite_with_rows("claude", "s1");
        restored.restore_tool_result_observations(&snapshot);
        restored.tool_result_admission_gap(admission);
        let generic: Vec<_> = restored
            .native_evidence()
            .iter()
            .filter_map(|event| match &event.fact {
                NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                    Some(value.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(generic, restored.tool_result_observations());
        assert_eq!(generic.len(), 1);
        assert_eq!(generic[0].admission(), admission);
        let context = generic[0].context.as_ref().unwrap();
        assert!(!context.main_session && !context.complete_through_event);
        assert_eq!(
            restored.native_evidence_bytes,
            restored
                .native_evidence
                .iter()
                .map(|event| serde_json::to_vec(event).unwrap().len())
                .sum::<usize>()
        );
        assert_eq!(
            restored.tool_result_observation_sizes,
            restored
                .tool_result_observations
                .iter()
                .map(|result| serde_json::to_vec(result).unwrap().len())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            restored.tool_result_observations_bytes,
            restored.tool_result_observation_sizes.iter().sum::<usize>()
        );
        restored.record(NormalizedRecord::ToolResult(Box::new(generic[0].clone())));
        assert_eq!(restored.native_evidence().len(), 1);
        assert_eq!(
            restored.native_evidence_bytes,
            serde_json::to_vec(&restored.native_evidence[0])
                .unwrap()
                .len()
        );
    }
}

#[test]
fn unsafe_generic_result_growth_keeps_the_collection_inside_its_byte_bound() {
    use crate::analysis::{
        NativeEvidenceEvent, NativeEvidenceFact, NativeMessageObservation, ToolResultAdmission,
        ToolResultContext,
    };
    let mut sink = composite_with_rows("claude", "s1");
    let mut result = native_result("limit", "");
    let mut context = ToolResultContext::pending(result.position, true);
    context.complete_through_event = true;
    result.context = Some(context);
    let envelope = NativeEvidenceEvent {
        fact: NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result.clone())),
        live_eligible: false,
    };
    let overhead = serde_json::to_vec(&envelope).unwrap().len();
    result.content = Some("x".repeat(MAX_NATIVE_EVIDENCE_BYTES - overhead - 1));
    sink.record(NormalizedRecord::ToolResult(Box::new(result)));
    assert_eq!(sink.native_evidence_bytes, MAX_NATIVE_EVIDENCE_BYTES - 1);
    sink.tool_result_admission_gap(ToolResultAdmission::UnsafeOwnership);
    assert!(sink.native_evidence().is_empty());
    assert_eq!(sink.native_evidence_bytes, 0);
    assert!(!sink.native_evidence_complete());
    assert_eq!(sink.tool_result_observations().len(), 1);
    assert_eq!(
        sink.tool_result_observations()[0].admission(),
        ToolResultAdmission::UnsafeOwnership
    );
}

#[test]
fn generic_evidence_count_limit_keeps_a_recent_window_without_history_growth() {
    use crate::analysis::{NativeActivityObservation, NativeEvidenceFact, ToolResultPosition};
    let mut sink = composite_with_rows("claude", "s1");
    let fact = |index| {
        NativeEvidenceFact::Activity(NativeActivityObservation {
            source_format: crate::analysis::SourceFormat::ClaudeJsonl,
            session_id: "s1".into(),
            position: ToolResultPosition::Jsonl {
                record_index: index,
                item_index: 0,
            },
            token: format!("activity-{index:08}"),
            native_at_ms: None,
        })
    };
    for index in 100_000..100_000 + MAX_NATIVE_EVIDENCE_OBSERVATIONS as u64 * 2 {
        sink.retain_native_evidence(fact(index));
    }
    assert_eq!(sink.native_evidence.len(), MAX_NATIVE_EVIDENCE_OBSERVATIONS);
    assert!(!sink.native_evidence_complete);
    let before = serde_json::to_vec(&sink.native_evidence).unwrap().len();
    assert!(sink.native_admission_failure.is_none());
    let bytes_before = sink.native_evidence_bytes;
    sink.observe_source_outcome(VisitOutcome::AcceptedFull);
    sink.finish(SessionSummary::default());
    let snapshot = sink
        .snapshot(AdapterResume {
            point: crate::analysis::ResumePoint {
                offset: 0,
                tail_hash: 0,
                tail_len: 0,
            },
            adapter: crate::analysis::AdapterSnapshot(Vec::new()),
        })
        .unwrap();
    let snapshot = StreamSnapshot::decode(&snapshot.encode()).unwrap();
    let mut restored = composite_with_rows("claude", "s1");
    restored.restore_native_evidence(&snapshot);
    assert_eq!(restored.native_evidence, sink.native_evidence);
    assert_eq!(restored.native_evidence_bytes, bytes_before);
    assert!(!restored.native_evidence_complete);
    let start = 100_000 + MAX_NATIVE_EVIDENCE_OBSERVATIONS as u64 * 2;
    for index in start..start + MAX_NATIVE_EVIDENCE_OBSERVATIONS as u64 {
        restored.retain_native_evidence(fact(index));
    }
    assert_eq!(
        restored.native_evidence.len(),
        MAX_NATIVE_EVIDENCE_OBSERVATIONS
    );
    assert_eq!(restored.native_evidence_bytes, bytes_before);
    assert_eq!(
        serde_json::to_vec(&restored.native_evidence).unwrap().len(),
        before
    );
    assert_eq!(restored.native_evidence.first().unwrap().fact, fact(start));
}

#[test]
fn generic_evidence_byte_limit_rejects_oversize_facts_and_stops_history_growth() {
    use crate::analysis::{NativeEvidenceFact, NativeMessageObservation};
    let mut sink = composite_with_rows("claude", "s1");
    let fact = |index, content_bytes| {
        let mut result = native_result(&format!("result-{index:08}"), "");
        result.content = Some("x".repeat(content_bytes));
        NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result))
    };
    for index in 0..8 {
        sink.retain_native_evidence(fact(index, MAX_NATIVE_EVIDENCE_BYTES / 4));
    }
    assert!(sink.native_evidence.len() < MAX_NATIVE_EVIDENCE_OBSERVATIONS);
    assert_eq!(sink.native_evidence.len(), 3);
    assert!(!sink.native_evidence_complete);
    let bytes_before = sink.native_evidence_bytes;
    let encoded_before = postcard::to_allocvec(
        &sink
            .native_evidence
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect::<Vec<_>>(),
    )
    .unwrap()
    .len();
    for index in 8..16 {
        sink.retain_native_evidence(fact(index, MAX_NATIVE_EVIDENCE_BYTES / 4));
    }
    assert_eq!(sink.native_evidence.len(), 3);
    assert_eq!(sink.native_evidence_bytes, bytes_before);
    assert!(sink.native_evidence_bytes <= MAX_NATIVE_EVIDENCE_BYTES);
    assert_eq!(
        sink.native_evidence_bytes,
        sink.native_evidence
            .iter()
            .map(|event| serde_json::to_vec(event).unwrap().len())
            .sum::<usize>()
    );
    assert_eq!(
        postcard::to_allocvec(
            &sink
                .native_evidence
                .iter()
                .map(|event| serde_json::to_string(event).unwrap())
                .collect::<Vec<_>>()
        )
        .unwrap()
        .len(),
        encoded_before
    );
    let prior = sink.native_evidence.clone();
    sink.retain_native_evidence(fact(16, MAX_NATIVE_EVIDENCE_BYTES));
    assert_eq!(sink.native_evidence, prior);
    assert_eq!(sink.native_evidence_bytes, bytes_before);
    assert!(sink.native_admission_failure.is_none());
}

#[test]
fn typed_source_admission_invalidates_every_public_kind_and_survives_snapshot_eviction() {
    use crate::analysis::{
        NativeAdmissionFailure, NativeEvidenceFact, NativeMessageObservation,
        ToolResultCompleteness, ToolResultPosition,
    };
    for reason in [
        ToolResultCompleteness::UnsafeIdentity,
        ToolResultCompleteness::UnsafeOrder,
        ToolResultCompleteness::UnsafeOwnership,
    ] {
        let mut sink = composite_with_rows("codex", "facts");
        crate::analysis::reader_for("codex")
            .visit(
                &crate::analysis::SessionInput {
                    agent: "codex".into(),
                    session_id: "facts".into(),
                    source_format: crate::analysis::SourceFormat::CodexRolloutJsonl,
                    source: RawSource::Jsonl(
                        include_str!("../../../tests/fixtures/public_events/codex.jsonl").into(),
                    ),
                    fork_parent_session_id: None,
                },
                &mut sink,
            )
            .unwrap();
        sink.observe_source_outcome(VisitOutcome::AcceptedFull);
        let mut future = sink
            .native_evidence
            .iter()
            .find_map(|event| match &event.fact {
                NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) => {
                    Some(value.clone())
                }
                _ => None,
            })
            .unwrap();
        future.record_id = "future".into();
        let failure = NativeAdmissionFailure {
            source_format: crate::analysis::SourceFormat::CodexRolloutJsonl,
            session_id: "facts".into(),
            position: ToolResultPosition::Jsonl {
                record_index: 12,
                item_index: u32::MAX,
            },
            reason,
        };
        sink.native_admission_failure(failure.clone());
        assert!(
            sink.native_evidence
                .iter()
                .any(|event| matches!(event.fact, NativeEvidenceFact::ResponseCycle(_)))
        );
        for event in &sink.native_evidence {
            match &event.fact {
                NativeEvidenceFact::Message(NativeMessageObservation::User(value)) => {
                    assert!(!value.context.main_session);
                    assert_eq!(
                        value.message.completeness,
                        crate::analysis::UserMessageCompleteness::Complete
                    );
                    assert_eq!(
                        value.message.change,
                        crate::analysis::UserMessageChange::Present
                    );
                }
                NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) => {
                    assert!(!value.eligible);
                    assert_eq!(value.completeness, reason);
                }
                NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(value)) => {
                    assert_eq!(value.completeness, reason)
                }
                NativeEvidenceFact::ResponseCycle(value) => {
                    assert!(!value.context.main_session && !value.context.complete_through_event)
                }
                _ => {}
            }
        }
        if reason == ToolResultCompleteness::UnsafeIdentity {
            for index in 0..MAX_NATIVE_EVIDENCE_OBSERVATIONS {
                sink.native_activity(crate::analysis::NativeActivityObservation {
                    source_format: failure.source_format,
                    session_id: "facts".into(),
                    position: ToolResultPosition::Jsonl {
                        record_index: 100 + index as u64,
                        item_index: 0,
                    },
                    token: index.to_string(),
                    native_at_ms: None,
                });
            }
            assert!(
                !sink
                    .native_evidence
                    .iter()
                    .any(|event| matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
            );
        }
        let snapshot = sink
            .snapshot(AdapterResume {
                point: crate::analysis::ResumePoint {
                    offset: 0,
                    tail_hash: 0,
                    tail_len: 0,
                },
                adapter: crate::analysis::AdapterSnapshot(Vec::new()),
            })
            .unwrap();
        let snapshot = StreamSnapshot::decode(&snapshot.encode()).unwrap();
        let mut restored = composite_with_rows("codex", "facts");
        restored.restore_tool_result_observations(&snapshot);
        restored.restore_user_messages(&snapshot);
        assert_eq!(restored.native_admission_failure.as_ref(), Some(&failure));
        restored.record(NormalizedRecord::PublicMessage(Box::new(future)));
        assert!(restored.native_evidence.iter().any(|event| matches!(&event.fact, NativeEvidenceFact::Message(NativeMessageObservation::Public(value)) if value.record_id == "future" && !value.eligible && value.completeness == reason)));
        assert!(
            restored
                .tool_result_observations
                .iter()
                .all(|result| result.completeness == reason)
        );
        assert_eq!(
            restored.native_evidence_bytes,
            restored
                .native_evidence
                .iter()
                .map(|event| serde_json::to_vec(event).unwrap().len())
                .sum::<usize>()
        );
    }
}

#[test]
fn conflicting_native_tool_result_revisions_remove_the_ambiguous_result() {
    let mut sink = composite_with_rows("claude", "s1");
    sink.record(NormalizedRecord::ToolResult(Box::new(native_result(
        "result-1", "first",
    ))));
    sink.record(NormalizedRecord::ToolResult(Box::new(native_result(
        "result-1", "conflict",
    ))));

    assert!(sink.tool_result_observations().is_empty());
    assert!(!sink.tool_result_observations_complete());
}

#[test]
fn native_tool_result_count_overflow_marks_collection_incomplete() {
    let mut sink = composite_with_rows("claude", "s1");
    for index in 0..=MAX_TOOL_RESULT_OBSERVATIONS {
        sink.record(NormalizedRecord::ToolResult(Box::new(native_result(
            &format!("result-{index}"),
            "ok",
        ))));
    }

    assert_eq!(
        sink.tool_result_observations().len(),
        MAX_TOOL_RESULT_OBSERVATIONS
    );
    assert!(!sink.tool_result_observations_complete());
    assert_eq!(
        sink.tool_result_observations()[0].result_id.as_deref(),
        Some("result-1")
    );
    assert_eq!(
        sink.tool_result_observations()
            .last()
            .unwrap()
            .result_id
            .as_deref(),
        Some(format!("result-{MAX_TOOL_RESULT_OBSERVATIONS}").as_str())
    );
}

#[test]
fn native_tool_result_byte_overflow_marks_collection_incomplete() {
    let mut sink = composite_with_rows("claude", "s1");
    sink.record(NormalizedRecord::ToolResult(Box::new(native_result(
        "result-1",
        &"x".repeat(MAX_TOOL_RESULT_BYTES + 1),
    ))));

    assert!(sink.tool_result_observations().is_empty());
    assert!(!sink.tool_result_observations_complete());
    sink.record(NormalizedRecord::ToolResult(Box::new(native_result(
        "later", "safe",
    ))));
    assert_eq!(
        sink.tool_result_observations()
            .last()
            .unwrap()
            .result_id
            .as_deref(),
        Some("later")
    );
}

#[test]
fn tool_result_resume_keeps_a_recent_window_and_accepts_new_results_after_overflow() {
    let mut sink = composite_with_rows("claude", "s1");
    let content = "x".repeat(crate::analysis::MAX_CONTENT_PART_BYTES);
    for index in 0..20 {
        let mut result = native_result(&format!("result-{index}"), &content);
        result.position = crate::analysis::ToolResultPosition::Jsonl {
            record_index: index,
            item_index: 0,
        };
        sink.record(NormalizedRecord::ToolResult(Box::new(result)));
    }
    sink.observe_source_outcome(VisitOutcome::AcceptedFull);
    sink.finish(SessionSummary::default());
    let snapshot = sink
        .snapshot(AdapterResume {
            point: crate::analysis::ResumePoint {
                offset: 0,
                tail_hash: 0,
                tail_len: 0,
            },
            adapter: crate::analysis::AdapterSnapshot(Vec::new()),
        })
        .unwrap();
    let snapshot = StreamSnapshot::decode(&snapshot.encode()).unwrap();
    let mut restored = composite_with_rows("claude", "s1");
    restored.restore_tool_result_observations(&snapshot);
    assert_eq!(
        restored.tool_result_observations(),
        sink.tool_result_observations()
    );
    assert!(!restored.tool_result_observations_complete());
    let replay = restored.tool_result_observations().last().unwrap().clone();
    restored.record(NormalizedRecord::ToolResult(Box::new(replay)));
    assert_eq!(
        restored.tool_result_observations(),
        sink.tool_result_observations()
    );
    restored.record(NormalizedRecord::ToolResult(Box::new(native_result(
        "later", "new",
    ))));
    assert_eq!(
        restored
            .tool_result_observations()
            .last()
            .unwrap()
            .result_id
            .as_deref(),
        Some("later")
    );
    assert!(restored.tool_result_observations_bytes <= MAX_TOOL_RESULT_BYTES);
    restored.tool_result_admission_gap(crate::analysis::ToolResultAdmission::UnsafeOrder);
    assert!(
        restored
            .tool_result_observations()
            .iter()
            .all(|result| result.admission() == crate::analysis::ToolResultAdmission::UnsafeOrder)
    );
}

fn native_request(message_id: &str, record_index: u64, text: &str) -> UserMessage {
    use crate::analysis::{
        SourceFormat, UserMessageChange, UserMessageCompleteness, UserMessageOrigin,
        UserMessagePart, UserMessagePosition, UserMessageScope,
    };

    UserMessage {
        source_format: SourceFormat::ClaudeJsonl,
        session_id: "s1".to_owned(),
        message_id: message_id.to_owned(),
        position: UserMessagePosition::Jsonl { record_index },
        origin: UserMessageOrigin::ClaudeUserContent,
        scope: UserMessageScope::MainSession,
        change: UserMessageChange::Present,
        completeness: UserMessageCompleteness::Complete,
        parts: vec![UserMessagePart {
            item_index: 0,
            native_part_id: None,
            text: text.to_owned(),
        }],
        proof_revision: 1,
    }
}

#[test]
fn user_message_replays_deduplicate_without_changing_first_seen_order() {
    let mut sink = composite_with_rows("claude", "s1");
    let first = native_request("message-1", 4, "first request");
    let second = native_request("message-2", 9, "second request");
    sink.user_message(first.clone());
    sink.user_message(second.clone());
    sink.user_message(first.clone());

    assert_eq!(sink.user_messages(), &[first, second]);
    assert!(sink.user_messages_complete());
}

#[test]
fn conflicting_user_message_identity_removes_the_ambiguous_fact() {
    let mut sink = composite_with_rows("claude", "s1");
    let first = native_request("message-1", 4, "first request");
    let second = native_request("message-2", 9, "second request");
    sink.user_message(first);
    sink.user_message(second.clone());
    sink.user_message(native_request("message-1", 4, "changed request"));

    assert_eq!(sink.user_messages(), &[second]);
    assert!(!sink.user_messages_complete());
}

#[test]
fn explicit_user_message_conflicts_make_the_collection_incomplete() {
    let mut sink = composite_with_rows("claude", "s1");
    let mut conflict = native_request("message-1", 4, "first request");
    conflict.change = crate::analysis::UserMessageChange::Conflict;
    conflict.parts.clear();
    sink.user_message(conflict);

    assert!(sink.user_messages().is_empty());
    assert!(!sink.user_messages_complete());
}

#[test]
fn user_message_count_overflow_keeps_the_latest_bounded_history() {
    let mut sink = composite_with_rows("claude", "s1");
    for index in 0..=MAX_USER_MESSAGE_OBSERVATIONS {
        sink.user_message(native_request(
            &format!("message-{index}"),
            index as u64,
            "request",
        ));
    }

    assert_eq!(sink.user_messages().len(), MAX_USER_MESSAGE_OBSERVATIONS);
    assert!(!sink.user_messages_complete());
    assert_eq!(sink.user_messages()[0].message_id, "message-1");
    assert_eq!(
        sink.user_messages().last().unwrap().message_id,
        format!("message-{MAX_USER_MESSAGE_OBSERVATIONS}")
    );
}

#[test]
fn user_message_byte_overflow_keeps_the_latest_bounded_history() {
    let mut sink = composite_with_rows("claude", "s1");
    let text = "x".repeat(crate::analysis::MAX_USER_MESSAGE_BYTES);
    for index in 0..16 {
        sink.user_message(native_request(&format!("message-{index}"), index, &text));
    }

    assert!(sink.user_messages().len() < 16);
    assert!(!sink.user_messages_complete());
    assert_eq!(
        sink.user_messages().last().unwrap().message_id,
        "message-15"
    );
}

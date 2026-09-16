use std::collections::BTreeMap;

use antiburn_local::analysis::{
    ContextSourceEvidence, EvidenceSource, EvidenceValue, SessionEvidenceAccumulator,
    SourceCapabilities, SourceKind, ToolDefinition, ToolEvidence, ToolUse, TurnFacts,
};
use antiburn_local::insights::{SessionTokenBurnEvidence, TokenBurnSourceEvidence};

use super::*;

fn evidence(agent: AgentKind, session_id: &str) -> SessionEvidence {
    SessionEvidenceAccumulator::new(EvidenceSource {
        agent: agent.slug().into(),
        session_id: session_id.into(),
        kind: SourceKind::File,
        capabilities: match agent {
            AgentKind::Claude => SourceCapabilities::claude(),
            AgentKind::Codex => SourceCapabilities::codex(),
            AgentKind::OpenCode => SourceCapabilities::opencode(),
            AgentKind::Pi => SourceCapabilities::pi(),
            _ => unreachable!(),
        },
    })
    .evidence(&TurnFacts::default())
}

fn inventory(agent: AgentKind, resources: Vec<AdvisoryResource>) -> ResourceInventory {
    ResourceInventory {
        agent,
        resources,
        issues: Vec::new(),
    }
}

fn candidate(
    agent: AgentKind,
    kind: ResourceKind,
    name: &str,
    scope: ResourceScope,
) -> AdvisoryResource {
    AdvisoryResource {
        agent,
        kind,
        canonical_name: name.into(),
        enabled: EnabledState::Enabled,
        scope,
        provenance: vec![crate::agent_config::ResourceProvenance::StandardConfig],
        definition_tokens: None,
    }
}

fn report() -> EfficiencyReport {
    use antiburn_local::insights::{
        CoverageCounts, EfficiencyReportAccumulator, ReportContext, ReportWindow,
    };

    EfficiencyReportAccumulator::new().finish(ReportContext {
        environment_key: "native".into(),
        window: ReportWindow {
            start_epoch: 0,
            end_epoch: 1,
        },
        computed_at_epoch: 1,
        parser_revision: 0,
        analyzer_revision: 0,
        evidence_schema_revision: 0,
        coverage: CoverageCounts::default(),
    })
}

fn report_with_tokens(tokens: u128) -> EfficiencyReport {
    use antiburn_local::insights::{
        CoverageCounts, EfficiencyReportAccumulator, ReportContext, ReportWindow,
    };

    let mut accumulator = EfficiencyReportAccumulator::new();
    let mut token_evidence = SessionTokenBurnEvidence::default();
    token_evidence.total_tokens = Some(tokens);
    accumulator
        .observe_session_with_token_burn(evidence(AgentKind::Claude, "tokens"), token_evidence);
    accumulator.finish(ReportContext {
        environment_key: "native".into(),
        window: ReportWindow {
            start_epoch: 0,
            end_epoch: 1,
        },
        computed_at_epoch: 1,
        parser_revision: 0,
        analyzer_revision: 0,
        evidence_schema_revision: 0,
        coverage: CoverageCounts::default(),
    })
}

fn set_complete_empty_sources(evidence: &mut SessionEvidence) {
    evidence.context_sources = EvidenceValue::Complete(ContextSourceEvidence {
        skills: BTreeMap::new(),
        mcp_servers: BTreeMap::new(),
        skill_coverage: EvidenceValue::Complete(()),
        mcp_coverage: EvidenceValue::Complete(()),
        tool_definitions: EvidenceValue::Complete(BTreeMap::new()),
    });
    evidence.tools = EvidenceValue::Complete(ToolEvidence {
        by_name: BTreeMap::new(),
    });
}

#[test]
fn exact_use_suppresses_one_candidate_across_the_window() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        inventory(
            AgentKind::Claude,
            vec![candidate(
                AgentKind::Claude,
                ResourceKind::McpServer,
                "docs",
                ResourceScope::Global,
            )],
        ),
        None,
    );
    let mut evidence = evidence(AgentKind::Claude, "used");
    set_complete_empty_sources(&mut evidence);
    evidence.tools.as_complete_mut().unwrap().by_name.insert(
        "mcp__docs__search".into(),
        ToolUse {
            calls: 1,
            class: ToolClass::Mcp,
        },
    );
    builder.observe_session("native", AgentKind::Claude, "used", None, &evidence, None);

    let assessment = builder.finish(&report());
    let detector = assessment.detector(DetectorId::UnusedMcpServers).unwrap();
    assert_eq!(detector.candidate_count, 1);
    assert_eq!(detector.used_count, 1);
    assert!(detector.targets.is_empty());
}

#[test]
fn project_use_does_not_suppress_the_same_name_in_another_repository() {
    let temporary = tempfile::tempdir().unwrap();
    let project_a = temporary.path().join("a");
    let project_b = temporary.path().join("b");
    let mut builder = ResourceAssessmentBuilder::default();
    for project in [&project_a, &project_b] {
        builder.observe_inventory(
            inventory(
                AgentKind::OpenCode,
                vec![candidate(
                    AgentKind::OpenCode,
                    ResourceKind::Skill,
                    "review",
                    ResourceScope::Project,
                )],
            ),
            Some(project),
        );
    }
    let mut evidence = evidence(AgentKind::OpenCode, "used");
    set_complete_empty_sources(&mut evidence);
    evidence.tools.as_complete_mut().unwrap().by_name.insert(
        "review".into(),
        ToolUse {
            calls: 1,
            class: ToolClass::Skill,
        },
    );
    builder.observe_session(
        "native",
        AgentKind::OpenCode,
        "used",
        Some(&project_a),
        &evidence,
        None,
    );

    let assessment = builder.finish(&report());
    let detector = assessment.detector(DetectorId::UnusedSkills).unwrap();
    assert_eq!(detector.candidate_count, 2);
    assert_eq!(detector.used_count, 1);
    assert_eq!(detector.targets.len(), 1);
    assert_eq!(
        detector.targets[0].scope,
        ResourceAssessmentScope::Project(project_b)
    );
}

#[test]
fn project_candidate_takes_precedence_over_the_same_global_name() {
    let temporary = tempfile::tempdir().unwrap();
    let project = temporary.path().join("project");
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        inventory(
            AgentKind::Pi,
            vec![
                candidate(
                    AgentKind::Pi,
                    ResourceKind::BuiltInTool,
                    "read",
                    ResourceScope::Global,
                ),
                candidate(
                    AgentKind::Pi,
                    ResourceKind::BuiltInTool,
                    "read",
                    ResourceScope::Project,
                ),
            ],
        ),
        Some(&project),
    );
    let mut evidence = evidence(AgentKind::Pi, "used");
    set_complete_empty_sources(&mut evidence);
    evidence.tools.as_complete_mut().unwrap().by_name.insert(
        "read".into(),
        ToolUse {
            calls: 1,
            class: ToolClass::Unclassified,
        },
    );
    builder.observe_session(
        "native",
        AgentKind::Pi,
        "used",
        Some(&project),
        &evidence,
        None,
    );

    let assessment = builder.finish(&report());
    let detector = assessment.detector(DetectorId::UnusedBuiltInTools).unwrap();
    assert_eq!(detector.used_count, 1);
    assert_eq!(detector.targets.len(), 1);
    assert_eq!(detector.targets[0].scope, ResourceAssessmentScope::Global);
}

#[test]
fn provider_mcp_prefixes_suppress_only_the_matching_server() {
    for (agent, tool) in [
        (AgentKind::Claude, "mcp__docs__search"),
        (AgentKind::Codex, "mcp__docs__search"),
        (AgentKind::OpenCode, "docs_search"),
        (AgentKind::Pi, "mcp_docs_search"),
    ] {
        let mut builder = ResourceAssessmentBuilder::default();
        builder.observe_inventory(
            inventory(
                agent,
                vec![
                    candidate(
                        agent,
                        ResourceKind::McpServer,
                        "docs",
                        ResourceScope::Global,
                    ),
                    candidate(
                        agent,
                        ResourceKind::McpServer,
                        "other",
                        ResourceScope::Global,
                    ),
                ],
            ),
            None,
        );
        let mut evidence = evidence(agent, "used");
        set_complete_empty_sources(&mut evidence);
        evidence.tools.as_complete_mut().unwrap().by_name.insert(
            tool.into(),
            ToolUse {
                calls: 1,
                class: if matches!(agent, AgentKind::Claude | AgentKind::Codex) {
                    ToolClass::Mcp
                } else {
                    ToolClass::Unclassified
                },
            },
        );
        builder.observe_session("native", agent, "used", None, &evidence, None);
        let assessment = builder.finish(&report());
        let detector = assessment.detector(DetectorId::UnusedMcpServers).unwrap();
        assert_eq!(detector.used_count, 1, "{agent:?}");
        assert_eq!(detector.targets[0].canonical_name, "other");
    }
}

#[test]
fn partial_positive_evidence_suppresses_but_blocks_clean() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        inventory(
            AgentKind::Codex,
            vec![candidate(
                AgentKind::Codex,
                ResourceKind::BuiltInTool,
                "exec",
                ResourceScope::Global,
            )],
        ),
        None,
    );
    let mut evidence = evidence(AgentKind::Codex, "partial");
    set_complete_empty_sources(&mut evidence);
    evidence.tools = EvidenceValue::Partial {
        observed: ToolEvidence {
            by_name: BTreeMap::from([(
                "functions.exec".into(),
                ToolUse {
                    calls: 1,
                    class: ToolClass::Unclassified,
                },
            )]),
        },
        reason: antiburn_local::analysis::CoverageReason::CapExceeded,
    };
    builder.observe_session("native", AgentKind::Codex, "partial", None, &evidence, None);

    let assessment = builder.finish(&report());
    let detector = assessment.detector(DetectorId::UnusedBuiltInTools).unwrap();
    assert!(detector.targets.is_empty());
    assert!(!detector.clean);
    assert!(detector.unavailable);
}

#[test]
fn indexed_definitions_create_one_target_with_bounded_samples() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(inventory(AgentKind::Claude, Vec::new()), None);
    for index in 0..5 {
        let mut evidence = evidence(AgentKind::Claude, &format!("session-{index}"));
        set_complete_empty_sources(&mut evidence);
        evidence
            .context_sources
            .as_complete_mut()
            .unwrap()
            .tool_definitions = EvidenceValue::Complete(BTreeMap::from([(
            "Read".into(),
            ToolDefinition {
                tokens: 10,
                invoked: false,
                deferred: false,
            },
        )]));
        builder.observe_session(
            "native",
            AgentKind::Claude,
            &format!("session-{index}"),
            None,
            &evidence,
            None,
        );
    }

    let assessment = builder.finish(&report());
    let detector = assessment.detector(DetectorId::UnusedBuiltInTools).unwrap();
    assert_eq!(detector.targets.len(), 1);
    assert_eq!(detector.targets[0].observations, 5);
    assert_eq!(detector.targets[0].supporting_sessions.len(), 3);
}

#[test]
fn skill_target_and_category_use_the_same_replicated_listing_tokens() {
    let mut builder = ResourceAssessmentBuilder::default();
    let mut skill = candidate(
        AgentKind::Claude,
        ResourceKind::Skill,
        "review",
        ResourceScope::Global,
    );
    skill.definition_tokens = Some(25);
    builder.observe_inventory(inventory(AgentKind::Claude, vec![skill]), None);
    builder.observe_turn(AgentKind::Claude, None, 0, 100);
    builder.observe_turn(AgentKind::Claude, None, 1, 100);

    let assessment = builder.finish(&report_with_tokens(1_000));
    let detector = assessment.detector(DetectorId::UnusedSkills).unwrap();

    assert_eq!(detector.targets[0].replicated_tokens, Some(50));
    assert_eq!(
        detector.targets[0].estimated_token_burn_basis_points,
        Some(500)
    );
    assert_eq!(detector.replicated_tokens, Some(50));
    assert_eq!(detector.estimated_token_burn_basis_points, Some(500));
    assert_eq!(
        assessment.measured_finding_tokens_by_session(),
        Some(vec![(0, 25), (1, 25)])
    );
}

#[test]
fn mcp_estimate_requires_measured_indexed_definition_tokens() {
    let make_builder = || {
        let mut builder = ResourceAssessmentBuilder::default();
        builder.observe_inventory(
            inventory(
                AgentKind::Claude,
                vec![candidate(
                    AgentKind::Claude,
                    ResourceKind::McpServer,
                    "docs",
                    ResourceScope::Global,
                )],
            ),
            None,
        );
        builder
    };
    let report = report_with_tokens(1_000);
    let without_measurement = make_builder().finish(&report);
    assert_eq!(
        without_measurement
            .detector(DetectorId::UnusedMcpServers)
            .unwrap()
            .targets[0]
            .replicated_tokens,
        None
    );

    let mut builder = make_builder();
    let mut token_evidence = SessionTokenBurnEvidence::default();
    token_evidence.mcp_sources = Some(vec![TokenBurnSourceEvidence {
        scope: "claude-code:user".into(),
        name: "docs".into(),
        replicated_tokens: 40,
        invoked: false,
        replicated_cost_usd: None,
    }]);
    builder.observe_resource_estimates(AgentKind::Claude, None, 0, &token_evidence);
    let measured = builder.finish(&report);
    let detector = measured.detector(DetectorId::UnusedMcpServers).unwrap();
    assert_eq!(detector.targets[0].replicated_tokens, Some(40));
    assert_eq!(detector.estimated_token_burn_basis_points, Some(400));
}

#[test]
fn bare_opencode_tool_name_does_not_suppress_same_named_mcp_server() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        inventory(
            AgentKind::OpenCode,
            vec![candidate(
                AgentKind::OpenCode,
                ResourceKind::McpServer,
                "read",
                ResourceScope::Global,
            )],
        ),
        None,
    );
    let mut session = evidence(AgentKind::OpenCode, "tool-call");
    let EvidenceValue::Complete(tools) = &mut session.tools else {
        panic!("tool evidence must be complete");
    };
    tools.by_name.insert(
        "read".into(),
        ToolUse {
            calls: 1,
            class: ToolClass::Unclassified,
        },
    );
    builder.observe_session(
        "native",
        AgentKind::OpenCode,
        "tool-call",
        None,
        &session,
        None,
    );

    let assessment = builder.finish(&report());
    assert_eq!(
        assessment
            .detector(DetectorId::UnusedMcpServers)
            .unwrap()
            .unused_count,
        1
    );
}

#[test]
fn ambiguous_opencode_and_pi_mcp_tool_names_suppress_all_possible_findings() {
    for (agent, tool_name) in [
        (AgentKind::OpenCode, "foo_bar_search"),
        (AgentKind::Pi, "mcp_foo_bar_search"),
    ] {
        let mut builder = ResourceAssessmentBuilder::default();
        builder.observe_inventory(
            inventory(
                agent,
                vec![
                    candidate(agent, ResourceKind::McpServer, "foo", ResourceScope::Global),
                    candidate(
                        agent,
                        ResourceKind::McpServer,
                        "foo_bar",
                        ResourceScope::Global,
                    ),
                ],
            ),
            None,
        );
        let mut session = evidence(agent, "tool-call");
        session.tools.as_complete_mut().unwrap().by_name.insert(
            tool_name.into(),
            ToolUse {
                calls: 1,
                class: ToolClass::Unclassified,
            },
        );
        builder.observe_session("native", agent, "tool-call", None, &session, None);

        let assessment = builder.finish(&report());
        let detector = assessment.detector(DetectorId::UnusedMcpServers).unwrap();
        assert_eq!(detector.candidate_count, 2);
        assert_eq!(detector.used_count, 0);
        assert_eq!(detector.unused_count, 0);
        assert!(detector.targets.is_empty());
        assert!(!detector.clean);
        assert!(detector.unavailable);
    }
}

#[test]
fn claude_built_in_estimate_uses_measured_session_replication() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        inventory(
            AgentKind::Claude,
            vec![candidate(
                AgentKind::Claude,
                ResourceKind::BuiltInTool,
                "WebSearch",
                ResourceScope::Global,
            )],
        ),
        None,
    );
    let mut token_evidence = SessionTokenBurnEvidence::default();
    token_evidence.built_in_tool_sources = Some(vec![TokenBurnSourceEvidence {
        scope: "claude-code:bundled".into(),
        name: "WebSearch".into(),
        replicated_tokens: 75,
        invoked: false,
        replicated_cost_usd: None,
    }]);
    builder.observe_resource_estimates(AgentKind::Claude, None, 0, &token_evidence);

    let assessment = builder.finish(&report_with_tokens(1_000));
    let target = &assessment
        .detector(DetectorId::UnusedBuiltInTools)
        .unwrap()
        .targets[0];
    assert_eq!(target.replicated_tokens, Some(75));
    assert_eq!(target.estimated_token_burn_basis_points, Some(750));
}

#[test]
fn pinned_pi_built_in_estimate_replicates_across_applicable_turns() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        inventory(
            AgentKind::Pi,
            vec![candidate(
                AgentKind::Pi,
                ResourceKind::BuiltInTool,
                "read",
                ResourceScope::Global,
            )],
        ),
        None,
    );
    builder.observe_turn(AgentKind::Pi, None, 0, 1_000);

    let assessment = builder.finish(&report_with_tokens(1_000));
    let target = &assessment
        .detector(DetectorId::UnusedBuiltInTools)
        .unwrap()
        .targets[0];
    assert_eq!(target.replicated_tokens, Some(155));
    assert_eq!(target.estimated_token_burn_basis_points, Some(1_550));
}

#[test]
fn ambiguous_skill_suffix_suppresses_no_candidate() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        inventory(
            AgentKind::Claude,
            vec![
                candidate(
                    AgentKind::Claude,
                    ResourceKind::Skill,
                    "first:review",
                    ResourceScope::Global,
                ),
                candidate(
                    AgentKind::Claude,
                    ResourceKind::Skill,
                    "second:review",
                    ResourceScope::Global,
                ),
            ],
        ),
        None,
    );
    let mut evidence = evidence(AgentKind::Claude, "ambiguous");
    set_complete_empty_sources(&mut evidence);
    evidence.tools.as_complete_mut().unwrap().by_name.insert(
        "review".into(),
        ToolUse {
            calls: 1,
            class: ToolClass::Skill,
        },
    );
    builder.observe_session(
        "native",
        AgentKind::Claude,
        "ambiguous",
        None,
        &evidence,
        None,
    );

    let assessment = builder.finish(&report());
    let detector = assessment.detector(DetectorId::UnusedSkills).unwrap();
    assert_eq!(detector.used_count, 0);
    assert_eq!(detector.unused_count, 2);
    assert_eq!(detector.targets.len(), 2);
}

#[test]
fn unrelated_inventory_limit_keeps_a_known_target() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        ResourceInventory {
            agent: AgentKind::OpenCode,
            resources: vec![candidate(
                AgentKind::OpenCode,
                ResourceKind::Skill,
                "review",
                ResourceScope::Global,
            )],
            issues: vec![InventoryIssue {
                kind: Some(ResourceKind::McpServer),
                scope: ResourceScope::Global,
                reason: crate::agent_config::InventoryIssueReason::UnsupportedShape,
            }],
        },
        None,
    );

    let assessment = builder.finish(&report());
    let skills = assessment.detector(DetectorId::UnusedSkills).unwrap();
    assert_eq!(skills.unused_count, 1);
    assert_eq!(skills.targets[0].canonical_name, "review");
}

#[test]
fn candidate_retention_is_bounded_per_detector() {
    let mut builder = ResourceAssessmentBuilder::default();
    let resources = (0..600)
        .map(|index| {
            candidate(
                AgentKind::OpenCode,
                ResourceKind::Skill,
                &format!("skill-{index}"),
                ResourceScope::Global,
            )
        })
        .collect::<Vec<_>>();
    builder.observe_inventory(
        inventory(AgentKind::OpenCode, resources[..300].to_vec()),
        None,
    );
    builder.observe_inventory(
        inventory(AgentKind::OpenCode, resources[300..].to_vec()),
        None,
    );

    let assessment = builder.finish(&report());
    let detector = assessment.detector(DetectorId::UnusedSkills).unwrap();
    assert_eq!(detector.candidate_count, 512);
    assert_eq!(detector.targets.len(), 512);
    assert!(detector.truncated);
    assert!(!detector.clean);
}

#[test]
fn measured_resource_session_retention_is_bounded() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        inventory(
            AgentKind::Claude,
            vec![candidate(
                AgentKind::Claude,
                ResourceKind::McpServer,
                "docs",
                ResourceScope::Global,
            )],
        ),
        None,
    );
    let mut token_evidence = SessionTokenBurnEvidence::default();
    token_evidence.mcp_sources = Some(vec![TokenBurnSourceEvidence {
        scope: "claude-code:user".into(),
        name: "docs".into(),
        replicated_tokens: 10,
        invoked: false,
        replicated_cost_usd: None,
    }]);
    for session in 0..=MAX_RESOURCE_TURN_GROUPS {
        builder.observe_resource_estimates(AgentKind::Claude, None, session, &token_evidence);
    }

    let assessment = builder.finish(&report_with_tokens(100_000));
    let detector = assessment.detector(DetectorId::UnusedMcpServers).unwrap();
    assert_eq!(detector.targets[0].replicated_tokens, None);
    assert_eq!(detector.estimated_token_burn_basis_points, None);
}

#[test]
fn complete_scan_and_use_permit_a_clean_result() {
    let mut builder = ResourceAssessmentBuilder::default();
    builder.observe_inventory(
        inventory(
            AgentKind::Pi,
            vec![candidate(
                AgentKind::Pi,
                ResourceKind::BuiltInTool,
                "read",
                ResourceScope::Global,
            )],
        ),
        None,
    );
    let mut evidence = evidence(AgentKind::Pi, "used");
    set_complete_empty_sources(&mut evidence);
    evidence.tools.as_complete_mut().unwrap().by_name.insert(
        "read".into(),
        ToolUse {
            calls: 1,
            class: ToolClass::Unclassified,
        },
    );
    builder.observe_session("native", AgentKind::Pi, "used", None, &evidence, None);

    let assessment = builder.finish(&report());
    let detector = assessment.detector(DetectorId::UnusedBuiltInTools).unwrap();
    assert_eq!(detector.unused_count, 0);
    assert!(detector.clean);
    assert!(!detector.unavailable);
}

trait CompleteMut<T> {
    fn as_complete_mut(&mut self) -> Option<&mut T>;
}

impl<T> CompleteMut<T> for EvidenceValue<T> {
    fn as_complete_mut(&mut self) -> Option<&mut T> {
        match self {
            EvidenceValue::Complete(value) => Some(value),
            _ => None,
        }
    }
}

use super::*;
use crate::analysis::{ContextSourceEvidence, CoverageReason, LoadedSource};

fn scope() -> SkillScope {
    SkillScope {
        agent: AgentKind::Claude,
        project_identity: Some("project".into()),
        environment_identity: "native-home".into(),
    }
}

fn skill() -> SkillDefinition {
    SkillDefinition {
        identity: "definition-id".into(),
        revision: "revision".into(),
        name: "review".into(),
        aliases: vec!["code-review".into()],
        description: "Review code and find defects.".into(),
        frontmatter: serde_json::json!({"description": "Review code and find defects.", "metadata": {"version": 1}}),
        scope: scope(),
        enabled: true,
        created_at_ms: Some(100),
    }
}

fn usage() -> SkillUseEvidence {
    SkillUseEvidence {
        session_identity: "session".into(),
        scope: scope(),
        status: SkillUseStatus::Complete,
        ordering: SkillUseOrdering::Monotonic,
        events: vec![],
    }
}

fn work(time: Option<i64>) -> SkillWorkContext {
    SkillWorkContext {
        session_identity: "session".into(),
        scope: scope(),
        relevant_work_at_ms: time,
    }
}

fn snapshot(skills: Vec<SkillDefinition>) -> SkillOpportunitySnapshot {
    SkillOpportunitySnapshot::new(scope(), skills, true).unwrap()
}

#[test]
fn creation_is_compared_to_relevant_work_not_episode_end() {
    let snapshot = snapshot(vec![skill()]);
    for (time, expected) in [(99, 0), (100, 1), (101, 1)] {
        assert_eq!(
            snapshot
                .eligible_candidates(&work(Some(time)), &usage())
                .unwrap()
                .len(),
            expected
        );
    }
    // Creation at 100 falls inside an episode from 90 to 110.
    assert!(
        snapshot
            .eligible_candidates(&work(Some(90)), &usage())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        snapshot
            .eligible_candidates(&work(Some(110)), &usage())
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn unknown_times_are_advisory_and_never_prove_historical_visibility() {
    let mut definition = skill();
    definition.created_at_ms = None;
    let candidates = snapshot(vec![definition])
        .eligible_candidates(&work(None), &usage())
        .unwrap();
    assert_eq!(
        candidates[0].limitations(),
        &[
            SkillOpportunityLimit::CurrentInventoryOnly,
            SkillOpportunityLimit::CreationTimeUnknown,
            SkillOpportunityLimit::WorkTimeUnknown,
        ]
    );
}

#[test]
fn disabled_and_ambiguous_definitions_are_not_candidates() {
    let mut disabled = skill();
    disabled.enabled = false;
    assert!(
        snapshot(vec![disabled])
            .eligible_candidates(&work(Some(100)), &usage())
            .unwrap()
            .is_empty()
    );
    let mut duplicate = skill();
    duplicate.identity = "other-file".into();
    assert!(
        snapshot(vec![skill(), duplicate])
            .eligible_candidates(&work(Some(100)), &usage())
            .unwrap()
            .is_empty()
    );
    let mut alias = skill();
    alias.identity = "alias-file".into();
    alias.name = "code-review".into();
    alias.aliases.clear();
    assert!(
        snapshot(vec![skill(), alias])
            .eligible_candidates(&work(Some(100)), &usage())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn agent_project_environment_and_session_must_match() {
    for changed in 0..4 {
        let mut work = work(Some(100));
        match changed {
            0 => work.scope.agent = AgentKind::Codex,
            1 => work.scope.project_identity = Some("other".into()),
            2 => work.scope.environment_identity = "remote".into(),
            _ => work.session_identity = "other-session".into(),
        }
        assert_eq!(
            snapshot(vec![skill()]).eligible_candidates(&work, &usage()),
            Err(SkillInputError::WrongSessionOrScope)
        );
    }
}

fn event(
    identity: &str,
    identity_kind: SkillUseIdentity,
    lifecycle: SkillUseLifecycle,
) -> SkillUseEvent {
    SkillUseEvent {
        identity: identity.into(),
        identity_kind,
        lifecycle,
        source_identity: "session".into(),
        source_field: "tool_request".into(),
        timestamp_ms: Some(90),
        order: Some(1),
    }
}

#[test]
fn exact_identity_and_conservative_alias_requests_exclude_used_skills() {
    for (identity, kind) in [
        ("definition-id", SkillUseIdentity::Exact),
        ("review", SkillUseIdentity::Inferred),
        ("code-review", SkillUseIdentity::Inferred),
    ] {
        for lifecycle in [
            SkillUseLifecycle::Requested,
            SkillUseLifecycle::Succeeded,
            SkillUseLifecycle::Failed,
        ] {
            let mut usage = usage();
            usage.events.push(event(identity, kind, lifecycle));
            assert!(
                snapshot(vec![skill()])
                    .eligible_candidates(&work(Some(100)), &usage)
                    .unwrap()
                    .is_empty()
            );
        }
    }
    let mut usage = usage();
    usage.events.push(event(
        "review",
        SkillUseIdentity::Exact,
        SkillUseLifecycle::Requested,
    ));
    assert_eq!(
        snapshot(vec![skill()])
            .eligible_candidates(&work(Some(100)), &usage)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn incomplete_use_and_unknown_identity_have_explicit_limits() {
    for status in [
        SkillUseStatus::Partial,
        SkillUseStatus::Unsupported,
        SkillUseStatus::Unknown,
    ] {
        let mut usage = usage();
        usage.status = status;
        usage.ordering = SkillUseOrdering::Unknown;
        usage.events.push(event(
            "unresolved",
            SkillUseIdentity::Inferred,
            SkillUseLifecycle::Unknown,
        ));
        let candidates = snapshot(vec![skill()])
            .eligible_candidates(&work(Some(100)), &usage)
            .unwrap();
        assert!(
            candidates[0]
                .limitations()
                .contains(&SkillOpportunityLimit::UseEvidenceIncomplete)
        );
        assert!(
            candidates[0]
                .limitations()
                .contains(&SkillOpportunityLimit::UseIdentityInferred)
        );
        assert!(
            candidates[0]
                .limitations()
                .contains(&SkillOpportunityLimit::UseOrderUnknown)
        );
    }
}

#[test]
fn reference_is_selected_and_revision_tracks_time_use_and_description() {
    let snapshot = snapshot(vec![skill()]);
    let first = snapshot
        .eligible_candidates(&work(Some(100)), &usage())
        .unwrap()[0]
        .reference_snapshot();
    assert_eq!(first.kind, "skill_opportunity");
    assert_eq!(first.fields["description"], skill().description);
    assert!(first.fields.get("scope").is_none());
    assert!(first.fields.get("events").is_none());
    let later = snapshot
        .eligible_candidates(&work(Some(101)), &usage())
        .unwrap()[0]
        .reference_snapshot();
    assert_ne!(first.revision, later.revision);
    let mut unknown = usage();
    unknown.status = SkillUseStatus::Unknown;
    assert_ne!(
        first.revision,
        snapshot
            .eligible_candidates(&work(Some(100)), &unknown)
            .unwrap()[0]
            .reference_snapshot()
            .revision
    );
    let mut changed = skill();
    changed.description = "Review security boundaries.".into();
    assert_ne!(
        first.revision,
        super::tests::snapshot(vec![changed])
            .eligible_candidates(&work(Some(100)), &usage())
            .unwrap()[0]
            .reference_snapshot()
            .revision
    );
}

#[test]
fn name_only_and_oversized_definitions_are_rejected() {
    let mut empty = skill();
    empty.description.clear();
    assert_eq!(
        SkillOpportunitySnapshot::new(scope(), vec![empty], true),
        Err(SkillInputError::InvalidDefinition)
    );
    let mut large = skill();
    large.description = "x".repeat(MAX_SKILL_DESCRIPTION_BYTES + 1);
    assert_eq!(
        SkillOpportunitySnapshot::new(scope(), vec![large], true),
        Err(SkillInputError::LimitExceeded)
    );
}

#[test]
fn source_and_session_provenance_are_required() {
    let mut usage = usage();
    usage.events.push(event(
        "unrelated",
        SkillUseIdentity::Inferred,
        SkillUseLifecycle::Requested,
    ));
    usage.events[0].source_identity = "other-session".into();
    assert_eq!(
        snapshot(vec![skill()]).eligible_candidates(&work(Some(100)), &usage),
        Err(SkillInputError::InvalidUseEvidence)
    );
}

#[test]
fn existing_recorded_requests_and_injected_mentions_remain_distinct() {
    let mut evidence = crate::checks::test_support::claude_evidence("one");
    evidence.provenance.source_acceptance = SourceAcceptance::AcceptedFull;
    evidence.context_sources = EvidenceValue::Complete(ContextSourceEvidence {
        skills: std::collections::BTreeMap::from([(
            "review".into(),
            LoadedSource {
                description: Some("Review code.".into()),
                configured: false,
                available: true,
                injected: true,
                invoked: false,
                token_count: None,
                origin: EvidenceValue::Unsupported,
            },
        )]),
        mcp_servers: Default::default(),
        skill_coverage: EvidenceValue::Complete(()),
        mcp_coverage: EvidenceValue::Unsupported,
        tool_definitions: EvidenceValue::Unsupported,
    });
    let mentions = SkillUseEvidence::from_session(&evidence, &[], scope()).unwrap();
    assert!(mentions.events.is_empty());
    let recorded = vec![SkillUse {
        name: "review".into(),
        progress: 0.5,
        description: None,
        duration_ms: None,
        tokens_out: 0,
        context_tokens: 0,
    }];
    let requests = SkillUseEvidence::from_session(&evidence, &recorded, scope()).unwrap();
    assert_eq!(requests.events[0].lifecycle, SkillUseLifecycle::Requested);
    assert_eq!(requests.events[0].identity_kind, SkillUseIdentity::Inferred);
    assert_eq!(requests.events[0].timestamp_ms, None);
    assert_eq!(requests.events[0].order, None);
    assert_eq!(
        requests.events[0].source_identity,
        requests.session_identity
    );
    let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
        unreachable!()
    };
    sources.skills.get_mut("review").unwrap().invoked = true;
    sources.skill_coverage = EvidenceValue::Partial {
        observed: (),
        reason: CoverageReason::IncompleteTail,
    };
    let partial = SkillUseEvidence::from_session(&evidence, &[], scope()).unwrap();
    assert_eq!(partial.status, SkillUseStatus::Partial);
    assert_eq!(partial.events[0].lifecycle, SkillUseLifecycle::Requested);
    evidence.identity.session_id = "two".into();
    assert_ne!(
        partial.session_identity,
        SkillUseEvidence::from_session(&evidence, &[], scope())
            .unwrap()
            .session_identity
    );
    let mut wrong_scope = scope();
    wrong_scope.agent = AgentKind::Codex;
    assert_eq!(
        SkillUseEvidence::from_session(&evidence, &[], wrong_scope),
        Err(SkillInputError::WrongSessionOrScope)
    );
}

#[test]
fn tool_coverage_and_source_acceptance_constrain_use_completeness() {
    let mut evidence = crate::checks::test_support::claude_evidence("coverage");
    evidence.context_sources = EvidenceValue::Complete(ContextSourceEvidence {
        skills: Default::default(),
        mcp_servers: Default::default(),
        skill_coverage: EvidenceValue::Complete(()),
        mcp_coverage: EvidenceValue::Unsupported,
        tool_definitions: EvidenceValue::Unsupported,
    });
    evidence.provenance.source_acceptance = SourceAcceptance::AcceptedFull;
    assert_eq!(
        SkillUseEvidence::from_session(&evidence, &[], scope())
            .unwrap()
            .status,
        SkillUseStatus::Partial
    );
    let EvidenceValue::Complete(tools) = evidence.tools.clone() else {
        unreachable!()
    };
    evidence.tools = EvidenceValue::Partial {
        observed: tools,
        reason: CoverageReason::IncompleteTail,
    };
    assert_eq!(
        SkillUseEvidence::from_session(&evidence, &[], scope())
            .unwrap()
            .status,
        SkillUseStatus::Partial
    );
    evidence.tools = EvidenceValue::Unsupported;
    assert_eq!(
        SkillUseEvidence::from_session(&evidence, &[], scope())
            .unwrap()
            .status,
        SkillUseStatus::Unknown
    );
    evidence.capabilities.tool_invocations = false;
    assert_eq!(
        SkillUseEvidence::from_session(&evidence, &[], scope())
            .unwrap()
            .status,
        SkillUseStatus::Unsupported
    );
    evidence.provenance.source_acceptance = SourceAcceptance::SourceChanged;
    assert_eq!(
        SkillUseEvidence::from_session(&evidence, &[], scope())
            .unwrap()
            .status,
        SkillUseStatus::Unknown
    );
}

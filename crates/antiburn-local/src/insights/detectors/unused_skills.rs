//! Unused Skills: skills loaded into eligible sessions and never
//! invoked.
//!
//! Findings are not grouped by installed/project/plugin/bundled origin
//! yet: `LoadedSource::origin` carries no classification.
//!
//! Findings require complete coverage of observed skills, tools, and eligibility.
//! Partial tool or skill coverage can hide an invocation and blocks a finding.
//! An unrelated partial resource group does not block a finding.
//! Observed skills do not prove a full historical inventory, so the report cannot claim clean.

use crate::analysis::{EvidenceCoverage, EvidenceValue, SessionEvidence};
use crate::insights::SessionTokenBurnEvidence;
use crate::insights::report::{Fact, FactState};
use crate::remediation::FindingCause;

use super::{Observation, complete};

pub(crate) fn evaluate(evidence: &SessionEvidence) -> Observation {
    let (Some(sources), Some(_tools), Some(eligibility)) = (
        match &evidence.context_sources {
            EvidenceValue::Complete(sources)
            | EvidenceValue::Partial {
                observed: sources, ..
            } => Some(sources),
            EvidenceValue::Unsupported => None,
        },
        complete(&evidence.tools),
        complete(&evidence.eligibility),
    ) else {
        return Observation::NoFinding;
    };
    if eligibility.assistant_turns == 0 || complete(&sources.skill_coverage).is_none() {
        return Observation::NoFinding;
    }
    if sources
        .skills
        .values()
        .any(|skill| skill.injected && !skill.invoked)
    {
        return Observation::Finding;
    }
    Observation::NoFinding
}

pub(super) fn finding_causes(evidence: &SessionEvidence) -> Vec<FindingCause> {
    let Some(sources) = super::observed(&evidence.context_sources) else {
        return Vec::new();
    };
    sources
        .skills
        .iter()
        .filter(|(_, skill)| skill.injected && !skill.invoked)
        .map(|(skill, _)| FindingCause::UnusedSkill {
            skill: skill.clone(),
            tokens: None,
            cost_usd: None,
            pricing_revision: None,
        })
        .collect()
}

/// True when this session's flat skill inventory cannot resolve, but the
/// report's per-turn source attribution still measured a replicated
/// document. Mirrors `unused_built_in_tools::source_assessable`.
pub(super) fn source_assessable(
    evidence: &SessionEvidence,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> bool {
    source_evidence
        .and_then(|value| value.skill_sources.as_ref())
        .is_some()
        && Fact::SkillInventory.state(evidence) == FactState::Unsupported
        && matches!(evidence.coverage, EvidenceCoverage::Complete)
        && matches!(&evidence.tools, EvidenceValue::Complete(_))
        && complete(&evidence.eligibility).is_some_and(|value| value.assistant_turns > 0)
}

pub(super) fn evaluate_with_source_evidence(
    evidence: &SessionEvidence,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> Observation {
    if !source_assessable(evidence, source_evidence) {
        return evaluate(evidence);
    }
    if unused_sources(source_evidence).next().is_some() {
        Observation::Finding
    } else {
        Observation::NoFinding
    }
}

fn unused_sources(
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> impl Iterator<Item = &crate::insights::TokenBurnSourceEvidence> {
    source_evidence
        .and_then(|value| value.skill_sources.as_ref())
        .into_iter()
        .flatten()
        .filter(|source| source.replicated_tokens > 0 && !source.invoked)
}

pub(super) fn finding_causes_with_source_evidence(
    evidence: &SessionEvidence,
    source_evidence: Option<&SessionTokenBurnEvidence>,
) -> Vec<FindingCause> {
    if !source_assessable(evidence, source_evidence) {
        return finding_causes(evidence);
    }
    let pricing_revision = source_evidence.and_then(|value| value.pricing_revision.clone());
    let mut causes = unused_sources(source_evidence)
        .map(|source| FindingCause::UnusedSkill {
            skill: source.name.clone(),
            tokens: Some(source.replicated_tokens),
            cost_usd: source.replicated_cost_usd,
            pricing_revision: pricing_revision.clone(),
        })
        .collect::<Vec<_>>();
    causes.sort_by(|left, right| match (left, right) {
        (
            FindingCause::UnusedSkill { skill: left, .. },
            FindingCause::UnusedSkill { skill: right, .. },
        ) => left.cmp(right),
        _ => core::cmp::Ordering::Equal,
    });
    causes
}

#[cfg(test)]
mod tests {
    use super::super::test_support::claude_evidence;
    use super::*;
    use crate::analysis::{CoverageReason, EvidenceValue, LoadedSource};

    fn with_skill(invoked: bool) -> SessionEvidence {
        let mut evidence = claude_evidence("skills");
        let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
            unreachable!()
        };
        eligibility.assistant_turns = 2;
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.skill_coverage = EvidenceValue::Complete(());
        sources.skills.insert(
            "skill-a".to_owned(),
            LoadedSource {
                description: None,
                configured: true,
                available: true,
                injected: true,
                invoked,
                token_count: None,
                origin: EvidenceValue::Unsupported,
            },
        );
        evidence
    }

    #[test]
    fn loaded_and_never_invoked_skill_is_a_finding() {
        assert_eq!(evaluate(&with_skill(false)), Observation::Finding);
    }

    #[test]
    fn invoked_skill_is_no_finding() {
        assert_eq!(evaluate(&with_skill(true)), Observation::NoFinding);
    }

    #[test]
    fn listed_only_skill_is_no_finding_even_with_listing_tokens() {
        let mut evidence = with_skill(false);
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        let skill = sources.skills.get_mut("skill-a").unwrap();
        skill.injected = false;
        skill.token_count = Some(200);
        assert_eq!(evaluate(&evidence), Observation::NoFinding);
    }

    #[test]
    fn a_loaded_basename_does_not_make_a_namespaced_invocation_look_unused() {
        use crate::analysis::{
            ContextSourceKind, EvidenceObservation, EvidenceSource, NormalizedRecord,
            SessionEvidenceAccumulator, SourceCapabilities, SourceKind, TurnFacts,
        };
        let mut sink = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "skill-basename".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        });
        for observation in [
            EvidenceObservation::ContextSource {
                kind: ContextSourceKind::Skill,
                name: "plugin:review".to_owned(),
                description: None,
            },
            EvidenceObservation::SkillInjection {
                name: "review".to_owned(),
                invoked: false,
            },
            EvidenceObservation::SkillInjection {
                name: "plugin:review".to_owned(),
                invoked: true,
            },
        ] {
            sink.observe(&NormalizedRecord::Observation(Box::new(observation)));
        }
        let mut facts = TurnFacts::default();
        facts.eligibility.assistant_turns = 1;
        let evidence = sink.evidence(&facts);
        assert!(matches!(
            evidence.context_sources,
            EvidenceValue::Partial {
                reason: CoverageReason::AttributionIncomplete,
                ..
            }
        ));
        assert_eq!(evaluate(&evidence), Observation::NoFinding);
    }

    #[test]
    fn claude_skill_documents_and_listings_have_distinct_evidence() {
        use crate::analysis::{
            EvidenceSource, NormalizedRecord, RawSource, RecordSink, SessionEvidenceAccumulator,
            SessionInput, SessionSummary, SourceCapabilities, SourceKind, TurnFacts, reader_for,
        };
        use serde_json::json;

        struct Sink(SessionEvidenceAccumulator);
        impl RecordSink for Sink {
            fn record(&mut self, record: NormalizedRecord) {
                self.0.observe(&record);
            }

            fn finish(&mut self, summary: SessionSummary) {
                self.0.observe_summary(&summary);
                // A report-time summary must not promote listing tokens into document evidence.
                self.0.observe_summary(&summary);
            }
        }

        let listing = json!({"type":"attachment","attachment":{
            "type":"skill_listing","content":"- plugin:review: Review code.\n- other:review: Review another project.\n- local-review: Review local code."
        }});
        let document = json!({"type":"user","isMeta":true,"message":{
            "role":"user","content":"Base directory for this skill: /synthetic/skills/local-review\n\nCheck the changes."
        }});
        let invoked = json!({"type":"attachment","attachment":{
            "type":"invoked_skills","skills":[{
                "name":"plugin:review","path":"plugin:review","content":"Check the code."
            }]
        }});
        let dynamic = json!({"type":"attachment","attachment":{
            "type":"dynamic_skill","skillDir":"/synthetic/skills","displayPath":".claude/skills",
            "skillNames":["dynamic:review"]
        }});
        let tool = json!({"type":"assistant","message":{"role":"assistant","content":[{
            "type":"tool_use","id":"skill-call","name":"Skill","input":{"skill":"local-review"}
        }]}});
        for (records, loaded, used, expected) in [
            (
                vec![listing.clone(), dynamic],
                false,
                false,
                Observation::NoFinding,
            ),
            (
                vec![listing.clone(), document.clone()],
                true,
                false,
                Observation::Finding,
            ),
            (
                vec![document, listing.clone(), tool],
                true,
                true,
                Observation::NoFinding,
            ),
            (vec![invoked, listing], false, false, Observation::NoFinding),
        ] {
            let mut sink = Sink(SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "claude".to_owned(),
                session_id: "skill-state".to_owned(),
                kind: SourceKind::Jsonl,
                capabilities: SourceCapabilities::claude(),
            }));
            let input = SessionInput {
                agent: "claude".to_owned(),
                session_id: "skill-state".to_owned(),
                source: RawSource::Jsonl(
                    records
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                fork_parent_session_id: None,
            };
            reader_for("claude").visit(&input, &mut sink).unwrap();
            let mut facts = TurnFacts::default();
            facts.eligibility.assistant_turns = 1;
            let evidence = sink.0.evidence(&facts);
            let EvidenceValue::Complete(sources) = &evidence.context_sources else {
                panic!("expected complete skill evidence");
            };
            assert_eq!(sources.skills["local-review"].injected, loaded);
            assert_eq!(sources.skills["local-review"].invoked, used);
            assert_eq!(
                sources.skills["plugin:review"].description.as_deref(),
                Some("Review code.")
            );
            assert!(!sources.skills.contains_key("plugin"));
            assert!(!sources.skills["other:review"].injected);
            if let Some(dynamic) = sources.skills.get("dynamic:review") {
                assert!(dynamic.available);
                assert!(!dynamic.injected);
            }
            if records
                .iter()
                .any(|record| record.pointer("/attachment/type") == Some(&json!("invoked_skills")))
            {
                assert!(sources.skills["plugin:review"].injected);
                assert!(sources.skills["plugin:review"].invoked);
                assert!(!sources.skills["other:review"].invoked);
            }
            assert_eq!(evaluate(&evidence), expected);
        }
    }

    #[test]
    fn partial_skill_coverage_never_claims_the_absence_finding() {
        let mut evidence = with_skill(false);
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.skill_coverage = EvidenceValue::Partial {
            observed: (),
            reason: CoverageReason::CapExceeded,
        };
        assert_eq!(evaluate(&evidence), Observation::NoFinding);
    }

    #[test]
    fn partial_other_resources_do_not_block_skill_findings() {
        let mut evidence = with_skill(false);
        evidence.context_sources = match evidence.context_sources {
            EvidenceValue::Complete(observed) => EvidenceValue::Partial {
                observed,
                reason: CoverageReason::CapExceeded,
            },
            _ => unreachable!(),
        };

        assert_eq!(evaluate(&evidence), Observation::Finding);
    }

    fn unsupported_coverage_evidence() -> SessionEvidence {
        let mut evidence = with_skill(false);
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.skill_coverage = EvidenceValue::Unsupported;
        evidence
    }

    #[test]
    fn source_attribution_produces_a_priced_cause_when_flat_coverage_is_unsupported() {
        let evidence = unsupported_coverage_evidence();
        let mut source_evidence = SessionTokenBurnEvidence::default();
        source_evidence.skill_sources = Some(vec![crate::insights::TokenBurnSourceEvidence {
            scope: "agent:user".to_owned(),
            name: "skill-a".to_owned(),
            replicated_tokens: 150,
            invoked: false,
            replicated_cost_usd: Some(0.015),
        }]);
        source_evidence.pricing_revision = Some("pricing-generation-9".to_owned());

        assert!(source_assessable(&evidence, Some(&source_evidence)));
        assert_eq!(
            evaluate_with_source_evidence(&evidence, Some(&source_evidence)),
            Observation::Finding
        );
        assert_eq!(
            finding_causes_with_source_evidence(&evidence, Some(&source_evidence)),
            vec![FindingCause::UnusedSkill {
                skill: "skill-a".to_owned(),
                tokens: Some(150),
                cost_usd: Some(0.015),
                pricing_revision: Some("pricing-generation-9".to_owned()),
            }]
        );
    }

    #[test]
    fn source_attribution_is_not_assessable_when_flat_coverage_resolves() {
        // skill_coverage is Complete here, so the flat path already applies
        // and source evidence must not override it.
        let evidence = with_skill(false);
        let mut source_evidence = SessionTokenBurnEvidence::default();
        source_evidence.skill_sources = Some(vec![crate::insights::TokenBurnSourceEvidence {
            scope: "agent:user".to_owned(),
            name: "skill-a".to_owned(),
            replicated_tokens: 150,
            invoked: false,
            replicated_cost_usd: Some(0.015),
        }]);

        assert!(!source_assessable(&evidence, Some(&source_evidence)));
        assert_eq!(
            evaluate_with_source_evidence(&evidence, Some(&source_evidence)),
            evaluate(&evidence)
        );
    }

    #[test]
    fn an_invoked_source_is_excluded_from_source_attribution() {
        let evidence = unsupported_coverage_evidence();
        let mut source_evidence = SessionTokenBurnEvidence::default();
        source_evidence.skill_sources = Some(vec![crate::insights::TokenBurnSourceEvidence {
            scope: "agent:user".to_owned(),
            name: "skill-a".to_owned(),
            replicated_tokens: 150,
            invoked: true,
            replicated_cost_usd: Some(0.015),
        }]);

        assert_eq!(
            evaluate_with_source_evidence(&evidence, Some(&source_evidence)),
            Observation::NoFinding
        );
        assert!(finding_causes_with_source_evidence(&evidence, Some(&source_evidence)).is_empty());
    }

    #[test]
    fn a_zero_token_source_is_excluded_from_source_attribution() {
        let evidence = unsupported_coverage_evidence();
        let mut source_evidence = SessionTokenBurnEvidence::default();
        source_evidence.skill_sources = Some(vec![crate::insights::TokenBurnSourceEvidence {
            scope: "agent:user".to_owned(),
            name: "skill-a".to_owned(),
            replicated_tokens: 0,
            invoked: false,
            replicated_cost_usd: None,
        }]);

        assert_eq!(
            evaluate_with_source_evidence(&evidence, Some(&source_evidence)),
            Observation::NoFinding
        );
        assert!(finding_causes_with_source_evidence(&evidence, Some(&source_evidence)).is_empty());
    }
}

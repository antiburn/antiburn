//! Nine detector rule sets over assessed-cohort session evidence.
//!
//! Each detector produces exactly one status per report: findings, clean,
//! or not assessed with a structured reason. Clean requires that every
//! eligible session carries complete required evidence and shows no
//! finding. Incomplete absence of a signal never produces clean.
//! Thresholds and catalogs are report-time policy inputs. Evidence stays
//! rule-neutral (Locked Decision 2).
//!
//! An evidence-bearing or capped unknown record blocks clean results. A structurally inert unknown drops no evidence and keeps complete coverage.
//! A session with only inert records has zero work. `in_denominator` excludes it for two detectors only.

mod cache_churn;
mod model_overthinking;
mod model_registry;
mod old_model_usage;
mod overpowered_subagents;
mod overuse_of_fast_mode;
mod sessions_over_depth;
mod unused_built_in_tools;
mod unused_mcp_servers;
mod unused_skills;

use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::{EvidenceValue, SessionEvidence};
use crate::pricing::canonical_model_key;
use crate::remediation::FindingCause;

use super::report::{DetectorCounts, MAX_EXAMPLES_PER_DETECTOR, SessionExample};
use super::status::DetectorId;

pub use model_registry::{
    ModelRegistry, ModelReplacementEntry, ModelReplacementRule, REGISTRY_REVISION,
};

/// One report-level status for one detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectorStatus {
    Findings(DetectorFindings),
    Clean,
    NotAssessed(NotAssessedReason),
}

/// States why a detector could not assess its category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotAssessedReason {
    /// The assessed cohort holds no session for the window.
    NoSessionsInWindow,
    /// Sessions exist, but none carries the required capabilities.
    CapabilityMissing,
    /// Eligible sessions exist, but incomplete evidence coverage
    /// prevents a clean conclusion, and no finding was observed.
    IncompleteEvidence,
    /// The evidence schema does not yet carry the payload the rule
    /// needs, so neither a finding nor clean is expressible.
    EvidenceContractIncomplete,
    /// The source supports the signal, but no turn in this session
    /// carries it. No conclusion is possible. A turn without the
    /// signal is not itself negative evidence. See
    /// `overuse_of_fast_mode` and `model_overthinking` for the rule
    /// that assesses only the turns that do carry the signal.
    SignalMissing,
}

/// Bounded finding summary for one detector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectorFindings {
    pub finding_sessions: u64,
    pub examples: Vec<SessionExample>,
}

/// One per-session rule result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Observation {
    /// The rule observed at least one finding in this session.
    Finding,
    /// The rule observed no finding. Only complete required evidence
    /// lets the report turn this into a clean claim.
    NoFinding,
    /// The evidence contract cannot express the fact the rule needs.
    ContractIncomplete,
    /// The source supports the signal, but no turn in this session
    /// carries it. No conclusion is possible. A turn without the
    /// signal is not itself negative evidence. See
    /// `overuse_of_fast_mode` and `model_overthinking` for the rule
    /// that assesses only the turns that do carry the signal.
    SignalMissing,
}

/// One allocation-free detector result used by aggregate report evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetectorEvaluation {
    pub observation: Observation,
}

impl PartialEq<Observation> for DetectorEvaluation {
    fn eq(&self, other: &Observation) -> bool {
        self.observation == *other
    }
}

impl PartialEq<DetectorEvaluation> for Observation {
    fn eq(&self, other: &DetectorEvaluation) -> bool {
        *self == other.observation
    }
}

/// Bounded per-detector fold state across the assessed cohort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DetectorFold {
    pub finding_sessions: u64,
    pub examples: Vec<SessionExample>,
    pub contract_incomplete: u64,
    pub signal_missing: u64,
    pub partial_sessions: u64,
}

impl DetectorFold {
    pub(crate) fn observe(&mut self, observation: Observation, evidence: &SessionEvidence) {
        self.partial_sessions += u64::from(matches!(
            evidence.coverage,
            crate::analysis::EvidenceCoverage::Partial(_)
        ));
        match observation {
            Observation::Finding => {
                self.finding_sessions += 1;
                if self.examples.len() < MAX_EXAMPLES_PER_DETECTOR {
                    self.examples.push(SessionExample {
                        agent: evidence.identity.agent.clone(),
                        session_id: evidence.identity.session_id.clone(),
                    });
                }
            }
            Observation::NoFinding => {}
            Observation::ContractIncomplete => self.contract_incomplete += 1,
            Observation::SignalMissing => self.signal_missing += 1,
        }
    }
}

/// A model family, derived from the normalized model key's prefix.
/// Tier policy is keyed by family, not by harness, because OpenCode and
/// Pi can run any vendor's models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModelFamily {
    Claude,
    OpenAi,
    Google,
    /// No known vendor prefix matched. A tier or premium check can
    /// never classify an unknown family; it always reports a contract
    /// gap instead of a finding or clean result.
    Unknown,
}

/// Classifies a model key into its family from the canonical model key's
/// prefix (provider namespace stripped, date suffix stripped, lowercased;
/// see [`canonical_model_key`]).
pub fn model_family(model: &str) -> ModelFamily {
    let canonical = canonical_model_key(model);
    if canonical.starts_with("claude-") {
        ModelFamily::Claude
    } else if canonical.starts_with("gpt-")
        || canonical.starts_with("o1")
        || canonical.starts_with("o3")
        || canonical.starts_with("o4")
    {
        ModelFamily::OpenAi
    } else if canonical.starts_with("gemini-") {
        ModelFamily::Google
    } else {
        ModelFamily::Unknown
    }
}

/// One family's reasoning-effort tier policy: which normalized labels
/// count as above the recommended cap, and which labels the family
/// recognizes at all. A recognized label that is not above the cap is
/// clean; an unrecognized label with turns blocks clean until the
/// policy classifies it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EffortPolicy {
    pub above_cap: BTreeSet<String>,
    pub recognized: BTreeSet<String>,
}

/// One family's fast-mode speed policy: the normalized labels it
/// recognizes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeedPolicy {
    pub recognized: BTreeSet<String>,
}

/// One family's premium-tier policy for Overpowered Subagents.
///
/// `reviewed` states whether a maintainer has classified this family's
/// premium tier at all. An unreviewed family's models can never prove
/// premium or non-premium; the detector reports a contract gap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PremiumPolicy {
    pub reviewed: bool,
    /// Canonical model keys that a maintainer reviewed as premium.
    pub models: BTreeSet<String>,
    /// Canonical model keys that a maintainer reviewed as nonpremium.
    pub exceptions: BTreeSet<String>,
}

impl PremiumPolicy {
    /// Returns the reviewed tier verdict for a canonical model key.
    /// An unmatched key has no reviewed tier identity.
    pub fn verdict(&self, canonical: &str) -> Option<bool> {
        if !self.reviewed {
            return None;
        }
        if self.exceptions.contains(canonical) {
            return Some(false);
        }
        if self.models.contains(canonical) {
            Some(true)
        } else {
            None
        }
    }

    /// Returns true only for a reviewed premium model.
    pub fn is_premium(&self, canonical: &str) -> bool {
        self.verdict(canonical) == Some(true)
    }
}

/// One model family's full tier policy.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FamilyPolicy {
    pub effort: EffortPolicy,
    pub speed: SpeedPolicy,
    pub premium: PremiumPolicy,
    /// Whether Cache Churn has a reviewed threshold for this family.
    pub cache_policy_reviewed: bool,
    /// The overpay multiple (`RepeatedContext::paid_tokens` divided by
    /// unique paid tokens) at or above which Cache Churn calls a
    /// finding. This is the reviewed average-efficiency band bound.
    /// `cache_policy_reviewed` gates this field. An unreviewed family's
    /// models never prove a finding.
    pub cache_overpay_multiple_threshold: f64,
}

/// Report-time policy inputs. Catalogs change without reparsing
/// transcripts and without touching persisted evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportCatalogs {
    pub revision: i64,
    /// A request whose observed context depth exceeds this cap is a
    /// Sessions Over Depth finding.
    pub depth_cap_tokens: u64,
    /// Reviewed tier policy, one entry per known model family.
    pub families: BTreeMap<ModelFamily, FamilyPolicy>,
    /// Curated deprecated-model registry, keyed by normalized model
    /// name and alias.
    pub model_replacements: ModelRegistry,
    /// Delegated fast-tier turns at or above this count are a finding.
    /// Zero observed delegated turns never fire, whatever the value.
    pub fast_mode_delegated_turns_threshold: u64,
}

/// Effort tiers above the recommended cap in every reviewed family:
/// `xhigh`, `max`, and `ultra`. The reviewed production data did not contain
/// `ultrathink` as an `effort` value, so this set does not include it.
fn above_cap_effort_tiers() -> BTreeSet<String> {
    ["xhigh", "max", "ultra"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// These per-family average-efficiency bands use production data from
/// 2026-08-05. The data covers 115 Claude Code users and 33 Codex users
/// with more than 500k paid tokens in 30 days. Each band has four bounds:
/// `[good, fair, poor, very poor]`. `cache_overpay_multiple_threshold`
/// uses the fair bound as the finding threshold:
/// - Claude: `[1.9, 2.35, 3.35, 4.45]`, threshold `2.35`.
/// - Codex (OpenAI): `[1.7, 2.0, 2.35, 2.8]`, threshold `2.0`.
///
/// The source data computes this multiple per user over a 30-day window.
/// This rule computes it per session. A small session is noisier than the
/// per-user aggregate, so review findings based on few paid tokens carefully.
impl Default for ReportCatalogs {
    fn default() -> Self {
        let mut families = BTreeMap::new();
        families.insert(
            ModelFamily::Claude,
            FamilyPolicy {
                effort: EffortPolicy {
                    above_cap: above_cap_effort_tiers(),
                    recognized: ["low", "medium", "high"]
                        .into_iter()
                        .map(str::to_owned)
                        .chain(above_cap_effort_tiers())
                        .collect(),
                },
                speed: SpeedPolicy {
                    recognized: ["fast", "standard"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                },
                premium: PremiumPolicy {
                    reviewed: true,
                    models: [
                        "claude-opus-4-6",
                        "claude-opus-4-7",
                        "claude-opus-4-8",
                        "claude-opus-5",
                        "claude-fable-5",
                        "claude-mythos-5",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                    exceptions: BTreeSet::new(),
                },
                cache_policy_reviewed: true,
                cache_overpay_multiple_threshold: 2.35,
            },
        );
        families.insert(
            ModelFamily::OpenAi,
            FamilyPolicy {
                effort: EffortPolicy {
                    above_cap: above_cap_effort_tiers(),
                    recognized: ["none", "minimal", "low", "medium", "high"]
                        .into_iter()
                        .map(str::to_owned)
                        .chain(above_cap_effort_tiers())
                        .collect(),
                },
                speed: SpeedPolicy {
                    recognized: ["fast", "standard"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                },
                premium: PremiumPolicy {
                    reviewed: true,
                    models: [
                        "gpt-5.5",
                        "gpt-5.5-fast",
                        "gpt-5.6",
                        "gpt-5.6-sol",
                        "gpt-6-astra",
                        "gpt-6-astra-fast",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                    exceptions: [
                        "gpt-5.6-terra",
                        "gpt-5.6-luna",
                        "gpt-5.3-codex-spark",
                        "codex-auto-review",
                    ]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                },
                cache_policy_reviewed: true,
                cache_overpay_multiple_threshold: 2.0,
            },
        );
        families.insert(
            ModelFamily::Google,
            FamilyPolicy {
                premium: PremiumPolicy {
                    reviewed: true,
                    models: ["gemini-3.1-pro", "gemini-3.8-pro", "gemini-3.8-pro-preview"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    exceptions: BTreeSet::new(),
                },
                cache_policy_reviewed: false,
                ..FamilyPolicy::default()
            },
        );
        // `Unknown` stays fully default: `premium.reviewed` is `false`
        // and `cache_overpay_multiple_threshold` is `0.0`. Neither
        // matters, since an unreviewed family is never scored.
        families.insert(ModelFamily::Unknown, FamilyPolicy::default());

        Self {
            revision: 9,
            depth_cap_tokens: 400_000,
            families,
            model_replacements: model_registry::default_registry(),
            fast_mode_delegated_turns_threshold: 1,
        }
    }
}

/// Runs one detector rule over one eligible session.
pub(crate) fn evaluate(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> DetectorEvaluation {
    let observation = match detector {
        DetectorId::SessionsOverDepth => sessions_over_depth::evaluate(evidence, catalogs),
        DetectorId::ModelOverthinking => model_overthinking::evaluate(evidence, catalogs),
        DetectorId::OverpoweredSubagents => overpowered_subagents::evaluate(evidence, catalogs),
        DetectorId::UnusedMcpServers => unused_mcp_servers::evaluate(evidence),
        DetectorId::UnusedBuiltInTools => unused_built_in_tools::evaluate(evidence),
        DetectorId::UnusedSkills => unused_skills::evaluate(evidence),
        DetectorId::OldModelUsage => old_model_usage::evaluate(evidence, catalogs),
        DetectorId::OveruseOfFastMode => overuse_of_fast_mode::evaluate(evidence, catalogs),
        DetectorId::CacheChurn => cache_churn::evaluate(evidence, catalogs),
    };
    DetectorEvaluation { observation }
}

/// Builds exact causes only for a selected detector that has a finding.
pub(crate) fn finding_causes(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
) -> Vec<FindingCause> {
    let causes = match detector {
        DetectorId::SessionsOverDepth => sessions_over_depth::finding_causes(evidence, catalogs),
        DetectorId::ModelOverthinking => model_overthinking::finding_causes(evidence, catalogs),
        DetectorId::OverpoweredSubagents => {
            overpowered_subagents::finding_causes(evidence, catalogs)
        }
        DetectorId::UnusedMcpServers => unused_mcp_servers::finding_causes(evidence),
        DetectorId::UnusedBuiltInTools => unused_built_in_tools::finding_causes(evidence),
        DetectorId::UnusedSkills => unused_skills::finding_causes(evidence),
        DetectorId::OldModelUsage => old_model_usage::finding_causes(evidence, catalogs),
        DetectorId::OveruseOfFastMode => overuse_of_fast_mode::finding_causes(evidence, catalogs),
        DetectorId::CacheChurn => cache_churn::finding_causes(evidence, catalogs),
    };
    debug_assert!(causes.iter().all(|cause| cause.detector() == detector));
    causes
}

pub(crate) fn built_in_source_assessable(
    detector: DetectorId,
    evidence: &SessionEvidence,
    source_evidence: Option<&super::report::SessionTokenBurnEvidence>,
) -> bool {
    match detector {
        DetectorId::UnusedBuiltInTools => {
            unused_built_in_tools::source_assessable(evidence, source_evidence)
        }
        _ => false,
    }
}

pub(crate) fn evaluate_with_source_evidence(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
    source_evidence: Option<&super::report::SessionTokenBurnEvidence>,
) -> DetectorEvaluation {
    match detector {
        DetectorId::UnusedBuiltInTools => DetectorEvaluation {
            observation: unused_built_in_tools::evaluate_with_source_evidence(
                evidence,
                source_evidence,
            ),
        },
        _ => evaluate(detector, evidence, catalogs),
    }
}

pub(crate) fn finding_causes_with_source_evidence(
    detector: DetectorId,
    evidence: &SessionEvidence,
    catalogs: &ReportCatalogs,
    source_evidence: Option<&super::report::SessionTokenBurnEvidence>,
) -> Vec<FindingCause> {
    let causes = match detector {
        DetectorId::UnusedBuiltInTools => {
            unused_built_in_tools::finding_causes_with_source_evidence(evidence, source_evidence)
        }
        _ => finding_causes(detector, evidence, catalogs),
    };
    debug_assert!(causes.iter().all(|cause| cause.detector() == detector));
    causes
}

/// Returns whether this session belongs in the detector's eligible
/// denominator. Unused MCP Servers and Unused Skills make absence
/// claims about assistant work; a session is excluded only when
/// complete eligibility evidence proves zero assistant turns, so an
/// all-idle cohort cannot read clean. Absence read from partial
/// evidence is untrustworthy (see `observed`), so a partial-
/// eligibility session stays in the denominator whatever its observed
/// count: the assessed-only-when-complete rule holds it at
/// eligible-but-unassessed, blocking a clean claim.
pub(crate) fn in_denominator(detector: DetectorId, evidence: &SessionEvidence) -> bool {
    match detector {
        DetectorId::UnusedMcpServers | DetectorId::UnusedSkills => complete(&evidence.eligibility)
            .is_none_or(|eligibility| eligibility.assistant_turns > 0),
        _ => true,
    }
}

/// Reduces one detector's counts and fold state to its one status.
///
/// Findings win first because partial coverage can support an observed
/// finding. Clean requires at least one eligible session and a clean outcome
/// for every eligible session. Capability gaps outside that denominator do
/// not invalidate the clean result. Everything else is not assessed.
pub(crate) fn status(
    counts: DetectorCounts,
    fold: DetectorFold,
    assessed_sessions: u64,
) -> DetectorStatus {
    if fold.finding_sessions > 0 {
        return DetectorStatus::Findings(DetectorFindings {
            finding_sessions: fold.finding_sessions,
            examples: fold.examples,
        });
    }
    if assessed_sessions == 0 {
        return DetectorStatus::NotAssessed(NotAssessedReason::NoSessionsInWindow);
    }
    if counts.eligible == 0 {
        return DetectorStatus::NotAssessed(NotAssessedReason::CapabilityMissing);
    }
    if fold.contract_incomplete > 0 {
        return DetectorStatus::NotAssessed(NotAssessedReason::EvidenceContractIncomplete);
    }
    if fold.signal_missing > 0 {
        return DetectorStatus::NotAssessed(NotAssessedReason::SignalMissing);
    }
    if counts.clean == counts.eligible && fold.partial_sessions == 0 {
        return DetectorStatus::Clean;
    }
    DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
}

/// Returns the observed value from complete or partial evidence.
/// Presence read from partial evidence is trustworthy; absence is not.
pub(crate) fn observed<T>(value: &EvidenceValue<T>) -> Option<&T> {
    match value {
        EvidenceValue::Unsupported => None,
        EvidenceValue::Partial { observed, .. } => Some(observed),
        EvidenceValue::Complete(value) => Some(value),
    }
}

/// Returns the value only when the evidence is complete.
/// Only a complete value can prove that an event did not happen.
pub(crate) fn complete<T>(value: &EvidenceValue<T>) -> Option<&T> {
    match value {
        EvidenceValue::Complete(value) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::analysis::{
        EvidenceSource, SessionEvidence, SessionEvidenceAccumulator, SourceCapabilities,
        SourceKind, TurnFacts,
    };

    /// Builds empty complete evidence with the Claude capability set.
    pub(crate) fn claude_evidence(session_id: &str) -> SessionEvidence {
        SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: session_id.to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::ToolDefinition;
    use crate::remediation::BuiltInToolTokens;

    fn counts(eligible: u64, finding: u64, clean: u64, unavailable: u64) -> DetectorCounts {
        DetectorCounts {
            eligible,
            assessed: finding + clean,
            finding,
            clean,
            unavailable,
            not_applicable: 0,
        }
    }

    #[test]
    fn findings_take_precedence_over_incomplete_coverage() {
        let fold = DetectorFold {
            finding_sessions: 2,
            examples: Vec::new(),
            contract_incomplete: 1,
            signal_missing: 0,
            partial_sessions: 0,
        };

        assert!(matches!(
            status(counts(3, 2, 0, 1), fold, 3),
            DetectorStatus::Findings(DetectorFindings {
                finding_sessions: 2,
                ..
            })
        ));
    }

    #[test]
    fn empty_cohort_is_not_assessed() {
        assert_eq!(
            status(counts(0, 0, 0, 0), DetectorFold::default(), 0),
            DetectorStatus::NotAssessed(NotAssessedReason::NoSessionsInWindow)
        );
    }

    #[test]
    fn missing_capabilities_are_not_assessed() {
        assert_eq!(
            status(counts(0, 0, 0, 4), DetectorFold::default(), 4),
            DetectorStatus::NotAssessed(NotAssessedReason::CapabilityMissing)
        );
    }

    #[test]
    fn incomplete_absence_never_yields_clean() {
        // One of two eligible sessions carries only partial evidence.
        // The zero-finding result must not read as clean.
        assert_eq!(
            status(counts(2, 0, 1, 1), DetectorFold::default(), 2),
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
        );
    }

    #[test]
    fn contract_incomplete_sessions_prevent_clean() {
        let fold = DetectorFold {
            finding_sessions: 0,
            examples: Vec::new(),
            contract_incomplete: 1,
            signal_missing: 0,
            partial_sessions: 0,
        };

        assert_eq!(
            status(counts(2, 0, 1, 1), fold, 2),
            DetectorStatus::NotAssessed(NotAssessedReason::EvidenceContractIncomplete)
        );
    }

    #[test]
    fn signal_missing_sessions_prevent_clean() {
        let fold = DetectorFold {
            finding_sessions: 0,
            examples: Vec::new(),
            contract_incomplete: 0,
            signal_missing: 1,
            partial_sessions: 0,
        };

        assert_eq!(
            status(counts(2, 0, 1, 1), fold, 2),
            DetectorStatus::NotAssessed(NotAssessedReason::SignalMissing)
        );
    }

    #[test]
    fn complete_absence_yields_clean() {
        assert_eq!(
            status(counts(2, 0, 2, 0), DetectorFold::default(), 2),
            DetectorStatus::Clean
        );
    }

    #[test]
    fn partial_session_fold_blocks_clean_but_preserves_findings() {
        let mut evidence = test_support::claude_evidence("partial-fold");
        evidence.coverage = crate::analysis::EvidenceCoverage::Partial(
            crate::analysis::CoverageReason::MalformedRecord,
        );
        let mut fold = DetectorFold::default();
        fold.observe(Observation::NoFinding, &evidence);
        assert_eq!(
            status(counts(1, 0, 1, 0), fold.clone(), 1),
            DetectorStatus::NotAssessed(NotAssessedReason::IncompleteEvidence)
        );
        fold.observe(Observation::Finding, &evidence);
        assert!(matches!(
            status(counts(2, 1, 1, 0), fold, 2),
            DetectorStatus::Findings(_)
        ));
    }

    #[test]
    fn model_family_classifies_a_provider_prefixed_openai_key() {
        assert_eq!(model_family("openai.gpt-5.6-sol"), ModelFamily::OpenAi);
    }

    #[test]
    fn model_family_classifies_a_slash_namespaced_claude_key() {
        assert_eq!(
            model_family("anthropic/claude-opus-4.8"),
            ModelFamily::Claude
        );
    }

    #[test]
    fn model_family_classifies_an_antigravity_prefixed_claude_key() {
        assert_eq!(
            model_family("antigravity-claude-opus-4-6-thinking"),
            ModelFamily::Claude
        );
    }

    #[test]
    fn model_family_classifies_an_antigravity_prefixed_gemini_key() {
        assert_eq!(
            model_family("antigravity-gemini-3.8-pro-preview"),
            ModelFamily::Google
        );
    }

    #[test]
    fn google_premium_policy_flags_canonical_gemini_pro() {
        let catalogs = ReportCatalogs::default();
        let canonical = canonical_model_key("antigravity-gemini-3.8-pro-preview");
        assert_eq!(
            catalogs.families[&ModelFamily::Google]
                .premium
                .verdict(&canonical),
            Some(true)
        );
    }

    #[test]
    fn openai_premium_policy_flags_bare_gpt_5_6() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert_eq!(policy.verdict("gpt-5.6"), Some(true));
    }

    #[test]
    fn openai_premium_policy_flags_gpt_5_5_fast() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert_eq!(policy.verdict("gpt-5.5-fast"), Some(true));
    }

    #[test]
    fn openai_premium_policy_flags_gpt_6_astra() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert_eq!(policy.verdict("gpt-6-astra"), Some(true));
        assert_eq!(policy.verdict("gpt-6-astra-fast"), Some(true));
    }

    #[test]
    fn openai_premium_policy_does_not_guess_other_gpt_6_models() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert_eq!(policy.verdict("gpt-6-unknown"), None);
    }

    #[test]
    fn premium_policy_does_not_guess_an_unreviewed_premium_named_model() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::Claude].premium;
        assert_eq!(policy.verdict("claude-opus-unreviewed"), None);
    }

    #[test]
    fn openai_premium_policy_excepts_gpt_5_6_terra() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert_eq!(policy.verdict("gpt-5.6-terra"), Some(false));
    }

    #[test]
    fn openai_premium_policy_excepts_gpt_5_6_luna() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert_eq!(policy.verdict("gpt-5.6-luna"), Some(false));
    }

    #[test]
    fn claude_premium_policy_flags_mythos() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::Claude].premium;
        assert_eq!(policy.verdict("claude-mythos-5"), Some(true));
    }

    #[test]
    fn claude_premium_policy_does_not_flag_sonnet() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::Claude].premium;
        assert_eq!(policy.verdict("claude-sonnet-5"), None);
    }

    #[test]
    fn claude_premium_policy_does_not_flag_haiku() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::Claude].premium;
        assert_eq!(policy.verdict("claude-haiku-4-5"), None);
    }

    #[test]
    fn detector_derived_causes_match_their_observation_and_detector() {
        let mut evidence = test_support::claude_evidence("cause-invariant");
        let EvidenceValue::Complete(eligibility) = &mut evidence.eligibility else {
            unreachable!()
        };
        eligibility.assistant_turns = 1;
        let mut definitions = BTreeMap::new();
        definitions.insert(
            "Read".to_owned(),
            ToolDefinition {
                tokens: 73,
                invoked: false,
                deferred: false,
            },
        );
        let EvidenceValue::Complete(sources) = &mut evidence.context_sources else {
            unreachable!()
        };
        sources.tool_definitions = EvidenceValue::Complete(definitions);

        for detector in DetectorId::ALL {
            let catalogs = ReportCatalogs::default();
            let observation = evaluate(detector, &evidence, &catalogs).observation;
            let causes = if observation == Observation::Finding {
                finding_causes(detector, &evidence, &catalogs)
            } else {
                Vec::new()
            };
            assert_eq!(observation == Observation::Finding, !causes.is_empty());
            assert!(causes.iter().all(|cause| cause.detector() == detector));
        }
        let causes = finding_causes(
            DetectorId::UnusedBuiltInTools,
            &evidence,
            &ReportCatalogs::default(),
        );
        assert_eq!(
            causes,
            vec![FindingCause::UnusedBuiltInTool {
                tool: "Read".to_owned(),
                tokens: BuiltInToolTokens::Definition(73),
            }]
        );
    }
}

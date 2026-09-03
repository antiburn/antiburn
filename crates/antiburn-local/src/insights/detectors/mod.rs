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

/// Bounded per-detector fold state across the assessed cohort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DetectorFold {
    pub finding_sessions: u64,
    pub examples: Vec<SessionExample>,
    pub contract_incomplete: u64,
    pub signal_missing: u64,
}

impl DetectorFold {
    pub(crate) fn observe(&mut self, observation: Observation, evidence: &SessionEvidence) {
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

/// One family's premium-tier policy for Overpowered Subagents, matching
/// Cadence `SubagentTier::classify`
/// (`crates/analysis/src/efficiency_findings.rs`).
///
/// `reviewed` states whether a maintainer has classified this family's
/// premium tier at all. An unreviewed family's models can never prove
/// premium or non-premium; the detector reports a contract gap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PremiumPolicy {
    pub reviewed: bool,
    /// A canonical model key is premium when it contains any of these
    /// substrings (Claude: `opus`, `fable`, `mythos`).
    pub substrings: Vec<String>,
    /// A canonical model key is premium when it starts with any of these
    /// prefixes (OpenAI: `gpt-5.6`, `gpt-5.5`), unless it is in
    /// `exceptions`.
    pub prefixes: Vec<String>,
    /// Canonical model keys that a substring or prefix match would
    /// otherwise call premium, but a maintainer has reviewed as budget
    /// tiers within that prefix (e.g. `gpt-5.6-terra`, `gpt-5.6-luna`).
    pub exceptions: BTreeSet<String>,
}

impl PremiumPolicy {
    /// Whether a canonical model key is premium under this policy.
    /// Callers check `reviewed` separately: an unreviewed policy's
    /// verdict here is not meaningful.
    pub fn is_premium(&self, canonical: &str) -> bool {
        if self.exceptions.contains(canonical) {
            return false;
        }
        self.substrings
            .iter()
            .any(|s| canonical.contains(s.as_str()))
            || self
                .prefixes
                .iter()
                .any(|p| canonical.starts_with(p.as_str()))
    }
}

/// One model family's full tier policy.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FamilyPolicy {
    pub effort: EffortPolicy,
    pub speed: SpeedPolicy,
    pub premium: PremiumPolicy,
    /// The overpay multiple (`RepeatedContext::paid_tokens` divided by
    /// unique paid tokens) at or above which Cache Churn calls a
    /// finding, matching Cadence's `CACHE_OVERPAY_BAND_BOUNDS` "avg
    /// efficiency" band bound. `premium.reviewed` gates this field the
    /// same way it gates premium status: an unreviewed family's models
    /// never prove a finding under this bound.
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
/// `xhigh`, `max`, and `ultra`. Cadence's production census
/// (`REASONING_EFFORT_TIER_POLICY`,
/// `crates/analysis/src/efficiency_findings.rs`) never observed
/// `ultrathink` as an `effort` value, so it is not in this set.
fn above_cap_effort_tiers() -> BTreeSet<String> {
    ["xhigh", "max", "ultra"]
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// Cadence's per-family cache-overpay "avg efficiency" bands
/// (`CACHE_OVERPAY_BAND_BOUNDS`,
/// `web/src/components/efficiency/EfficiencyContent.tsx`), from the
/// 2026-08-05 Cadence corpus: 115 Claude Code users and 33 Codex users
/// with over 500k paid tokens in 30 days. Each band is four bounds —
/// `[good, fair, poor, very poor]` — and `cache_overpay_multiple_threshold`
/// below takes the "fair" bound, the point Cadence calls a finding:
/// - Claude: `[1.9, 2.35, 3.35, 4.45]`, threshold `2.35`.
/// - Codex (OpenAI): `[1.7, 2.0, 2.35, 2.8]`, threshold `2.0`.
///
/// Cadence computes this multiple per user over a 30-day window; this
/// rule computes it per session. A very small session's multiple is
/// noisier than Cadence's per-user aggregate, so a session that trips
/// the bound on a handful of paid tokens deserves a skeptical read.
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
                    substrings: ["opus", "fable", "mythos"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    prefixes: Vec::new(),
                    exceptions: BTreeSet::new(),
                },
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
                    substrings: Vec::new(),
                    prefixes: vec!["gpt-5.6".to_owned(), "gpt-5.5".to_owned()],
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
                cache_overpay_multiple_threshold: 2.0,
            },
        );
        // `Unknown` stays fully default: `premium.reviewed` is `false`
        // and `cache_overpay_multiple_threshold` is `0.0`. Neither
        // matters, since an unreviewed family is never scored.
        families.insert(ModelFamily::Unknown, FamilyPolicy::default());

        Self {
            revision: 6,
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
) -> Observation {
    match detector {
        DetectorId::SessionsOverDepth => sessions_over_depth::evaluate(evidence, catalogs),
        DetectorId::ModelOverthinking => model_overthinking::evaluate(evidence, catalogs),
        DetectorId::OverpoweredSubagents => overpowered_subagents::evaluate(evidence, catalogs),
        DetectorId::UnusedMcpServers => unused_mcp_servers::evaluate(evidence),
        DetectorId::UnusedBuiltInTools => unused_built_in_tools::evaluate(evidence),
        DetectorId::UnusedSkills => unused_skills::evaluate(evidence),
        DetectorId::OldModelUsage => old_model_usage::evaluate(evidence, catalogs),
        DetectorId::OveruseOfFastMode => overuse_of_fast_mode::evaluate(evidence, catalogs),
        DetectorId::CacheChurn => cache_churn::evaluate(evidence, catalogs),
    }
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
    if counts.clean == counts.eligible {
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
    fn openai_premium_policy_flags_bare_gpt_5_6() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert!(policy.is_premium("gpt-5.6"));
    }

    #[test]
    fn openai_premium_policy_flags_gpt_5_5_fast() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert!(policy.is_premium("gpt-5.5-fast"));
    }

    #[test]
    fn openai_premium_policy_excepts_gpt_5_6_terra() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert!(!policy.is_premium("gpt-5.6-terra"));
    }

    #[test]
    fn openai_premium_policy_excepts_gpt_5_6_luna() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::OpenAi].premium;
        assert!(!policy.is_premium("gpt-5.6-luna"));
    }

    #[test]
    fn claude_premium_policy_flags_mythos() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::Claude].premium;
        assert!(policy.is_premium("claude-mythos-5"));
    }

    #[test]
    fn claude_premium_policy_does_not_flag_sonnet() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::Claude].premium;
        assert!(!policy.is_premium("claude-sonnet-5"));
    }

    #[test]
    fn claude_premium_policy_does_not_flag_haiku() {
        let policy = &ReportCatalogs::default().families[&ModelFamily::Claude].premium;
        assert!(!policy.is_premium("claude-haiku-4-5"));
    }
}

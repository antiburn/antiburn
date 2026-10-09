//! Local Burn Check implementations and shared check contracts.

pub mod ignored_instructions;
pub mod over_exploring;
pub mod scope_creep;
pub mod skill_opportunities;

mod id;
pub use id::{DetectorId, DetectorSelection};

mod assessment;
mod cache_churn;
mod fast_mode_overuse;
mod findings;
mod model_overthinking;
mod model_replacements;
mod old_model_usage;
mod overpowered_subagents;
mod policy;
pub mod requirements;
pub mod sampling;
mod session_overdepth;
mod unused_built_in_tools;
mod unused_mcp_servers;
mod unused_skills;

pub(crate) use assessment::{
    Observation, complete, evaluate, evaluate_with_source_evidence, in_denominator, observed,
    source_assessable,
};
pub use findings::{DetectorFindings, DetectorStatus, NotAssessedReason};
pub(crate) use findings::{DetectorFold, finding_causes_with_source_evidence, status};
pub use model_replacements::{
    ModelRegistry, ModelReplacementEntry, ModelReplacementRule, REGISTRY_REVISION,
};
pub use policy::{
    EffortPolicy, FamilyPolicy, ModelFamily, PremiumPolicy, ReportCatalogs, SpeedPolicy,
    model_family,
};

#[cfg(test)]
pub(crate) mod test_support {
    use crate::analysis::{
        EvidenceSource, SessionEvidence, SessionEvidenceAccumulator, SourceCapabilities,
        SourceKind, TurnFacts,
    };

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

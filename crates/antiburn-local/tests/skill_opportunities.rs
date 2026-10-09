pub use antiburn_local::{analysis, model};

pub mod checks {
    pub use crate::skill_opportunities;
    pub use antiburn_local::checks::*;

    pub(crate) mod test_support {
        use crate::analysis::{
            EvidenceSource, SessionEvidence, SessionEvidenceAccumulator, SourceCapabilities,
            SourceKind, TurnFacts,
        };

        pub(crate) fn claude_evidence(session_id: &str) -> SessionEvidence {
            SessionEvidenceAccumulator::new(EvidenceSource {
                agent: "claude".into(),
                session_id: session_id.into(),
                kind: SourceKind::File,
                capabilities: SourceCapabilities::claude(),
            })
            .evidence(&TurnFacts::default())
        }
    }
}

#[path = "../src/checks/skill_opportunities/mod.rs"]
pub mod skill_opportunities;

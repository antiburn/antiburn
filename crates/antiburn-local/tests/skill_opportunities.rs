pub use antiburn_local::analysis as production_analysis;
pub use antiburn_local::model;

// The source-mounted check needs the private parser types. Keep the production
// module private and compile the same parser source in this integration crate.
pub mod analysis {
    pub use crate::production_analysis::*;

    pub mod jev {
        pub use crate::production_analysis::jev::*;

        pub(crate) mod markdown_ranges {
            include!("../src/analysis/jev/markdown_ranges.rs");
        }
    }
}

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

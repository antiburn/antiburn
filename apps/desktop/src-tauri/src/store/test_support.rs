use antiburn_local::analysis::{
    EvidenceSource, SessionEvidenceAccumulator, SourceCapabilities, SourceKind, TurnFacts,
};

use super::SessionKey;

pub(crate) fn evidence_json(key: &SessionKey) -> String {
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: key.agent.clone(),
        session_id: key.session_id.clone(),
        kind: SourceKind::Jsonl,
        capabilities: SourceCapabilities::claude(),
    })
    .evidence(&TurnFacts::default());
    serde_json::to_string(&evidence).expect("serialize test evidence")
}

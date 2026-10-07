use super::findings::CurrentFindingSession;
use super::*;

use crate::scope_creep_worker::{SourceFence, current_publication, source_supported};

pub(crate) fn scope_creep_findings(
    connection: &rusqlite::Connection,
    session: &CurrentFindingSession,
) -> Result<Option<Vec<Finding>>> {
    scope_creep_findings_for_session(
        connection,
        &session.evidence,
        IgnoredInstructionSessionIdentity {
            environment_key: &session.environment_key,
            agent: &session.agent,
            session_id: &session.session_id,
            incarnation: session.incarnation,
            source_generation: session.source_generation,
            source_fingerprint: session.source_fingerprint.as_deref(),
            published_fence: session.published_fence,
        },
    )
}

pub(super) fn scope_creep_findings_for_session(
    connection: &rusqlite::Connection,
    evidence: &SessionEvidence,
    session: IgnoredInstructionSessionIdentity<'_>,
) -> Result<Option<Vec<Finding>>> {
    if evidence.identity.agent != session.agent
        || evidence.identity.session_id != session.session_id
        || !source_supported(session.agent, evidence.capabilities.source_format)
    {
        return Ok(None);
    }
    let key =
        crate::store::SessionKey::new(session.environment_key, session.agent, session.session_id);
    let Some(publication) = current_publication(
        connection,
        &SourceFence {
            key: &key,
            incarnation: session.incarnation,
            source_generation: session.source_generation,
            source_fingerprint: session.source_fingerprint,
            published_fence: session.published_fence,
        },
    )?
    else {
        return Ok(None);
    };
    Ok(publication
        .assessment
        .findings
        .iter()
        .map(|finding| Finding::scope_creep(evidence, finding))
        .collect())
}

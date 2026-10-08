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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_report_requires_current_source_bound_publication() {
        let fixture = crate::scope_creep_worker::tests::NativeFixture::new(1);
        let candidate = fixture.publish_finding();
        let evidence: SessionEvidence = serde_json::from_str(
            &fixture
                .store
                .evidence(&candidate.session.key)
                .unwrap()
                .unwrap()
                .evidence_json
                .unwrap(),
        )
        .unwrap();
        let read = |generation| {
            scope_creep_findings_for_session(
                &fixture.store.lock(),
                &evidence,
                IgnoredInstructionSessionIdentity {
                    environment_key: &candidate.session.key.environment_key,
                    agent: &candidate.session.key.agent,
                    session_id: &candidate.session.key.session_id,
                    incarnation: candidate.incarnation,
                    source_generation: generation,
                    source_fingerprint: candidate.source_fingerprint.as_deref(),
                    published_fence: candidate.published_fence,
                },
            )
            .unwrap()
        };
        let findings = read(candidate.source_generation).unwrap();
        assert!(!findings.is_empty());
        assert!(
            findings[0]
                .display()
                .unwrap()
                .observation
                .starts_with("Attempts")
        );
        assert!(read(candidate.source_generation + 1).is_none());
        fixture.store.lock().execute(
            "UPDATE burn_check_assessment SET result_revision = 'stale' WHERE check_id = 'scope_creep'", [],
        ).unwrap();
        assert!(read(candidate.source_generation).is_none());
    }
}

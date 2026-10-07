use super::findings::CurrentFindingSession;
use super::*;

use crate::jev::worker::JevCheckDescriptor;
use crate::over_exploring_worker::{
    CHECK, CHECK_ID, Publication, publication_has_clean_coverage, publishable_finding,
    source_supported,
};

pub(crate) fn over_exploring_findings(
    connection: &rusqlite::Connection,
    session: &CurrentFindingSession,
) -> Result<Option<Vec<Finding>>> {
    over_exploring_findings_for_session(
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

pub(super) fn over_exploring_findings_for_session(
    connection: &rusqlite::Connection,
    evidence: &SessionEvidence,
    session: IgnoredInstructionSessionIdentity<'_>,
) -> Result<Option<Vec<Finding>>> {
    if session.environment_key != "native"
        || evidence.identity.agent != session.agent
        || evidence.identity.session_id != session.session_id
        || !source_supported(session.agent, evidence.capabilities.source_format)
    {
        return Ok(None);
    }
    let current_evidence = crate::store::revision_sql::current_evidence("e", "s");
    let sql = format!(
        "SELECT a.status, a.input_revision, a.result_revision, a.result_json, a.last_error_category
           FROM burn_check_assessment a
           JOIN session s ON s.environment_key = a.environment_key AND s.agent = a.agent
             AND s.session_id = a.session_id
           JOIN session_evidence e ON e.environment_key = s.environment_key AND e.agent = s.agent
             AND e.session_id = s.session_id
           WHERE a.environment_key = :environment_key AND a.agent = :agent AND a.session_id = :session_id AND a.check_id = :check_id
             AND s.incarnation = :incarnation AND a.incarnation = s.incarnation
             AND s.source_generation = :source_generation AND a.source_generation = s.source_generation
             AND s.source_fingerprint IS :source_fingerprint AND a.source_fingerprint IS s.source_fingerprint
             AND e.published_fence = :published_fence AND a.published_fence = e.published_fence
             AND a.evaluator_revision = :evaluator_revision AND e.status = 'ready'
             AND e.processed_fingerprint IS s.source_fingerprint
             AND {current_evidence}"
    );
    let stored = connection
        .query_row(
            &sql,
            named_params![
                ":environment_key": session.environment_key,
                ":agent": session.agent,
                ":session_id": session.session_id,
                ":check_id": CHECK_ID,
                ":incarnation": session.incarnation,
                ":source_generation": session.source_generation,
                ":source_fingerprint": session.source_fingerprint,
                ":published_fence": session.published_fence,
                ":evaluator_revision": CHECK.evaluator_revision(),
                ":parser_revision": PARSER_REVISION,
                ":analyzer_revision": ANALYZER_REVISION,
                ":evidence_schema_revision": EVIDENCE_SCHEMA_REVISION,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((status, Some(input), Some(revision), Some(json), category)) = stored else {
        return Ok(None);
    };
    let Ok(publication) = serde_json::from_str::<Publication>(&json) else {
        return Ok(None);
    };
    if input != revision
        || publication.input_revision != revision
        || publication.snapshot_revision.is_empty()
        || publication.semantic_revision.is_empty()
        || publication.model.is_empty()
    {
        return Ok(None);
    }
    let complete = status == "completed" && publication_has_clean_coverage(&publication.assessment);
    let partial = status == "failed"
        && category.as_deref() == Some("sampling_incomplete")
        && !publication.assessment.findings.is_empty();
    if !complete && !partial {
        return Ok(None);
    }
    let mut seen = BTreeSet::new();
    let findings: Option<Vec<_>> = publication
        .assessment
        .findings
        .iter()
        .map(|finding| {
            if !seen.insert(&finding.work_item_id) || !publishable_finding(finding, &publication) {
                return None;
            }
            Finding::over_exploring(evidence, finding)
        })
        .collect();
    Ok(findings.filter(|findings| complete || !findings.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::over_exploring_worker::{prepare, publication, tests::fixture};
    use antiburn_local::analysis::jev::JevCheck;
    use antiburn_local::checks::over_exploring::OverExploringCheck;

    #[test]
    fn native_read_publications_reach_findings_prompts_and_session_statuses() {
        use crate::over_exploring_worker::tests::reduced;
        use crate::scope_creep_worker::tests::native_sources;
        for (agent, session, format, records) in native_sources::read_sources() {
            let directory = tempfile::tempdir().unwrap();
            let store = crate::store::Store::open(directory.path()).unwrap();
            for detector in [DetectorId::OverExploring, DetectorId::ScopeCreep] {
                store.set_check_enabled(detector, true).unwrap();
            }
            store
                .capture_burn_check_boundaries(&[CHECK_ID, crate::scope_creep_worker::CHECK_ID], 0)
                .unwrap();
            let mut candidate = native_sources::publish(
                &store,
                &agent,
                &session,
                format,
                &records,
                directory.path(),
            );
            let snapshot = store
                .load_smart_check_inputs(
                    &candidate.session.key,
                    candidate.published_fence,
                    candidate.source_generation,
                    crate::smart_check_inputs::DetectorInput::OverExploring,
                )
                .unwrap();
            candidate
                .boundary_positions
                .insert(snapshot.boundary().source_key.clone(), u64::MAX);
            candidate.historical = true;
            let input = prepare(
                &candidate,
                snapshot,
                &antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default(),
            )
            .unwrap();
            let result = reduced(
                &input,
                antiburn_local::checks::over_exploring::Reason::UnrelatedFiles,
            );
            assert!(!result.findings.is_empty(), "{agent}");
            let published = publication(&input, result);
            assert!(
                store
                    .queue_burn_check_assessment(&input.durable, 1000, 180)
                    .unwrap()
            );
            assert!(
                store
                    .claim_burn_check_assessment(&input.durable, 1000, 300, 180)
                    .unwrap()
            );
            assert!(
                store
                    .complete_burn_check_assessment(
                        &input.durable,
                        &serde_json::to_string(&published).unwrap(),
                        1000,
                        180
                    )
                    .unwrap()
            );
            let evidence: SessionEvidence = serde_json::from_str(
                &store
                    .evidence(&candidate.session.key)
                    .unwrap()
                    .unwrap()
                    .evidence_json
                    .unwrap(),
            )
            .unwrap();
            let read = || {
                over_exploring_findings_for_session(
                    &store.lock(),
                    &evidence,
                    IgnoredInstructionSessionIdentity {
                        environment_key: &candidate.session.key.environment_key,
                        agent: &candidate.session.key.agent,
                        session_id: &candidate.session.key.session_id,
                        incarnation: candidate.incarnation,
                        source_generation: candidate.source_generation,
                        source_fingerprint: candidate.source_fingerprint.as_deref(),
                        published_fence: candidate.published_fence,
                    },
                )
                .unwrap()
            };
            let findings = read().unwrap();
            assert!(!findings.is_empty(), "{agent}");
            let prompt = antiburn_local::remediation::remediation_prompt(&findings[0]).unwrap();
            assert!(
                prompt.as_str().contains("does not repair or verify"),
                "{agent}"
            );
            let statuses = super::super::findings::smart_session_statuses(
                directory.path(),
                std::slice::from_ref(&candidate.session.key),
                DetectorId::OverExploring,
                true,
            )
            .unwrap();
            assert_eq!(
                statuses[0].status,
                crate::dto::SessionHygieneStatus::Finding,
                "{agent}"
            );
            store
                .lock()
                .execute(
                    "UPDATE session_evidence SET published_fence = published_fence + 1",
                    [],
                )
                .unwrap();
            assert!(read().is_none(), "{agent}");
        }
    }

    #[test]
    fn opencode_sqlite_observed_reads_reach_source_bound_report() {
        use crate::over_exploring_worker::tests::reduced;
        use antiburn_local::checks::over_exploring::Reason;

        let (store, candidate) = fixture(None, "user");
        let snapshot = store
            .load_smart_check_inputs(
                &candidate.session.key,
                candidate.published_fence,
                candidate.source_generation,
                crate::smart_check_inputs::DetectorInput::OverExploring,
            )
            .unwrap();
        let reads = snapshot
            .content()
            .actions
            .iter()
            .filter(|action| action.kind == "tool_input")
            .filter_map(|action| action.metadata.read_request.as_ref())
            .map(|request| request.reference_id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(reads.len(), 2);

        let input = prepare(
            &candidate,
            snapshot,
            &antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default(),
        )
        .unwrap();
        let publication = publication(&input, reduced(&input, Reason::UnrelatedFiles));
        assert!(
            publication
                .assessment
                .findings
                .iter()
                .all(|finding| publishable_finding(finding, &publication))
        );
        assert!(
            store
                .queue_burn_check_assessment(&input.durable, 1000, 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(&input.durable, 1000, 300, 180)
                .unwrap()
        );
        assert!(
            store
                .complete_burn_check_assessment(
                    &input.durable,
                    &serde_json::to_string(&publication).unwrap(),
                    1000,
                    180,
                )
                .unwrap()
        );

        let evidence: SessionEvidence = serde_json::from_str(
            &store
                .evidence(&candidate.session.key)
                .unwrap()
                .unwrap()
                .evidence_json
                .unwrap(),
        )
        .unwrap();
        let findings = over_exploring_findings_for_session(
            &store.lock(),
            &evidence,
            IgnoredInstructionSessionIdentity {
                environment_key: &candidate.session.key.environment_key,
                agent: &candidate.session.key.agent,
                session_id: &candidate.session.key.session_id,
                incarnation: candidate.incarnation,
                source_generation: candidate.source_generation,
                source_fingerprint: candidate.source_fingerprint.as_deref(),
                published_fence: candidate.published_fence,
            },
        )
        .unwrap()
        .expect("completed source-bound publication reaches report findings");
        assert!(!findings.is_empty());
        for finding in &publication.assessment.findings {
            assert!(
                finding
                    .reads
                    .iter()
                    .all(|read| reads.contains(&read.request_id))
            );
        }
        assert!(findings.iter().all(|finding| {
            let prompt = antiburn_local::remediation::remediation_prompt(finding).unwrap();
            !prompt.as_str().is_empty()
        }));
    }

    #[test]
    fn report_rejects_absence_provider_failure_stale_publication_and_deleted_sessions() {
        let (store, candidate) = fixture(None, "user");
        let snapshot = store
            .load_smart_check_inputs(
                &candidate.session.key,
                candidate.published_fence,
                candidate.source_generation,
                crate::smart_check_inputs::DetectorInput::OverExploring,
            )
            .unwrap();
        let input = prepare(
            &candidate,
            snapshot,
            &antiburn_local::analysis::jev::capabilities::ModelCapabilities::jev_default(),
        )
        .unwrap();
        let evidence: SessionEvidence = serde_json::from_str(
            &store
                .evidence(&candidate.session.key)
                .unwrap()
                .unwrap()
                .evidence_json
                .unwrap(),
        )
        .unwrap();
        let read = || {
            over_exploring_findings_for_session(
                &store.lock(),
                &evidence,
                IgnoredInstructionSessionIdentity {
                    environment_key: &candidate.session.key.environment_key,
                    agent: &candidate.session.key.agent,
                    session_id: &candidate.session.key.session_id,
                    incarnation: candidate.incarnation,
                    source_generation: candidate.source_generation,
                    source_fingerprint: candidate.source_fingerprint.as_deref(),
                    published_fence: candidate.published_fence,
                },
            )
            .unwrap()
        };
        assert!(read().is_none());
        assert!(
            store
                .queue_burn_check_assessment(&input.durable, crate::jev::worker::unix_now(), 180)
                .unwrap()
        );
        assert!(
            store
                .claim_burn_check_assessment(
                    &input.durable,
                    crate::jev::worker::unix_now(),
                    300,
                    180
                )
                .unwrap()
        );
        let incomplete = OverExploringCheck.reduce(&input.plan, &[], false).unwrap();
        let json = serde_json::to_string(&publication(&input, incomplete)).unwrap();
        assert!(
            store
                .complete_burn_check_assessment(
                    &input.durable,
                    &json,
                    crate::jev::worker::unix_now(),
                    180
                )
                .unwrap()
        );
        assert!(read().is_none());
        let mut result = OverExploringCheck.reduce(&input.plan, &[], true).unwrap();
        result.unassessed.clear();
        result.completed_episode_ids = input
            .plan
            .prepared
            .candidates
            .iter()
            .map(|candidate| candidate.episode_id)
            .collect();
        result.clean_episode_ids = result.completed_episode_ids.clone();
        result.completed_work_item_ids = input
            .plan
            .work_items
            .iter()
            .map(|item| item.id.clone())
            .collect();
        let json = serde_json::to_string(&publication(&input, result)).unwrap();
        store
            .lock()
            .execute("UPDATE burn_check_assessment SET result_json = ?1", [&json])
            .unwrap();
        assert_eq!(read(), Some(Vec::new()));
        for update in [
            "status = 'failed', last_error_category = 'provider_unavailable'",
            "status = 'queued'",
            "status = 'running'",
            "status = 'superseded'",
            "evaluator_revision = 'old'",
            "result_revision = 'old'",
            "source_generation = 999",
            "source_fingerprint = 'old'",
            "published_fence = 999",
            "incarnation = 999",
        ] {
            store
                .lock()
                .execute(&format!("UPDATE burn_check_assessment SET {update}"), [])
                .unwrap();
            assert!(read().is_none(), "{update}");
            store.lock().execute("UPDATE burn_check_assessment SET status = 'completed', last_error_category = NULL,
                evaluator_revision = ?1, result_revision = input_revision, source_generation = ?2,
                source_fingerprint = ?3, published_fence = ?4, incarnation = ?5",
                params![CHECK.evaluator_revision(), candidate.source_generation, candidate.source_fingerprint,
                    candidate.published_fence, candidate.incarnation]).unwrap();
        }
        store.delete_session(&candidate.session.key).unwrap();
        assert!(read().is_none());
    }
}

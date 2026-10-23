use antiburn_local::{
    analysis::SessionEvidence,
    checks::NotAssessedReason,
    insights::{
        BadgeStatus, DetectorSelection, ReportCatalogs, SessionBadge, session_badges_with_selection,
    },
};
use serde_json::json;

use super::model::{CheckAssessment, CheckAssessmentSource, EvidenceRecord};

pub(crate) fn derive_badges(evidence: &EvidenceRecord) -> Result<Vec<CheckAssessment>, String> {
    let badges = assess_badges(evidence)?;
    Ok(badges.into_iter().map(badge_assessment).collect())
}

fn assess_badges(evidence: &EvidenceRecord) -> Result<Vec<SessionBadge>, String> {
    if !matches!(evidence.status.as_str(), "ready" | "pending" | "processing") {
        return Ok(Vec::new());
    }
    let Some(data) = &evidence.data else {
        return Ok(Vec::new());
    };

    let session_evidence: SessionEvidence = serde_json::from_value(data.clone())
        .map_err(|error| format!("invalid typed Antiburn session evidence: {error}"))?;
    Ok(session_badges_with_selection(
        &session_evidence,
        &ReportCatalogs::default(),
        &DetectorSelection::all(),
    ))
}

fn badge_assessment(badge: SessionBadge) -> CheckAssessment {
    let (status, result) = match badge.status {
        BadgeStatus::Finding => ("finding", json!({"outcome": "finding"})),
        BadgeStatus::Clean => ("clean", json!({"outcome": "clean"})),
        BadgeStatus::NotAssessed(reason) => (
            "not_assessed",
            json!({
                "outcome": "not_assessed",
                "reason": not_assessed_reason_key(reason),
            }),
        ),
    };
    CheckAssessment {
        source: CheckAssessmentSource::SessionEvidenceBadge,
        check_id: badge.id.detector().key().to_owned(),
        status: status.to_owned(),
        persisted: None,
        result: Some(result),
    }
}

fn not_assessed_reason_key(reason: NotAssessedReason) -> &'static str {
    match reason {
        NotAssessedReason::NoSessionsInWindow => "no_sessions_in_window",
        NotAssessedReason::CapabilityMissing => "capability_missing",
        NotAssessedReason::IncompleteEvidence => "incomplete_evidence",
        NotAssessedReason::EvidenceContractIncomplete => "evidence_contract_incomplete",
        NotAssessedReason::SignalMissing => "signal_missing",
    }
}

#[cfg(test)]
mod tests {
    use antiburn_local::{
        analysis::{
            EvidenceSource, SessionEvidenceAccumulator, SourceCapabilities, SourceKind, TurnFacts,
        },
        insights::{BadgeStatus, ReportCatalogs},
    };
    use serde_json::json;

    use super::{EvidenceRecord, assess_badges, derive_badges};

    fn evidence_json() -> serde_json::Value {
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "synthetic-session".to_owned(),
            kind: SourceKind::File,
            capabilities: SourceCapabilities::claude(),
        })
        .evidence(&TurnFacts::default());
        serde_json::to_value(evidence).unwrap()
    }

    fn evidence(status: &str, data: Option<serde_json::Value>) -> EvidenceRecord {
        EvidenceRecord {
            status: status.to_owned(),
            analyzed_generation: None,
            processed_fingerprint: None,
            parser_revision: None,
            analyzer_revision: None,
            evidence_schema_revision: None,
            published_fence: None,
            data,
            diagnostics: None,
        }
    }

    #[test]
    fn evaluates_all_six_engine_badges_and_preserves_their_states() {
        let mut data = evidence_json();
        data["context"] = json!({
            "state": "complete",
            "value": {
                "maxRequestContextTokens": ReportCatalogs::default().depth_cap_tokens + 1,
                "topDepthExamples": []
            }
        });
        let record = evidence("ready", Some(data));

        let badges = assess_badges(&record).unwrap();
        let ids = badges
            .iter()
            .map(|badge| badge.id.detector().key())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "sessions_over_depth",
                "model_overthinking",
                "overpowered_subagents",
                "old_model_usage",
                "overuse_of_fast_mode",
                "cache_churn",
            ]
        );
        assert_eq!(badges[0].status, BadgeStatus::Finding);
        assert_eq!(
            badges[1].status,
            BadgeStatus::NotAssessed(antiburn_local::checks::NotAssessedReason::SignalMissing)
        );
        assert_eq!(badges[5].status, BadgeStatus::Clean);

        let checks = derive_badges(&record).unwrap();
        assert_eq!(checks.len(), 6);
        assert_eq!(checks[0].check_id, "sessions_over_depth");
        assert_eq!(checks[0].status, "finding");
        assert_eq!(checks[0].result, Some(json!({"outcome": "finding"})));
        assert!(checks[0].persisted.is_none());
    }

    #[test]
    fn partial_coverage_does_not_produce_a_clean_badge() {
        let mut data = evidence_json();
        data["coverage"] = json!({"partial": "malformed_record"});
        let badges = assess_badges(&evidence("ready", Some(data.clone()))).unwrap();
        assert!(
            badges
                .iter()
                .any(|badge| badge.status != BadgeStatus::Clean)
        );
        let checks = derive_badges(&evidence("ready", Some(data))).unwrap();
        assert!(checks.iter().all(|check| check.status != "clean"));
    }

    #[test]
    fn missing_or_unavailable_evidence_does_not_produce_badges() {
        for record in [
            evidence("missing", None),
            evidence("failed", Some(evidence_json())),
            evidence("unsupported", Some(evidence_json())),
        ] {
            assert!(derive_badges(&record).unwrap().is_empty());
        }
    }

    #[test]
    fn malformed_typed_evidence_fails_closed() {
        assert!(derive_badges(&evidence("ready", Some(json!({"unexpected": true})))).is_err());
    }
}

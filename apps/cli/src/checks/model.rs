use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct ChecksResponse {
    pub session_id: String,
    pub identity: SessionIdentity,
    pub freshness_evaluated: bool,
    #[serde(skip)]
    pub evidence: EvidenceRecord,
    pub checks: Vec<CheckAssessment>,
}

#[derive(Debug, Serialize)]
pub struct SessionIdentity {
    pub environment_key: String,
    pub agent: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvidenceRecord {
    pub status: String,
    pub analyzed_generation: Option<i64>,
    pub processed_fingerprint: Option<String>,
    pub parser_revision: Option<i64>,
    pub analyzer_revision: Option<i64>,
    pub evidence_schema_revision: Option<i64>,
    pub published_fence: Option<i64>,
    pub data: Option<Value>,
    pub diagnostics: Option<Value>,
}

impl EvidenceRecord {
    pub fn missing() -> Self {
        Self {
            status: "missing".to_owned(),
            analyzed_generation: None,
            processed_fingerprint: None,
            parser_revision: None,
            analyzer_revision: None,
            evidence_schema_revision: None,
            published_fence: None,
            data: None,
            diagnostics: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckAssessment {
    pub source: CheckAssessmentSource,
    pub check_id: String,
    pub status: String,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub persisted: Option<Box<PersistedAssessmentMetadata>>,
    pub result: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckAssessmentSource {
    BurnCheckAssessment,
    SessionEvidenceBadge,
    PredefinedYaml,
    SessionPriceOutlier,
}

#[derive(Debug, Clone, Serialize)]
pub struct PersistedAssessmentMetadata {
    pub input_revision: Option<String>,
    pub evaluator_revision: Option<String>,
    pub result_revision: Option<String>,
    pub result_matches_input_revision: Option<bool>,
    pub incarnation: i64,
    pub boundary_generation: Option<i64>,
    pub boundary_activity_cursor: Option<String>,
    pub boundary_at_epoch: Option<i64>,
    pub source_generation: Option<i64>,
    pub source_fingerprint: Option<String>,
    pub published_fence: Option<i64>,
    pub request_count: i64,
    pub created_at_epoch: i64,
    pub updated_at_epoch: i64,
    pub last_error_category: Option<String>,
    pub progress: Value,
}

impl CheckAssessment {
    pub fn result_matches_input_revision(
        input_revision: &Option<String>,
        result_revision: &Option<String>,
    ) -> Option<bool> {
        Some(input_revision.as_ref()? == result_revision.as_ref()?)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        CheckAssessment, CheckAssessmentSource, ChecksResponse, EvidenceRecord,
        PersistedAssessmentMetadata, SessionIdentity,
    };

    #[test]
    fn response_json_contains_only_check_output_not_raw_evidence() {
        let response = ChecksResponse {
            session_id: "cli-session".into(),
            identity: SessionIdentity {
                environment_key: "native".into(),
                agent: "pi".into(),
                session_id: "cli-session".into(),
            },
            freshness_evaluated: false,
            evidence: EvidenceRecord {
                status: "ready".into(),
                analyzed_generation: Some(4),
                processed_fingerprint: Some("mtime:size".into()),
                parser_revision: Some(1),
                analyzer_revision: Some(2),
                evidence_schema_revision: Some(3),
                published_fence: Some(7),
                data: Some(json!({"subagents": {"spawnCount": 2}})),
                diagnostics: None,
            },
            checks: vec![CheckAssessment {
                source: CheckAssessmentSource::BurnCheckAssessment,
                check_id: "example".into(),
                status: "completed".into(),
                persisted: Some(Box::new(PersistedAssessmentMetadata {
                    input_revision: Some("input-1".into()),
                    evaluator_revision: Some("evaluator-1".into()),
                    result_revision: Some("input-1".into()),
                    result_matches_input_revision: Some(true),
                    incarnation: 1,
                    boundary_generation: Some(3),
                    boundary_activity_cursor: Some("cursor".into()),
                    boundary_at_epoch: Some(10),
                    source_generation: Some(4),
                    source_fingerprint: Some("mtime:size".into()),
                    published_fence: Some(7),
                    request_count: 1,
                    created_at_epoch: 11,
                    updated_at_epoch: 12,
                    last_error_category: None,
                    progress: json!({"completed": 1}),
                })),
                result: Some(json!({"findings": []})),
            }],
        };
        let value = serde_json::to_value(response).unwrap();

        assert_eq!(value["session_id"], "cli-session");
        assert_eq!(value["freshness_evaluated"], false);
        assert_eq!(value["checks"][0]["source"], "burn_check_assessment");
        assert_eq!(value["checks"][0]["result"]["findings"], json!([]));
        assert_eq!(value["checks"][0]["result_matches_input_revision"], true);
    }
}

#[cfg(test)]
mod missing_evidence_tests {
    use super::EvidenceRecord;

    #[test]
    fn missing_evidence_is_not_a_clean_result() {
        let evidence = EvidenceRecord::missing();
        assert_eq!(evidence.status, "missing");
        assert!(evidence.data.is_none());
    }
}

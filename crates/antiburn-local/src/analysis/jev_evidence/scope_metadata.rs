//! Private, source-normalized question and plan records. These are not approvals.

use crate::analysis::SourceFormat;
use serde::{Deserialize, Serialize};

/// A source adapter supplies this binding after it validates the producer shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevScopeEvidenceSource {
    pub source_format: SourceFormat,
    pub role: JevScopeEvidenceRole,
    pub native_record_id: Option<String>,
    pub call_id: Option<String>,
    pub question_id: Option<String>,
    /// Order within the native record. Turn and branch order come from the content row.
    pub order: u32,
    /// Producer acceptance order, when recorded. This is not transcript row order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_order: Option<u64>,
    pub provenance: JevScopeEvidenceProvenance,
    /// The pinned producer contract used by the adapter, not a tool name.
    pub producer_revision: String,
    pub normalization_revision: u32,
    pub bindings: Vec<super::JevNativeFieldRange>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevScopeEvidenceRole {
    User,
    Assistant,
    Tool,
    System,
    Developer,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevScopeEvidenceProvenance {
    RecordedUser,
    RecognizedQuestionWorkflow,
    RecognizedPlanWorkflow,
    RecordedAssistant,
    SessionLinkedCompanion,
    DelegatedReport,
    Unknown,
}

/// Lifecycle and origin are independent. A submitted answer can have unknown origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevUserAnswerStatus {
    Submitted,
    Cancelled,
    TimedOut,
    Pending,
    Unknown,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevUserAnswerOrigin {
    User,
    Synthetic,
    Automatic,
    UnknownOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevQuestionOption {
    pub id: Option<String>,
    pub value: Option<String>,
    pub label: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevAnswerSelection {
    pub option_id: Option<String>,
    pub option_index: Option<u32>,
    pub value: Option<String>,
    pub label: Option<String>,
    pub custom: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevUserAnswer {
    pub source: JevScopeEvidenceSource,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub options: Vec<JevQuestionOption>,
    pub multi_select: Option<bool>,
    pub selections: Vec<JevAnswerSelection>,
    pub free_text: Option<String>,
    pub status: JevUserAnswerStatus,
    pub origin: JevUserAnswerOrigin,
}

impl JevUserAnswer {
    /// This proves a submitted user response, not the meaning of its approval.
    pub fn is_authoritative_user_response(&self) -> bool {
        !self.source.truncated
            && !self.source.producer_revision.is_empty()
            && self.source.normalization_revision > 0
            && self.status == JevUserAnswerStatus::Submitted
            && self.origin == JevUserAnswerOrigin::User
            && match self.source.provenance {
                JevScopeEvidenceProvenance::RecordedUser => {
                    self.source.role == JevScopeEvidenceRole::User
                }
                JevScopeEvidenceProvenance::RecognizedQuestionWorkflow => {
                    self.source
                        .call_id
                        .as_deref()
                        .is_some_and(|id| !id.is_empty())
                        && matches!(
                            self.source.role,
                            JevScopeEvidenceRole::Tool | JevScopeEvidenceRole::User
                        )
                }
                _ => false,
            }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevPlanStatus {
    Proposed,
    Approved,
    Rejected,
    Feedback,
    Superseded,
    Pending,
    Cancelled,
    TimedOut,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JevPlanContentStatus {
    Recorded,
    VersionMatchedCompanion,
    MutableCompanion,
    Missing,
    Unresolved,
}

/// A reference does not prove that current companion bytes are the approved version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JevPlanReference {
    pub source: JevScopeEvidenceSource,
    pub plan_id: Option<String>,
    pub path: Option<String>,
    pub revision: Option<String>,
    pub content_digest: Option<String>,
    pub approved_revision: Option<String>,
    pub approved_content_digest: Option<String>,
    pub text: Option<String>,
    pub feedback: Option<String>,
    pub status: JevPlanStatus,
    pub origin: JevUserAnswerOrigin,
    pub content_status: JevPlanContentStatus,
}

#[cfg(test)]
pub(crate) fn scope_answer_fixture() -> JevUserAnswer {
    JevUserAnswer {
        source: JevScopeEvidenceSource {
            source_format: SourceFormat::ClaudeJsonl,
            role: JevScopeEvidenceRole::Tool,
            native_record_id: Some("record-1".into()),
            call_id: Some("call-1".into()),
            question_id: Some("question-1".into()),
            order: 2,
            acceptance_order: Some(7),
            provenance: JevScopeEvidenceProvenance::RecognizedQuestionWorkflow,
            producer_revision: "synthetic-contract-v1".into(),
            normalization_revision: 1,
            bindings: Vec::new(),
            truncated: false,
        },
        prompt: "Which scope?".into(),
        header: Some("Scope".into()),
        context: Some("Keep the exclusion.\n  Keep nested conditions.".into()),
        comment: Some("Except billing.\n  Keep retries.".into()),
        options: vec![JevQuestionOption {
            id: Some("small".into()),
            value: Some("small-value".into()),
            label: "Small".into(),
            description: Some(
                "Do not change billing.\n  Keep nested conditions. 实现 API only.".into(),
            ),
        }],
        multi_select: Some(true),
        selections: vec![
            JevAnswerSelection {
                option_id: Some("small".into()),
                option_index: Some(0),
                value: Some("small-value".into()),
                label: Some("Small".into()),
                custom: Some(false),
            },
            JevAnswerSelection {
                option_id: None,
                option_index: None,
                value: Some("Also keep the existing retry limit.\nDo not add retries.".into()),
                label: None,
                custom: Some(true),
            },
        ],
        free_text: Some("Well, no. Actually, keep this:\n```\n  x = 2\n```".into()),
        status: JevUserAnswerStatus::Submitted,
        origin: JevUserAnswerOrigin::User,
    }
}

#[cfg(test)]
pub(crate) fn scope_plan_fixture() -> JevPlanReference {
    let mut source = scope_answer_fixture().source;
    source.provenance = JevScopeEvidenceProvenance::RecognizedPlanWorkflow;
    source.order = 3;
    JevPlanReference {
        source,
        plan_id: Some("plan-1".into()),
        path: Some("plans/task.md".into()),
        revision: Some("v2".into()),
        content_digest: Some("current-digest".into()),
        approved_revision: Some("v1".into()),
        approved_content_digest: Some("approved-digest".into()),
        text: Some("# Scope\n- Keep API\n  - Do not change billing\n".into()),
        feedback: Some("No, keep the earlier exclusion.".into()),
        status: JevPlanStatus::Feedback,
        origin: JevUserAnswerOrigin::User,
        content_status: JevPlanContentStatus::MutableCompanion,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::interface::{ContentKind, ContentPart, MAX_CONTENT_PART_BYTES};

    #[test]
    fn legacy_scope_metadata_defaults_optional_display_order_and_binding_identity() {
        let mut value = serde_json::to_value(scope_answer_fixture()).unwrap();
        for key in ["header", "context", "comment"] {
            value.as_object_mut().unwrap().remove(key);
        }
        value["source"]
            .as_object_mut()
            .unwrap()
            .remove("acceptance_order");
        value["source"]["bindings"] = serde_json::json!([{
            "field":"user_answer", "container":"record", "pointer":"/answer", "start":0, "end":4
        }]);
        let answer: JevUserAnswer = serde_json::from_value(value).unwrap();
        assert_eq!(answer.header, None);
        assert_eq!(answer.context, None);
        assert_eq!(answer.comment, None);
        assert_eq!(answer.source.acceptance_order, None);
        assert_eq!(answer.source.bindings[0].native_record_id, None);
        let encoded = serde_json::to_value(answer).unwrap();
        assert!(encoded.get("header").is_none());
        assert!(encoded.get("context").is_none());
        assert!(encoded.get("comment").is_none());
        assert!(encoded["source"].get("acceptance_order").is_none());
        assert!(
            encoded["source"]["bindings"][0]
                .get("native_record_id")
                .is_none()
        );
    }

    #[test]
    fn scope_answer_authority_requires_recorded_submission_and_known_user_origin() {
        let answer = scope_answer_fixture();
        assert!(answer.is_authoritative_user_response());
        for status in [
            JevUserAnswerStatus::Skipped,
            JevUserAnswerStatus::Cancelled,
            JevUserAnswerStatus::TimedOut,
            JevUserAnswerStatus::Pending,
            JevUserAnswerStatus::Unknown,
        ] {
            let mut changed = answer.clone();
            changed.status = status;
            assert!(!changed.is_authoritative_user_response());
        }
        for origin in [
            JevUserAnswerOrigin::Synthetic,
            JevUserAnswerOrigin::Automatic,
            JevUserAnswerOrigin::UnknownOrigin,
        ] {
            let mut changed = answer.clone();
            changed.origin = origin;
            assert!(!changed.is_authoritative_user_response());
        }
        for provenance in [
            JevScopeEvidenceProvenance::Unknown,
            JevScopeEvidenceProvenance::RecordedAssistant,
            JevScopeEvidenceProvenance::DelegatedReport,
            JevScopeEvidenceProvenance::SessionLinkedCompanion,
            JevScopeEvidenceProvenance::RecognizedPlanWorkflow,
        ] {
            let mut changed = answer.clone();
            changed.source.provenance = provenance;
            assert!(!changed.is_authoritative_user_response());
        }
        let mut changed = answer;
        changed.source.call_id = None;
        assert!(!changed.is_authoritative_user_response());
        changed.source.call_id = Some(String::new());
        assert!(!changed.is_authoritative_user_response());
    }

    #[test]
    fn scope_metadata_rejects_thinking_and_oversize_without_truncating_meaning() {
        assert!(
            ContentPart::new(ContentKind::Thinking, "private")
                .with_scope_evidence(vec![scope_answer_fixture()], Vec::new())
                .is_err()
        );
        let mut answer = scope_answer_fixture();
        answer.free_text = Some("字".repeat(MAX_CONTENT_PART_BYTES));
        assert!(
            ContentPart::new(ContentKind::ToolResult, "result")
                .with_scope_evidence(vec![answer], Vec::new())
                .is_err()
        );
        let legacy: super::super::JevOperationMetadata =
            serde_json::from_str(r#"{"state":"completed","bindings":[]}"#).unwrap();
        assert!(legacy.user_answers.is_empty());
        assert!(legacy.plan_references.is_empty());
        let mut answer = scope_answer_fixture();
        answer.source.truncated = true;
        assert!(!answer.is_authoritative_user_response());
        let part = ContentPart::new(ContentKind::ToolResult, "result")
            .with_scope_evidence(vec![answer], Vec::new())
            .unwrap();
        assert!(!part.truncated);
        assert!(part.metadata.user_answers[0].source.truncated);
    }
}

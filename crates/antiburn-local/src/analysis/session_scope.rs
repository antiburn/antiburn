//! Retained user context for a single source branch at a published boundary.
//! Assistant text supplies context, never user authority. No approval is inferred.
//! Companion bytes need normalized session and version proof. Mutable or missing
//! references remain unresolved. This module has no filesystem resolver.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::jev::capabilities::ModelCapabilities;
use super::jev::{
    JevEvidenceReference, JevEvidenceRole, JevInputField, JevInputSelection, JevPackingResult,
    JevSharedRequestContext, JevWorkItem, pack_work_items_with_shared_context,
};
use super::jev_evidence::{
    ContentAction, ContentEventReference, JevPlanContentStatus, JevScopeEvidenceProvenance,
    JevScopeEvidenceSource, is_recorded_skill_selection, prepare_session_content, source_supported,
};
use super::jev_evidence::{
    JevPlanStatus, JevScopeEvidenceRole, JevUserAnswerOrigin, JevUserAnswerStatus,
};
use super::{
    ContentKind, PublishedContent, PublishedContentBoundary, SelectedContentScope, SourceFormat,
};

pub const SCOPE_SELECTION: JevInputSelection = JevInputSelection::from_fields(&[
    JevInputField::UserMessage,
    JevInputField::AssistantMessage,
    JevInputField::UserAnswer,
    JevInputField::PlanReference,
]);
const MAX_SCOPE_BYTES: usize = 16 * 1024 * 1024;
const MAX_SCOPE_PARTS: usize = 65_536;

/// The caller obtains this boundary from the candidate's published source row.
/// A timestamp is not a branch boundary. An enablement watermark is not scope start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionScopeBoundary {
    pub source_key: String,
    pub thread_id: String,
    pub turn_index: u64,
    pub part_index: u32,
    pub branch: SessionScopeBranch,
}

impl SessionScopeBoundary {
    pub fn content_scope(&self) -> Result<SelectedContentScope, SessionScopeError> {
        let native_record_ids = match &self.branch {
            SessionScopeBranch::ProvenLinear => None,
            SessionScopeBranch::NativeRecords(ids) if !ids.is_empty() => Some(ids.clone()),
            _ => {
                return Err(SessionScopeError::Missing(
                    ScopeMissingReason::BranchUnresolved,
                ));
            }
        };
        Ok(SelectedContentScope {
            source_key: self.source_key.clone(),
            thread_id: self.thread_id.clone(),
            turn_index: self.turn_index,
            part_index: self.part_index,
            native_record_ids,
        })
    }
}

/// Supply proof from the source adapter. A shared thread ID does not prove
/// ancestry. NativeRecords contains the complete ancestor chain through boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionScopeBranch {
    ProvenLinear,
    NativeRecords(BTreeSet<String>),
    Unresolved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScopeMissingReason {
    UnsupportedSource,
    IncompleteSource,
    PublicationChanged,
    IncompletePaging,
    TruncatedEvidence,
    BoundaryMissing,
    NoUserContext,
    InvalidEvidence,
    UnresolvedInfluence,
    MissingCapabilities,
    BranchUnresolved,
    AssemblyLimitReached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionScopeError {
    Missing(ScopeMissingReason),
    ScopeTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeAuthority {
    User,
    SupportingContext,
    NonAuthorizing,
    UnknownInfluence,
}

/// Each occurrence keeps its own reference, including repeated exact text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeOccurrence {
    pub reference: ContentEventReference,
    pub field: JevInputField,
    pub authority: ScopeAuthority,
    pub value_index: usize,
    pub native_source: Option<JevScopeEvidenceSource>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionScopeSnapshot {
    publication_fence: i64,
    source_generation: i64,
    values: Vec<Value>,
    occurrences: Vec<ScopeOccurrence>,
    limitations: Vec<ScopeMissingReason>,
    source_complete: bool,
}

impl SessionScopeSnapshot {
    pub fn publication_fence(&self) -> i64 {
        self.publication_fence
    }
    pub fn source_generation(&self) -> i64 {
        self.source_generation
    }
    pub fn values(&self) -> &[Value] {
        &self.values
    }
    pub fn occurrences(&self) -> &[ScopeOccurrence] {
        &self.occurrences
    }

    pub fn limitations(&self) -> &[ScopeMissingReason] {
        &self.limitations
    }

    /// Partial inputs keep unknown influences visible without treating them as approvals.
    pub fn scope_creep_context(&self) -> Result<JevSharedRequestContext, SessionScopeError> {
        if self.source_complete
            && self
                .occurrences
                .iter()
                .any(|item| item.authority == ScopeAuthority::UnknownInfluence)
        {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::UnresolvedInfluence,
            ));
        }
        Ok(self.user_context())
    }

    pub fn user_context(&self) -> JevSharedRequestContext {
        let occurrences = self
            .occurrences
            .iter()
            .map(|item| {
                json!({
                    "value": item.value_index, "field": item.field, "authority": item.authority,
                    "acceptance_order": item.native_source.as_ref().and_then(|source| source.acceptance_order),
                })
            })
            .collect::<Vec<_>>();
        let evidence = self
            .occurrences
            .iter()
            .enumerate()
            .map(|(index, item)| JevEvidenceReference {
                part_id: format!("shared_context.occurrences[{index}]"),
                source_id: item.reference.id.clone(),
                content_kind: format!("{:?}", item.field),
                role: if item.authority == ScopeAuthority::User {
                    JevEvidenceRole::Instruction
                } else {
                    JevEvidenceRole::SupportingContext
                },
            })
            .collect();
        JevSharedRequestContext {
            fields: json!({"values": self.values, "occurrences": occurrences, "limitations": self.limitations}),
            evidence,
        }
    }

    /// Use the real candidate and questions, including JSON and model overhead.
    /// Never retry a failed fit with reduced scope.
    pub fn pack_scope_creep(
        &self,
        items: &[JevWorkItem],
        capabilities: &ModelCapabilities,
    ) -> Result<JevPackingResult, SessionScopeError> {
        let shared = self.scope_creep_context()?;
        pack_context(items, capabilities, &shared)
    }

    pub fn pack_user_context(
        &self,
        items: &[JevWorkItem],
        capabilities: &ModelCapabilities,
    ) -> Result<JevPackingResult, SessionScopeError> {
        pack_context(items, capabilities, &self.user_context())
    }
}

fn pack_context(
    items: &[JevWorkItem],
    capabilities: &ModelCapabilities,
    shared: &JevSharedRequestContext,
) -> Result<JevPackingResult, SessionScopeError> {
    if capabilities.usable_input_tokens().is_none()
        || capabilities.usable_state_tokens().is_none()
        || capabilities.request_body_bytes.value.is_none()
        || capabilities.questions_per_request.value.is_none()
        || capabilities.criteria_per_question.value.is_none()
        || capabilities.response_body_bytes.value.is_none()
        || capabilities.tokenizer.is_none()
    {
        return Err(SessionScopeError::Missing(
            ScopeMissingReason::MissingCapabilities,
        ));
    }
    if items.is_empty()
        || items
            .iter()
            .any(|item| item.id.is_empty() || item.questions.is_empty())
    {
        return Err(SessionScopeError::Missing(
            ScopeMissingReason::InvalidEvidence,
        ));
    }
    // This fit probe is never dispatched. It tests the smallest valid request
    // envelope, so an oversized activity cannot become a scope-size failure.
    let minimum = JevWorkItem {
        id: "scope_fit".into(),
        window: super::jev::JevInputWindow {
            fields: json!({}),
            evidence: Vec::new(),
        },
        questions: BTreeMap::from([(
            "fit".into(),
            super::jev::JevQuestion::Noul {
                instructions: json!("Scope"),
                criteria: None,
            },
        )]),
    };
    if pack_work_items_with_shared_context(&[minimum], capabilities, shared)
        .batches
        .is_empty()
    {
        return Err(SessionScopeError::ScopeTooLarge);
    }
    Ok(pack_work_items_with_shared_context(
        items,
        capabilities,
        shared,
    ))
}

/// Consume selected pages from the source start. The caller attests source
/// completeness. Partial sources keep loss limitations beside retained context.
pub struct SessionScopeBuilder {
    source_format: SourceFormat,
    boundary: SessionScopeBoundary,
    publication_fence: i64,
    source_generation: i64,
    actions: Vec<ContentAction>,
    bytes: usize,
    ended: bool,
    boundary_seen: bool,
    failure: Option<SessionScopeError>,
    source_complete: bool,
    limitations: Vec<ScopeMissingReason>,
}

impl SessionScopeBuilder {
    pub fn new(
        source_format: SourceFormat,
        boundary: SessionScopeBoundary,
        publication_fence: i64,
        source_generation: i64,
        source_complete: bool,
    ) -> Result<Self, SessionScopeError> {
        if !source_supported(source_format) {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::UnsupportedSource,
            ));
        }
        if boundary.source_key.is_empty() || boundary.thread_id.is_empty() {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::InvalidEvidence,
            ));
        }
        if matches!(&boundary.branch, SessionScopeBranch::Unresolved)
            || matches!(&boundary.branch, SessionScopeBranch::NativeRecords(ids) if ids.is_empty())
        {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::BranchUnresolved,
            ));
        }
        Ok(Self {
            source_format,
            boundary,
            publication_fence,
            source_generation,
            actions: Vec::new(),
            bytes: 0,
            ended: false,
            boundary_seen: false,
            failure: None,
            source_complete,
            limitations: if source_complete {
                Vec::new()
            } else {
                vec![ScopeMissingReason::IncompleteSource]
            },
        })
    }

    pub fn push_page(
        &mut self,
        content: PublishedContent,
        has_next: bool,
    ) -> Result<(), SessionScopeError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let result = self.push_page_inner(content, has_next);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    /// The store query proves this boundary without reading the activity body.
    pub fn bind_boundary(
        &mut self,
        proof: &PublishedContentBoundary,
    ) -> Result<(), SessionScopeError> {
        if proof.publication_fence != self.publication_fence
            || proof.source_generation != self.source_generation
        {
            self.failure = Some(SessionScopeError::Missing(
                ScopeMissingReason::PublicationChanged,
            ));
        } else if proof.scope != self.boundary.content_scope()? {
            self.failure = Some(SessionScopeError::Missing(
                ScopeMissingReason::BoundaryMissing,
            ));
        } else {
            self.boundary_seen = true;
        }
        self.failure.clone().map_or(Ok(()), Err)
    }

    fn push_page_inner(
        &mut self,
        mut content: PublishedContent,
        has_next: bool,
    ) -> Result<(), SessionScopeError> {
        if self.ended {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::IncompletePaging,
            ));
        }
        if content.publication_fence != self.publication_fence
            || content.source_generation != Some(self.source_generation)
        {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::PublicationChanged,
            ));
        }
        if content.coverage.more_parts != has_next {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::IncompletePaging,
            ));
        }
        if !has_next && (content.coverage.parts_capped || content.coverage.bytes_capped) {
            if self.source_complete {
                return Err(SessionScopeError::Missing(
                    ScopeMissingReason::IncompletePaging,
                ));
            }
            if !self
                .limitations
                .contains(&ScopeMissingReason::IncompletePaging)
            {
                self.limitations.push(ScopeMissingReason::IncompletePaging);
            }
        }
        if content.coverage.oversized_parts > 0
            || content.coverage.stored_truncated_parts > 0
            || content.coverage.context_capped
        {
            if self.source_complete {
                return Err(SessionScopeError::Missing(
                    ScopeMissingReason::TruncatedEvidence,
                ));
            }
            if !self
                .limitations
                .contains(&ScopeMissingReason::TruncatedEvidence)
            {
                self.limitations.push(ScopeMissingReason::TruncatedEvidence);
            }
        }
        content.parts.retain(|item| {
            item.source_key == self.boundary.source_key
                && item.thread_id == self.boundary.thread_id
                && item.scope == "main"
                && match &self.boundary.branch {
                    SessionScopeBranch::ProvenLinear => true,
                    SessionScopeBranch::NativeRecords(ids) => item
                        .uuid
                        .as_ref()
                        .or(item.message_id.as_ref())
                        .is_some_and(|id| ids.contains(id)),
                    SessionScopeBranch::Unresolved => false,
                }
                && (item.turn_index, item.part_index)
                    <= (self.boundary.turn_index, self.boundary.part_index)
        });
        content.parts.retain(|item| {
            if !self.source_complete && (item.context_only || item.part.truncated) {
                if !self
                    .limitations
                    .contains(&ScopeMissingReason::TruncatedEvidence)
                {
                    self.limitations.push(ScopeMissingReason::TruncatedEvidence);
                }
                return false;
            }
            true
        });
        for item in &content.parts {
            self.boundary_seen |= (item.turn_index, item.part_index)
                == (self.boundary.turn_index, self.boundary.part_index);
            if item.part.kind == ContentKind::Thinking {
                continue;
            }
            if item.context_only || item.part.truncated {
                return Err(SessionScopeError::Missing(
                    ScopeMissingReason::TruncatedEvidence,
                ));
            }
            let size = item.part.text.len().saturating_add(
                serde_json::to_vec(&item.part.metadata)
                    .map_err(|_| SessionScopeError::Missing(ScopeMissingReason::InvalidEvidence))?
                    .len(),
            );
            self.bytes = self.bytes.saturating_add(size);
            if self.bytes > MAX_SCOPE_BYTES
                || self.actions.len().saturating_add(content.parts.len()) > MAX_SCOPE_PARTS
            {
                return Err(SessionScopeError::Missing(
                    ScopeMissingReason::AssemblyLimitReached,
                ));
            }
        }
        // This conversion preserves user and assistant text and native scope metadata.
        let normalized = prepare_session_content("scope", self.source_format, content, Vec::new());
        for action in &normalized.actions {
            if matches!(action.kind.as_str(), "user" | "user_text")
                && action.authority != "user"
                && !is_recorded_skill_selection(action)
                && !action
                    .metadata
                    .non_authorizing_context
                    .as_ref()
                    .is_some_and(|fact| fact.matches_action(action, self.source_format))
            {
                return Err(SessionScopeError::Missing(
                    ScopeMissingReason::InvalidEvidence,
                ));
            }
        }
        self.actions.extend(normalized.actions);
        self.ended = !has_next;
        Ok(())
    }

    pub fn finish(mut self) -> Result<SessionScopeSnapshot, SessionScopeError> {
        if let Some(error) = self.failure {
            return Err(error);
        }
        if !self.ended {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::IncompletePaging,
            ));
        }
        if !self.boundary_seen {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::BoundaryMissing,
            ));
        }
        self.actions
            .sort_by_key(|item| (item.reference.turn_index, item.reference.part_index));
        let mut snapshot = SessionScopeSnapshot {
            publication_fence: self.publication_fence,
            source_generation: self.source_generation,
            values: Vec::new(),
            occurrences: Vec::new(),
            limitations: self.limitations,
            source_complete: self.source_complete,
        };
        let mut dictionary = BTreeMap::new();
        let mut seen = BTreeSet::new();
        // Retain every public assistant part before the next user response. This
        // avoids guessing what a short response such as "yes" refers to.
        let mut assistants = Vec::new();
        for action in &self.actions {
            if !seen.insert(action.reference.id.clone()) {
                return Err(SessionScopeError::Missing(
                    ScopeMissingReason::InvalidEvidence,
                ));
            }
            if matches!(action.kind.as_str(), "user" | "user_text") && action.authority != "user" {
                continue;
            }
            if action.kind == "assistant" && action.authority == "assistant" {
                assistants.push(action);
            }
            let has_user = action.kind == "user" && action.authority == "user"
                || action
                    .metadata
                    .user_answers
                    .iter()
                    .any(|answer| answer.is_authoritative_user_response());
            if has_user {
                for assistant in assistants.drain(..) {
                    add_occurrence(
                        &mut snapshot,
                        &mut dictionary,
                        assistant,
                        JevInputField::AssistantMessage,
                        ScopeAuthority::SupportingContext,
                        Value::String(assistant.text.clone()),
                        None,
                    )?;
                }
            }
            if action.kind == "user" && action.authority == "user" {
                add_occurrence(
                    &mut snapshot,
                    &mut dictionary,
                    action,
                    JevInputField::UserMessage,
                    ScopeAuthority::User,
                    Value::String(action.text.clone()),
                    None,
                )?;
            }
            for answer in &action.metadata.user_answers {
                let authority = if answer.source.source_format == self.source_format
                    && answer.is_authoritative_user_response()
                {
                    ScopeAuthority::User
                } else if answer.source.source_format == self.source_format
                    && matches!(
                        answer.status,
                        JevUserAnswerStatus::Cancelled | JevUserAnswerStatus::Skipped
                    )
                    && !answer.source.truncated
                    && !answer.source.producer_revision.is_empty()
                    && answer.source.normalization_revision > 0
                    && answer.source.provenance
                        == JevScopeEvidenceProvenance::RecognizedQuestionWorkflow
                    && matches!(
                        answer.source.role,
                        JevScopeEvidenceRole::Tool | JevScopeEvidenceRole::User
                    )
                    && answer
                        .source
                        .call_id
                        .as_ref()
                        .is_some_and(|id| !id.is_empty())
                {
                    ScopeAuthority::NonAuthorizing
                } else {
                    ScopeAuthority::UnknownInfluence
                };
                add_occurrence(
                    &mut snapshot,
                    &mut dictionary,
                    action,
                    JevInputField::UserAnswer,
                    authority,
                    scope_value(answer)?,
                    Some(answer.source.clone()),
                )?;
            }
            for plan in &action.metadata.plan_references {
                let proven = !plan.source.truncated
                    && plan.source.source_format == self.source_format
                    && !plan.source.producer_revision.is_empty()
                    && plan.source.normalization_revision > 0
                    && matches!(
                        plan.source.provenance,
                        JevScopeEvidenceProvenance::RecognizedPlanWorkflow
                            | JevScopeEvidenceProvenance::RecordedUser
                            | JevScopeEvidenceProvenance::RecordedAssistant
                            | JevScopeEvidenceProvenance::SessionLinkedCompanion
                    )
                    && (plan.content_status == JevPlanContentStatus::Recorded
                        || plan.content_status == JevPlanContentStatus::VersionMatchedCompanion
                            && plan.source.provenance
                                == JevScopeEvidenceProvenance::SessionLinkedCompanion
                            && plan
                                .source
                                .native_record_id
                                .as_ref()
                                .is_some_and(|id| !id.is_empty())
                            && version_matches(plan))
                    && plan.text.is_some()
                    && match plan.status {
                        JevPlanStatus::Proposed => true,
                        JevPlanStatus::Approved => {
                            plan.origin == JevUserAnswerOrigin::User && version_matches(plan)
                        }
                        JevPlanStatus::Rejected | JevPlanStatus::Feedback => {
                            plan.origin == JevUserAnswerOrigin::User
                        }
                        _ => false,
                    };
                // The adapter must prove session and version linkage for companion
                // bytes. A current file or a path alone cannot supply past scope.
                let value = scope_value(plan)?;
                add_occurrence(
                    &mut snapshot,
                    &mut dictionary,
                    action,
                    JevInputField::PlanReference,
                    if proven {
                        ScopeAuthority::SupportingContext
                    } else {
                        ScopeAuthority::UnknownInfluence
                    },
                    value,
                    Some(plan.source.clone()),
                )?;
            }
        }
        if snapshot
            .occurrences
            .iter()
            .any(|item| item.authority == ScopeAuthority::UnknownInfluence)
            && !snapshot
                .limitations
                .contains(&ScopeMissingReason::UnresolvedInfluence)
        {
            snapshot
                .limitations
                .push(ScopeMissingReason::UnresolvedInfluence);
        }
        snapshot.occurrences.sort_by_key(|item| {
            (
                item.reference.turn_index,
                item.reference.part_index,
                item.native_source.as_ref().map(|source| source.order),
            )
        });
        if !snapshot
            .occurrences
            .iter()
            .any(|item| item.authority == ScopeAuthority::User)
        {
            if snapshot.source_complete {
                return Err(SessionScopeError::Missing(
                    ScopeMissingReason::NoUserContext,
                ));
            }
            snapshot.limitations.push(ScopeMissingReason::NoUserContext);
        }
        Ok(snapshot)
    }
}

fn version_matches(plan: &super::jev_evidence::JevPlanReference) -> bool {
    (plan
        .revision
        .as_ref()
        .is_some_and(|revision| Some(revision) == plan.approved_revision.as_ref())
        || plan
            .content_digest
            .as_ref()
            .is_some_and(|digest| Some(digest) == plan.approved_content_digest.as_ref()))
        && !matches!((&plan.revision, &plan.approved_revision), (Some(current), Some(approved)) if current != approved)
        && !matches!((&plan.content_digest, &plan.approved_content_digest), (Some(current), Some(approved)) if current != approved)
}

fn scope_value(value: &impl Serialize) -> Result<Value, SessionScopeError> {
    let mut value = serde_json::to_value(value)
        .map_err(|_| SessionScopeError::Missing(ScopeMissingReason::InvalidEvidence))?;
    value
        .as_object_mut()
        .ok_or(SessionScopeError::Missing(
            ScopeMissingReason::InvalidEvidence,
        ))?
        .remove("source");
    Ok(value)
}

fn add_occurrence(
    snapshot: &mut SessionScopeSnapshot,
    dictionary: &mut BTreeMap<String, usize>,
    action: &ContentAction,
    field: JevInputField,
    authority: ScopeAuthority,
    value: Value,
    native_source: Option<JevScopeEvidenceSource>,
) -> Result<(), SessionScopeError> {
    if native_source
        .as_ref()
        .is_some_and(|source| source.truncated)
    {
        if snapshot.source_complete {
            return Err(SessionScopeError::Missing(
                ScopeMissingReason::TruncatedEvidence,
            ));
        }
        if !snapshot
            .limitations
            .contains(&ScopeMissingReason::TruncatedEvidence)
        {
            snapshot
                .limitations
                .push(ScopeMissingReason::TruncatedEvidence);
        }
    }
    let key = serde_json::to_string(&value)
        .map_err(|_| SessionScopeError::Missing(ScopeMissingReason::InvalidEvidence))?;
    let value_index = *dictionary.entry(key).or_insert_with(|| {
        let index = snapshot.values.len();
        snapshot.values.push(value);
        index
    });
    snapshot.occurrences.push(ScopeOccurrence {
        reference: action.reference.clone(),
        field,
        authority,
        value_index,
        native_source,
    });
    Ok(())
}

#[cfg(test)]
mod tests;

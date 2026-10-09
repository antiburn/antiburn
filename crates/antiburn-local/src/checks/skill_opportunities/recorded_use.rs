//! Source-bound skill use from persisted selected content.

use std::collections::{BTreeMap, BTreeSet};

use crate::analysis::SourceFormat;
use crate::analysis::jev::{JevInputField, JevInputSelection};
use crate::analysis::jev_evidence::{ContentAction, ContentEventReference, SessionContentEvidence};
use crate::checks::ignored_instructions::sha256_hex;
use serde::Serialize;
use serde_json::Value;

use super::{
    MAX_SKILL_USE_EVENTS, SkillInputError, SkillScope, SkillUseEvent, SkillUseEvidence,
    SkillUseIdentity, SkillUseLifecycle, SkillUseOrdering, SkillUseStatus,
};

pub const SKILL_USE_SELECTION: JevInputSelection = JevInputSelection::from_fields(&[
    JevInputField::OtherToolInput,
    JevInputField::OtherToolOutput,
    JevInputField::UserMessage,
]);
const MAX_SELECTED_PARTS: usize = 288;
const MAX_SELECTED_BYTES: usize = 1024 * 1024;
const MAX_NATIVE_RECORD_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillUseBoundary {
    pub session_identity: String,
    pub native_session_id: String,
    pub publication_fence: i64,
    pub scope: SkillScope,
}

/// Supplemental records retain boundary and size checks for existing callers.
/// Persisted selected metadata supplies lifecycle evidence.
pub struct ParsedSkillRecord<'a> {
    pub session_identity: &'a str,
    pub publication_fence: i64,
    pub reference: &'a ContentEventReference,
    pub record: &'a Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecordedSkillIdentity {
    Name { name: String },
    InferredName { name: String },
    Document { name: String, path_digest: String },
    Unknown,
}

impl RecordedSkillIdentity {
    fn name(&self) -> Option<&str> {
        match self {
            Self::Name { name } | Self::InferredName { name } | Self::Document { name, .. } => {
                Some(name)
            }
            Self::Unknown => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillUseProducer {
    ClaudeSkillEnvelope,
    OpenCodeSkill77239205,
    CodexSkillDocumentE7637306,
    PiExplicitSkillRequest,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordedSkillUse {
    pub skill: RecordedSkillIdentity,
    pub lifecycle: SkillUseLifecycle,
    pub source_format: SourceFormat,
    pub reference: ContentEventReference,
    pub request_reference: Option<ContentEventReference>,
    pub tool_call_id: Option<String>,
    pub timestamp_ms: Option<i64>,
    pub turn_role: String,
    pub authority: String,
    pub turn_scope: String,
    pub producer: SkillUseProducer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillUseLimit {
    SelectedWindowOnly,
    AggregateCannotProveAbsence,
    NativeMetadataUnavailable,
    AmbiguousCallIdentity,
    UnknownSkillIdentity,
    SnapshotLocalEventIdentity,
    TimestampUnavailable,
    OutOfOrder,
    IncompleteSelectedContent,
    RequiredFieldNotSelected,
    UnsupportedSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillUseCoverage {
    pub status: SkillUseStatus,
    pub ordering: SkillUseOrdering,
    pub limitations: Vec<SkillUseLimit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillUseSnapshot {
    evidence: SkillUseEvidence,
    events: Vec<RecordedSkillUse>,
    coverage: SkillUseCoverage,
    boundary: Option<SkillUseBoundary>,
    revision: String,
    selected_input_revision: Option<String>,
}

impl SkillUseSnapshot {
    pub fn from_aggregates(evidence: SkillUseEvidence) -> Result<Self, SkillInputError> {
        if evidence.events.len() > MAX_SKILL_USE_EVENTS {
            return Err(SkillInputError::LimitExceeded);
        }
        let mut evidence = evidence;
        if evidence.status == SkillUseStatus::Complete {
            evidence.status = SkillUseStatus::Partial;
        }
        for event in &mut evidence.events {
            event.identity_kind = SkillUseIdentity::Inferred;
            event.lifecycle = SkillUseLifecycle::Requested;
            event.timestamp_ms = None;
            event.order = None;
        }
        evidence.ordering = SkillUseOrdering::Unknown;
        Self::finish(
            evidence,
            vec![],
            None,
            vec![SkillUseLimit::AggregateCannotProveAbsence],
            None,
        )
    }

    pub fn from_published_content(
        content: &SessionContentEvidence,
        boundary: &SkillUseBoundary,
    ) -> Result<Self, SkillInputError> {
        Self::from_selected_content(content, boundary, &[])
    }

    pub fn from_selected_content(
        content: &SessionContentEvidence,
        boundary: &SkillUseBoundary,
        native_records: &[ParsedSkillRecord<'_>],
    ) -> Result<Self, SkillInputError> {
        validate_boundary(content, boundary)?;
        let mut limits = BTreeSet::from([SkillUseLimit::SelectedWindowOnly]);
        let supported = source_matches(content.source_format, boundary.scope.agent);
        if !supported {
            limits.insert(SkillUseLimit::UnsupportedSource);
        }
        if !content.complete {
            limits.insert(SkillUseLimit::IncompleteSelectedContent);
        }
        let fields = [
            JevInputField::OtherToolInput,
            JevInputField::OtherToolOutput,
            JevInputField::UserMessage,
        ];
        if fields.iter().any(|field| {
            !content
                .field_availability
                .iter()
                .any(|availability| availability.field == *field && availability.selected)
        }) {
            limits.insert(SkillUseLimit::RequiredFieldNotSelected);
        }
        native_index(content, native_records)?;
        let mut events = Vec::new();
        if supported {
            collect_requests(content, boundary, &mut events, &mut limits);
            collect_results(content, boundary, &mut events, &mut limits);
            collect_documents(content, boundary, &mut events, &mut limits);
        }
        events.sort_by(|a, b| position(&a.reference).cmp(&position(&b.reference)));
        let ordering = ordering(&events, &mut limits);
        let status = if !supported {
            SkillUseStatus::Unsupported
        } else if limits.contains(&SkillUseLimit::RequiredFieldNotSelected) {
            SkillUseStatus::Unknown
        } else if limits
            .iter()
            .any(|limit| *limit != SkillUseLimit::SelectedWindowOnly)
        {
            SkillUseStatus::Partial
        } else {
            SkillUseStatus::Complete
        };
        let evidence = SkillUseEvidence {
            session_identity: content.session_identity_digest.clone(),
            scope: boundary.scope.clone(),
            status,
            ordering,
            // Native names do not establish the identity of a current mutable file.
            events: events
                .iter()
                .map(|event| SkillUseEvent {
                    identity: event.skill.name().unwrap_or("").to_owned(),
                    identity_kind: SkillUseIdentity::Inferred,
                    lifecycle: event.lifecycle,
                    source_identity: content.session_identity_digest.clone(),
                    source_field: event.reference.id.clone(),
                    timestamp_ms: event.timestamp_ms,
                    order: Some(event.reference.turn_index),
                })
                .collect(),
        };
        Self::finish(
            evidence,
            events,
            Some(boundary.clone()),
            limits.into_iter().collect(),
            Some(content.selected_input_digest.clone()),
        )
    }

    fn finish(
        evidence: SkillUseEvidence,
        events: Vec<RecordedSkillUse>,
        boundary: Option<SkillUseBoundary>,
        limitations: Vec<SkillUseLimit>,
        selected_input_revision: Option<String>,
    ) -> Result<Self, SkillInputError> {
        if evidence.session_identity.is_empty()
            || evidence.session_identity.len() > 256
            || evidence.scope.environment_identity.is_empty()
            || evidence.scope.environment_identity.len() > 256
            || evidence
                .scope
                .project_identity
                .as_ref()
                .is_some_and(|identity| identity.len() > 256)
            || evidence.events.iter().any(|event| {
                event.identity.len() > 256
                    || event.source_field.len() > 256
                    || event.source_identity != evidence.session_identity
                    || event.source_field.is_empty()
            })
        {
            return Err(SkillInputError::InvalidUseEvidence);
        }
        let coverage = SkillUseCoverage {
            status: evidence.status,
            ordering: evidence.ordering,
            limitations,
        };
        let revision = hash(
            serde_json::json!((
                &evidence,
                &events,
                &coverage,
                &selected_input_revision,
                boundary
                    .as_ref()
                    .map(|boundary| (boundary.publication_fence, &boundary.native_session_id))
            ))
            .to_string()
            .as_bytes(),
        );
        Ok(Self {
            evidence,
            events,
            coverage,
            boundary,
            revision,
            selected_input_revision,
        })
    }

    pub fn evidence(&self) -> &SkillUseEvidence {
        &self.evidence
    }
    pub fn events(&self) -> &[RecordedSkillUse] {
        &self.events
    }
    pub fn coverage(&self) -> &SkillUseCoverage {
        &self.coverage
    }
    pub fn publication_fence(&self) -> Option<i64> {
        self.boundary
            .as_ref()
            .map(|boundary| boundary.publication_fence)
    }
    pub fn revision(&self) -> &str {
        &self.revision
    }
    pub fn selected_input_revision(&self) -> Option<&str> {
        self.selected_input_revision.as_deref()
    }
    pub fn matches_boundary(&self, boundary: &SkillUseBoundary) -> bool {
        self.boundary.as_ref() == Some(boundary)
    }
    /// Even complete selected records do not establish all equivalent skill use.
    pub fn proves_session_wide_absence(&self) -> bool {
        false
    }
}

fn validate_boundary(
    content: &SessionContentEvidence,
    boundary: &SkillUseBoundary,
) -> Result<(), SkillInputError> {
    if content.session_identity_digest != boundary.session_identity
        || content.publication_fence != boundary.publication_fence
        || boundary.session_identity.is_empty()
        || boundary.native_session_id.is_empty()
        || boundary.native_session_id.len() > 256
    {
        return Err(SkillInputError::WrongSessionOrScope);
    }
    if content.actions.len() > MAX_SELECTED_PARTS
        || content
            .actions
            .iter()
            .try_fold(0usize, |bytes, action| bytes.checked_add(action.text.len()))
            .is_none_or(|bytes| bytes > MAX_SELECTED_BYTES)
    {
        return Err(SkillInputError::LimitExceeded);
    }
    let mut ids = BTreeSet::new();
    if content.actions.iter().any(|action| {
        action.reference.id.is_empty()
            || action.reference.id.len() > 256
            || action.reference.source_key_digest.len() > 256
            || action.reference.thread_digest.len() > 256
            || action
                .reference
                .native_record_id
                .as_ref()
                .is_some_and(|id| id.len() > 256)
            || action
                .tool_call_id
                .as_ref()
                .is_some_and(|id| id.len() > 256)
            || action.turn_scope.len() > 32
            || action.turn_role.len() > 32
            || action.authority.len() > 32
            || !ids.insert(&action.reference.id)
    }) {
        return Err(SkillInputError::InvalidUseEvidence);
    }
    Ok(())
}

fn source_matches(source: SourceFormat, agent: crate::model::AgentKind) -> bool {
    use crate::model::AgentKind;
    matches!(
        (source, agent),
        (SourceFormat::ClaudeJsonl, AgentKind::Claude)
            | (SourceFormat::OpenCodeSqliteV2, AgentKind::OpenCode)
            | (SourceFormat::CodexRolloutJsonl, AgentKind::Codex)
            | (SourceFormat::PiV3Jsonl, AgentKind::Pi)
    )
}

fn native_index<'a>(
    content: &SessionContentEvidence,
    records: &'a [ParsedSkillRecord<'a>],
) -> Result<BTreeMap<String, &'a Value>, SkillInputError> {
    if records.len() > MAX_SELECTED_PARTS {
        return Err(SkillInputError::LimitExceeded);
    }
    let mut native = BTreeMap::new();
    let mut bytes = 0usize;
    for record in records {
        bytes = bytes.saturating_add(native_bytes(record.record)?);
        if bytes > MAX_SELECTED_BYTES {
            return Err(SkillInputError::LimitExceeded);
        }
        if record.session_identity != content.session_identity_digest
            || record.publication_fence != content.publication_fence
        {
            return Err(SkillInputError::WrongSessionOrScope);
        }
        if !content
            .actions
            .iter()
            .any(|action| action.reference == *record.reference)
            || native
                .insert(record.reference.id.clone(), record.record)
                .is_some()
        {
            return Err(SkillInputError::InvalidUseEvidence);
        }
    }
    Ok(native)
}

fn native_bytes(record: &Value) -> Result<usize, SkillInputError> {
    let mut pending = vec![(record, 0)];
    let mut nodes = 0;
    let mut bytes = 0usize;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if depth > 16 || nodes > 4096 {
            return Err(SkillInputError::LimitExceeded);
        }
        match value {
            Value::String(text) => bytes = bytes.saturating_add(text.len()),
            Value::Array(items) if items.len() <= 256 => {
                pending.extend(items.iter().map(|value| (value, depth + 1)))
            }
            Value::Object(fields) if fields.len() <= 256 => {
                bytes = fields
                    .keys()
                    .fold(bytes, |bytes, key| bytes.saturating_add(key.len()));
                pending.extend(fields.values().map(|value| (value, depth + 1)));
            }
            Value::Array(_) | Value::Object(_) => return Err(SkillInputError::LimitExceeded),
            _ => bytes = bytes.saturating_add(8),
        }
        if bytes > MAX_NATIVE_RECORD_BYTES {
            return Err(SkillInputError::LimitExceeded);
        }
    }
    Ok(bytes)
}

fn producer(source: SourceFormat) -> SkillUseProducer {
    match source {
        SourceFormat::ClaudeJsonl => SkillUseProducer::ClaudeSkillEnvelope,
        SourceFormat::OpenCodeSqliteV2 => SkillUseProducer::OpenCodeSkill77239205,
        SourceFormat::CodexRolloutJsonl => SkillUseProducer::CodexSkillDocumentE7637306,
        SourceFormat::PiV3Jsonl => SkillUseProducer::PiExplicitSkillRequest,
        _ => SkillUseProducer::Unknown,
    }
}

fn recorded(
    action: &ContentAction,
    source: SourceFormat,
    skill: RecordedSkillIdentity,
    lifecycle: SkillUseLifecycle,
    request: Option<&ContentAction>,
) -> RecordedSkillUse {
    RecordedSkillUse {
        skill,
        lifecycle,
        source_format: source,
        reference: action.reference.clone(),
        request_reference: request.map(|request| request.reference.clone()),
        tool_call_id: action.tool_call_id.clone(),
        timestamp_ms: action.timestamp_ms,
        turn_role: action.turn_role.clone(),
        authority: action.authority.clone(),
        turn_scope: action.turn_scope.clone(),
        producer: producer(source),
    }
}

fn collect_requests(
    content: &SessionContentEvidence,
    boundary: &SkillUseBoundary,
    events: &mut Vec<RecordedSkillUse>,
    limits: &mut BTreeSet<SkillUseLimit>,
) {
    for action in &content.actions {
        let Some(fact) = action.metadata.recorded_skill_result.as_ref() else {
            continue;
        };
        if action.kind != "tool_input" || fact.field != JevInputField::OtherToolInput {
            continue;
        }
        if action.authority != "assistant" || action.turn_role != "assistant" || action.truncated {
            limits.insert(SkillUseLimit::IncompleteSelectedContent);
            continue;
        }
        if !fact.matches_action(action, content.source_format)
            || fact
                .session_id
                .as_deref()
                .is_some_and(|id| id != boundary.native_session_id)
            || !fact.complete
            || fact.truncated
        {
            limits.insert(SkillUseLimit::IncompleteSelectedContent);
            continue;
        }
        let skill = super::native_skill_results::identity(fact);
        if matches!(
            skill,
            RecordedSkillIdentity::Unknown | RecordedSkillIdentity::InferredName { .. }
        ) {
            limits.insert(SkillUseLimit::UnknownSkillIdentity);
        }
        if action.tool_call_id.as_deref().is_none_or(str::is_empty) {
            limits.insert(SkillUseLimit::AmbiguousCallIdentity);
        }
        events.push(recorded(
            action,
            content.source_format,
            skill,
            SkillUseLifecycle::Requested,
            None,
        ));
    }
}

fn position(reference: &ContentEventReference) -> (&str, &str, u64, u32) {
    (
        &reference.source_key_digest,
        &reference.thread_digest,
        reference.turn_index,
        reference.part_index,
    )
}

fn same_call(request: &ContentAction, result: &ContentAction) -> bool {
    request
        .tool_call_id
        .as_deref()
        .is_some_and(|id| !id.is_empty())
        && request.tool_call_id == result.tool_call_id
        && request.tool_name == result.tool_name
        && request.reference.source_key_digest == result.reference.source_key_digest
        && request.reference.thread_digest == result.reference.thread_digest
        && request.turn_scope == result.turn_scope
}

fn collect_results(
    content: &SessionContentEvidence,
    boundary: &SkillUseBoundary,
    events: &mut Vec<RecordedSkillUse>,
    limits: &mut BTreeSet<SkillUseLimit>,
) {
    for result in &content.actions {
        let Some(fact) = result.metadata.recorded_skill_result.as_ref() else {
            continue;
        };
        if result.kind != "tool_result" || fact.field != JevInputField::OtherToolOutput {
            continue;
        }
        let requests: Vec<_> = content
            .actions
            .iter()
            .filter(|request| {
                request.kind == "tool_input"
                    && same_call(request, result)
                    && request
                        .metadata
                        .recorded_skill_result
                        .as_ref()
                        .is_some_and(|fact| fact.field == JevInputField::OtherToolInput)
            })
            .collect();
        let result_count = content
            .actions
            .iter()
            .filter(|other| other.kind == "tool_result" && same_call(result, other))
            .count();
        if requests.len() != 1 || result_count != 1 {
            limits.insert(SkillUseLimit::AmbiguousCallIdentity);
            continue;
        }
        let request = requests[0];
        if position(&request.reference) >= position(&result.reference)
            || request.authority != "assistant"
            || request.turn_role != "assistant"
            || result.authority != "tool"
            || !matches!(result.turn_role.as_str(), "tool" | "assistant")
            || request.truncated
            || result.truncated
        {
            limits.insert(SkillUseLimit::IncompleteSelectedContent);
            continue;
        }
        let request_fact = request
            .metadata
            .recorded_skill_result
            .as_ref()
            .expect("selected skill request");
        let requested = super::native_skill_results::identity(request_fact);
        let identity = super::native_skill_results::identity(fact);
        let bound = fact.matches_action(result, content.source_format)
            && request_fact.matches_action(request, content.source_format)
            && request_fact
                .session_id
                .as_deref()
                .is_none_or(|id| id == boundary.native_session_id)
            && fact
                .session_id
                .as_deref()
                .is_none_or(|id| id == boundary.native_session_id)
            && fact
                .request_message_id
                .as_ref()
                .is_none_or(|id| Some(id) == request.reference.native_record_id.as_ref())
            && fact
                .request_part_index
                .is_none_or(|index| index == request.reference.part_index)
            && request_fact.complete
            && !request_fact.truncated
            && fact.complete
            && !fact.truncated;
        let skill = if identity == RecordedSkillIdentity::Unknown {
            requested.clone()
        } else {
            identity
        };
        let lifecycle =
            if !bound || !content.complete || !request.reference.stable || !result.reference.stable
            {
                limits.insert(SkillUseLimit::IncompleteSelectedContent);
                SkillUseLifecycle::Unknown
            } else if skill.name() != requested.name() {
                limits.insert(SkillUseLimit::UnknownSkillIdentity);
                SkillUseLifecycle::Unknown
            } else {
                super::native_skill_results::lifecycle(fact)
            };
        if lifecycle == SkillUseLifecycle::Unknown {
            limits.insert(SkillUseLimit::NativeMetadataUnavailable);
        }
        events.push(recorded(
            result,
            content.source_format,
            skill,
            lifecycle,
            Some(request),
        ));
    }
}

fn collect_documents(
    content: &SessionContentEvidence,
    boundary: &SkillUseBoundary,
    events: &mut Vec<RecordedSkillUse>,
    limits: &mut BTreeSet<SkillUseLimit>,
) {
    for action in &content.actions {
        let Some(fact) = action.metadata.recorded_skill_result.as_ref() else {
            continue;
        };
        if fact.field != JevInputField::UserMessage {
            continue;
        }
        if action.truncated
            || !content.complete
            || !action.reference.stable
            || !fact.complete
            || fact.truncated
        {
            limits.insert(SkillUseLimit::IncompleteSelectedContent);
            continue;
        }
        if fact.matches_action(action, content.source_format)
            && fact
                .session_id
                .as_deref()
                .is_none_or(|id| id == boundary.native_session_id)
            && matches!(action.kind.as_str(), "user" | "user_text")
            && action.turn_role == "user"
            && action.turn_scope == "main"
        {
            let skill = super::native_skill_results::identity(fact);
            let lifecycle = super::native_skill_results::lifecycle(fact);
            events.push(recorded(
                action,
                content.source_format,
                skill,
                lifecycle,
                None,
            ));
        } else {
            limits.insert(SkillUseLimit::UnknownSkillIdentity);
        }
    }
}

fn ordering(events: &[RecordedSkillUse], limits: &mut BTreeSet<SkillUseLimit>) -> SkillUseOrdering {
    let mut last = BTreeMap::new();
    let mut ordering = SkillUseOrdering::Monotonic;
    for event in events {
        if !event.reference.stable {
            limits.insert(SkillUseLimit::SnapshotLocalEventIdentity);
        }
        let Some(timestamp) = event.timestamp_ms else {
            limits.insert(SkillUseLimit::TimestampUnavailable);
            if ordering != SkillUseOrdering::OutOfOrder {
                ordering = SkillUseOrdering::Unknown;
            }
            continue;
        };
        if last
            .insert(
                (
                    &event.reference.source_key_digest,
                    &event.reference.thread_digest,
                ),
                timestamp,
            )
            .is_some_and(|previous| previous > timestamp)
        {
            limits.insert(SkillUseLimit::OutOfOrder);
            ordering = SkillUseOrdering::OutOfOrder;
        }
    }
    if events.is_empty() {
        SkillUseOrdering::Unknown
    } else {
        ordering
    }
}

pub(super) fn hash(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

//! Source-native public evidence uses explicit ordered deltas.
//!
//! Persist these DTOs as typed JSON, not directly in postcard cursor state.
//! Keep private reasoning and unfinished streaming text out of public messages.

use serde::{Deserialize, Serialize};

use super::{
    NativeRequestAttribution, NativeToolFacts, SourceFormat, ToolResultChange,
    ToolResultCompleteness, ToolResultObservation, ToolResultPosition, UserMessage,
    UserMessagePart,
};

pub type NativeEvidencePosition = ToolResultPosition;
pub type PublicTextPart = UserMessagePart;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeEventContext {
    /// Stable first public boundary, separate from the current mutation position.
    pub first_eligible_position: NativeEvidencePosition,
    pub main_session: bool,
    pub complete_through_event: bool,
    pub user_messages: Vec<UserMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessageObservation {
    pub message: UserMessage,
    /// Producer mutation revision, not the parser's `UserMessage.proof_revision`.
    pub revision: u64,
    pub context: NativeEventContext,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeToolInput {
    Json(serde_json::Value),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicMessagePayload {
    AssistantText {
        parts: Vec<PublicTextPart>,
        generator: NativeRequestAttribution,
    },
    ToolCall {
        tool_name: String,
        input: Option<NativeToolInput>,
        facts: Option<NativeToolFacts>,
        provider_executed: Option<bool>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicMessageObservation {
    pub source_format: SourceFormat,
    pub session_id: String,
    pub record_id: String,
    pub part_id: Option<String>,
    pub call_id: Option<String>,
    pub position: NativeEvidencePosition,
    pub revision: u64,
    pub change: ToolResultChange,
    /// The source proves this public boundary. A correction can withdraw it.
    pub eligible: bool,
    pub completeness: ToolResultCompleteness,
    pub context: Option<NativeEventContext>,
    pub payload: PublicMessagePayload,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeMessageObservation {
    User(UserMessageObservation),
    Public(PublicMessageObservation),
    ToolResult(ToolResultObservation),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeCycleKey {
    CodexTurn {
        turn_id: String,
    },
    OpenCodeV2Execution {
        start_event_id: String,
        start_sequence: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseCycleStatus {
    Completed,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResponseCycleObservation {
    pub source_format: SourceFormat,
    pub session_id: String,
    pub key: NativeCycleKey,
    pub start_position: NativeEvidencePosition,
    pub position: NativeEvidencePosition,
    pub revision: u64,
    pub change: ToolResultChange,
    pub status: ResponseCycleStatus,
    pub native_reason: Option<String>,
    pub native_error_code: Option<String>,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
    pub requester: NativeRequestAttribution,
    pub context: NativeEventContext,
    /// Complete admitted public message envelopes in source order.
    pub messages: Vec<NativeMessageObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeActivityObservation {
    pub source_format: SourceFormat,
    pub session_id: String,
    pub position: NativeEvidencePosition,
    pub token: String,
    pub native_at_ms: Option<i64>,
}

/// An explicit recorded selection update does not reset idle activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSessionSelectionObservation {
    pub source_format: SourceFormat,
    pub session_id: String,
    pub position: ToolResultPosition,
    /// Current selection state, not attribution for an earlier request.
    pub selection: NativeRequestAttribution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeMessageRemoval {
    pub source_format: SourceFormat,
    pub session_id: String,
    pub message_id: String,
    pub position: NativeEvidencePosition,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeAdmissionFailure {
    pub source_format: SourceFormat,
    pub session_id: String,
    pub position: NativeEvidencePosition,
    #[serde(deserialize_with = "deserialize_unsafe_reason")]
    pub reason: ToolResultCompleteness,
}

fn deserialize_unsafe_reason<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<ToolResultCompleteness, D::Error> {
    let reason = ToolResultCompleteness::deserialize(deserializer)?;
    if unsafe_reason(reason) {
        Ok(reason)
    } else {
        Err(serde::de::Error::custom(
            "admission failure needs an unsafe reason",
        ))
    }
}

pub(crate) fn unsafe_reason(reason: ToolResultCompleteness) -> bool {
    matches!(
        reason,
        ToolResultCompleteness::UnsafeIdentity
            | ToolResultCompleteness::UnsafeOrder
            | ToolResultCompleteness::UnsafeOwnership
    )
}

pub(crate) fn admission_reason(
    admission: super::ToolResultAdmission,
) -> Option<ToolResultCompleteness> {
    use super::ToolResultAdmission;
    match admission {
        ToolResultAdmission::UnsafeIdentity => Some(ToolResultCompleteness::UnsafeIdentity),
        ToolResultAdmission::UnsafeOrder => Some(ToolResultCompleteness::UnsafeOrder),
        ToolResultAdmission::UnsafeOwnership => Some(ToolResultCompleteness::UnsafeOwnership),
        ToolResultAdmission::Admitted | ToolResultAdmission::Unavailable => None,
    }
}

impl NativeAdmissionFailure {
    pub(crate) fn valid(&self) -> bool {
        unsafe_reason(self.reason) && !self.session_id.is_empty() && self.session_id.len() <= 4096
    }

    pub(crate) fn admission(&self) -> super::ToolResultAdmission {
        use super::ToolResultAdmission;
        match self.reason {
            ToolResultCompleteness::UnsafeIdentity => ToolResultAdmission::UnsafeIdentity,
            ToolResultCompleteness::UnsafeOrder => ToolResultAdmission::UnsafeOrder,
            ToolResultCompleteness::UnsafeOwnership => ToolResultAdmission::UnsafeOwnership,
            ToolResultCompleteness::Complete | ToolResultCompleteness::Incomplete => {
                ToolResultAdmission::Unavailable
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NativeEvidenceFact {
    Message(NativeMessageObservation),
    ResponseCycle(ResponseCycleObservation),
    Activity(NativeActivityObservation),
    SessionSelection(NativeSessionSelectionObservation),
    MessageRemoval(NativeMessageRemoval),
    AdmissionFailure(NativeAdmissionFailure),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeEvidenceEvent {
    pub fact: NativeEvidenceFact,
    /// The adapter asserts live eligibility; the host must verify the claim and baseline.
    pub live_eligible: bool,
}

/// An ordered delta. Absence never means removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeEvidenceBatch {
    pub source_format: SourceFormat,
    pub events: Vec<NativeEvidenceEvent>,
    /// Retained history coverage, not complete current membership.
    pub history_complete: bool,
}

impl NativeEventContext {
    pub(crate) fn pending(position: NativeEvidencePosition, main_session: bool) -> Self {
        Self {
            first_eligible_position: position,
            main_session,
            complete_through_event: false,
            user_messages: Vec::new(),
        }
    }
}

impl NativeMessageObservation {
    pub(crate) fn position(&self) -> NativeEvidencePosition {
        match self {
            Self::User(value) => match value.message.position {
                super::UserMessagePosition::Jsonl { record_index } => ToolResultPosition::Jsonl {
                    record_index,
                    item_index: 0,
                },
                super::UserMessagePosition::FileBytes {
                    record_start,
                    record_end,
                } => ToolResultPosition::FileBytes {
                    record_start,
                    record_end,
                    item_index: 0,
                },
                super::UserMessagePosition::OpenCodeEvent { sequence } => {
                    ToolResultPosition::OpenCodeEvent {
                        sequence,
                        item_index: 0,
                    }
                }
                super::UserMessagePosition::OpenCodeV2Event { sequence } => {
                    ToolResultPosition::OpenCodeV2Event {
                        sequence,
                        item_index: 0,
                    }
                }
                super::UserMessagePosition::OpenCodeSqlite { .. } => {
                    value.context.first_eligible_position
                }
            },
            Self::Public(value) => value.position,
            Self::ToolResult(value) => value.position,
        }
    }
}

impl NativeEvidenceFact {
    pub fn position(&self) -> NativeEvidencePosition {
        match self {
            Self::Message(value) => value.position(),
            Self::ResponseCycle(value) => value.position,
            Self::Activity(value) => value.position,
            Self::SessionSelection(value) => value.position,
            Self::MessageRemoval(value) => value.position,
            Self::AdmissionFailure(value) => value.position,
        }
    }

    pub(crate) fn binding(&self) -> (SourceFormat, &str) {
        match self {
            Self::Message(NativeMessageObservation::User(value)) => {
                (value.message.source_format, &value.message.session_id)
            }
            Self::Message(NativeMessageObservation::Public(value)) => {
                (value.source_format, &value.session_id)
            }
            Self::Message(NativeMessageObservation::ToolResult(value)) => {
                (value.source_format, &value.session_id)
            }
            Self::ResponseCycle(value) => (value.source_format, &value.session_id),
            Self::Activity(value) => (value.source_format, &value.session_id),
            Self::SessionSelection(value) => (value.source_format, &value.session_id),
            Self::MessageRemoval(value) => (value.source_format, &value.session_id),
            Self::AdmissionFailure(value) => (value.source_format, &value.session_id),
        }
    }

    pub(crate) fn invalidate_admission(&mut self, reason: ToolResultCompleteness) {
        match self {
            Self::Message(message) => message.invalidate_admission(reason),
            Self::ResponseCycle(cycle) => {
                cycle.context.main_session = false;
                cycle.context.complete_through_event = false;
                for message in &mut cycle.messages {
                    message.invalidate_admission(reason);
                }
            }
            _ => {}
        }
    }
}

impl NativeMessageObservation {
    pub(crate) fn invalidate_admission(&mut self, reason: ToolResultCompleteness) {
        match self {
            Self::User(value) => {
                value.context.main_session = false;
                value.context.complete_through_event = false;
            }
            Self::Public(value) => {
                value.eligible = false;
                value.completeness = reason;
                if let Some(context) = &mut value.context {
                    context.main_session = false;
                    context.complete_through_event = false;
                }
            }
            Self::ToolResult(value) => {
                value.completeness = reason;
                if let Some(context) = &mut value.context {
                    context.main_session = false;
                    context.complete_through_event = false;
                }
            }
        }
    }
}

pub(crate) fn compare_positions(
    left: NativeEvidencePosition,
    right: NativeEvidencePosition,
) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (
            ToolResultPosition::Jsonl {
                record_index: left,
                item_index: left_item,
            },
            ToolResultPosition::Jsonl {
                record_index: right,
                item_index: right_item,
            },
        ) => Some((left, left_item).cmp(&(right, right_item))),
        (
            ToolResultPosition::FileBytes {
                record_start: left,
                record_end: left_end,
                item_index: left_item,
            },
            ToolResultPosition::FileBytes {
                record_start: right,
                record_end: right_end,
                item_index: right_item,
            },
        ) => Some((left, left_end, left_item).cmp(&(right, right_end, right_item))),
        (
            ToolResultPosition::OpenCodeEvent {
                sequence: left,
                item_index: left_item,
            },
            ToolResultPosition::OpenCodeEvent {
                sequence: right,
                item_index: right_item,
            },
        )
        | (
            ToolResultPosition::OpenCodeV2Event {
                sequence: left,
                item_index: left_item,
            },
            ToolResultPosition::OpenCodeV2Event {
                sequence: right,
                item_index: right_item,
            },
        ) => Some((left, left_item).cmp(&(right, right_item))),
        _ => None,
    }
}

pub(crate) fn user_observation(message: UserMessage) -> Option<UserMessageObservation> {
    let position = match message.position {
        super::UserMessagePosition::Jsonl { record_index } => ToolResultPosition::Jsonl {
            record_index,
            item_index: 0,
        },
        super::UserMessagePosition::FileBytes {
            record_start,
            record_end,
        } => ToolResultPosition::FileBytes {
            record_start,
            record_end,
            item_index: 0,
        },
        super::UserMessagePosition::OpenCodeEvent { sequence } => {
            ToolResultPosition::OpenCodeEvent {
                sequence,
                item_index: 0,
            }
        }
        super::UserMessagePosition::OpenCodeV2Event { sequence } => {
            ToolResultPosition::OpenCodeV2Event {
                sequence,
                item_index: 0,
            }
        }
        super::UserMessagePosition::OpenCodeSqlite { .. } => return None,
    };
    let revision = match position {
        ToolResultPosition::Jsonl { record_index, .. } => record_index.saturating_add(1),
        ToolResultPosition::FileBytes { record_end, .. } => record_end,
        ToolResultPosition::OpenCodeEvent { sequence, .. }
        | ToolResultPosition::OpenCodeV2Event { sequence, .. } => sequence,
    };
    Some(UserMessageObservation {
        context: NativeEventContext::pending(
            position,
            message.scope == super::UserMessageScope::MainSession,
        ),
        message,
        revision,
    })
}

pub(crate) fn freeze_message(
    mut message: NativeMessageObservation,
    position: NativeEvidencePosition,
    prefix: Vec<UserMessage>,
    history_complete: bool,
    main_session: bool,
) -> NativeMessageObservation {
    let original_position = message.position();
    let context = NativeEventContext {
        first_eligible_position: position,
        main_session,
        complete_through_event: history_complete && main_session,
        user_messages: prefix,
    };
    match &mut message {
        NativeMessageObservation::User(value) => {
            let frozen = (value.context.first_eligible_position != original_position)
                .then(|| value.context.clone());
            if let ToolResultPosition::FileBytes {
                record_start,
                record_end,
                ..
            } = position
            {
                value.message.position = super::UserMessagePosition::FileBytes {
                    record_start,
                    record_end,
                };
                value.revision = record_end;
            }
            value.context = frozen.unwrap_or(context);
        }
        NativeMessageObservation::Public(value) => {
            let frozen = value
                .context
                .as_ref()
                .filter(|context| context.first_eligible_position != value.position)
                .cloned();
            value.position = position;
            if let ToolResultPosition::FileBytes { record_end, .. } = position {
                value.revision = record_end;
            }
            value.context = Some(frozen.unwrap_or(context));
            if !main_session {
                value.eligible = false;
                value.completeness = ToolResultCompleteness::UnsafeOwnership;
            }
        }
        NativeMessageObservation::ToolResult(value) => {
            let frozen = value
                .context
                .as_ref()
                .filter(|context| context.result_position != value.position)
                .cloned();
            value.position = position;
            if let ToolResultPosition::FileBytes { record_end, .. } = position {
                value.revision = record_end;
            }
            value.context = Some(frozen.unwrap_or(super::ToolResultContext {
                result_position: position,
                main_session,
                complete_through_event: context.complete_through_event,
                user_messages: context.user_messages,
            }));
        }
    }
    message
}

pub(crate) fn emit_activity(
    sink: &mut dyn super::RecordSink,
    source_format: SourceFormat,
    session_id: &str,
    record_index: u64,
    token: &str,
    native_at_ms: Option<i64>,
) {
    sink.native_activity(NativeActivityObservation {
        source_format,
        session_id: session_id.to_owned(),
        position: ToolResultPosition::Jsonl {
            record_index,
            item_index: u32::MAX - 1,
        },
        token: format!("{record_index}:{token}"),
        native_at_ms,
    });
}

/// Encode typed state as JSON text inside postcard-backed state.
#[derive(Debug, Clone, Default)]
pub(crate) struct JsonState<T>(pub T);

impl<T: Serialize> Serialize for JsonState<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde_json::to_string(&self.0)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for JsonState<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        serde_json::from_str(&text)
            .map(Self)
            .map_err(serde::de::Error::custom)
    }
}

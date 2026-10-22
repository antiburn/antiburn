//! Bounded, resumable adaptation of committed source publications.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::domain::{
    BranchId, EvidenceDependency, EvidenceVersion, FrozenContext, Message, NativeOrderKey,
    Observation, ObservationDraft, ObservationKind,
};

/// Durable outbox sequence used to deliver one committed source publication.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PublicationCursor(pub u64);

/// Request for one bounded publication page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicationRead {
    /// Return events strictly after this sequence.
    pub after: PublicationCursor,
    /// Maximum number of events to return.
    pub limit: u16,
}

/// Source-native result facts copied from one immutable publication.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PublishedToolResult {
    /// Stable logical result identity. Corrections reuse this identity.
    pub identity: String,
    /// Native source position, encoded by the publisher without timestamp sorting.
    pub native_order: NativeOrderKey,
    /// First published terminal update for this identity. Corrections retain this cursor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_terminal_publication: Option<PublicationCursor>,
    /// Producer revision for this logical result.
    pub revision: u64,
    /// Tool name when the source proves it.
    pub tool_name: Option<String>,
    /// Normalized category name when the source proves it.
    pub category: Option<String>,
    /// Linked arguments. Missing data stays missing.
    pub arguments: Option<Value>,
    /// Captured result content. Missing data stays missing.
    pub content: Option<String>,
    /// Source-reported program outcome, separate from tool execution status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Value>,
    /// Whether the provider, rather than the local host, executed the tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_executed: Option<bool>,
    /// Producer-reported terminal status.
    pub status: PublishedResultStatus,
    /// Whether the source marks this logical result terminal.
    pub finality: bool,
    /// Whether required result payload fields are complete.
    #[serde(rename = "complete")]
    pub payload_complete: bool,
    /// Source identity, order, and ownership admission for this result.
    #[serde(
        rename = "selection_safe",
        serialize_with = "serialize_evidence_validity"
    )]
    pub validity: EvidenceValidity,
}

/// Whether source identity, order, and ownership support selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvidenceValidity {
    /// The source proves a stable identity, usable order, and admitted ownership.
    Valid,
    /// The source cannot prove whether the result is safe to select.
    Unknown,
    /// The source reports a conflicting result identity.
    UnsafeIdentity,
    /// The source does not prove result order or branch membership.
    UnsafeOrder,
    /// The source does not prove ownership by the admitted session.
    UnsafeOwnership,
}

fn serialize_evidence_validity<S>(
    validity: &EvidenceValidity,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_bool(*validity == EvidenceValidity::Valid)
}

/// Producer-reported result status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum PublishedResultStatus {
    Completed,
    Error,
    Interrupted,
    Unknown,
}

/// Proven context admitted with the publication. Unknown scope is never promoted.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PublishedContext {
    /// Stable identity and value for each admitted user-slot message.
    pub requests: Vec<(String, Value)>,
    /// Native session identity for compiler-supported session context.
    pub session_id: String,
    /// Agent name reported by the source index.
    pub agent_name: String,
    /// Known repository root, when the live project binding supplies one.
    pub project_root: Option<String>,
    /// Version of the immutable context snapshot.
    pub version: String,
    /// The publisher proved that the branch is the admitted main session.
    pub main_session: bool,
    /// The source proved complete request history through the result position.
    pub complete_through_event: bool,
}

/// One committed publication event.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedEvidenceEvent {
    /// Database-wide committed publication sequence.
    pub cursor: PublicationCursor,
    /// Stable branch identity provided by publication admission.
    pub branch: String,
    /// Immutable request context admitted for the event's source position.
    pub context: Option<PublishedContext>,
    /// Event payload and correction/removal/gap marker.
    pub change: PublishedEvidenceChange,
}

/// Mutation kind for one published result stream.
#[derive(Clone, Debug, PartialEq)]
pub enum PublishedEvidenceChange {
    /// A live source baseline or inactive-session event advances delivery only.
    Ignored,
    /// A finalized result or correction.
    Upsert(PublishedToolResult),
    /// A logical result was removed.
    Removal { identity: String, revision: u64 },
    /// A complete snapshot restored membership after a previously reported gap.
    CoverageRestored,
    /// The publisher cannot prove complete membership or ordering.
    Gap { reason: String },
    /// Continuity loss starts a new selection window while history stays partial.
    SegmentReset { reason: String },
}

/// A page returned by the desktop publication reader.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedEvidenceBatch {
    /// Events in strictly increasing durable publication order.
    pub events: Vec<PublishedEvidenceEvent>,
    /// Cursor after the last returned event, or the request cursor when empty.
    pub next: PublicationCursor,
    /// Whether more committed events remain after this page.
    pub has_more: bool,
}

/// Desktop-owned access to committed, immutable source publications.
pub trait PublishedEvidenceReader: Send + Sync {
    /// Reads events strictly after the requested cursor.
    fn read_batch(&self, request: PublicationRead) -> Result<PublishedEvidenceBatch, SourceError>;
}

/// One normalized change returned by [`PublishedEvidenceSource`].
#[derive(Clone, Debug, PartialEq)]
pub enum SourceEvent {
    /// A source event advances delivery without entering selection.
    Ignored { cursor: PublicationCursor },
    /// A final observation or correction. The logical identity is stable.
    Observation {
        cursor: PublicationCursor,
        identity: String,
        revision: u64,
        observation: Box<ObservationDraft>,
    },
    /// A prior logical result no longer exists.
    Removal {
        cursor: PublicationCursor,
        identity: String,
        revision: u64,
        branch: BranchId,
    },
    /// Source coverage is incomplete and later candidates must retain that limit.
    Gap {
        cursor: PublicationCursor,
        branch: BranchId,
        reason: String,
    },
    /// An update cannot prove safe identity, scope, or order for this branch.
    UnsafeUpdate {
        cursor: PublicationCursor,
        branch: BranchId,
        reason: String,
    },
    /// Continuity loss retires this branch's window and keeps historical coverage partial.
    SegmentReset {
        cursor: PublicationCursor,
        branch: BranchId,
        reason: String,
    },
    /// A complete snapshot restored coverage after a gap.
    CoverageRestored {
        cursor: PublicationCursor,
        branch: BranchId,
    },
}

/// One source observation assigned an engine position for atomic page application.
#[derive(Clone, Debug, PartialEq)]
pub struct CommittedSourceObservation {
    /// Publication that delivered this observation.
    pub cursor: PublicationCursor,
    /// Stable source identity. Corrections retain this identity.
    pub identity: String,
    /// Producer revision attached to this publication.
    pub revision: u64,
    /// Normalized observation with its committed engine position.
    pub observation: Observation,
    /// A correction invalidates branch membership, including candidates without this identity.
    pub window_invalidated: bool,
}

/// A bounded page of normalized changes.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceBatch {
    /// Normalized changes in publication order.
    pub events: Vec<SourceEvent>,
    /// Durable cursor to persist with application of this batch.
    pub next: PublicationCursor,
    /// Whether another read can catch up immediately.
    pub has_more: bool,
}

/// Converts committed native evidence into check-domain source events.
pub struct PublishedEvidenceSource<R> {
    reader: R,
    page_size: u16,
}

impl<R: PublishedEvidenceReader> PublishedEvidenceSource<R> {
    /// Creates a source with a bounded page size from 1 through 1000.
    pub fn new(reader: R, page_size: u16) -> Result<Self, SourceError> {
        if !(1..=1000).contains(&page_size) {
            return Err(SourceError::InvalidPageSize(page_size));
        }
        Ok(Self { reader, page_size })
    }

    /// Reads and normalizes one page after the durable resume cursor.
    pub fn read_batch(&self, after: PublicationCursor) -> Result<SourceBatch, SourceError> {
        let batch = self.reader.read_batch(PublicationRead {
            after,
            limit: self.page_size,
        })?;
        if batch.events.len() > usize::from(self.page_size) {
            return Err(SourceError::OversizedPage);
        }
        let mut previous = after;
        let mut events = Vec::with_capacity(batch.events.len());
        for event in batch.events {
            if event.cursor <= previous {
                return Err(SourceError::NonMonotonicCursor {
                    previous,
                    received: event.cursor,
                });
            }
            previous = event.cursor;
            events.push(normalize(event)?);
        }
        if batch.next != previous || batch.next < after {
            return Err(SourceError::InvalidNextCursor {
                expected: previous,
                received: batch.next,
            });
        }
        Ok(SourceBatch {
            events,
            next: batch.next,
            has_more: batch.has_more,
        })
    }
}

fn normalize(event: PublishedEvidenceEvent) -> Result<SourceEvent, SourceError> {
    let branch = BranchId(event.branch);
    match event.change {
        PublishedEvidenceChange::Ignored => Ok(SourceEvent::Ignored {
            cursor: event.cursor,
        }),
        PublishedEvidenceChange::Gap { reason } => Ok(SourceEvent::Gap {
            cursor: event.cursor,
            branch,
            reason,
        }),
        PublishedEvidenceChange::SegmentReset { reason } => Ok(SourceEvent::SegmentReset {
            cursor: event.cursor,
            branch,
            reason,
        }),
        PublishedEvidenceChange::Removal { identity, revision } => Ok(SourceEvent::Removal {
            cursor: event.cursor,
            identity,
            revision,
            branch,
        }),
        PublishedEvidenceChange::CoverageRestored => Ok(SourceEvent::CoverageRestored {
            cursor: event.cursor,
            branch,
        }),
        PublishedEvidenceChange::Upsert(result) => {
            let safe_context = event.context.as_ref().is_some_and(|context| {
                context.main_session
                    && !context.session_id.trim().is_empty()
                    && !context.agent_name.trim().is_empty()
            });
            if result.validity != EvidenceValidity::Valid
                || !safe_context
                || branch.0.trim().is_empty()
                || result.identity.trim().is_empty()
                || result.native_order.0.trim().is_empty()
            {
                return Ok(SourceEvent::UnsafeUpdate {
                    cursor: event.cursor,
                    branch,
                    reason: "Source identity, main-session scope, or native order is unavailable."
                        .to_owned(),
                });
            }
            let encoded = serde_json::to_vec(&(&branch, &result, &event.context))
                .map_err(|error| SourceError::Reader(error.to_string()))?;
            let version = EvidenceVersion(
                Sha256::digest(encoded)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
            );
            let context = event.context;
            let requests = context
                .as_ref()
                .map_or(&[][..], |context| &context.requests);
            let context_value = {
                let mut session = serde_json::json!({
                    "requests": requests.iter().map(|(id, value)| {
                        serde_json::json!({
                            "id": id,
                            "source": value.get("source").cloned().unwrap_or(Value::String("unknown".to_owned())),
                            "kind": value.get("kind").cloned().unwrap_or(Value::String("user_message".to_owned())),
                            "text": value.get("text").cloned().or_else(|| value.get("content").cloned()).unwrap_or(Value::String(String::new())),
                        })
                    }).collect::<Vec<_>>(),
                    "requests_complete": context.as_ref().is_some_and(|context| context.complete_through_event),
                    "source_coverage": if context.as_ref().is_some_and(|context| context.complete_through_event) { "complete" } else { "partial" },
                });
                if let Some(context) = context.as_ref() {
                    session["id"] = Value::String(context.session_id.clone());
                }
                let mut value = serde_json::json!({"session": session, "agent": {}, "repo": {}});
                if let Some(context) = context.as_ref() {
                    value["agent"]["name"] = Value::String(context.agent_name.clone());
                    if let Some(project_root) = &context.project_root {
                        value["repo"]["root"] = Value::String(project_root.clone());
                    }
                }
                value
            };
            let context_version = context
                .as_ref()
                .map(|context| context.version.clone())
                .unwrap_or_else(|| "unavailable".to_owned());
            let status = match result.status {
                PublishedResultStatus::Completed => "completed",
                PublishedResultStatus::Error => "error",
                PublishedResultStatus::Interrupted => "cancelled",
                PublishedResultStatus::Unknown => "unknown",
            }
            .to_owned();
            let payload_complete = result.finality && result.payload_complete;
            let mut message_value = serde_json::json!({
                "id": result.identity,
                "source": context.as_ref().expect("selection context was validated").agent_name,
                "kind": "tool_result",
                "role": "tool",
                "tool": {
                    "name": result.tool_name,
                    "category": result.category,
                    "arguments": result.arguments,
                    "result": {
                        "content": result.content,
                        "status": status,
                    },
                },
                "native_order": result.native_order.0,
                "quality": {
                    "source_complete": result.payload_complete,
                    "tool_name_available": result.tool_name.is_some(),
                    "category_available": result.category.is_some(),
                    "arguments_available": result.arguments.is_some(),
                    "result_content_available": result.content.is_some(),
                    "request_history_complete": context.as_ref().is_some_and(|context| context.complete_through_event),
                    "main_session": context.as_ref().is_some_and(|context| context.main_session),
                },
            });
            if result.tool_name.is_none() {
                message_value["tool"]
                    .as_object_mut()
                    .expect("tool is an object")
                    .remove("name");
            }
            if result.category.is_none() {
                message_value["tool"]
                    .as_object_mut()
                    .expect("tool is an object")
                    .remove("category");
            }
            if result.arguments.is_none() {
                message_value["tool"]
                    .as_object_mut()
                    .expect("tool is an object")
                    .remove("arguments");
            }
            if result.content.is_none() {
                message_value["tool"]["result"]
                    .as_object_mut()
                    .expect("result is an object")
                    .remove("content");
            }
            if let Some(outcome) = result.outcome {
                message_value["tool"]["result"]["outcome"] = outcome;
            }
            if let Some(provider_executed) = result.provider_executed {
                message_value["tool"]["provider_executed"] = Value::Bool(provider_executed);
            }
            let message = Message {
                id: result.identity.clone(),
                value: message_value,
            };
            Ok(SourceEvent::Observation {
                cursor: event.cursor,
                identity: result.identity,
                revision: result.revision,
                observation: Box::new(ObservationDraft {
                    branch,
                    native_order: result.native_order,
                    first_terminal_publication: result.first_terminal_publication,
                    version,
                    kind: if result.finality {
                        ObservationKind::ToolResult {
                            message,
                            category: result.category.unwrap_or_default(),
                            status,
                        }
                    } else {
                        ObservationKind::Other
                    },
                    context: FrozenContext {
                        value: context_value,
                        dependencies: vec![EvidenceDependency::Context {
                            id: "user-messages-through-event".to_owned(),
                            version: EvidenceVersion(context_version),
                        }],
                    },
                    complete: payload_complete,
                }),
            })
        }
    }
}

/// Compatibility name for callers that still use the earlier source name.
pub type FileEventSource<R> = PublishedEvidenceSource<R>;

/// A source read or contract failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceError {
    /// A page size is outside the supported bound.
    InvalidPageSize(u16),
    /// The reader returned more events than requested.
    OversizedPage,
    /// The reader returned an event at or before an earlier cursor.
    NonMonotonicCursor {
        previous: PublicationCursor,
        received: PublicationCursor,
    },
    /// The page's next cursor does not match its final event.
    InvalidNextCursor {
        expected: PublicationCursor,
        received: PublicationCursor,
    },
    /// The publication store failed to read.
    Reader(String),
}

#[cfg(test)]
mod tests;

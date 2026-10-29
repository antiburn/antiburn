//! Bounded, resumable adaptation of committed source publications.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::compiler::ToolCategory;
use crate::context::{CurrentInventories, ObservedAttribution};
use crate::domain::{
    BranchId, EvidenceDependency, EvidenceVersion, FrozenContext, Message, NativeOrderKey,
    Observation, ObservationDraft, ObservationKind,
};

mod message_validation;
mod messages;
mod observations;
pub use crate::domain::{ResponseCycleStatus, SessionStartReason};
pub use message_validation::validate_public_message;
pub use messages::{
    normalize_assistant_message, normalize_tool_call, normalize_tool_result_message,
    normalize_user_message,
};
pub use observations::{
    PublishedObservation, PublishedObservationPayload, PublishedResponseCycle,
    PublishedSourceEvidence,
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
    /// Optional recorded facts. No fact is recovered from output or current settings.
    #[serde(skip_serializing_if = "PublishedToolMetadata::is_empty")]
    pub metadata: Box<PublishedToolMetadata>,
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

/// Optional check-facing facts from exact native request/result bindings.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedToolMetadata {
    /// Recorded requester model and mode evidence, independent of session settings.
    #[serde(flatten)]
    pub attribution: ObservedAttribution,
    /// Source qualifier for recorded mode evidence, not an accuracy score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode_evidence: Option<String>,
    /// A category from a reviewed native identity. Never infer it from collapsed `other`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_category: Option<ToolCategory>,
    /// Known normalized operation paths. None is unavailable; an empty list is known empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<String>>,
    /// Whether the recorded operation path set is complete.
    #[serde(default)]
    pub paths_complete: bool,
    /// Path evidence and normalization limits, retained beside known paths.
    #[serde(default)]
    pub paths_limits: Vec<String>,
    /// Source-specific reason that a singular requester model is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_limit: Option<String>,
    /// Source-specific reason that requester mode evidence is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fast_mode_limit: Option<String>,
    /// Source-specific category evidence limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_limit: Option<String>,
}

impl PublishedToolMetadata {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Whether source identity, order, and ownership support selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Current inventories captured with this immutable publication, never reread on retry.
    #[serde(skip_serializing_if = "CurrentInventories::is_empty")]
    pub inventories: CurrentInventories,
    /// Explicit current selection, never the latest requester.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_selection: Option<ObservedAttribution>,
}

/// One committed publication event.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedEvidenceEvent {
    /// Database-wide committed publication sequence.
    pub cursor: PublicationCursor,
    /// Exact native-current token for this immutable publication, when supplied.
    pub source_evidence_version: Option<EvidenceVersion>,
    /// Stable branch identity provided by publication admission.
    pub branch: String,
    /// Immutable request context admitted for the event's source position.
    pub context: Option<PublishedContext>,
    /// Event payload and correction/removal/gap marker.
    pub change: PublishedEvidenceChange,
}

/// Mutation kind for one published evidence stream.
#[derive(Clone, Debug, PartialEq)]
pub enum PublishedEvidenceChange {
    /// A live source baseline or inactive-session event advances delivery only.
    Ignored,
    /// A finalized result or correction.
    Upsert(PublishedToolResult),
    /// An admitted public message, complete cycle, or host boundary.
    ObservationUpsert(PublishedObservation),
    /// A logical result was removed.
    Removal { identity: String, revision: u64 },
    /// A complete snapshot restored membership after a previously reported gap.
    CoverageRestored,
    /// The source no longer proves safe membership or ordering.
    Gap { reason: String },
    /// The source no longer proves safe identity, ownership, or order.
    UnsafeUpdate { reason: String },
    /// Content or history is partial; source identity, scope, and order stay safe.
    PartialGap { reason: String },
    /// Continuity loss starts a new selection window while history stays partial.
    SegmentReset { reason: String },
}

/// A page returned by the desktop publication reader.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishedEvidenceBatch {
    /// Events in strictly increasing durable publication order.
    pub events: Vec<PublishedEvidenceEvent>,
    /// Cursor of the last event. Empty pages can advance past filtered publications.
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
        /// Transient native binding, excluded from observation and checkpoint hashes.
        source_evidence_version: Option<EvidenceVersion>,
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
        if batch.next < previous || (!events.is_empty() && batch.next != previous) {
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
        PublishedEvidenceChange::Gap { reason } => Ok(SourceEvent::UnsafeUpdate {
            cursor: event.cursor,
            branch,
            reason,
        }),
        PublishedEvidenceChange::PartialGap { reason } => Ok(SourceEvent::Gap {
            cursor: event.cursor,
            branch,
            reason,
        }),
        PublishedEvidenceChange::UnsafeUpdate { reason } => Ok(SourceEvent::UnsafeUpdate {
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
        PublishedEvidenceChange::ObservationUpsert(observation) => {
            observations::normalize_observation(
                event.cursor,
                branch,
                event.context,
                event.source_evidence_version,
                observation,
            )
        }
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
                    if let Some(selection) = &context.session_selection {
                        messages::add_attribution(&mut session, selection);
                    }
                }
                let mut value =
                    serde_json::json!({"session": session, "agent": {}, "global": {}, "repo": {}});
                if let Some(context) = context.as_ref() {
                    let inventories = serde_json::to_value(&context.inventories)
                        .map_err(|error| SourceError::Reader(error.to_string()))?;
                    value["global"] = inventories["global"].clone();
                    if context.project_root.is_some() {
                        value["repo"] = inventories["repo"].clone();
                    } else {
                        value["repo"] = serde_json::json!({
                            "instructions_complete": false,
                            "instructions_limits": ["repository-root-unavailable"],
                            "skills_complete": false,
                            "skills_limits": ["repository-root-unavailable"],
                        });
                    }
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
            let category = result
                .metadata
                .check_category
                .map(|category| category.as_str().to_owned())
                .or_else(|| result.category.clone());
            let mut message_value = serde_json::json!({
                "id": result.identity,
                "source": context.as_ref().expect("selection context was validated").agent_name,
                "kind": "tool_result",
                "role": "tool",
                "tool": {
                    "name": result.tool_name,
                    "category": category,
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
                    "category_available": category.is_some(),
                    "arguments_available": result.arguments.is_some(),
                    "result_content_available": result.content.is_some(),
                    "request_history_complete": context.as_ref().is_some_and(|context| context.complete_through_event),
                    "main_session": context.as_ref().is_some_and(|context| context.main_session),
                    "paths_available": result.metadata.paths.is_some(),
                    "paths_complete": result.metadata.paths.is_some() && result.metadata.paths_complete,
                    "paths_limits": result.metadata.paths_limits,
                },
            });
            if let Some(model) = result.metadata.attribution.model {
                message_value["model"] = Value::String(model);
            }
            if let Some(mode) = result.metadata.attribution.fast_mode {
                message_value["fast_mode"] = Value::Bool(mode);
                if let Some(evidence) = result.metadata.fast_mode_evidence {
                    message_value["quality"]["fast_mode_evidence"] = Value::String(evidence);
                }
            }
            if let Some(paths) = result.metadata.paths {
                message_value["tool"]["paths"] = serde_json::json!(paths);
            }
            for (field, limit) in [
                ("model_limit", result.metadata.model_limit),
                ("fast_mode_limit", result.metadata.fast_mode_limit),
                ("category_limit", result.metadata.category_limit),
            ] {
                if let Some(limit) = limit {
                    message_value["quality"][field] = Value::String(limit);
                }
            }
            if result.tool_name.is_none() {
                message_value["tool"]
                    .as_object_mut()
                    .expect("tool is an object")
                    .remove("name");
            }
            if category.is_none() {
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
                source_evidence_version: event.source_evidence_version,
                identity: result.identity,
                revision: result.revision,
                observation: Box::new(ObservationDraft {
                    branch,
                    native_order: result.native_order,
                    first_terminal_publication: result.first_terminal_publication,
                    first_eligible_publication: None,
                    version,
                    kind: if result.finality {
                        ObservationKind::ToolResult {
                            message,
                            category: category.unwrap_or_default(),
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
    /// A typed public payload violates the normalized event contract.
    InvalidObservation(String),
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

//! Immutable check runtime identities, observations, and frozen inputs.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Committed engine observation order used for selection and accepted watermarks.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ObservationPosition(pub u64);

/// Source-native order for a logical record, independent of publication delivery.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NativeOrderKey(pub String);

/// A definition generation binds runtime work to one effective configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Generation(pub u64);

/// A stable identity for one active definition instance.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ActivationId {
    /// Environment, session CWD, and run that own this activation.
    pub scope: ActivationScope,
    /// The logical filename ID.
    pub definition_id: String,
    /// The persisted generation number.
    pub generation: Generation,
}

/// Runtime isolation boundary. An empty session CWD means it is unknown.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ActivationScope {
    /// Persisted environment identity, such as the local machine or remote host.
    pub environment: String,
    /// Exact stored session working directory, independent of the repository root.
    pub session_cwd: String,
    /// Live execution or one isolated replay invocation.
    pub run: RunScope,
}

/// Separates live state from each replay invocation.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RunScope {
    /// Normal installed-check execution.
    #[default]
    Live,
    /// Caller-assigned replay identity.
    Replay(String),
}

/// A stable branch identity supplied by the source adapter.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BranchId(pub String);

/// Source version attached to one immutable observation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EvidenceVersion(pub String);

/// A normalized message available to conditions and decision inputs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Stable source-derived message identity.
    pub id: String,
    /// The JSON-compatible normalized message value.
    pub value: Value,
}

/// A dependency on one message version or on complete collection membership.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum EvidenceDependency {
    /// A selected message must retain this source version.
    Message {
        id: String,
        version: EvidenceVersion,
    },
    /// The message collection was complete through this position.
    MembershipThrough(ObservationPosition),
    /// Required context was frozen at this position and version.
    Context {
        id: String,
        version: EvidenceVersion,
    },
}

/// Frozen values and the evidence facts that make them valid.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrozenContext {
    /// The immutable JSON context supplied to expressions and input templates.
    pub value: Value,
    /// Dependencies required to trust the context value.
    pub dependencies: Vec<EvidenceDependency>,
}

/// The normalized event kind that drives selection.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ObservationKind {
    /// A user-slot message that supplies context but does not trigger a run.
    UserMessage(Message),
    /// A finalized native tool result.
    ToolResult {
        /// The normalized message representation.
        message: Message,
        /// Normalized tool category name. Empty means unavailable, not `other`.
        category: String,
        /// Final source result status.
        status: String,
    },
    /// An observation that is retained for ordering but does not select a window.
    Other,
}

/// Normalized source evidence before the engine commits its observation position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObservationDraft {
    /// Stable session/branch identity.
    pub branch: BranchId,
    /// Source-native order for the logical record.
    pub native_order: NativeOrderKey,
    /// Immutable first-terminal publication, when the publisher retains it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_terminal_publication: Option<crate::source::PublicationCursor>,
    /// Source version used to detect corrections.
    pub version: EvidenceVersion,
    /// Observation payload kind.
    pub kind: ObservationKind,
    /// Required request/context snapshot through this observation.
    pub context: FrozenContext,
    /// Whether selected payload fields are complete. Source admission proves identity and order separately.
    pub complete: bool,
}

impl ObservationDraft {
    /// Assigns a position that the Store must commit with its source page.
    #[must_use]
    pub fn commit(self, position: ObservationPosition) -> Observation {
        Observation {
            position,
            branch: self.branch,
            native_order: self.native_order,
            first_terminal_publication: self.first_terminal_publication,
            version: self.version,
            kind: self.kind,
            context: self.context,
            complete: self.complete,
        }
    }
}

/// One immutable, normalized source observation with a committed engine position.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    /// Committed engine observation order.
    pub position: ObservationPosition,
    /// Stable session/branch identity.
    pub branch: BranchId,
    /// Source-native order for the logical record.
    pub native_order: NativeOrderKey,
    /// Immutable first-terminal publication, when the publisher retains it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_terminal_publication: Option<crate::source::PublicationCursor>,
    /// Source version used to detect corrections.
    pub version: EvidenceVersion,
    /// Observation payload kind.
    pub kind: ObservationKind,
    /// Required request/context snapshot through this observation.
    pub context: FrozenContext,
    /// Whether selected payload fields are complete. Partial payloads remain selectable.
    pub complete: bool,
}

/// An immutable candidate prepared from one rolling tool-result window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FrozenCandidate {
    /// Definition generation that owns the candidate.
    pub activation: ActivationId,
    /// Branch that supplied the selected messages.
    pub branch: BranchId,
    /// Trigger position; corrections do not create a second logical trigger.
    pub trigger_position: ObservationPosition,
    /// Selected messages in source order.
    pub messages: Vec<Message>,
    /// Frozen context captured at trigger time.
    pub context: FrozenContext,
    /// Exact evidence versions selected by the candidate.
    pub dependencies: Vec<EvidenceDependency>,
    /// Deterministic digest over activation, trigger, messages, and context.
    pub fingerprint: [u8; 32],
}

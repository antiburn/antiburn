//! Locally observed session lineage.
//!
//! Some vendors let a user branch an existing session into a new one. When
//! discovery can see that relationship in the vendor's own store — a rollout
//! header, a duplicated conversation prefix, a parent id column — it records a
//! [`ForkObservation`] alongside the child session so downstream consumers can
//! attribute inherited work to the parent instead of counting it twice.
//!
//! The observation is *evidence*, not a verdict: `confidence` and
//! `detection_source` describe how the link was found, and consumers decide
//! what to do with it.

use serde::{Deserialize, Serialize};

/// The key under which discovery embeds a [`ForkObservation`] in the synthetic
/// metadata header of a session it renders from a vendor database.
///
/// Adapters that materialize a transcript (Cursor's `store.db` and desktop
/// composer sources, OpenCode's SQLite store) write the observation here so a
/// consumer reading the rendered content can recover it without re-opening the
/// vendor store.
pub const FORK_OBSERVATION_KEY: &str = "local_fork_observation";

/// A locally detected link from a session to the session it was branched from.
///
/// Field names are the serialized contract: adapters embed this verbatim under
/// [`FORK_OBSERVATION_KEY`], and readers deserialize it back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForkObservation {
    /// Slug of the agent that owns the parent session (e.g. `"cursor"`).
    pub parent_agent: String,
    /// The parent session's vendor-assigned id.
    pub parent_agent_session_id: String,
    /// Shape of the relationship as the vendor models it (e.g. `"fork"`).
    pub fork_kind: String,
    /// Vendor id of the exact point the child branched from, when the store
    /// records one.
    pub provider_fork_point_id: Option<String>,
    /// How the link was detected (e.g. `"stable_id_prefix"`). Distinguishes a
    /// declared parent from an inferred one.
    pub detection_source: String,
    /// Confidence in the link, 0–100. 100 means the vendor stated it.
    pub confidence: u8,
    /// How many items the child inherited from the parent, when countable.
    pub inherited_item_count: Option<u32>,
    /// Version of the extractor that produced this observation, so a consumer
    /// can tell observations from different detection generations apart.
    pub extractor_version: String,
}

/// Detects that one session was duplicated from another by comparing the two
/// vendor-store payloads.
///
/// Vendors that duplicate a conversation without recording a parent id (Cursor's
/// desktop composers) leave only the copied content as evidence, and how much
/// overlap counts as a fork is a policy decision. Discovery therefore takes the
/// detector from the embedding application instead of hard-coding a threshold;
/// adapters that have no detector configured simply emit no observation for
/// that source.
pub type DuplicateForkDetector =
    fn(parent_store: &str, child_store: &str) -> Option<ForkObservation>;

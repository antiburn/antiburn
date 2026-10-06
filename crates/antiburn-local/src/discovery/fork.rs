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

/// Maximum number of leading transcript records searched for fork evidence.
const FORK_OBSERVATION_LINES: usize = 5;

/// Maximum nesting depth searched within a transcript metadata record.
const FORK_OBSERVATION_DEPTH: usize = 4;

/// Reads a declared fork parent from a bounded transcript preview.
pub fn fork_parent_from_content(content: &str) -> Option<String> {
    content
        .lines()
        .take(FORK_OBSERVATION_LINES)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|value| find_fork_parent(&value, FORK_OBSERVATION_DEPTH))
}

fn find_fork_parent(value: &serde_json::Value, depth: usize) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let object = value.as_object()?;
    if object.get("type").and_then(serde_json::Value::as_str) == Some("session_meta")
        && let Some(parent_id) = object
            .get("payload")
            .and_then(|payload| payload.get("forked_from_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|parent_id| !parent_id.is_empty())
    {
        return Some(parent_id.to_owned());
    }
    if let Some(observation) = object.get(FORK_OBSERVATION_KEY)
        && let Ok(observation) = serde_json::from_value::<ForkObservation>(observation.clone())
        && !observation.parent_agent_session_id.is_empty()
    {
        return Some(observation.parent_agent_session_id);
    }
    object
        .values()
        .find_map(|nested| find_fork_parent(nested, depth - 1))
}

#[cfg(test)]
mod fork_parent_tests {
    use super::*;

    #[test]
    fn reads_normalized_and_codex_fork_headers() {
        let normalized = serde_json::json!({
            "type": "session_meta",
            "metadata": {
                FORK_OBSERVATION_KEY: {
                    "parent_agent": "cursor",
                    "parent_agent_session_id": "parent-42",
                    "fork_kind": "fork",
                    "provider_fork_point_id": null,
                    "detection_source": "stable_id_prefix",
                    "confidence": 100,
                    "inherited_item_count": 12,
                    "extractor_version": "1"
                }
            }
        });
        let codex = serde_json::json!({
            "type": "session_meta",
            "payload": { "id": "child-42", "forked_from_id": "parent-42" }
        });
        assert_eq!(
            fork_parent_from_content(&format!("{normalized}\nignored\n")).as_deref(),
            Some("parent-42")
        );
        assert_eq!(
            fork_parent_from_content(&codex.to_string()).as_deref(),
            Some("parent-42")
        );
    }

    #[test]
    fn ignores_unscoped_empty_and_over_budget_fork_fields() {
        let message = serde_json::json!({
            "type": "response_item",
            "payload": { "forked_from_id": "not-a-parent" }
        });
        let empty = serde_json::json!({
            "type": "session_meta",
            "payload": { "forked_from_id": "  " }
        });
        let deep = serde_json::json!({ "a": { "b": { "c": { "d": {
            FORK_OBSERVATION_KEY: { "parent_agent_session_id": "too-deep" }
        }}}}});
        assert_eq!(fork_parent_from_content(&message.to_string()), None);
        assert_eq!(fork_parent_from_content(&empty.to_string()), None);
        assert_eq!(fork_parent_from_content(&deep.to_string()), None);
    }
}

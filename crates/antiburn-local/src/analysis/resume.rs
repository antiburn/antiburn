//! Snapshot types for verified-resume ingest.
//!
//! [`StreamSnapshot`] is the unit of resume: everything a pass needs to
//! continue a stream from a verified byte offset instead of starting over.
//! State for one session lives in three places — the adapter (a
//! whole-stream buffer like `ClaudeStreamState`), the metrics accumulator
//! (a reorder window, deferred cache patches, duration heaps), the evidence
//! accumulator (ordering and thread-link state a row query cannot answer),
//! and the tool-result and user-message collections — so the snapshot
//! bundles them all, plus the row sink's next index and the [`ResumePoint`]
//! the offset reader verifies on reopen.
//!
//! An adapter builds only its own half ([`AdapterResume`]); it never sees
//! the sink's concrete accumulator types, only `&mut dyn RecordSink`. The
//! sink's half comes from [`crate::analysis::evidence_sink::CompositeSink::snapshot`],
//! which combines the two into a full [`StreamSnapshot`].
//!
//! Restoring a snapshot and streaming from its offset must reproduce
//! exactly what a full pass over the same bytes gives. See
//! `crates/antiburn-local/tests/resume_parity.rs` for the proof.

use serde::{Deserialize, Serialize};

pub(crate) mod identity_filter;

use crate::analysis::evidence::SessionCoverageRecord;
use crate::analysis::evidence_sink::EvidenceResumeState;
use crate::analysis::metrics_sink::SessionMetricsAccumulator;
use crate::analysis::source_validity::ResumePoint;

/// Vendor-specific adapter state, opaque to everything but the adapter that
/// produced it. Keeps [`StreamSnapshot`] vendor-neutral: each adapter picks
/// its own serialization inside this envelope (`ClaudeSessionReader` uses
/// `postcard`, matching [`StreamSnapshot::encode`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdapterSnapshot(pub Vec<u8>);

/// One adapter's own resume state after a streaming pass: the verified
/// byte offset and tail hash an offset reader checks on reopen, plus the
/// adapter's own opaque whole-stream state.
///
/// This is the half of a [`StreamSnapshot`] an adapter can build on its
/// own — it never sees the sink's concrete accumulator types, only
/// `&mut dyn RecordSink`. [`crate::analysis::evidence_sink::CompositeSink::snapshot`]
/// combines this with the sink's own metrics, evidence, and row-index
/// state to build the full snapshot.
#[derive(Debug, Clone)]
pub struct AdapterResume {
    pub point: ResumePoint,
    pub adapter: AdapterSnapshot,
}

/// A session's evidence-accumulator state at a pause point: the coverage
/// record [`crate::analysis::SessionEvidenceAccumulator::coverage_record`]
/// produces, plus the two transient fields it leaves out. See
/// [`crate::analysis::SessionEvidenceAccumulator::from_coverage_record_with_resume`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceSnapshot {
    pub record: SessionCoverageRecord,
    pub resume: EvidenceResumeState,
}

/// Everything needed to resume a stream exactly where a prior pass left
/// off: the verified byte offset, the adapter's own whole-stream state, the
/// metrics and evidence accumulators, tool-result and user-message
/// observations with their completeness, and the row sink's next index.
///
/// `SessionSummary` fields an adapter can only state after its last record
/// (`context_window`, `model`, `initial_context`, …) need no snapshot field
/// of their own: the adapter's own snapshot already carries the
/// whole-stream state that produces them, and the next `finish` rebuilds
/// them the same way a full pass would.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamSnapshot {
    /// Must equal [`crate::analysis::RESUME_SNAPSHOT_REVISION`] — see
    /// [`Self::is_current`].
    pub revision: i64,
    pub resume: ResumePoint,
    pub adapter: AdapterSnapshot,
    pub metrics: SessionMetricsAccumulator,
    pub evidence: EvidenceSnapshot,
    pub next_turn_index: u64,
    /// Native tool-result observations encoded as JSON text because postcard
    /// cannot encode `serde_json::Value` directly.
    pub tool_result_observations_json: Vec<String>,
    /// Stable identity keys whose revisions had conflicting observations.
    /// An empty key means conflict tracking overflowed and blocks later results.
    pub native_tool_result_conflicts: Vec<String>,
    /// False when the observation limit, encoding, or conflicting input
    /// prevented the snapshot from retaining a complete collection.
    pub tool_result_observations_complete: bool,
    /// User-message observations encoded one per JSON string. Restore
    /// can reject an unreadable fact without discarding the rest of the snapshot.
    pub user_messages_json: Vec<String>,
    /// Stable identity keys whose observations conflicted. An empty key means
    /// conflict tracking overflowed and blocks later requests.
    pub user_message_conflicts: Vec<String>,
    /// False when a request was unreadable, the collection limit was reached,
    /// or an identity conflict prevented complete retention.
    pub user_messages_complete: bool,
    /// Ordered native evidence encoded as typed JSON, never raw postcard values.
    pub native_evidence_json: Vec<String>,
    pub native_evidence_complete: bool,
    /// The first observed unsafe admission remains known after collection eviction.
    #[serde(deserialize_with = "deserialize_admission_state")]
    pub native_admission_failure_json: Option<String>,
}

fn deserialize_admission_state<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let json = Option::<String>::deserialize(deserializer)?;
    if let Some(json) = &json {
        let failure = serde_json::from_str::<crate::analysis::NativeAdmissionFailure>(json)
            .map_err(serde::de::Error::custom)?;
        if !failure.valid() {
            return Err(serde::de::Error::custom(
                "invalid admission failure binding",
            ));
        }
    }
    Ok(json)
}

impl StreamSnapshot {
    /// True when `revision` matches the current
    /// [`crate::analysis::RESUME_SNAPSHOT_REVISION`]. A caller rejects a
    /// stale snapshot (an older revision) instead of restoring it, falling
    /// back to a full pass — see that constant's doc comment for what else
    /// invalidates a snapshot.
    pub fn is_current(&self) -> bool {
        self.revision == crate::analysis::RESUME_SNAPSHOT_REVISION
    }

    /// Encodes this snapshot as compact binary (`postcard`), the form a
    /// caller persists. JSON re-expands this type's interned IDs and packed
    /// slot indices into full field names and strings; `postcard` keeps
    /// them close to their in-memory size. See
    /// `crates/antiburn-local/tests/streaming_metrics_memory.rs` for a
    /// measured size on the largest corpus tier.
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("a StreamSnapshot always encodes")
    }

    /// Decodes a snapshot [`Self::encode`] produced. A failure of any kind —
    /// truncated bytes, a shape from a different revision's encoding, plain
    /// corruption — means the snapshot is not resumable: the caller falls
    /// back to a full pass rather than trying to interpret a partial
    /// result. Deliberately does not distinguish failure causes; nothing
    /// downstream of "not resumable" needs to.
    pub fn decode(bytes: &[u8]) -> Result<Self, postcard::Error> {
        postcard::from_bytes(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::RESUME_SNAPSHOT_REVISION;
    use crate::analysis::evidence::{SourceCapabilities, SourceKind};
    use crate::analysis::evidence_sink::SessionEvidenceAccumulator;

    fn sample_snapshot(revision: i64) -> StreamSnapshot {
        let evidence = SessionEvidenceAccumulator::new(crate::analysis::evidence::EvidenceSource {
            agent: "claude".to_owned(),
            session_id: "s1".to_owned(),
            kind: SourceKind::Jsonl,
            capabilities: SourceCapabilities::claude(),
        });
        StreamSnapshot {
            revision,
            resume: ResumePoint {
                offset: 0,
                tail_hash: 0,
                tail_len: 0,
            },
            adapter: AdapterSnapshot(Vec::new()),
            metrics: SessionMetricsAccumulator::new("claude", "s1"),
            evidence: EvidenceSnapshot {
                record: evidence.coverage_record(),
                resume: EvidenceResumeState::default(),
            },
            next_turn_index: 0,
            tool_result_observations_json: Vec::new(),
            native_tool_result_conflicts: Vec::new(),
            tool_result_observations_complete: true,
            user_messages_json: Vec::new(),
            user_message_conflicts: Vec::new(),
            user_messages_complete: true,
            native_evidence_json: Vec::new(),
            native_evidence_complete: true,
            native_admission_failure_json: None,
        }
    }

    #[test]
    fn a_snapshot_at_the_current_revision_is_current() {
        assert!(sample_snapshot(RESUME_SNAPSHOT_REVISION).is_current());
    }

    #[test]
    fn a_snapshot_at_a_stale_revision_is_not_current() {
        assert!(!sample_snapshot(RESUME_SNAPSHOT_REVISION - 1).is_current());
    }

    #[test]
    fn a_snapshot_round_trips_through_its_encoding() {
        let snapshot = sample_snapshot(RESUME_SNAPSHOT_REVISION);
        let encoded = snapshot.encode();
        let restored = StreamSnapshot::decode(&encoded).expect("decode snapshot");
        assert_eq!(restored.revision, snapshot.revision);
        assert_eq!(restored.next_turn_index, snapshot.next_turn_index);
        assert!(restored.is_current());
    }

    #[test]
    fn decoding_truncated_bytes_fails_instead_of_panicking() {
        let snapshot = sample_snapshot(RESUME_SNAPSHOT_REVISION);
        let mut encoded = snapshot.encode();
        encoded.truncate(encoded.len() / 2);
        assert!(StreamSnapshot::decode(&encoded).is_err());
    }

    #[test]
    fn invalid_admission_reasons_cannot_restore_a_snapshot() {
        for reason in [
            crate::analysis::ToolResultCompleteness::Complete,
            crate::analysis::ToolResultCompleteness::Incomplete,
        ] {
            let mut snapshot = sample_snapshot(RESUME_SNAPSHOT_REVISION);
            snapshot.native_admission_failure_json = Some(
                serde_json::to_string(&crate::analysis::NativeAdmissionFailure {
                    source_format: crate::analysis::SourceFormat::ClaudeJsonl,
                    session_id: "s1".into(),
                    position: crate::analysis::ToolResultPosition::Jsonl {
                        record_index: 0,
                        item_index: 0,
                    },
                    reason,
                })
                .unwrap(),
            );
            assert!(StreamSnapshot::decode(&snapshot.encode()).is_err());
        }
    }

    #[test]
    fn snapshot_admission_state_is_a_required_field() {
        let snapshot = sample_snapshot(RESUME_SNAPSHOT_REVISION);
        let mut json = serde_json::to_value(snapshot).unwrap();
        json.as_object_mut()
            .unwrap()
            .remove("native_admission_failure_json");
        assert!(serde_json::from_value::<StreamSnapshot>(json).is_err());
    }
}

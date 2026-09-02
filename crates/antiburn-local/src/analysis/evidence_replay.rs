//! Rebuilds `SessionEvidence` from persisted facts and a coverage record
//! alone.
//!
//! [`evidence_from_facts`] combines [`TurnFacts`] (read with SQL through
//! [`evidence_query::query_turn_facts`]) and a [`SessionCoverageRecord`] (the
//! bounded residual [`SessionEvidenceAccumulator::coverage_record`] captures,
//! for the facts a row query can never answer) to rebuild [`SessionEvidence`]
//! with no transcript or live fold involved. See
//! `crates/antiburn-local/tests/evidence_replay_parity.rs` for the proof.

use crate::analysis::evidence::{SessionCoverageRecord, SessionEvidence};
use crate::analysis::evidence_query::TurnFacts;
use crate::analysis::evidence_sink::SessionEvidenceAccumulator;

/// Rebuilds [`SessionEvidence`] from a session's [`TurnFacts`] and its
/// [`SessionCoverageRecord`], with no transcript, store, or live fold
/// involved.
pub fn evidence_from_facts(facts: &TurnFacts, record: &SessionCoverageRecord) -> SessionEvidence {
    SessionEvidenceAccumulator::from_coverage_record(record.clone()).evidence(facts)
}

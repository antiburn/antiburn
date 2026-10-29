use crate::store::SessionKey;

#[derive(Debug, Clone, PartialEq)]
pub struct BurnCheckObservationRecord {
    pub publication_sequence: i64,
    pub session: SessionKey,
    pub observation_id: String,
    pub source_format: String,
    pub native_identity: String,
    pub native_revision: i64,
    pub evidence_version: String,
    pub removed: bool,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BurnCheckCandidateRecord {
    pub candidate_id: String,
    pub check_id: String,
    pub activation_scope_key: String,
    pub generation: i64,
    pub branch_key: String,
    pub session: SessionKey,
    pub trigger_position: i64,
    pub selected_publication_head: i64,
    /// Map every selected observation ID to its exact source version.
    pub dependencies: serde_json::Value,
    pub frozen_input: serde_json::Value,
    pub input_fingerprint: String,
    pub created_at_epoch: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurnCheckCandidateClaim {
    pub candidate_id: String,
    pub owner: String,
    pub fence: i64,
}

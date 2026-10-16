//! Durable Burn Check runtime state.

mod assessments;
mod candidates;
mod observations;
mod records;
mod retention;
mod runtime;
pub use runtime::StoreCheckStore;

use sha2::{Digest, Sha256};

pub(super) fn payload_sha256(payload_json: &str) -> String {
    let digest = Sha256::digest(payload_json.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub use records::{BurnCheckCandidateClaim, BurnCheckCandidateRecord, BurnCheckObservationRecord};

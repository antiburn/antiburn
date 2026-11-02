//! Durable Burn Check runtime state.

mod assessments;
mod candidates;
mod connection_changes;
mod observations;
mod records;
mod retention;
mod runtime;
#[cfg(test)]
pub(crate) mod test_hooks;
#[cfg(test)]
pub(crate) use crate::trace_capture;
pub(crate) mod trace_failure;
pub(crate) mod trace_refs;
mod usage;
pub use runtime::StoreCheckStore;

use sha2::{Digest, Sha256};

pub(super) fn payload_sha256(payload_json: &str) -> String {
    let digest = Sha256::digest(payload_json.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub use records::{BurnCheckCandidateClaim, BurnCheckCandidateRecord, BurnCheckObservationRecord};

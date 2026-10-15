//! Pure Scope Creep preparation. The shared worker owns transport and scheduling.

mod compact;
mod planning;
mod questions;
mod reduction;

pub use planning::{
    INPUT_SELECTION, MAX_SCOPE_DESCRIPTOR_BYTES, ScopeCreepCheck, ScopeCreepInput,
    ScopeCreepPrepared, ScopeDescriptorInventory, ScopeInventoryLimit, WorkBinding, WorkGroup,
    WorkObservationKind, input_selection_for_source,
};
pub use questions::{DECISION_THRESHOLD, ScopeAnswer, ScopeQuestion};
pub use reduction::{
    ScopeCreepDecision, ScopeCreepFinding, ScopeCreepResult, ScopeCreepStatus, ScopeExcerpt,
    ScopeExplanationBasis, ScopeRelationship,
};

use crate::analysis::jev::JevCheckRevisions;

pub const REVISIONS: JevCheckRevisions = JevCheckRevisions {
    projection: 13,
    chunking: 10,
    questions: 31,
    reducer: 18,
};

#[cfg(test)]
mod tests;

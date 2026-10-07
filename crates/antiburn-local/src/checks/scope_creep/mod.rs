//! Pure Scope Creep preparation. The shared worker owns transport and scheduling.

mod planning;
mod questions;
mod reduction;

pub use planning::{
    INPUT_SELECTION, ScopeCreepCheck, ScopeCreepInput, ScopeCreepPrepared, WorkBinding, WorkGroup,
    input_selection_for_source,
};
pub use questions::{DECISION_THRESHOLD, ScopeAnswer, ScopeQuestion};
pub use reduction::{ScopeCreepDecision, ScopeCreepFinding, ScopeCreepResult, ScopeCreepStatus};

use crate::analysis::jev::JevCheckRevisions;

pub const REVISIONS: JevCheckRevisions = JevCheckRevisions {
    projection: 7,
    chunking: 2,
    questions: 22,
    reducer: 10,
};

#[cfg(test)]
mod tests;

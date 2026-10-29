//! Bounded discovery of current instruction files, without session comparisons.

mod context;
mod discovery;
mod snapshot;

pub use context::{
    CurrentContextRoot, CurrentContextSnapshot, CurrentInstruction, CurrentInstructionScope,
    CurrentSkill,
};
pub use discovery::{
    InstructionAdapter, InstructionDiscovery, MAX_INSTRUCTION_FILES, MAX_INSTRUCTION_TOTAL_BYTES,
    discover_current_instructions,
};
pub use snapshot::{CurrentInstructionSnapshot, InstructionScope, MAX_INSTRUCTION_BYTES};

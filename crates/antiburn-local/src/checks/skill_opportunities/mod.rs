//! Local reference inputs for Skill Opportunities. This module performs no I/O.

mod inputs;
pub use inputs::*;
mod recorded_use;
pub use recorded_use::*;
mod assessment;
mod native_skill_results;
pub use assessment::*;

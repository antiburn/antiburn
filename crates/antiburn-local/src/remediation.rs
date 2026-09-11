//! Typed findings and bounded remediation prompts.

mod estimates;
mod findings;
mod prompts;
mod verification;

pub use estimates::*;
pub use findings::*;
pub use prompts::*;
pub use verification::*;

pub const DETECTOR_REVISION: u32 = 1;
pub const FINDING_SCHEMA_REVISION: u32 = 1;
pub const REMEDIATION_POLICY_REVISION: u32 = 1;
pub const PROMPT_TEMPLATE_REVISION: u32 = 1;
pub const VERIFICATION_METHOD_REVISION: u32 = 1;
pub const SAVINGS_METHOD_REVISION: u32 = 1;
pub const MAX_PROMPT_BYTES: usize = 8 * 1024;
pub const MAX_PROMPT_IDENTITIES: usize = 8;
pub const MAX_DISPLAY_LABEL_BYTES: usize = 256;

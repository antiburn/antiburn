//! Source evidence and instruction snapshots for the Ignored Instructions check.
//!
//! This module only prepares bounded local evidence. It does not perform
//! inference, schedule work, or interpret vendor transcript formats itself.

mod evidence;
mod input;
mod sources;

pub use evidence::{
    ContentAction, ContentEventReference, ContentReferenceResolution, SessionContentEvidence,
    prepare_session_content, resolve_content_reference,
};

pub use input::{
    InstructionContentClass, InstructionProvenance, InstructionRuleSection, InstructionScope,
    InstructionSnapshot, MAX_INSTRUCTION_BYTES, MAX_INSTRUCTION_SECTIONS, MAX_RULE_SECTION_BYTES,
    MarkdownLimit, segment_markdown, sha256_hex, snapshot_from_text,
};
pub use sources::{
    InstructionAdapter, InstructionDiscovery, MAX_INSTRUCTION_FILES, MAX_INSTRUCTION_TOTAL_BYTES,
    discover_current_instructions,
};

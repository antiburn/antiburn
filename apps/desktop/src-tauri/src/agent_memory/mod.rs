//! Safe edits to Claude Code auto-memory files.
//!
//! A delete moves the file into antiburn's archive first. Undo restores it
//! from the archive. See `docs/remediation.md`, "Memory archive".

mod editor;
#[cfg(test)]
mod tests;

pub(crate) use editor::{archive_memory, remove_index_line, restore_memory};

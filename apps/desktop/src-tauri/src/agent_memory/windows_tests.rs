//! Windows builds have no memory editor. Each edit reports `Unavailable`.

use std::path::Path;

use super::{archive_memory, remove_index_line, restore_memory};
use crate::dto::MemoryEditOutcome;

fn unsupported() -> MemoryEditOutcome {
    MemoryEditOutcome::Unavailable {
        reason: "automaticapplyunsupported".to_owned(),
    }
}

#[test]
fn archive_is_unavailable() {
    let outcome = archive_memory(
        Path::new("C:\\Users\\a"),
        Path::new("C:\\archive"),
        "project",
        "note.md",
        12,
        Some(1),
    );
    assert_eq!(outcome, Ok(unsupported()));
}

#[test]
fn restore_is_unavailable() {
    let outcome = restore_memory(
        Path::new("C:\\Users\\a"),
        Path::new("C:\\archive"),
        "project",
        "1-note.md",
    );
    assert_eq!(outcome, Ok(unsupported()));
}

#[test]
fn index_line_removal_is_unavailable() {
    let outcome = remove_index_line(Path::new("C:\\Users\\a"), "project", 1, "note.md");
    assert_eq!(outcome, Ok(unsupported()));
}

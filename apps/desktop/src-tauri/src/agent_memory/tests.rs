use std::fs;
use std::path::{Path, PathBuf};

use super::*;
use crate::dto::MemoryEditOutcome;

const SLUG: &str = "-work-app";
const INDEX: &str =
    "- [Alpha](a.md) — first hook\n- [Beta](b.md) — second hook\n- [Gone](gone.md) — dangling\n";
const A: &str = "---\nname: alpha\n---\nAlpha body\n";
const B: &str = "---\nname: beta\n---\nBeta body\n";

struct Fixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
    archive: PathBuf,
    memory: PathBuf,
}

fn write(path: &Path, value: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn fixture_with_index(index: Option<&str>) -> Fixture {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let archive = temporary.path().join("app/memory-archive");
    let memory = home.join(".claude/projects").join(SLUG).join("memory");
    write(&memory.join("a.md"), A);
    write(&memory.join("b.md"), B);
    if let Some(index) = index {
        write(&memory.join("MEMORY.md"), index);
    }
    Fixture {
        _temporary: temporary,
        home,
        archive,
        memory,
    }
}

fn fixture() -> Fixture {
    fixture_with_index(Some(INDEX))
}

fn mtime_ms(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).unwrap().modified().ok()?;
    let elapsed = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

fn archive_file(fixture: &Fixture, name: &str) -> Result<MemoryEditOutcome, String> {
    let path = fixture.memory.join(name);
    let size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
    let modified = if path.exists() { mtime_ms(&path) } else { None };
    archive_memory(&fixture.home, &fixture.archive, SLUG, name, size, modified)
}

fn unavailable(outcome: Result<MemoryEditOutcome, String>) -> String {
    match outcome.unwrap() {
        MemoryEditOutcome::Unavailable { reason } => reason,
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

fn archive_id(outcome: Result<MemoryEditOutcome, String>) -> String {
    match outcome.unwrap() {
        MemoryEditOutcome::Archived { archive_id, .. } => archive_id,
        other => panic!("expected Archived, got {other:?}"),
    }
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn sidecar(fixture: &Fixture, id: &str) -> serde_json::Value {
    serde_json::from_str(&read(
        &fixture.archive.join(SLUG).join(format!("{id}.json")),
    ))
    .unwrap()
}

fn archive_entries(fixture: &Fixture) -> usize {
    fs::read_dir(fixture.archive.join(SLUG))
        .map_or(0, |entries| entries.filter_map(Result::ok).count())
}

fn leftover_temporaries(directory: &Path) -> usize {
    fs::read_dir(directory)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .count()
}

#[cfg(not(windows))]
mod writes {
    use super::*;

    #[test]
    fn archive_moves_the_file_and_removes_the_index_line() {
        let fixture = fixture();
        let outcome = archive_file(&fixture, "a.md");
        let MemoryEditOutcome::Archived {
            archive_id,
            index_line_removed,
        } = outcome.unwrap()
        else {
            panic!("expected Archived");
        };
        assert!(index_line_removed);
        assert!(!fixture.memory.join("a.md").exists());
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Beta](b.md) — second hook\n- [Gone](gone.md) — dangling\n"
        );
        assert_eq!(read(&fixture.memory.join("MEMORY.md.antiburn-bak")), INDEX);
        assert_eq!(read(&fixture.memory.join("b.md")), B);
        assert_eq!(read(&fixture.archive.join(SLUG).join(&archive_id)), A);
        assert!(archive_id.ends_with("-a.md"));
        let record = sidecar(&fixture, &archive_id);
        assert_eq!(record["slug"], SLUG);
        assert_eq!(record["file_name"], "a.md");
        assert_eq!(
            record["original_path"],
            fixture
                .memory
                .canonicalize()
                .unwrap()
                .join("a.md")
                .display()
                .to_string()
        );
        assert_eq!(record["index_line"], "- [Alpha](a.md) — first hook");
        assert_eq!(record["index_line_number"], 1);
        assert!(record["archived_at_ms"].as_i64().unwrap() > 0);
        assert!(record["restored_at_ms"].is_null());
        assert_eq!(leftover_temporaries(&fixture.memory), 0);
    }

    #[test]
    fn archive_keeps_crlf_line_endings() {
        let index = "- [Alpha](a.md) — one\r\n- [Beta](b.md) — two\r\n";
        let fixture = fixture_with_index(Some(index));
        archive_file(&fixture, "a.md").unwrap();
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Beta](b.md) — two\r\n"
        );
        assert_eq!(read(&fixture.memory.join("MEMORY.md.antiburn-bak")), index);
    }

    #[test]
    fn archive_keeps_a_missing_final_newline() {
        let fixture = fixture_with_index(Some("- [Alpha](a.md) — one\n- [Beta](b.md) — two"));
        archive_file(&fixture, "b.md").unwrap();
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Alpha](a.md) — one"
        );
    }

    #[test]
    fn archive_removes_every_line_for_the_file() {
        let fixture =
            fixture_with_index(Some("- [A](a.md)\n- [B](b.md)\n- [A again](./a.md) — x\n"));
        archive_file(&fixture, "a.md").unwrap();
        assert_eq!(read(&fixture.memory.join("MEMORY.md")), "- [B](b.md)\n");
    }

    #[test]
    fn size_mismatch_touches_nothing() {
        let fixture = fixture();
        let path = fixture.memory.join("a.md");
        let outcome = archive_memory(
            &fixture.home,
            &fixture.archive,
            SLUG,
            "a.md",
            A.len() as u64 + 1,
            mtime_ms(&path),
        );
        assert_eq!(outcome.unwrap(), MemoryEditOutcome::ChangedOnDisk);
        assert_eq!(read(&path), A);
        assert_eq!(read(&fixture.memory.join("MEMORY.md")), INDEX);
        assert!(!fixture.memory.join("MEMORY.md.antiburn-bak").exists());
        assert!(!fixture.archive.exists());
    }

    #[test]
    fn mtime_mismatch_touches_nothing() {
        let fixture = fixture();
        let path = fixture.memory.join("a.md");
        let outcome = archive_memory(
            &fixture.home,
            &fixture.archive,
            SLUG,
            "a.md",
            A.len() as u64,
            mtime_ms(&path).map(|value| value + 5_000),
        );
        assert_eq!(outcome.unwrap(), MemoryEditOutcome::ChangedOnDisk);
        assert_eq!(read(&path), A);
        assert!(!fixture.archive.exists());
    }

    #[test]
    fn archive_without_an_index_reports_no_line_removed() {
        let fixture = fixture_with_index(None);
        let outcome = archive_file(&fixture, "a.md").unwrap();
        assert!(matches!(
            outcome,
            MemoryEditOutcome::Archived {
                index_line_removed: false,
                ..
            }
        ));
        assert!(!fixture.memory.join("a.md").exists());
        assert!(!fixture.memory.join("MEMORY.md").exists());
        assert!(!fixture.memory.join("MEMORY.md.antiburn-bak").exists());
    }

    #[test]
    fn archive_of_a_file_outside_the_index_leaves_the_index_alone() {
        let fixture = fixture_with_index(Some("- [Beta](b.md) — two\n"));
        let id = archive_id(archive_file(&fixture, "a.md"));
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Beta](b.md) — two\n"
        );
        assert!(!fixture.memory.join("MEMORY.md.antiburn-bak").exists());
        let record = sidecar(&fixture, &id);
        assert!(record["index_line"].is_null());
        assert!(record["index_line_number"].is_null());
    }

    #[test]
    fn archive_of_a_missing_file_reports_missing() {
        let fixture = fixture();
        let outcome = archive_memory(&fixture.home, &fixture.archive, SLUG, "nope.md", 1, None);
        assert_eq!(outcome.unwrap(), MemoryEditOutcome::Missing);
        assert!(!fixture.archive.exists());
    }

    #[test]
    fn unsafe_file_names_are_unavailable() {
        let fixture = fixture();
        for name in [
            "../x.md",
            "MEMORY.md",
            "x.antiburn-bak.md",
            "a.txt",
            "sub/a.md",
            "",
        ] {
            let outcome = archive_memory(&fixture.home, &fixture.archive, SLUG, name, 1, None);
            assert_eq!(unavailable(outcome), "unsafepath", "{name}");
        }
        for slug in ["..", "a/b", ""] {
            let outcome = archive_memory(&fixture.home, &fixture.archive, slug, "a.md", 1, None);
            assert_eq!(unavailable(outcome), "unsafepath", "{slug}");
        }
        assert_eq!(read(&fixture.memory.join("a.md")), A);
        assert!(!fixture.archive.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_memory_file_is_unavailable() {
        let fixture = fixture();
        let outside = fixture.home.join("outside.md");
        write(&outside, "secret");
        fs::remove_file(fixture.memory.join("a.md")).unwrap();
        std::os::unix::fs::symlink(&outside, fixture.memory.join("a.md")).unwrap();
        let outcome = archive_memory(&fixture.home, &fixture.archive, SLUG, "a.md", 6, None);
        assert_eq!(unavailable(outcome), "symlinktarget");
        assert_eq!(read(&outside), "secret");
        assert!(!fixture.archive.exists());
        assert_eq!(read(&fixture.memory.join("MEMORY.md")), INDEX);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_backup_is_unavailable_and_nothing_changes() {
        let fixture = fixture();
        let outside = fixture.home.join("outside.txt");
        write(&outside, "keep");
        std::os::unix::fs::symlink(&outside, fixture.memory.join("MEMORY.md.antiburn-bak"))
            .unwrap();
        let outcome = archive_file(&fixture, "a.md");
        assert_eq!(unavailable(outcome), "symlinktarget");
        assert_eq!(read(&outside), "keep");
        assert_eq!(read(&fixture.memory.join("MEMORY.md")), INDEX);
        assert_eq!(read(&fixture.memory.join("a.md")), A);
        assert!(!fixture.archive.exists());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_index_is_unavailable() {
        let fixture = fixture();
        let outside = fixture.home.join("index-outside.md");
        write(&outside, INDEX);
        fs::remove_file(fixture.memory.join("MEMORY.md")).unwrap();
        std::os::unix::fs::symlink(&outside, fixture.memory.join("MEMORY.md")).unwrap();
        assert_eq!(unavailable(archive_file(&fixture, "a.md")), "symlinktarget");
        assert_eq!(read(&fixture.memory.join("a.md")), A);
        assert_eq!(read(&outside), INDEX);
    }

    #[test]
    fn a_second_archive_overwrites_the_backup() {
        let fixture = fixture();
        archive_file(&fixture, "a.md").unwrap();
        let after_first = read(&fixture.memory.join("MEMORY.md"));
        archive_file(&fixture, "b.md").unwrap();
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md.antiburn-bak")),
            after_first
        );
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Gone](gone.md) — dangling\n"
        );
    }

    #[test]
    fn restore_puts_the_file_and_line_back() {
        let fixture = fixture();
        let id = archive_id(archive_file(&fixture, "a.md"));
        let outcome = restore_memory(&fixture.home, &fixture.archive, SLUG, &id).unwrap();
        assert_eq!(
            outcome,
            MemoryEditOutcome::Restored {
                index_line_restored: true
            }
        );
        assert_eq!(read(&fixture.memory.join("a.md")), A);
        assert_eq!(read(&fixture.memory.join("MEMORY.md")), INDEX);
        let record = sidecar(&fixture, &id);
        assert!(record["restored_at_ms"].as_i64().unwrap() > 0);
        assert_eq!(read(&fixture.archive.join(SLUG).join(&id)), A);
        assert_eq!(leftover_temporaries(&fixture.memory), 0);
    }

    #[test]
    fn restore_refuses_when_the_file_exists() {
        let fixture = fixture();
        let id = archive_id(archive_file(&fixture, "a.md"));
        write(&fixture.memory.join("a.md"), "new text");
        let index_before = read(&fixture.memory.join("MEMORY.md"));
        let outcome = restore_memory(&fixture.home, &fixture.archive, SLUG, &id).unwrap();
        assert_eq!(outcome, MemoryEditOutcome::AlreadyExists);
        assert_eq!(read(&fixture.memory.join("a.md")), "new text");
        assert_eq!(read(&fixture.memory.join("MEMORY.md")), index_before);
        assert!(sidecar(&fixture, &id)["restored_at_ms"].is_null());
    }

    #[test]
    fn restore_clamps_the_line_position_when_the_index_shrank() {
        let fixture = fixture();
        let id = archive_id(archive_file(&fixture, "b.md"));
        // The index now has only the Alpha and Gone lines, so line 2 is the end.
        write(&fixture.memory.join("MEMORY.md"), "- [Alpha](a.md) — x");
        restore_memory(&fixture.home, &fixture.archive, SLUG, &id).unwrap();
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Alpha](a.md) — x\n- [Beta](b.md) — second hook\n"
        );
        let id = archive_id(archive_file(&fixture, "a.md"));
        write(&fixture.memory.join("MEMORY.md"), "");
        restore_memory(&fixture.home, &fixture.archive, SLUG, &id).unwrap();
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Alpha](a.md) — x\n"
        );
    }

    #[test]
    fn restore_inserts_before_lines_that_were_added_later() {
        let fixture = fixture();
        let id = archive_id(archive_file(&fixture, "a.md"));
        write(
            &fixture.memory.join("MEMORY.md"),
            "- [Beta](b.md) — second hook\n- [New](n.md) — n\n",
        );
        restore_memory(&fixture.home, &fixture.archive, SLUG, &id).unwrap();
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Alpha](a.md) — first hook\n- [Beta](b.md) — second hook\n- [New](n.md) — n\n"
        );
    }

    #[test]
    fn restore_skips_a_line_the_index_already_lists() {
        let fixture = fixture();
        let id = archive_id(archive_file(&fixture, "a.md"));
        let index = "- [Alpha again](./a.md) — hand written\n";
        write(&fixture.memory.join("MEMORY.md"), index);
        let outcome = restore_memory(&fixture.home, &fixture.archive, SLUG, &id).unwrap();
        assert_eq!(
            outcome,
            MemoryEditOutcome::Restored {
                index_line_restored: false
            }
        );
        assert_eq!(read(&fixture.memory.join("MEMORY.md")), index);
        assert_eq!(read(&fixture.memory.join("a.md")), A);
    }

    #[test]
    fn restore_creates_a_missing_index() {
        let fixture = fixture();
        let id = archive_id(archive_file(&fixture, "a.md"));
        fs::remove_file(fixture.memory.join("MEMORY.md")).unwrap();
        let outcome = restore_memory(&fixture.home, &fixture.archive, SLUG, &id).unwrap();
        assert_eq!(
            outcome,
            MemoryEditOutcome::Restored {
                index_line_restored: true
            }
        );
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Alpha](a.md) — first hook\n"
        );
    }

    #[test]
    fn restore_of_a_file_without_an_index_line_restores_only_the_file() {
        let fixture = fixture_with_index(None);
        let id = archive_id(archive_file(&fixture, "a.md"));
        let outcome = restore_memory(&fixture.home, &fixture.archive, SLUG, &id).unwrap();
        assert_eq!(
            outcome,
            MemoryEditOutcome::Restored {
                index_line_restored: false
            }
        );
        assert!(!fixture.memory.join("MEMORY.md").exists());
    }

    #[test]
    fn restore_rejects_unknown_and_unsafe_archive_ids() {
        let fixture = fixture();
        let id = archive_id(archive_file(&fixture, "a.md"));
        assert_eq!(
            restore_memory(&fixture.home, &fixture.archive, SLUG, "nope").unwrap(),
            MemoryEditOutcome::Missing
        );
        assert_eq!(
            restore_memory(&fixture.home, &fixture.archive, "-other", &id).unwrap(),
            MemoryEditOutcome::Missing
        );
        for bad in ["../x", "a/b", ".."] {
            let outcome = restore_memory(&fixture.home, &fixture.archive, SLUG, bad);
            assert_eq!(unavailable(outcome), "unsafepath", "{bad}");
        }
        assert!(!fixture.memory.join("a.md").exists());
    }

    #[test]
    fn restore_rejects_a_sidecar_for_another_slug() {
        let fixture = fixture();
        let id = archive_id(archive_file(&fixture, "a.md"));
        let other = fixture.archive.join("-other");
        fs::create_dir_all(&other).unwrap();
        fs::copy(
            fixture.archive.join(SLUG).join(format!("{id}.json")),
            other.join(format!("{id}.json")),
        )
        .unwrap();
        fs::copy(fixture.archive.join(SLUG).join(&id), other.join(&id)).unwrap();
        let outcome = restore_memory(&fixture.home, &fixture.archive, "-other", &id);
        assert_eq!(unavailable(outcome), "unsafepath");
    }

    #[test]
    fn remove_index_line_removes_only_that_line() {
        let fixture = fixture();
        let outcome = remove_index_line(&fixture.home, SLUG, 3, "gone.md").unwrap();
        assert_eq!(outcome, MemoryEditOutcome::IndexLineRemoved);
        assert_eq!(
            read(&fixture.memory.join("MEMORY.md")),
            "- [Alpha](a.md) — first hook\n- [Beta](b.md) — second hook\n"
        );
        assert_eq!(read(&fixture.memory.join("MEMORY.md.antiburn-bak")), INDEX);
    }

    #[test]
    fn remove_index_line_with_the_wrong_target_changes_nothing() {
        let fixture = fixture();
        let outcome = remove_index_line(&fixture.home, SLUG, 2, "gone.md").unwrap();
        assert_eq!(outcome, MemoryEditOutcome::ChangedOnDisk);
        let outcome = remove_index_line(&fixture.home, SLUG, 99, "gone.md").unwrap();
        assert_eq!(outcome, MemoryEditOutcome::ChangedOnDisk);
        assert_eq!(read(&fixture.memory.join("MEMORY.md")), INDEX);
        assert!(!fixture.memory.join("MEMORY.md.antiburn-bak").exists());
    }

    #[test]
    fn remove_index_line_for_an_absent_target_is_missing() {
        let fixture = fixture();
        let outcome = remove_index_line(&fixture.home, SLUG, 3, "other.md").unwrap();
        assert_eq!(outcome, MemoryEditOutcome::Missing);
        let fixture = fixture_with_index(None);
        let outcome = remove_index_line(&fixture.home, SLUG, 1, "gone.md").unwrap();
        assert_eq!(outcome, MemoryEditOutcome::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn rewritten_index_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = fixture();
        let index = fixture.memory.join("MEMORY.md");
        fs::set_permissions(&index, fs::Permissions::from_mode(0o600)).unwrap();
        archive_file(&fixture, "a.md").unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&index), 0o600);
        assert_eq!(mode(&fixture.memory.join("MEMORY.md.antiburn-bak")), 0o600);
        assert_eq!(mode(&fixture.archive.join(SLUG)), 0o700);
    }

    #[test]
    fn oversized_files_are_unavailable() {
        let fixture = fixture();
        write(&fixture.memory.join("a.md"), &"x".repeat(300 * 1024));
        let outcome = archive_file(&fixture, "a.md");
        assert_eq!(unavailable(outcome), "filetoolarge");
        assert!(!fixture.archive.exists());
    }

    #[test]
    fn archive_leaves_no_archive_when_the_index_rewrite_is_refused() {
        let fixture = fixture();
        // A backup that is a directory is not a regular file.
        fs::create_dir(fixture.memory.join("MEMORY.md.antiburn-bak")).unwrap();
        let outcome = archive_file(&fixture, "a.md");
        assert_eq!(unavailable(outcome), "nonregularfile");
        assert_eq!(archive_entries(&fixture), 0);
        assert_eq!(read(&fixture.memory.join("a.md")), A);
    }
}

#[cfg(windows)]
#[test]
fn every_operation_is_unavailable_on_windows() {
    let fixture = fixture();
    let outcome = archive_memory(&fixture.home, &fixture.archive, SLUG, "a.md", 1, None);
    assert_eq!(unavailable(outcome), "automaticapplyunsupported");
    let outcome = restore_memory(&fixture.home, &fixture.archive, SLUG, "x");
    assert_eq!(unavailable(outcome), "automaticapplyunsupported");
    let outcome = remove_index_line(&fixture.home, SLUG, 1, "a.md");
    assert_eq!(unavailable(outcome), "automaticapplyunsupported");
}

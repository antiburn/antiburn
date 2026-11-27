use super::*;
use tempfile::TempDir;

async fn write(path: &std::path::Path) {
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(path, "{}\n").await.unwrap();
}

#[tokio::test]
async fn ids_resolve_in_plain_and_nested_desktop_roots() {
    let home = TempDir::new().unwrap();
    let plain = home.path().join(".claude/projects/-repo");
    let nested = home
        .path()
        .join("Claude/local-agent-mode-sessions/org/account/ws/.claude/projects/-repo");
    write(&plain.join("sess-plain.jsonl")).await;
    write(&nested.join("sess-nested.jsonl")).await;
    let index = TranscriptIndex::new(&[plain.clone(), nested.clone()]);

    assert_eq!(
        index.resolve("sess-plain").await,
        Some(plain.join("sess-plain.jsonl"))
    );
    assert_eq!(
        index.resolve("sess-nested").await,
        Some(nested.join("sess-nested.jsonl"))
    );
    assert_eq!(index.resolve("sess-missing").await, None);
}

/// A probe ignores letter case only where the platform file system does.
#[tokio::test]
async fn letter_case_matches_like_the_platform_probe() {
    let home = TempDir::new().unwrap();
    let dir = home.path().join("projects/-repo");
    write(&dir.join("abc-def.jsonl")).await;
    write(&dir.join("UPPER-ID.JSONL")).await;
    let index = TranscriptIndex::new(std::slice::from_ref(&dir));

    assert_eq!(
        index.resolve("abc-def").await,
        Some(dir.join("abc-def.jsonl"))
    );
    let (lower, upper) = if CASE_INSENSITIVE_NAMES {
        (
            Some(dir.join("abc-def.jsonl")),
            Some(dir.join("UPPER-ID.JSONL")),
        )
    } else {
        (None, None)
    };
    assert_eq!(index.resolve("ABC-DEF").await, lower);
    assert_eq!(index.resolve("upper-id").await, upper);
}

/// Two directories hold names that differ only in letter case. The result is
/// the same as a probe on the default file system of the platform.
#[tokio::test]
async fn case_only_names_resolve_like_the_platform_probe() {
    let home = TempDir::new().unwrap();
    let first = home.path().join("projects/-a");
    let second = home.path().join("projects/-b");
    write(&first.join("CHILD-1.jsonl")).await;
    write(&second.join("child-1.jsonl")).await;
    let index = TranscriptIndex::new(&[first.clone(), second.clone()]);

    let expected = if CASE_INSENSITIVE_NAMES {
        first.join("CHILD-1.jsonl")
    } else {
        second.join("child-1.jsonl")
    };
    assert_eq!(index.resolve("child-1").await, Some(expected));
    assert_eq!(
        index.resolve("CHILD-1").await,
        Some(first.join("CHILD-1.jsonl"))
    );
}

/// Only a case-sensitive volume holds two names that differ only in case in
/// one directory. A probe there finds the exact name.
#[tokio::test]
async fn the_exact_name_wins_inside_one_directory() {
    let home = TempDir::new().unwrap();
    let dir = home.path().join("projects/-repo");
    write(&dir.join("CHILD-1.jsonl")).await;
    write(&dir.join("child-1.jsonl")).await;
    let mut names = Vec::new();
    let mut entries = tokio::fs::read_dir(&dir).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        names.push(entry.file_name());
    }
    if names.len() < 2 {
        // This volume ignores case, so the second write replaced the first.
        return;
    }
    let index = TranscriptIndex::new(std::slice::from_ref(&dir));

    assert_eq!(
        index.resolve("child-1").await,
        Some(dir.join("child-1.jsonl"))
    );
    assert_eq!(
        index.resolve("CHILD-1").await,
        Some(dir.join("CHILD-1.jsonl"))
    );
}

#[tokio::test]
async fn the_first_project_dir_wins_like_the_probe() {
    let home = TempDir::new().unwrap();
    let first = home.path().join("projects/-a");
    let second = home.path().join("projects/-b");
    write(&first.join("sess.jsonl")).await;
    write(&second.join("sess.jsonl")).await;

    let index = TranscriptIndex::new(&[second.clone(), first.clone()]);

    assert_eq!(index.resolve("sess").await, Some(second.join("sess.jsonl")));
}

#[tokio::test]
async fn unsafe_ids_audit_logs_and_other_files_never_resolve() {
    let home = TempDir::new().unwrap();
    let dir = home.path().join("projects/-repo");
    for name in [
        "audit.jsonl",
        "Audit1.jsonl",
        "notes.json",
        "sess.jsonl.bak",
    ] {
        write(&dir.join(name)).await;
    }
    let index = TranscriptIndex::new(std::slice::from_ref(&dir));

    for id in [
        "audit",
        "Audit1",
        "notes",
        "sess",
        "",
        "../etc/passwd",
        "abc/../def",
        "..\\..\\windows",
    ] {
        assert_eq!(index.resolve(id).await, None, "{id:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_transcript_is_refused_like_the_probe() {
    let home = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let dir = home.path().join("projects/-repo");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    let target = outside.path().join("elsewhere.jsonl");
    write(&target).await;
    std::os::unix::fs::symlink(&target, dir.join("sess-link.jsonl")).unwrap();

    let index = TranscriptIndex::new(std::slice::from_ref(&dir));

    assert_eq!(index.resolve("sess-link").await, None);
}

#[tokio::test]
async fn many_lookups_list_each_project_dir_once() {
    let home = TempDir::new().unwrap();
    let dir = home.path().join("projects/-repo");
    write(&dir.join("sess-1.jsonl")).await;
    track_listings(&dir);
    let index = TranscriptIndex::new(std::slice::from_ref(&dir));

    for id in ["sess-1", "sess-2", "sess-3", "sess-1"] {
        index.resolve(id).await;
    }

    assert_eq!(take_tracked_listings(&dir), 1);
}

#[tokio::test]
async fn an_unsafe_id_alone_lists_nothing() {
    let home = TempDir::new().unwrap();
    let dir = home.path().join("projects/-repo");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    track_listings(&dir);
    let index = TranscriptIndex::new(std::slice::from_ref(&dir));

    assert_eq!(index.resolve("../x").await, None);

    assert_eq!(take_tracked_listings(&dir), 0);
}

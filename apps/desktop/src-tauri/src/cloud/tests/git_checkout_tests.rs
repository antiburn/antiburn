use super::*;
use antiburn_cloud_capture::{ProjectSelection, discover_claude, validate_sections};
use serde_json::json;
use std::fs;

fn init(cwd: &Path) -> String {
    tauri::async_runtime::block_on(async {
        git(cwd, &["init", "--initial-branch=main"]).await.unwrap();
        git(
            cwd,
            &[
                "-c",
                "user.name=Synthetic",
                "-c",
                "user.email=synthetic@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "synthetic",
            ],
        )
        .await
        .unwrap();
        git(cwd, &["rev-parse", "HEAD"]).await.unwrap()
    })
    .trim_end()
    .to_owned()
}

#[test]
fn capture_sections_use_only_the_admitted_recorded_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let excluded = root.join("excluded");
    let repository = root.join("repo");
    let cwd = repository.join("subfolder");
    let source = root.join("source");
    for path in [&excluded, &cwd, &source] {
        fs::create_dir_all(path).unwrap();
    }
    init(&excluded);
    let commit = init(&repository);
    tauri::async_runtime::block_on(git(
        &repository,
        &[
            "remote",
            "add",
            "origin",
            "git@github.com:Example/synthetic.git",
        ],
    ))
    .unwrap();
    let session_id = "00000000-0000-4000-8000-000000000001";
    let records =
        [&excluded, &cwd].map(|cwd| json!({"type": "user", "sessionId": session_id, "cwd": cwd}));
    fs::write(
        source.join(format!("{session_id}.jsonl")),
        format!("{}\n{}\n", records[0], records[1]),
    )
    .unwrap();
    let original = discover_claude(&source, &Default::default())
        .unwrap()
        .remove(0);
    let allows_repository = |path: &Path| path == repository;
    assert!(capture_sections(&original, &|| true, &allows_repository).is_empty());
    let selection = ProjectSelection::new(vec![cwd.clone()]).unwrap();
    let admitted = selection.admit(&original).unwrap().unwrap();
    assert!(capture_sections(&admitted, &|| false, &allows_repository).is_empty());
    assert!(capture_sections(&admitted, &|| true, &|path| path.starts_with(&cwd)).is_empty());
    let sections = capture_sections(&admitted, &|| true, &allows_repository);
    validate_sections(&sections).unwrap();
    let section = &sections[SECTION_KEY];
    assert_eq!(section.schema, "antiburn.git-checkout.v1");
    assert!(matches!(
        section.observation,
        MetadataObservation::ObservedAt { .. }
    ));
    assert!(matches!(
        &section.evidence[..],
        [MetadataEvidence::LocalObservation { method, .. }] if method == "git-checkout"
    ));
    assert_eq!(
        section.data,
        json!({
            "branch": "main",
            "commit": commit,
            "kind": "checkout",
            "repository": {"name": "synthetic", "owner": "Example"}
        })
    );
}

#[test]
fn detached_linked_worktree_omits_branch_and_unsupported_origin() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let repository = root.join("repo");
    let worktree = root.join("linked");
    fs::create_dir(&repository).unwrap();
    let commit = init(&repository);
    let observed = tauri::async_runtime::block_on(async {
        git(
            &repository,
            &["worktree", "add", "--detach", worktree.to_str().unwrap()],
        )
        .await
        .unwrap();
        git(
            &repository,
            &[
                "remote",
                "add",
                "origin",
                "https://token@github.com/example/synthetic.git",
            ],
        )
        .await
        .unwrap();
        assert!(observe(&root, &|_| true).await.is_none());
        observe(&worktree, &|path| path == worktree).await.unwrap()
    });
    assert_eq!(
        section(observed).unwrap().data,
        json!({"commit": commit, "kind": "worktree"})
    );
}

#[test]
fn git_fields_reject_unbounded_or_ambiguous_values() {
    assert!(valid_commit(&"a".repeat(40)));
    assert!(valid_commit(&"f".repeat(64)));
    assert!(!valid_commit(&"A".repeat(40)));
    assert!(!valid_commit(&"a".repeat(63)));
    let bounded = format!("refs/heads/{}\n", "é".repeat(127));
    assert_eq!(branch_name(&bounded).unwrap().len(), 254);
    assert!(branch_name(&format!("refs/heads/{}\n", "é".repeat(128))).is_none());
    assert!(branch_name("refs/heads/one\nrefs/heads/two\n").is_none());

    let expected = Some(Repository {
        owner: "one".into(),
        name: "two".into(),
    });
    for remote in [
        "https://github.com/one/two.git",
        "ssh://git@GITHUB.COM/one/two",
        "git@github.com:one/two.git",
    ] {
        assert_eq!(github_repository(remote), expected, "{remote}");
    }
    for remote in [
        "https://github.com.example/one/two",
        "https://github.com@elsewhere.invalid/one/two",
        "https://token@github.com/one/two",
        "https://github.com/one/two?token=secret",
        "ssh://user@github.com/one/two",
        "git@github.com:one/two/three",
        "git@github.com:one/a%2fb",
        "git@github.com:one/..",
        "/local/one/two",
    ] {
        assert!(github_repository(remote).is_none(), "{remote}");
    }
}

#[cfg(unix)]
#[test]
fn verification_rejects_directory_aliases() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let alias = root.join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    assert!(verified_directory(&root));
    assert!(!verified_directory(&alias));
    assert!(!verified_directory(&root.join("absent")));
}

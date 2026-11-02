use super::*;

#[test]
fn ambiguous_rule_markers_do_not_widen_scope_or_remove_safe_rules() {
    let fixture = Fixture::new();
    let plain = "# Ordinary Markdown\nUse focused tests.\n";
    let conditional = "---\r\npaths: ['src/**/*.rs']\r\n---\r\nUse the Rust test command.\r\n";
    for base in [&fixture.home, &fixture.root] {
        fixture.write(base.join(".claude/rules/plain.md"), plain);
        fixture.write(base.join(".claude/rules/scoped.md"), conditional);
        for (index, marker) in [
            "--- ",
            "---\t",
            "\u{feff}---",
            " \u{feff}---",
            "\n---",
            "--- # header",
        ]
        .iter()
        .enumerate()
        {
            fixture.write(
                base.join(format!(".claude/rules/ambiguous-{index}.md")),
                format!("{marker}\npaths: ['src/**/*.rs']\n---\nDo not widen this rule.\n"),
            );
        }
    }
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    for (root, expected_scope) in [
        (&snapshot.global, &fixture.cwd),
        (&snapshot.repo, &fixture.root),
    ] {
        let entries = root.instructions.as_ref().unwrap();
        assert_eq!(entries.len(), 2);
        let scoped = entries
            .iter()
            .find(|entry| entry.path.ends_with("scoped.md"))
            .unwrap();
        assert_eq!(scoped.content, conditional);
        assert_eq!(scoped.scope.root.as_deref(), expected_scope.to_str());
        assert_eq!(scoped.scope.globs, ["src/**/*.rs"]);
        assert!(
            entries
                .iter()
                .any(|entry| entry.content == plain && entry.scope.globs.is_empty())
        );
        assert!(
            root.instructions_limits
                .contains(&"rule_frontmatter_marker_unsupported".to_owned())
        );
    }
}

#[test]
fn v2_keeps_siblings_and_nested_definitions_beside_valid_or_malformed_root_skills() {
    for (root_content, expected_count) in [
        ("---\nname: root\n---\nBody\n", 4),
        ("---\nname: [unsupported]\n---\nBody\n", 3),
    ] {
        let fixture = Fixture::new();
        fixture.write(
            fixture.root.join("opencode.json"),
            r#"{"skills":["./team-skills"]}"#,
        );
        let source = fixture.cwd.join("team-skills");
        fixture.write(source.join("SKILL.md"), root_content);
        fixture.write(source.join("review.md"), "---\nname: shared\n---\n");
        fixture.write(source.join("release/SKILL.md"), "---\nname: shared\n---\n");
        fixture.write(
            source.join("release/deploy/SKILL.md"),
            "---\nname: deploy\n---\n",
        );
        fixture.write(
            source.join("release/not-a-root-skill.md"),
            "---\nname: excluded\n---\n",
        );
        let snapshot = fixture.collect(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV2);
        let skills = snapshot.repo.skills.unwrap();
        assert_eq!(skills.len(), expected_count);
        assert_eq!(
            skills.iter().filter(|entry| entry.name == "shared").count(),
            2
        );
        assert!(skills.iter().any(|entry| entry.name == "deploy"));
        assert!(skills.iter().all(|entry| entry.name != "excluded"));
        if expected_count == 3 {
            assert!(
                snapshot
                    .repo
                    .skills_limits
                    .contains(&"skill_name_unsupported".to_owned())
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn v2_rejected_root_symlink_does_not_hide_safe_siblings() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let source = fixture.root.join(".opencode/skills");
    fixture.write(
        fixture.home.join("outside/SKILL.md"),
        "---\nname: outside\n---\n",
    );
    fixture.write(source.join("review.md"), "---\nname: review\n---\n");
    fixture.write(source.join("release/SKILL.md"), "---\nname: release\n---\n");
    symlink(
        fixture.home.join("outside/SKILL.md"),
        source.join("SKILL.md"),
    )
    .unwrap();
    let snapshot = fixture.collect(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV2);
    let skills = snapshot.repo.skills.unwrap();
    assert_eq!(skills.len(), 2);
    assert!(skills.iter().any(|entry| entry.name == "review"));
    assert!(skills.iter().any(|entry| entry.name == "release"));
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"symlink_not_read".to_owned())
    );
}

#[test]
fn v2_nested_scans_keep_the_shared_directory_and_record_bounds() {
    let fixture = Fixture::new();
    let source = fixture.root.join(".opencode/skills");
    fixture.write(source.join("00-first.md"), "---\nname: first\n---\n");
    for index in 0..150 {
        fixture.write(
            source.join(format!("group-{index}/SKILL.md")),
            "---\nname: repeated\n---\n",
        );
        fixture.write(
            source.join(format!("group-{index}/nested/SKILL.md")),
            "---\nname: repeated\n---\n",
        );
    }
    let snapshot = fixture.collect(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV2);
    let skills = snapshot.repo.skills.unwrap();
    assert!(skills.iter().any(|entry| entry.name == "first"));
    assert!(skills.len() <= 128);
    assert!(
        snapshot
            .repo
            .skills_limits
            .iter()
            .any(|limit| limit == "directory_work_limit" || limit == "skill_file_limit")
    );
}

#[test]
fn pi_still_stops_at_a_skill_definition() {
    let fixture = Fixture::new();
    let source = fixture.cwd.join(".pi/skills");
    fixture.write(source.join("review.md"), "---\nname: review\n---\n");
    fixture.write(source.join("release/SKILL.md"), "---\nname: release\n---\n");
    fixture.write(
        source.join("release/nested/SKILL.md"),
        "---\nname: nested\n---\n",
    );
    let snapshot = fixture.collect(AgentKind::Pi, SourceFormat::PiV3Jsonl);
    let skills = snapshot.repo.skills.unwrap();
    assert_eq!(skills.len(), 2);
    assert!(skills.iter().all(|entry| entry.name != "nested"));
}

#[cfg(target_os = "macos")]
#[test]
fn configured_protected_targets_need_their_own_recorded_grant() {
    for format in [
        SourceFormat::OpenCodeSqliteV1,
        SourceFormat::OpenCodeSqliteV2,
    ] {
        let mut fixture = Fixture::new();
        let skills = if format == SourceFormat::OpenCodeSqliteV2 {
            serde_json::json!(["~/Documents/team-skills", "~/safe-skills"])
        } else {
            serde_json::json!({"paths": ["~/Documents/team-skills", "~/safe-skills"]})
        };
        fixture.write(fixture.home.join(".config/opencode/opencode.json"), serde_json::json!({
            "instructions": ["~/Documents/private-rules.md", "~/safe-rules.md"], "skills": skills,
        }).to_string());
        fixture.write(
            fixture.home.join("Documents/private-rules.md"),
            "Protected policy\n",
        );
        fixture.write(fixture.home.join("safe-rules.md"), "Safe policy\n");
        fixture.write(
            fixture.home.join("Documents/team-skills/private/SKILL.md"),
            "---\nname: protected\n---\n",
        );
        fixture.write(
            fixture.home.join("safe-skills/safe/SKILL.md"),
            "---\nname: safe\n---\n",
        );
        let denied = fixture.collect(AgentKind::OpenCode, format);
        assert_eq!(denied.global.instructions.as_ref().unwrap().len(), 1);
        assert_eq!(denied.global.skills.as_ref().unwrap()[0].name, "safe");
        assert!(
            denied
                .global
                .instructions_limits
                .contains(&"protected_path_not_granted".to_owned())
        );
        assert!(
            denied
                .global
                .skills_limits
                .contains(&"protected_path_not_granted".to_owned())
        );
        fixture.granted_dirs.insert("Documents".to_owned());
        let granted = fixture.collect(AgentKind::OpenCode, format);
        assert_eq!(granted.global.instructions.unwrap().len(), 2);
        assert_eq!(granted.global.skills.unwrap().len(), 2);
        assert_ne!(
            denied.global.snapshot_revision,
            granted.global.snapshot_revision
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn home_directory_sources_check_protected_children_before_inspection() {
    let fixture = Fixture::new();
    fixture.write(
        fixture.home.join(".config/opencode/opencode.json"),
        r#"{"skills":["~/"]}"#,
    );
    fixture.write(fixture.home.join("safe/SKILL.md"), "---\nname: safe\n---\n");
    fixture.write(
        fixture.home.join("Documents/private/SKILL.md"),
        "---\nname: protected\n---\n",
    );
    let snapshot = fixture.collect(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV2);
    let skills = snapshot.global.skills.unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "safe");
    assert!(
        snapshot
            .global
            .skills_limits
            .contains(&"protected_path_not_granted".to_owned())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn protected_roots_and_cwds_leave_repo_unavailable_but_keep_safe_globals() {
    for root_is_protected in [true, false] {
        let mut fixture = Fixture::new();
        fixture.write(
            fixture.home.join(".claude/CLAUDE.md"),
            "Safe global policy\n",
        );
        let protected = fixture.home.join("Documents/repo");
        fixture.write(protected.join("CLAUDE.md"), "Protected repo policy\n");
        fixture.root = if root_is_protected {
            protected.clone()
        } else {
            fixture.home.clone()
        };
        fixture.cwd = protected;
        let denied = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
        assert_eq!(denied.global.instructions.unwrap().len(), 1);
        assert!(denied.repo.instructions.is_none());
        assert!(denied.repo.skills.is_none());
        assert_eq!(
            denied.repo.instructions_limits,
            ["protected_path_not_granted"]
        );
        fixture.granted_dirs.insert("Documents".to_owned());
        let granted = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
        assert!(
            granted
                .repo
                .instructions
                .unwrap()
                .iter()
                .any(|entry| entry.content == "Protected repo policy\n")
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn denied_files_and_configs_are_rejected_before_missing_path_lookup() {
    let fixture = Fixture::new();
    let mut reads = read::ReadBudget::new(PathAdmission::new(&fixture.home, &fixture.granted_dirs));
    for relative in [
        "Documents/missing.md",
        "Documents/missing/opencode.json",
        "Documents/missing/SKILL.md",
    ] {
        assert_eq!(
            reads.read(&fixture.home.join(relative), &fixture.home, false),
            Err("protected_path_not_granted")
        );
    }
    assert_eq!(reads.files, 253);
    assert_eq!(reads.bytes, 1024 * 1024);
    assert_eq!(
        PathAdmission::new(&fixture.home, &fixture.granted_dirs)
            .canonical_directory(&fixture.home.join("Documents/missing")),
        Err("protected_path_not_granted")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn symlinked_workspace_roots_do_not_bypass_path_admission() {
    use std::os::unix::fs::symlink;
    let mut fixture = Fixture::new();
    fixture.write(
        fixture.home.join(".claude/CLAUDE.md"),
        "Safe global policy\n",
    );
    let protected = fixture.home.join("Documents/repo");
    fixture.write(protected.join("CLAUDE.md"), "Protected policy\n");
    let alias = fixture.home.join("alias");
    symlink(protected, &alias).unwrap();
    fixture.root = alias.clone();
    fixture.cwd = alias;
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    assert_eq!(snapshot.global.instructions.unwrap().len(), 1);
    assert!(snapshot.repo.instructions.is_none());
    assert!(
        snapshot
            .repo
            .instructions_limits
            .contains(&"symlink_not_read".to_owned())
    );
}

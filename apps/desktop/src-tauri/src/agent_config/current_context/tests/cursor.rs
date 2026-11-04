use super::*;

fn write_skill(fixture: &Fixture, path: PathBuf, name: &str, description: &str) {
    fixture.write(
        path,
        format!("---\nname: {name}\ndescription: {description}\n---\nPrivate skill body\n"),
    );
}

#[test]
fn cursor_reads_native_skill_roots_beside_partial_instruction_observations() {
    let fixture = Fixture::new();
    for (base, relative, name) in [
        (&fixture.home, ".cursor/skills/native/SKILL.md", "native"),
        (&fixture.home, ".agents/skills/shared/SKILL.md", "shared"),
        (&fixture.root, ".cursor/skills/repo/SKILL.md", "repo"),
        (&fixture.root, ".agents/skills/common/SKILL.md", "common"),
        (&fixture.cwd, ".cursor/skills/package/SKILL.md", "package"),
    ] {
        write_skill(
            &fixture,
            base.join(relative),
            name,
            "Useful declared guidance",
        );
    }
    write_skill(
        &fixture,
        fixture.root.join(".claude/skills/compat/SKILL.md"),
        "compat",
        "Not in this subset",
    );
    write_skill(
        &fixture,
        fixture
            .root
            .join("other/.cursor/skills/descendant/SKILL.md"),
        "descendant",
        "Outside the CWD ancestry",
    );
    fixture.write(
        fixture.root.join("AGENTS.md"),
        "Current repository instruction",
    );
    fixture.write(
        fixture.root.join(".cursor/rules/review.mdc"),
        "Rule text is not admitted here",
    );

    let snapshot = fixture.collect(AgentKind::Cursor, SourceFormat::CursorIdeComposer);
    assert_eq!(snapshot.global.skills.as_ref().unwrap().len(), 2);
    let repo = snapshot.repo.skills.as_ref().unwrap();
    assert_eq!(repo.len(), 3);
    assert!(
        repo.iter()
            .all(|entry| entry.description == "Useful declared guidance")
    );
    assert!(
        repo.iter()
            .all(|entry| Path::new(&entry.path).is_absolute())
    );
    assert!(snapshot.global.instructions.is_none());
    let instructions = snapshot.repo.instructions.as_ref().unwrap();
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].content, "Current repository instruction");
    assert!(
        snapshot
            .global
            .instructions_limits
            .contains(&"cursor_instruction_sources_not_read".to_owned())
    );
    assert!(
        snapshot
            .repo
            .instructions_limits
            .contains(&"cursor_rules_user_team_and_activation_controls_not_resolved".to_owned())
    );
    for scope in [&snapshot.global, &snapshot.repo] {
        assert!(!scope.instructions_complete);
        assert!(!scope.skills_complete);
        assert!(
            scope.skills_limits.contains(
                &"cursor_compatibility_plugins_and_skill_controls_not_resolved".to_owned()
            )
        );
        assert!(scope.snapshot_revision.is_some());
        assert!(
            !scope
                .skills_limits
                .contains(&"unsupported_agent_source".to_owned())
        );
    }
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"cursor_nested_project_skills_not_read".to_owned())
    );
}

#[test]
fn cursor_current_agents_files_keep_directory_scope_and_snapshot_identity() {
    let fixture = Fixture::new();
    fixture.write(fixture.root.join("AGENTS.md"), "Root instruction\n");
    fixture.write(fixture.cwd.join("AGENTS.md"), "Package instruction\n");
    for path in ["other/AGENTS.md", ".cursor/rules/rule.mdc", "CLAUDE.md"] {
        fixture.write(fixture.root.join(path), "Outside the observed subset\n");
    }
    let snapshot = fixture.collect(AgentKind::Cursor, SourceFormat::CursorIdeComposer);
    let entries = snapshot.repo.instructions.as_ref().unwrap();
    assert_eq!(entries.len(), 2);
    for (path, scope) in [
        (fixture.root.join("AGENTS.md"), &fixture.root),
        (fixture.cwd.join("AGENTS.md"), &fixture.cwd),
    ] {
        let entry = entries
            .iter()
            .find(|entry| Path::new(&entry.path) == path)
            .unwrap();
        assert_eq!(entry.scope.root.as_deref(), scope.to_str());
        assert!(entry.scope.globs.is_empty());
        assert_eq!(entry.observed_at, "2026-10-23T12:00:00Z");
    }
    assert!(!snapshot.repo.instructions_complete);
    fixture.write(
        fixture.cwd.join("AGENTS.md"),
        "Changed current instruction\n",
    );
    let changed = fixture.collect(AgentKind::Cursor, SourceFormat::CursorIdeComposer);
    assert_ne!(
        snapshot.repo.snapshot_revision,
        changed.repo.snapshot_revision
    );
}

#[test]
fn cursor_walks_nested_skill_folders_without_reading_loose_markdown() {
    let fixture = Fixture::new();
    write_skill(
        &fixture,
        fixture.root.join(".cursor/skills/parent/SKILL.md"),
        "parent",
        "Parent description",
    );
    write_skill(
        &fixture,
        fixture.root.join(".cursor/skills/parent/child/SKILL.md"),
        "child",
        "Child description",
    );
    write_skill(
        &fixture,
        fixture.root.join(".cursor/skills/loose.md"),
        "loose",
        "Not a SKILL.md definition",
    );
    let snapshot = fixture.collect(AgentKind::Cursor, SourceFormat::CursorIdeComposer);
    let entries = snapshot.repo.skills.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| entry.name == "parent"));
    assert!(entries.iter().any(|entry| entry.name == "child"));
}

#[test]
fn cursor_keeps_safe_descriptions_beside_malformed_and_overdeep_definitions() {
    let fixture = Fixture::new();
    let mut bytes = b"---\nname: good\ndescription: Declared metadata only\n---\n".to_vec();
    bytes.extend(vec![0xff; 1024 * 1024]);
    fixture.write(fixture.root.join(".cursor/skills/good/SKILL.md"), bytes);
    fixture.write(
        fixture.root.join(".cursor/skills/bad/SKILL.md"),
        "---\nname: bad\ndescription: [not, text]\n---\n",
    );
    write_skill(
        &fixture,
        fixture
            .root
            .join(".cursor/skills/a/b/c/d/e/f/g/deep/SKILL.md"),
        "deep",
        "Beyond the bounded walk",
    );
    let snapshot = fixture.collect(AgentKind::Cursor, SourceFormat::CursorIdeComposer);
    let entries = snapshot.repo.skills.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].description, "Declared metadata only");
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"skill_description_unsupported".to_owned())
    );
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"skill_depth_limit".to_owned())
    );
}

#[test]
fn cursor_known_empty_skills_are_distinct_from_unavailable_scopes() {
    let fixture = Fixture::new();
    let snapshot = fixture.collect(AgentKind::Cursor, SourceFormat::CursorIdeComposer);
    assert_eq!(snapshot.global.skills, Some(Vec::new()));
    assert_eq!(snapshot.repo.skills, Some(Vec::new()));
    assert_eq!(snapshot.repo.instructions, Some(Vec::new()));
    assert!(!snapshot.repo.instructions_complete);
    let mut request = fixture.request(AgentKind::Cursor, SourceFormat::CursorIdeComposer);
    request.repo_root = None;
    let unavailable = collect_current_context(request);
    assert_eq!(unavailable.global.skills, Some(Vec::new()));
    assert!(unavailable.repo.skills.is_none());
    assert!(unavailable.repo.instructions.is_none());
    assert!(
        unavailable
            .repo
            .skills_limits
            .contains(&"workspace_unavailable".to_owned())
    );
}

#[test]
fn cursor_inventory_requires_its_native_desktop_source_pair() {
    let fixture = Fixture::new();
    write_skill(
        &fixture,
        fixture.root.join(".cursor/skills/repo/SKILL.md"),
        "repo",
        "Do not cross source identities",
    );
    for (agent, format) in [
        (AgentKind::Cursor, SourceFormat::ClaudeJsonl),
        (AgentKind::Claude, SourceFormat::CursorIdeComposer),
    ] {
        let snapshot = fixture.collect(agent, format);
        assert!(snapshot.global.skills.is_none());
        assert!(snapshot.repo.skills.is_none());
    }
    let mut request = fixture.request(AgentKind::Cursor, SourceFormat::CursorIdeComposer);
    request.native_environment = false;
    let snapshot = collect_current_context(request);
    assert!(snapshot.global.skills.is_none());
    assert!(snapshot.repo.skills.is_none());
    assert_eq!(snapshot.repo.skills_limits, ["non_native_environment"]);
}

#[cfg(unix)]
#[test]
fn cursor_does_not_follow_skill_directory_symlinks() {
    let fixture = Fixture::new();
    let outside = fixture._temp.path().join("outside");
    write_skill(
        &fixture,
        outside.join("SKILL.md"),
        "outside",
        "Must not be observed",
    );
    fs::create_dir_all(fixture.root.join(".cursor/skills")).unwrap();
    std::os::unix::fs::symlink(&outside, fixture.root.join(".cursor/skills/linked")).unwrap();
    write_skill(
        &fixture,
        fixture.root.join(".cursor/skills/safe/SKILL.md"),
        "safe",
        "Safe sibling",
    );
    let snapshot = fixture.collect(AgentKind::Cursor, SourceFormat::CursorIdeComposer);
    let entries = snapshot.repo.skills.unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "safe");
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"symlink_not_read".to_owned())
    );
}

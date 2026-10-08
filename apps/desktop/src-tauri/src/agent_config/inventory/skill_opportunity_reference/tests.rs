use super::*;
use antiburn_local::model::AgentKind;

fn roots() -> (tempfile::TempDir, ConfigContext) {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let project = temporary.path().join("project");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&project).unwrap();
    let context = ConfigContext::native(AgentKind::Claude, home, Some(project));
    (temporary, context)
}

fn write(path: &Path, value: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value).unwrap();
}

#[test]
fn semantic_frontmatter_preserves_multiline_descriptions_metadata_and_comments() {
    let (name, description, fields) = parse_frontmatter(
        br#"---
# Private comment is not a semantic field.
name: code-review # inline comment
description: >-
  Review code for defects,
  including resource ownership.
metadata:
  version: 2
  supported: [rust, typescript]
allowed-tools: "Read, Bash"
---
Private skill body.
"#,
        "directory-alias",
    )
    .unwrap();
    assert_eq!(name, "code-review");
    assert_eq!(
        description,
        "Review code for defects, including resource ownership."
    );
    assert_eq!(fields["metadata"]["version"], 2);
    assert_eq!(fields["metadata"]["supported"][1], "typescript");
    assert!(!fields.to_string().contains("Private"));
    let (_, literal, _) = parse_frontmatter(
        b"---\ndescription: |\n  First line.\n  Second line.\n---\n",
        "review",
    )
    .unwrap();
    assert_eq!(literal, "First line.\nSecond line.");
    let (_, quoted, _) = parse_frontmatter(
        b"---\ndescription: 'Read # values: preserve them.' # comment\n---\n",
        "review",
    )
    .unwrap();
    assert_eq!(quoted, "Read # values: preserve them.");
}

#[test]
fn malformed_duplicate_and_oversized_shapes_are_unavailable() {
    for text in [
        "---\ndescription: []\n---\n",
        "---\ndescription: one\ndescription: two\n---\n",
        "---\ndescription: unfinished\n",
    ] {
        assert!(
            parse_frontmatter(text.as_bytes(), "review").is_none(),
            "{text}"
        );
    }
    let large = format!(
        "---\ndescription: {}\n---\n",
        "x".repeat(MAX_SKILL_DESCRIPTION_BYTES + 1)
    );
    assert!(parse_frontmatter(large.as_bytes(), "review").is_none());
}

#[test]
fn snapshot_is_immutable_source_bound_and_has_no_filesystem_paths_in_identity() {
    let (_temporary, context) = roots();
    let path = context.home_root.join(".claude/skills/review/SKILL.md");
    write(
        &path,
        "---\nname: review\ndescription: Review code.\nmetadata:\n  version: 1\n---\nPrivate body.\n",
    );
    let first = skill_opportunity_snapshot(&context).unwrap();
    let definition = &first.skills()[0];
    assert_eq!(definition.description, "Review code.");
    assert_eq!(definition.identity.len(), 64);
    assert_eq!(definition.revision.len(), 64);
    assert!(
        !serde_json::to_string(definition)
            .unwrap()
            .contains(context.home_root.to_str().unwrap())
    );
    let expected_birth = std::fs::metadata(&path)
        .unwrap()
        .created()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| i64::try_from(duration.as_millis()).ok());
    assert_eq!(definition.created_at_ms, expected_birth);
    write(
        &path,
        "---\n# New comment.\nname: review\ndescription: Review code. # semantic value is unchanged\nmetadata:\n  version: 1\n---\nDifferent body.\n",
    );
    let comments = skill_opportunity_snapshot(&context).unwrap();
    assert_eq!(definition.identity, comments.skills()[0].identity);
    assert_eq!(definition.revision, comments.skills()[0].revision);
    write(
        &path,
        "---\nname: review\ndescription: Review code.\nmetadata:\n  version: 2\n---\n",
    );
    assert_ne!(
        definition.revision,
        skill_opportunity_snapshot(&context).unwrap().skills()[0].revision
    );
    write(
        &path,
        "---\nname: review\ndescription: Review security.\nmetadata:\n  version: 2\n---\n",
    );
    let changed = skill_opportunity_snapshot(&context).unwrap();
    assert_eq!(definition.identity, changed.skills()[0].identity);
    assert_ne!(definition.revision, changed.skills()[0].revision);
    assert_eq!(first.skills()[0].description, "Review code.");
    std::fs::remove_file(&path).unwrap();
    assert_eq!(first.skills().len(), 1);
}

#[test]
fn full_description_is_not_cut_to_the_existing_aggregate_name_limit() {
    let description = "Review the resource ownership and error paths. ".repeat(40);
    let text = format!("---\r\nname: review\r\ndescription: \"{description}\"\r\n---\r\n");
    let (_, parsed, _) = parse_frontmatter(text.as_bytes(), "review").unwrap();
    assert_eq!(parsed, description.trim());
    assert!(parsed.len() > 256);
}

#[test]
fn missing_null_blank_descriptions_and_plain_markdown_select_full_markdown() {
    for text in [
        "---\n---\nFALLBACK_BODY_MARKER\n",
        "---\nname: review\n---\nFALLBACK_BODY_MARKER\n",
        "---\ndescription: null\n---\nFALLBACK_BODY_MARKER\n",
        "---\ndescription: '  '\n---\nFALLBACK_BODY_MARKER\n",
        "# Review\n\nFALLBACK_BODY_MARKER\n",
    ] {
        let (_, reference, _) = parse_frontmatter(text.as_bytes(), "review").unwrap();
        assert_eq!(reference, text);
    }
    for text in [
        "---\ndescription: 42\n---\nBODY\n",
        "---\ndescription: true\n---\nBODY\n",
        "---\ndescription: [broken\n---\nBODY\n",
    ] {
        assert!(parse_frontmatter(text.as_bytes(), "review").is_none());
    }
}

#[test]
fn fallback_body_changes_revision_and_large_markdown_remains_eligible() {
    let (_temporary, context) = roots();
    let path = context.home_root.join(".claude/skills/review/SKILL.md");
    write(&path, "---\nname: review\n---\nOriginal fallback.\n");
    let first = skill_opportunity_snapshot(&context).unwrap();
    write(&path, "---\nname: review\n---\nChanged fallback.\n");
    let changed = skill_opportunity_snapshot(&context).unwrap();
    assert_ne!(first.skills()[0].revision, changed.skills()[0].revision);
    let large = "# Review\n\nReview resource ownership.\n".repeat(1000);
    write(&path, &large);
    let snapshot = skill_opportunity_snapshot(&context).unwrap();
    assert_eq!(snapshot.skills()[0].description, large);
}

#[test]
fn disabled_semantic_aliases_and_dynamic_environments_are_not_enabled() {
    let (_temporary, mut context) = roots();
    write(
        &context.home_root.join(".claude/skills/folder/SKILL.md"),
        "---\nname: semantic-name\ndescription: Review code.\n---\n",
    );
    write(
        &context.home_root.join(".claude/settings.json"),
        r#"{"skillOverrides":{"semantic-name":"off"}}"#,
    );
    let disabled = skill_opportunity_snapshot(&context).unwrap();
    assert_eq!(disabled.skills()[0].aliases, vec!["folder"]);
    assert!(!disabled.skills()[0].enabled);
    write(&context.home_root.join(".claude/settings.json"), "{}");
    context.runtime_override_present = true;
    assert!(!skill_opportunity_snapshot(&context).unwrap().skills()[0].enabled);
    context.runtime_override_present = false;
    context.native_environment = false;
    assert_eq!(
        skill_opportunity_snapshot(&context),
        Err(SkillSnapshotError::Config(
            ConfigUnavailableReason::UnsupportedEnvironment
        ))
    );
}

#[test]
fn discovery_binds_agent_project_and_environment() {
    let (_temporary, context) = roots();
    write(
        &context.home_root.join(".claude/skills/global/SKILL.md"),
        "---\ndescription: Global review.\n---\n",
    );
    write(
        &context
            .workspace_cwd
            .as_ref()
            .unwrap()
            .join(".claude/skills/project/SKILL.md"),
        "---\ndescription: Project review.\n---\n",
    );
    let snapshot = skill_opportunity_snapshot(&context).unwrap();
    assert_eq!(snapshot.skills().len(), 2);
    let mut other_agent = context.clone();
    other_agent.agent = AgentKind::Codex;
    assert!(
        skill_opportunity_snapshot(&other_agent)
            .unwrap()
            .skills()
            .is_empty()
    );
    let other_project = context.home_root.parent().unwrap().join("other-project");
    std::fs::create_dir(&other_project).unwrap();
    let other = ConfigContext::native(AgentKind::Claude, &context.home_root, Some(other_project));
    let other_snapshot = skill_opportunity_snapshot(&other).unwrap();
    assert_eq!(other_snapshot.skills().len(), 1);
    assert_ne!(
        snapshot.scope().project_identity,
        other_snapshot.scope().project_identity
    );
    assert_eq!(
        snapshot.scope().environment_identity,
        other_snapshot.scope().environment_identity
    );
    let other_home = context.home_root.parent().unwrap().join("other-home");
    std::fs::create_dir(&other_home).unwrap();
    let other = ConfigContext::native(AgentKind::Claude, other_home, None);
    assert_ne!(
        snapshot.scope().environment_identity,
        skill_opportunity_snapshot(&other)
            .unwrap()
            .scope()
            .environment_identity
    );
}

#[test]
fn advisory_inventory_does_not_retain_or_serialize_new_definitions() {
    let (_temporary, context) = roots();
    write(
        &context.home_root.join(".claude/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review code.\n---\n",
    );
    let builder = discover_inventory(&context, [], false).unwrap();
    assert!(builder.skill_definitions.is_empty());
    let inventory = builder.finish();
    assert_eq!(
        inventory
            .resources
            .iter()
            .filter(|resource| resource.kind == ResourceKind::Skill)
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn metadata_identity_rejects_replacement_and_symlinked_definitions() {
    let (_temporary, context) = roots();
    let path = context.home_root.join(".claude/skills/review/SKILL.md");
    write(&path, "---\ndescription: Review code.\n---\n");
    let before = std::fs::metadata(&path).unwrap();
    std::fs::rename(&path, path.with_extension("old")).unwrap();
    write(&path, "---\ndescription: Review code.\n---\n");
    assert!(!same_file(&before, &std::fs::metadata(&path).unwrap()));
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink(path.with_extension("old"), &path).unwrap();
    assert!(read_definition(&path, &context.home_root).is_err());
    assert!(
        skill_opportunity_snapshot(&context)
            .unwrap()
            .skills()
            .is_empty()
    );
}

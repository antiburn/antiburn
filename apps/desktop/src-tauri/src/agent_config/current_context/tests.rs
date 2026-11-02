use super::*;
use std::fs;

mod paths;
mod protected_case_aliases;
mod review;

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    root: PathBuf,
    cwd: PathBuf,
    granted_dirs: HashSet<String>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let root = temp.path().join("repo");
        let cwd = root.join("package");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        Self {
            _temp: temp,
            home: home.canonicalize().unwrap(),
            root: root.canonicalize().unwrap(),
            cwd: cwd.canonicalize().unwrap(),
            granted_dirs: HashSet::new(),
        }
    }

    fn write(&self, path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) {
        fs::create_dir_all(path.as_ref().parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn request(&self, agent: AgentKind, format: SourceFormat) -> CurrentContextRequest<'_> {
        CurrentContextRequest {
            agent,
            source_format: Some(format),
            home: &self.home,
            granted_dirs: &self.granted_dirs,
            cwd: Some(&self.cwd),
            repo_root: Some(&self.root),
            native_environment: true,
            runtime_override_present: false,
            managed_configuration_present: false,
        }
    }

    fn collect(&self, agent: AgentKind, format: SourceFormat) -> CurrentContextSnapshot {
        collect_at(
            self.request(agent, format),
            "2026-10-23T12:00:00Z".to_owned(),
        )
    }
}

#[test]
fn claude_keeps_known_policy_beside_malformed_or_unrepresentable_rules() {
    let fixture = Fixture::new();
    fixture.write(fixture.home.join(".claude/CLAUDE.md"), "Global rule\n");
    fixture.write(fixture.root.join("CLAUDE.md"), "Repo rule\n");
    fixture.write(
        fixture.root.join(".claude/rules/api.md"),
        "---\npaths: [src/**/*.rs]\n---\nScoped rule\n",
    );
    fixture.write(
        fixture.root.join(".claude/rules/unknown.md"),
        "---\npaths: ['src/*.{rs,ts}']\n---\nDo not broaden\n",
    );
    fixture.write(
        fixture.root.join(".claude/rules/broken.md"),
        "---\npaths: [\n---\nBroken\n",
    );
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    assert_eq!(snapshot.global.instructions.as_ref().unwrap().len(), 1);
    assert!(!snapshot.global.instructions_complete);
    let entries = snapshot.repo.instructions.unwrap();
    assert_eq!(entries.len(), 2);
    let scoped = entries
        .iter()
        .find(|entry| entry.path.ends_with("api.md"))
        .unwrap();
    assert_eq!(scoped.scope.root.as_deref(), fixture.root.to_str());
    assert_eq!(scoped.scope.globs, ["src/**/*.rs"]);
    assert_eq!(
        scoped.content,
        "---\npaths: [src/**/*.rs]\n---\nScoped rule\n"
    );
    assert_eq!(scoped.observed_at, "2026-10-23T12:00:00Z");
    assert!(
        snapshot
            .repo
            .instructions_limits
            .contains(&"conditional_scope_unsupported".to_owned())
    );
    assert!(
        snapshot
            .repo
            .instructions_limits
            .contains(&"rule_frontmatter_invalid".to_owned())
    );
}

#[test]
fn global_conditional_rules_use_the_exact_session_cwd() {
    let fixture = Fixture::new();
    fixture.write(
        fixture.home.join(".claude/rules/scoped.md"),
        "---\npaths: '**/*.rs'\n---\nUse safe Rust\n",
    );
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    let scoped = &snapshot.global.instructions.unwrap()[0];
    assert_eq!(scoped.scope.root.as_deref(), fixture.cwd.to_str());
    assert_eq!(scoped.scope.globs, ["**/*.rs"]);
    let mut request = fixture.request(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    request.cwd = None;
    let absent_scope = collect_current_context(request);
    assert!(absent_scope.global.instructions.unwrap().is_empty());
    assert!(
        absent_scope
            .global
            .instructions_limits
            .contains(&"conditional_scope_root_unavailable".to_owned())
    );
    assert!(absent_scope.repo.instructions.is_none());
}

#[test]
fn changed_files_change_revision_without_mutating_a_saved_snapshot() {
    let fixture = Fixture::new();
    fixture.write(fixture.root.join("CLAUDE.md"), "Before\r\n");
    let before = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    assert!(before.repo.snapshot_revision.is_some());
    let saved = serde_json::to_value(&before).unwrap();
    assert_eq!(
        before,
        fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl)
    );
    fixture.write(fixture.root.join("CLAUDE.md"), "After\n");
    let after = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    assert!(after.repo.snapshot_revision.is_some());
    assert_ne!(before.repo.snapshot_revision, after.repo.snapshot_revision);
    assert_eq!(serde_json::to_value(&before).unwrap(), saved);
    assert_eq!(before.repo.instructions.unwrap()[0].content, "Before\r\n");
    assert_eq!(after.repo.instructions.unwrap()[0].content, "After\n");
}

#[test]
fn skills_keep_duplicate_names_missing_descriptions_and_exact_paths() {
    let fixture = Fixture::new();
    for base in [&fixture.root, &fixture.cwd] {
        fixture.write(
            base.join(".claude/skills/review/SKILL.md"),
            "---\nname: review\n---\nBody is not input\n",
        );
    }
    fixture.write(
        fixture.home.join(".claude/skills/review/SKILL.md"),
        "---\nname: review\ndescription: ' Exact description '\n---\nBody\n",
    );
    fixture.write(
        fixture.root.join(".claude/skills/bad/SKILL.md"),
        "---\ndescription: [unsupported]\n---\nBody\n",
    );
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    let entries = snapshot.repo.skills.unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|entry| entry.name == "review"
        && entry.description.is_empty()
        && Path::new(&entry.path).is_absolute()));
    assert_ne!(entries[0].path, entries[1].path);
    assert_eq!(
        snapshot.global.skills.unwrap()[0].description,
        " Exact description "
    );
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"skill_description_unsupported".to_owned())
    );
}

#[test]
fn skill_bodies_are_not_read_even_when_large_or_not_utf8() {
    let fixture = Fixture::new();
    let mut bytes = b"---\nname: review\ndescription: Check code\n---\n".to_vec();
    bytes.extend(vec![0xff; 1024 * 1024]);
    fixture.write(fixture.root.join(".agents/skills/review/SKILL.md"), bytes);
    let snapshot = fixture.collect(AgentKind::Codex, SourceFormat::CodexRolloutJsonl);
    assert_eq!(snapshot.repo.skills.unwrap()[0].description, "Check code");
}

#[test]
fn source_and_environment_mismatch_leave_collections_absent() {
    let fixture = Fixture::new();
    fixture.write(
        fixture.root.join("AGENTS.md"),
        "Do not expose for wrong source\n",
    );
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::OpenCodeSqliteV2);
    assert!(snapshot.global.skills.is_none());
    assert!(snapshot.repo.instructions.is_none());
    let mut request = fixture.request(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV2);
    request.native_environment = false;
    let snapshot = collect_current_context(request);
    assert!(snapshot.repo.instructions.is_none());
    assert_eq!(
        snapshot.repo.instructions_limits,
        ["non_native_environment"]
    );
}

#[test]
fn opencode_v2_uses_v2_arrays_and_does_not_load_claude_fallback() {
    let fixture = Fixture::new();
    fixture.write(fixture.root.join("CLAUDE.md"), "V1 fallback only\n");
    fixture.write(
        fixture.home.join(".claude/CLAUDE.md"),
        "V1 global fallback only\n",
    );
    fixture.write(fixture.root.join("opencode.jsonc"), b"{\n// V2 config\n\"skills\": [\"./team-skills\", 4], \"instructions\": [\"guide.md\", \"https://example.test/rules\"],\n}");
    fixture.write(fixture.cwd.join("guide.md"), "Package guide\n");
    fixture.write(
        fixture.cwd.join("team-skills/review.md"),
        "---\nname: Review code\ndescription: Review changes\n---\nBody\n",
    );
    fixture.write(
        fixture.cwd.join("team-skills/group/release/SKILL.md"),
        "---\nname: Release\n---\nBody\n",
    );
    let v2 = fixture.collect(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV2);
    assert!(v2.global.instructions.unwrap().is_empty());
    let instructions = v2.repo.instructions.unwrap();
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].content, "Package guide\n");
    assert_eq!(v2.repo.skills.unwrap().len(), 2);
    assert!(
        v2.repo
            .skills_limits
            .contains(&"configured_source_unsupported".to_owned())
    );
    assert!(
        v2.repo
            .instructions_limits
            .contains(&"remote_source_not_read".to_owned())
    );
    let v1 = fixture.collect(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV1);
    assert_eq!(v1.global.instructions.unwrap().len(), 1);
    assert!(
        v1.repo
            .instructions
            .unwrap()
            .iter()
            .any(|entry| entry.path.ends_with("CLAUDE.md"))
    );
    assert!(v1.repo.skills.unwrap().is_empty());
    assert!(
        v1.repo
            .skills_limits
            .contains(&"opencode_v1_skills_shape_unsupported".to_owned())
    );
}

#[test]
fn opencode_v1_skill_paths_are_not_interpreted_as_v2_sources() {
    let fixture = Fixture::new();
    fixture.write(
        fixture.root.join("opencode.json"),
        r#"{"skills":{"paths":["./team-skills"]}}"#,
    );
    fixture.write(
        fixture.cwd.join("team-skills/review/SKILL.md"),
        "---\nname: review\n---\nBody\n",
    );
    assert_eq!(
        fixture
            .collect(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV1)
            .repo
            .skills
            .unwrap()
            .len(),
        1
    );
    let v2 = fixture.collect(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV2);
    assert!(v2.repo.skills.unwrap().is_empty());
    assert!(
        v2.repo
            .skills_limits
            .contains(&"configured_sources_shape_unsupported".to_owned())
    );
}

#[test]
fn bounded_scans_keep_entries_and_report_limits() {
    let fixture = Fixture::new();
    for index in 0..300 {
        fixture.write(
            fixture
                .root
                .join(format!(".claude/skills/skill-{index}/SKILL.md")),
            "---\nname: repeated\n---\nBody\n",
        );
    }
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    let skills = snapshot.repo.skills.unwrap();
    assert!(!skills.is_empty());
    assert!(skills.len() <= 128);
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"directory_entry_limit".to_owned())
    );
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"skill_file_limit".to_owned())
    );
}

#[test]
fn instruction_bytes_and_skill_headers_are_bounded_independently() {
    let fixture = Fixture::new();
    fixture.write(
        fixture.root.join("CLAUDE.md"),
        "x".repeat(read::MAX_FILE_BYTES + 1),
    );
    fixture.write(
        fixture.root.join(".claude/skills/large/SKILL.md"),
        format!("---\nname: {}\n---\n", "x".repeat(read::MAX_HEADER_BYTES)),
    );
    fixture.write(
        fixture.root.join(".claude/skills/good/SKILL.md"),
        "---\nname: good\n---\n",
    );
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    assert!(snapshot.repo.instructions.unwrap().is_empty());
    assert_eq!(snapshot.repo.skills.unwrap()[0].name, "good");
    assert!(
        snapshot
            .repo
            .instructions_limits
            .contains(&"file_byte_limit".to_owned())
    );
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"skill_frontmatter_byte_limit".to_owned())
    );
}

#[cfg(unix)]
#[test]
fn symlinks_do_not_expand_the_read_roots_or_remove_safe_siblings() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    fixture.write(
        fixture.home.join("outside/SKILL.md"),
        "---\nname: outside\n---\n",
    );
    fixture.write(
        fixture.root.join(".claude/skills/safe/SKILL.md"),
        "---\nname: safe\n---\n",
    );
    symlink(
        fixture.home.join("outside"),
        fixture.root.join(".claude/skills/linked"),
    )
    .unwrap();
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    assert_eq!(snapshot.repo.skills.unwrap()[0].name, "safe");
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"symlink_not_read".to_owned())
    );
}

#[test]
fn request_adapter_preserves_the_exact_config_context_paths() {
    let fixture = Fixture::new();
    let mut context = ConfigContext::native_workspace(
        AgentKind::OpenCode,
        &fixture.home,
        &fixture.cwd,
        &fixture.root,
    );
    context.source_format = Some(SourceFormat::OpenCodeSqliteV2);
    let request = CurrentContextRequest::from_context(&context, &fixture.granted_dirs);
    assert_eq!(request.home, fixture.home);
    assert_eq!(request.cwd, Some(fixture.cwd.as_path()));
    assert_eq!(request.repo_root, Some(fixture.root.as_path()));
    assert_eq!(request.source_format, context.source_format);
    assert!(std::ptr::eq(request.granted_dirs, &fixture.granted_dirs));
}

#[test]
fn serialized_inventory_matches_the_core_owner_contract() {
    let fixture = Fixture::new();
    fixture.write(fixture.root.join("CLAUDE.md"), "Repo policy\n");
    fixture.write(
        fixture.root.join(".claude/skills/review/SKILL.md"),
        "---\nname: review\n---\n",
    );
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    let json = serde_json::to_value(&snapshot).unwrap();
    let core: antiburn_checks::context::CurrentInventories = serde_json::from_value(json).unwrap();
    assert_eq!(core.repo.snapshot_revision, snapshot.repo.snapshot_revision);
    assert_eq!(
        core.repo.instructions.as_ref().unwrap()[0].content,
        "Repo policy\n"
    );
    assert_eq!(core.repo.skills.unwrap()[0].description, "");
    assert!(!core.repo.instructions_complete);
}

#[test]
fn total_reads_stay_bounded_without_removing_an_earlier_known_policy() {
    let fixture = Fixture::new();
    fixture.write(fixture.root.join("CLAUDE.md"), "Known policy\n");
    for index in 0..12 {
        fixture.write(
            fixture.root.join(format!(".claude/rules/rule-{index}.md")),
            "x".repeat(read::MAX_FILE_BYTES),
        );
    }
    let snapshot = fixture.collect(AgentKind::Claude, SourceFormat::ClaudeJsonl);
    let entries = snapshot.repo.instructions.unwrap();
    assert!(
        entries
            .iter()
            .any(|entry| entry.content == "Known policy\n")
    );
    assert!(
        entries
            .iter()
            .map(|entry| entry.content.len())
            .sum::<usize>()
            <= 1024 * 1024
    );
    assert!(
        snapshot
            .repo
            .instructions_limits
            .contains(&"file_byte_limit".to_owned())
    );
}

#[test]
fn pi_keeps_known_agent_and_repo_files_without_claiming_package_sources() {
    let fixture = Fixture::new();
    fixture.write(
        fixture.home.join(".pi/agent/AGENTS.md"),
        "Global guidance\n",
    );
    fixture.write(fixture.root.join("AGENTS.md"), "Root guidance\n");
    fixture.write(
        fixture.cwd.join(".pi/APPEND_SYSTEM.md"),
        "Append guidance\n",
    );
    fixture.write(
        fixture.cwd.join(".pi/skills/review/SKILL.md"),
        "---\nname: review\n---\n",
    );
    let snapshot = fixture.collect(AgentKind::Pi, SourceFormat::PiV3Jsonl);
    assert_eq!(snapshot.global.instructions.unwrap().len(), 1);
    assert_eq!(snapshot.repo.instructions.unwrap().len(), 2);
    assert_eq!(snapshot.repo.skills.unwrap()[0].name, "review");
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"pi_settings_packages_and_runtime_sources_not_resolved".to_owned())
    );
}

#[cfg(unix)]
#[test]
fn a_non_utf8_conditional_root_is_not_replaced_with_global_scope() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let cwd = fixture.root.join(std::ffi::OsString::from_vec(vec![0xff]));
    assert_eq!(
        super::instructions::instruction_scope(true, Some(&cwd), vec!["**/*.rs".to_owned()]),
        Err("scope_root_unavailable")
    );
}

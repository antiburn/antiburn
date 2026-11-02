use super::*;

fn unjoined_path(root: &Path, parts: &[&str]) -> PathBuf {
    let mut path = root.as_os_str().to_owned();
    for part in parts {
        path.push(std::path::MAIN_SEPARATOR_STR);
        path.push(part);
    }
    path.into()
}

#[test]
fn canonical_directories_accept_existing_paths_but_reject_unsafe_paths() {
    let fixture = Fixture::new();
    let admission = PathAdmission::new(&fixture.home, &fixture.granted_dirs);
    for path in [&fixture.home, &fixture.root, &fixture.cwd] {
        assert_eq!(admission.canonical_directory(path), Ok(path.clone()));
    }
    for path in [
        PathBuf::from("relative"),
        unjoined_path(&fixture.root, &["package", ".."]),
    ] {
        assert_eq!(
            admission.canonical_directory(&path),
            Err("path_outside_allowed_root")
        );
    }
    fixture.write(fixture.root.join("file"), "Not a directory\n");
    for path in [
        fixture.root.join("missing"),
        fixture.root.join("file"),
        fixture.root.join("file/child"),
    ] {
        assert_eq!(
            admission.canonical_directory(&path),
            Err("directory_unavailable")
        );
    }
    let invalid_home = unjoined_path(&fixture.home, &["..", "home"]);
    assert_eq!(
        PathAdmission::new(&invalid_home, &fixture.granted_dirs).canonical_directory(&fixture.root),
        Err("home_unavailable")
    );
}

#[test]
fn unsafe_workspaces_keep_safe_globals_without_repo_snapshots() {
    let fixture = Fixture::new();
    fixture.write(fixture.home.join(".claude/CLAUDE.md"), "Safe global\n");
    fixture.write(fixture.root.join("CLAUDE.md"), "Repo policy\n");
    let parent = unjoined_path(&fixture.root, &["package", ".."]);
    let sibling = fixture.root.with_file_name("repo-copy");
    fs::create_dir(&sibling).unwrap();
    for (root, cwd, reason) in [
        (&parent, &fixture.cwd, "path_outside_allowed_root"),
        (&fixture.root, &parent, "path_outside_allowed_root"),
        (&fixture.root, &sibling, "workspace_unavailable"),
    ] {
        let mut request = fixture.request(AgentKind::Claude, SourceFormat::ClaudeJsonl);
        request.repo_root = Some(root);
        request.cwd = Some(cwd);
        let snapshot = collect_current_context(request);
        assert_eq!(
            snapshot.global.instructions.unwrap()[0].content,
            "Safe global\n"
        );
        assert!(snapshot.repo.instructions.is_none());
        assert!(snapshot.repo.skills.is_none());
        assert!(snapshot.repo.snapshot_revision.is_none());
        assert_eq!(snapshot.repo.instructions_limits, [reason]);
    }
}

#[test]
fn rejected_reads_do_not_consume_bytes_or_escape_the_allowed_root() {
    let fixture = Fixture::new();
    let admission = PathAdmission::new(&fixture.home, &fixture.granted_dirs);
    let mut reads = read::ReadBudget::new(admission);
    fixture.write(fixture.home.join("private.md"), "Do not read\n");
    for path in [
        unjoined_path(&fixture.root, &["package", "..", "private.md"]),
        fixture.home.join("private.md"),
    ] {
        assert_eq!(
            reads.read(&path, &fixture.root, false),
            Err("path_outside_allowed_root")
        );
    }
    assert_eq!(reads.files, 254);
    assert_eq!(reads.bytes, 1024 * 1024);
}

#[test]
fn configured_parent_paths_are_rejected_before_joins_and_keep_safe_siblings() {
    for format in [
        SourceFormat::OpenCodeSqliteV1,
        SourceFormat::OpenCodeSqliteV2,
    ] {
        let fixture = Fixture::new();
        for (base, config, prefix) in [
            (
                &fixture.home,
                fixture.home.join(".config/opencode/opencode.json"),
                "~/",
            ),
            (&fixture.cwd, fixture.root.join("opencode.json"), "./"),
        ] {
            fixture.write(base.join("safe.md"), "Safe policy\n");
            fixture.write(base.join("private.md"), "Do not read\n");
            fixture.write(
                base.join("safe-skills/review/SKILL.md"),
                "---\nname: safe\n---\n",
            );
            fixture.write(
                base.join("private-skills/review/SKILL.md"),
                "---\nname: private\n---\n",
            );
            let mut instructions = vec![
                format!("{prefix}safe.md"),
                format!("{prefix}nested/../private.md"),
                format!("{prefix}*.md"),
            ];
            let mut skills = vec![
                format!("{prefix}safe-skills"),
                format!("{prefix}nested/../private-skills"),
            ];
            if cfg!(windows) {
                instructions.push(format!("{prefix}nested\\..\\private.md"));
                skills.push(format!("{prefix}nested\\..\\private-skills"));
            }
            let skills = if format == SourceFormat::OpenCodeSqliteV1 {
                serde_json::json!({"paths": skills})
            } else {
                serde_json::json!(skills)
            };
            fixture.write(
                config,
                serde_json::json!({"instructions": instructions, "skills": skills}).to_string(),
            );
        }
        let snapshot = fixture.collect(AgentKind::OpenCode, format);
        for root in [snapshot.global, snapshot.repo] {
            let instructions = root.instructions.unwrap();
            assert_eq!(instructions.len(), 1);
            assert_eq!(instructions[0].content, "Safe policy\n");
            let skills = root.skills.unwrap();
            assert_eq!(skills.len(), 1);
            assert_eq!(skills[0].name, "safe");
            assert!(
                root.instructions_limits
                    .contains(&"path_outside_allowed_root".to_owned())
            );
            assert!(
                root.skills_limits
                    .contains(&"path_outside_allowed_root".to_owned())
            );
            assert!(
                root.instructions_limits
                    .contains(&"dynamic_or_glob_source_not_read".to_owned())
            );
        }
    }
}

#[cfg(windows)]
fn ordinary_path(path: &Path) -> PathBuf {
    let text = path.to_str().unwrap();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(text.strip_prefix(r"\\?\").unwrap())
    }
}

#[cfg(windows)]
fn create_fixture_junction(fixture: &Fixture) -> PathBuf {
    let output = std::process::Command::new("cmd")
        .current_dir(fixture._temp.path())
        .args(["/D", "/C", "mklink", "/J", r"repo\linked", r"home\outside"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    fixture.root.join("linked")
}

#[cfg(windows)]
#[test]
fn windows_invalid_path_shapes_are_rejected_before_lookup() {
    let home = Path::new(r"\\?\C:\synthetic\home");
    let grants = HashSet::new();
    let admission = PathAdmission::new(home, &grants);
    for path in [
        r"C:synthetic\repo",
        r"\synthetic\repo",
        r"\\?\C:\synthetic\repo\nested\..\private",
        r"\\?\C:\synthetic\repo\nested/../private",
        r"\\.\C:\synthetic\repo",
        r"\\?\GLOBALROOT\Device\synthetic\repo",
        r"\\?\Volume{synthetic}\repo",
        r"//?/Volume{synthetic}/repo",
        r"//?/GLOBALROOT/Device/synthetic/repo",
        r"//./C:/synthetic/repo",
        r"\\?/GLOBALROOT/Device/synthetic/repo",
    ] {
        assert_eq!(
            admission.check(Path::new(path)),
            Err("path_outside_allowed_root")
        );
        assert_eq!(
            admission.canonical_directory(Path::new(path)),
            Err("path_outside_allowed_root")
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_ordinary_ambiguous_components_are_rejected_before_lookup() {
    let home = Path::new(r"\\?\C:\synthetic\home");
    let grants = HashSet::new();
    let admission = PathAdmission::new(home, &grants);
    let mut reads = read::ReadBudget::new(admission);
    for (path, root) in [
        (r"C:\synthetic\gate \private.md", r"C:\synthetic"),
        (r"C:\synthetic\gate.\private.md", r"C:\synthetic"),
        (r"C:\synthetic\private.md ", r"C:\synthetic"),
        (r"C:\synthetic\private.md.", r"C:\synthetic"),
        (r"\\server\share\gate \review", r"\\server\share"),
        (r"\\server\share\gate.\review", r"\\server\share"),
    ] {
        let path = Path::new(path);
        let root = Path::new(root);
        assert_eq!(admission.check(path), Err("path_outside_allowed_root"));
        assert_eq!(
            admission.canonical_directory(path),
            Err("path_outside_allowed_root")
        );
        assert_eq!(
            admission.configured_root(path, root),
            Err("path_outside_allowed_root")
        );
        assert_eq!(
            reads.read(path, root, false),
            Err("path_outside_allowed_root")
        );
    }
    assert_eq!(reads.files, 250);
    assert_eq!(reads.bytes, 1024 * 1024);
    for path in [
        r"\\?\C:\synthetic\gate \private.md",
        r"\\?\C:\synthetic\gate.\private.md",
        r"\\?\UNC\server\share\gate \review",
        r"\\?\UNC\server\share\gate.\review",
        r"C:\synthetic\gate space\private.md",
        r"C:\synthetic\gate.name\private.md",
    ] {
        assert_eq!(admission.check(Path::new(path)), Ok(()));
    }
    for home in [
        r"C:\synthetic\home ",
        r"\\server\share\home.",
        r"\\.\C:\synthetic\home",
        r"\\?\GLOBALROOT\Device\synthetic\home",
        r"//?/Volume{synthetic}/home",
        r"//?/GLOBALROOT/Device/synthetic/home",
        r"//./C:/synthetic/home",
    ] {
        let admission = PathAdmission::new(Path::new(home), &grants);
        assert_eq!(
            admission.check(Path::new(r"C:\synthetic\repo")),
            Err("home_unavailable")
        );
        assert_eq!(
            admission.canonical_directory(Path::new(r"C:\synthetic\repo")),
            Err("home_unavailable")
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_requests_accept_ordinary_and_verbatim_directories() {
    let fixture = Fixture::new();
    fixture.write(fixture.home.join(".claude/CLAUDE.md"), "Global policy\n");
    fixture.write(fixture.root.join("CLAUDE.md"), "Repo policy\n");
    let home = ordinary_path(&fixture.home);
    let root = ordinary_path(&fixture.root);
    let cwd = ordinary_path(&fixture.cwd);
    for (home, root, cwd) in [
        (&home, &root, &cwd),
        (&fixture.home, &fixture.root, &fixture.cwd),
        (&home, &fixture.root, &cwd),
    ] {
        let mut request = fixture.request(AgentKind::Claude, SourceFormat::ClaudeJsonl);
        request.home = home;
        request.repo_root = Some(root);
        request.cwd = Some(cwd);
        let snapshot = collect_current_context(request);
        assert_eq!(
            snapshot.global.instructions.unwrap()[0].content,
            "Global policy\n"
        );
        assert_eq!(
            snapshot.repo.instructions.unwrap()[0].content,
            "Repo policy\n"
        );
        assert!(snapshot.global.snapshot_revision.is_some());
        assert!(snapshot.repo.snapshot_revision.is_some());
    }
}

#[cfg(windows)]
#[test]
fn windows_configured_absolute_paths_keep_their_namespace_and_allowed_roots() {
    for format in [
        SourceFormat::OpenCodeSqliteV1,
        SourceFormat::OpenCodeSqliteV2,
    ] {
        for verbatim in [false, true] {
            let fixture = Fixture::new();
            let instruction = fixture.cwd.join("guide.md");
            let source = fixture.cwd.join("team-skills");
            fixture.write(&instruction, "Configured policy\n");
            fixture.write(
                source.join("review/SKILL.md"),
                "---\nname: review\n---\nPrivate body\n",
            );
            let outside = fixture.root.with_file_name("repo-copy");
            fixture.write(outside.join("private.md"), "Do not read\n");
            let (instruction, source, outside) = if verbatim {
                (instruction, source, outside)
            } else {
                (
                    ordinary_path(&instruction),
                    ordinary_path(&source),
                    ordinary_path(&outside),
                )
            };
            let skills = if format == SourceFormat::OpenCodeSqliteV1 {
                serde_json::json!({"paths": [&source, &outside]})
            } else {
                serde_json::json!([&source, &outside])
            };
            fixture.write(
                fixture.root.join("opencode.json"),
                serde_json::json!({
                    "instructions": [&instruction, &outside.join("private.md")], "skills": skills,
                })
                .to_string(),
            );
            let snapshot = fixture.collect(AgentKind::OpenCode, format);
            let instructions = snapshot.repo.instructions.unwrap();
            assert_eq!(instructions.len(), 1);
            assert_eq!(instructions[0].content, "Configured policy\n");
            assert_eq!(Path::new(&instructions[0].path), instruction);
            let skills = snapshot.repo.skills.unwrap();
            assert_eq!(skills.len(), 1);
            assert_eq!(skills[0].name, "review");
            assert_eq!(Path::new(&skills[0].path), source.join("review/SKILL.md"));
            assert!(
                snapshot
                    .repo
                    .instructions_limits
                    .contains(&"configured_path_outside_allowed_roots".to_owned())
            );
            assert!(
                snapshot
                    .repo
                    .skills_limits
                    .contains(&"configured_path_outside_allowed_roots".to_owned())
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn windows_namespace_aliases_must_resolve_to_the_same_allowed_root() {
    let fixture = Fixture::new();
    let literal_root = fixture.root.with_file_name("repo.");
    fs::create_dir(&literal_root).unwrap();
    let ordinary = ordinary_path(&literal_root);
    assert_eq!(ordinary.canonicalize().unwrap(), fixture.root);
    let admission = PathAdmission::new(&fixture.home, &fixture.granted_dirs);
    assert_eq!(
        admission.configured_root(&ordinary.join("private.md"), &literal_root),
        Err("path_outside_allowed_root")
    );
    assert_eq!(
        admission.configured_root(&literal_root.join("private.md"), &literal_root),
        Ok(Some(literal_root))
    );
}

#[cfg(windows)]
#[test]
fn windows_junctions_do_not_expand_workspace_or_configured_read_roots() {
    let fixture = Fixture::new();
    let outside = fixture.home.join("outside");
    fixture.write(outside.join("private.md"), "Do not read\n");
    fixture.write(outside.join("review/SKILL.md"), "---\nname: private\n---\n");
    fixture.write(fixture.cwd.join("safe.md"), "Safe policy\n");
    fixture.write(
        fixture.cwd.join("safe-skills/review/SKILL.md"),
        "---\nname: safe\n---\n",
    );
    let linked = create_fixture_junction(&fixture);
    let admission = PathAdmission::new(&fixture.home, &fixture.granted_dirs);
    assert_eq!(
        admission.canonical_directory(&linked),
        Err("symlink_not_read")
    );
    assert_eq!(
        admission.canonical_directory(&linked.join("review")),
        Err("symlink_not_read")
    );
    fixture.write(
        fixture.root.join("opencode.json"),
        serde_json::json!({
            "instructions": [ordinary_path(&linked.join("private.md")), PathBuf::from("./safe.md")],
            "skills": [ordinary_path(&linked), PathBuf::from("./safe-skills")],
        })
        .to_string(),
    );
    let snapshot = fixture.collect(AgentKind::OpenCode, SourceFormat::OpenCodeSqliteV2);
    let instructions = snapshot.repo.instructions.unwrap();
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].content, "Safe policy\n");
    let skills = snapshot.repo.skills.unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "safe");
    assert!(
        snapshot
            .repo
            .instructions_limits
            .contains(&"symlink_not_read".to_owned())
    );
    assert!(
        snapshot
            .repo
            .skills_limits
            .contains(&"unsafe_directory_not_read".to_owned())
    );
    fs::remove_dir(linked).unwrap();
}

#[cfg(windows)]
#[test]
fn windows_trailing_space_junctions_keep_only_safe_configured_sources() {
    for format in [
        SourceFormat::OpenCodeSqliteV1,
        SourceFormat::OpenCodeSqliteV2,
    ] {
        let fixture = Fixture::new();
        let gate = fixture.root.join("gate");
        fixture.write(gate.join("safe.md"), "Safe policy\n");
        fixture.write(gate.join("review/SKILL.md"), "---\nname: safe\n---\n");
        let outside = fixture.home.join("outside");
        fixture.write(outside.join("private.md"), "Do not read\n");
        fixture.write(outside.join("review/SKILL.md"), "---\nname: private\n---\n");
        fixture.write(outside.join("review/CLAUDE.md"), "Do not read\n");
        fixture.write(fixture.home.join(".claude/CLAUDE.md"), "Safe global\n");
        let linked = create_fixture_junction(&fixture);
        let literal = fixture.root.join("gate ");
        // Use verbatim paths so rename retains the junction's final space.
        fs::rename(linked, &literal).unwrap();
        let admission = PathAdmission::new(&fixture.home, &fixture.granted_dirs);
        assert_eq!(admission.check(&literal), Ok(()));
        assert_eq!(
            admission.canonical_directory(&literal.join("review")),
            Err("symlink_not_read")
        );
        assert_eq!(
            admission.canonical_directory(&fixture.root),
            Ok(fixture.root.clone())
        );
        let mut device = std::ffi::OsString::from(r"\\.\");
        device.push(ordinary_path(&literal.join("review")));
        let device = PathBuf::from(device);
        assert_eq!(admission.check(&device), Err("path_outside_allowed_root"));
        assert_eq!(
            admission.canonical_directory(&device),
            Err("path_outside_allowed_root")
        );
        let mut request = fixture.request(AgentKind::Claude, SourceFormat::ClaudeJsonl);
        request.repo_root = Some(&device);
        request.cwd = Some(&device);
        let snapshot = collect_current_context(request);
        assert!(snapshot.repo.instructions.is_none());
        assert!(snapshot.repo.skills.is_none());
        assert!(snapshot.repo.snapshot_revision.is_none());
        assert_eq!(
            snapshot.repo.instructions_limits,
            ["path_outside_allowed_root"]
        );
        let global = snapshot.global.instructions.unwrap();
        assert_eq!(global.len(), 1);
        assert_eq!(global[0].content, "Safe global\n");
        for (source, instruction_limit, skill_limit) in [
            (
                ordinary_path(&literal),
                "path_outside_allowed_root",
                "path_outside_allowed_root",
            ),
            (
                literal.clone(),
                "symlink_not_read",
                "unsafe_directory_not_read",
            ),
        ] {
            let safe_instruction = ordinary_path(&gate.join("safe.md"));
            let safe_skill = ordinary_path(&gate.join("review"));
            let private_skill = source.join("review");
            let skills = if format == SourceFormat::OpenCodeSqliteV1 {
                serde_json::json!({"paths": [&private_skill, &safe_skill]})
            } else {
                serde_json::json!([&private_skill, &safe_skill])
            };
            fixture.write(fixture.root.join("opencode.json"), serde_json::json!({
                "instructions": [source.join("private.md"), &safe_instruction], "skills": skills,
            }).to_string());
            let snapshot = fixture.collect(AgentKind::OpenCode, format);
            let instructions = snapshot.repo.instructions.unwrap();
            assert_eq!(instructions.len(), 1);
            assert_eq!(instructions[0].content, "Safe policy\n");
            assert_eq!(Path::new(&instructions[0].path), safe_instruction);
            let skills = snapshot.repo.skills.unwrap();
            assert_eq!(skills.len(), 1);
            assert_eq!(skills[0].name, "safe");
            assert_eq!(Path::new(&skills[0].path), safe_skill.join("SKILL.md"));
            assert!(
                snapshot
                    .repo
                    .instructions_limits
                    .contains(&instruction_limit.to_owned())
            );
            assert!(
                snapshot
                    .repo
                    .skills_limits
                    .contains(&skill_limit.to_owned())
            );
        }
        fs::remove_dir(literal).unwrap();
    }
}

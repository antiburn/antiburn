use super::*;

#[cfg(target_os = "macos")]
#[test]
fn protected_case_aliases_need_the_exact_directory_grant_without_probes() {
    let home = Path::new("/Synthetic/Users/TestHome");
    for (name, aliases) in [
        ("Documents", ["documents", "dOcUmEnTs"]),
        ("Desktop", ["desktop", "dEsKtOp"]),
        ("Downloads", ["downloads", "dOwNlOaDs"]),
    ] {
        for home_alias in [
            home,
            Path::new("/synthetic/users/testhome"),
            Path::new("/sYnThEtIc/uSeRs/tEsThOmE"),
        ] {
            for alias in aliases {
                let path = home_alias.join(alias).join("missing/rules.md");
                for grants in [
                    HashSet::new(),
                    HashSet::from([alias.to_owned()]),
                    HashSet::from(["Other".to_owned()]),
                ] {
                    let admission = PathAdmission::new(home, &grants);
                    assert_eq!(admission.check(&path), Err("protected_path_not_granted"));
                    assert_eq!(
                        admission.canonical_directory(&path),
                        Err("protected_path_not_granted")
                    );
                }
                let grants = HashSet::from([name.to_owned()]);
                assert_eq!(PathAdmission::new(home, &grants).check(&path), Ok(()));
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn protected_alias_matching_is_conservative_but_keeps_component_boundaries() {
    let home = Path::new("/Synthetic/Users/TéstHome");
    let grants = HashSet::new();
    let admission = PathAdmission::new(home, &grants);
    for path in [
        "/synthetic/users/tÉsthome/documents/rules.md",
        "/synthetic/users/Te\u{301}stHome/DoCuMeNtS/rules.md",
        "/synthetic/users/OtherHome/Documents/rules.md",
        "/Synthetic/Users/TéstHome/Documénts/rules.md",
    ] {
        assert_eq!(
            admission.check(Path::new(path)),
            Err("protected_path_not_granted")
        );
    }
    let documents_grant = HashSet::from(["Documents".to_owned()]);
    assert_eq!(
        PathAdmission::new(home, &documents_grant)
            .check(Path::new("/synthetic/users/tÉsthome/documents/rules.md")),
        Ok(())
    );
    for path in [
        "/Synthetic/Users/TéstHome/Documents-copy/rules.md",
        "/Synthetic/Users/TéstHome/safe/Documents/rules.md",
        "/Synthetic/Users/TéstHome",
        "/Other/Users/TéstHome/Documents/rules.md",
    ] {
        assert_eq!(admission.check(Path::new(path)), Ok(()));
    }
    let ascii_home = Path::new("/Synthetic/Users/TestHome");
    assert_eq!(
        PathAdmission::new(ascii_home, &grants).check(Path::new(
            "/Synthetic/Users/TestHome-copy/Documents/rules.md"
        )),
        Ok(())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn denied_case_aliases_do_not_reach_file_lookup_or_consume_bytes() {
    let home = Path::new("/Synthetic/Users/TestHome");
    let grants = HashSet::new();
    let mut reads = read::ReadBudget::new(PathAdmission::new(home, &grants));
    for (path, header_only) in [
        ("/synthetic/users/testhome/documents/missing.md", false),
        ("/sYnThEtIc/uSeRs/tEsThOmE/dOcUmEnTs/missing/SKILL.md", true),
    ] {
        assert_eq!(
            reads.read(Path::new(path), Path::new("/"), header_only),
            Err("protected_path_not_granted")
        );
    }
    assert_eq!(reads.files, 254);
    assert_eq!(reads.bytes, 1024 * 1024);
    assert_eq!(
        reads.read(&home.join("documents/../safe.md"), home, false),
        Err("path_outside_allowed_root")
    );
}

#[cfg(target_os = "macos")]
#[test]
fn denied_case_aliases_stop_instruction_fallback_and_skill_walks() {
    let home = Path::new("/Synthetic/Users/TestHome");
    let grants = HashSet::new();
    let mut collector = Collector {
        snapshot: CurrentContextSnapshot::default(),
        observed_at: "2026-10-27T12:00:00Z".to_owned(),
        reads: read::ReadBudget::new(PathAdmission::new(home, &grants)),
        directories_left: 128,
        entries_left: 4096,
        instructions_left: 64,
        skills_left: 128,
        config_entries_left: 64,
        source_binding: String::new(),
    };
    collector.available(true);
    collector.collect_global(
        Producer::OpenCodeV1,
        Path::new("/synthetic/users/testhome/dOcUmEnTs"),
        None,
    );
    assert_eq!(collector.reads.files, 255);
    assert_eq!(collector.reads.bytes, 1024 * 1024);
    assert_eq!(collector.entries_left, 4096);
    assert_eq!(collector.instructions_left, 64);
    assert_eq!(collector.skills_left, 128);
    assert!(collector.snapshot.global.instructions.unwrap().is_empty());
    assert!(collector.snapshot.global.skills.unwrap().is_empty());
    assert!(
        collector
            .snapshot
            .global
            .instructions_limits
            .contains(&"protected_path_not_granted".to_owned())
    );
    assert!(
        collector
            .snapshot
            .global
            .skills_limits
            .contains(&"protected_path_not_granted".to_owned())
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn case_aliases_do_not_add_protected_directories_on_other_platforms() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path();
    let grants = HashSet::new();
    let admission = PathAdmission::new(home, &grants);
    for name in [
        "Documents",
        "documents",
        "dOcUmEnTs",
        "Desktop",
        "downloads",
    ] {
        assert_eq!(admission.check(&home.join(name).join("rules.md")), Ok(()));
    }
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "requires TMPDIR on a case-insensitive volume; uses only temporary synthetic files"]
fn configured_instruction_and_skill_case_aliases_on_a_case_insensitive_volume() {
    use std::os::unix::fs::MetadataExt;

    for format in [
        SourceFormat::OpenCodeSqliteV1,
        SourceFormat::OpenCodeSqliteV2,
    ] {
        for (name, aliases) in [
            ("Documents", ["documents", "dOcUmEnTs"]),
            ("Desktop", ["desktop", "dEsKtOp"]),
            ("Downloads", ["downloads", "dOwNlOaDs"]),
        ] {
            for (alias, home_alias) in aliases.into_iter().zip(["testhome", "tEsThOmE"]) {
                let mut fixture = Fixture::new();
                let home = fixture.home.with_file_name("TestHome");
                fs::rename(&fixture.home, &home).unwrap();
                fixture.home = home.canonicalize().unwrap();
                let home_alias = fixture.home.with_file_name(home_alias);
                let instruction = fixture.home.join(name).join("private-rules.md");
                let skill = fixture.home.join(name).join("team-skills/private/SKILL.md");
                fixture.write(&instruction, "Protected policy\n");
                fixture.write(
                    &skill,
                    "---\nname: protected\ndescription: Protected skill\n---\nPrivate body\n",
                );
                fixture.write(fixture.home.join("safe-rules.md"), "Safe policy\n");
                fixture.write(
                    fixture.home.join("safe-skills/safe/SKILL.md"),
                    "---\nname: safe\n---\n",
                );
                let skills = if format == SourceFormat::OpenCodeSqliteV2 {
                    serde_json::json!([format!("~/{alias}/team-skills"), "~/safe-skills"])
                } else {
                    serde_json::json!({"paths": [format!("~/{alias}/team-skills"), "~/safe-skills"]})
                };
                fixture.write(
                    fixture.home.join(".config/opencode/opencode.json"),
                    serde_json::json!({
                        "instructions": [format!("~/{alias}/private-rules.md"), "~/safe-rules.md"],
                        "skills": skills,
                    })
                    .to_string(),
                );
                for (actual, case_alias) in [
                    (
                        &instruction,
                        home_alias.join(alias).join("private-rules.md"),
                    ),
                    (
                        &skill,
                        home_alias.join(alias).join("team-skills/private/SKILL.md"),
                    ),
                ] {
                    let expected = fs::metadata(actual).unwrap();
                    let aliased = fs::metadata(case_alias)
                        .expect("set TMPDIR to a case-insensitive temporary volume");
                    assert_eq!(
                        (expected.dev(), expected.ino()),
                        (aliased.dev(), aliased.ino())
                    );
                }
                for grants in [HashSet::new(), HashSet::from([alias.to_owned()])] {
                    let mut request = fixture.request(AgentKind::OpenCode, format);
                    request.home = &home_alias;
                    request.granted_dirs = &grants;
                    let denied = collect_current_context(request);
                    let instructions = denied.global.instructions.as_ref().unwrap();
                    assert_eq!(instructions.len(), 1);
                    assert_eq!(instructions[0].content, "Safe policy\n");
                    let skills = denied.global.skills.as_ref().unwrap();
                    assert_eq!(skills.len(), 1);
                    assert_eq!(skills[0].name, "safe");
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
                }
                fixture.granted_dirs.insert(name.to_owned());
                let mut request = fixture.request(AgentKind::OpenCode, format);
                request.home = &home_alias;
                let granted = collect_current_context(request);
                let instructions = granted.global.instructions.unwrap();
                assert_eq!(instructions.len(), 2);
                assert!(
                    instructions
                        .iter()
                        .any(|entry| entry.content == "Protected policy\n")
                );
                let skills = granted.global.skills.unwrap();
                assert_eq!(skills.len(), 2);
                assert!(skills.iter().any(
                    |entry| entry.name == "protected" && entry.description == "Protected skill"
                ));
                assert!(
                    !granted
                        .global
                        .instructions_limits
                        .contains(&"protected_path_not_granted".to_owned())
                );
                assert!(
                    !granted
                        .global
                        .skills_limits
                        .contains(&"protected_path_not_granted".to_owned())
                );
            }
        }
    }
}

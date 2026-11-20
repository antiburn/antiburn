use super::super::{Phase, Snapshot, Status, inspect_ready, state};
use super::*;
use sha2::{Digest, Sha256};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};

struct Fixture {
    _root: tempfile::TempDir,
    installation: Installation,
    runtime: CliRuntime,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root
            .path()
            .canonicalize()
            .unwrap()
            .join("owner's ``` $HOME");
        fs::DirBuilder::new().mode(0o700).create(&base).unwrap();
        let data = base.join(DEBUG_IDENTIFIER);
        let profile = base.join("selected opencode");
        for path in [&data, &data.join("cli"), &profile] {
            fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        Self {
            runtime: CliRuntime {
                executable: data.join("cli/bin/antiburn-cli"),
                version: "0.1.0".into(),
                sha256: "synthetic".into(),
                delivery_protocols: vec!["delivery-v1".into()],
            },
            installation: Installation { data, profile },
            _root: root,
        }
    }

    fn install_cli(&mut self) -> antiburn_cli_runtime::Installation {
        let payload = self
            ._root
            .path()
            .canonicalize()
            .unwrap()
            .join("cli-payload");
        fs::DirBuilder::new().mode(0o700).create(&payload).unwrap();
        let bytes = b"synthetic CLI payload";
        fs::write(payload.join(antiburn_cli_runtime::file_name()), bytes).unwrap();
        let manifest = antiburn_cli_runtime::Manifest {
            schema: 1,
            target: antiburn_cli_runtime::TARGET.into(),
            executable: antiburn_cli_runtime::file_name().into(),
            version: "0.1.0".into(),
            sha256: Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            delivery_protocols: vec!["delivery-v1".into()],
        };
        fs::write(
            payload.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let cli = antiburn_cli_runtime::Installation {
            root: self.installation.data.join("cli"),
            terminal: self.installation.data.join("cli/terminal/antiburn-cli"),
            payload,
            target: manifest.target,
        };
        cli.reconcile().unwrap();
        self.runtime = cli.resolve().unwrap();
        cli
    }
}

#[test]
fn lock_contention_does_not_store_failure_or_block_later_file_inspection() {
    let mut fixture = Fixture::new();
    let cli = fixture.install_cli();
    let i = &fixture.installation;
    i.reconcile(&fixture.runtime).unwrap();
    let mut state = state::State::new(Status::initial(None));
    let (_, worker) = state.begin_setup();
    state.finish_setup(
        worker.unwrap(),
        Snapshot {
            status: Status {
                revision: 0,
                phase: Phase::Ready,
                cli_installed: true,
                cli_tested: true,
                skill_installed: true,
                reason: None,
            },
            tested_sha256: Some(fixture.runtime.sha256.clone()),
        },
    );
    let snapshot = state.snapshot();
    let (lock, _) = cli.runtime_for_test().unwrap();
    let mut cache = antiburn_cli_runtime::RuntimeDigestCache::default();
    let inspected = cli.resolve_cached(&mut cache);
    assert!(matches!(
        inspected
            .as_ref()
            .unwrap_err()
            .downcast_ref::<fs::TryLockError>(),
        Some(fs::TryLockError::WouldBlock)
    ));
    let busy = inspect_ready(&snapshot, inspected, |_| {
        panic!("Busy inspection must not inspect generated files")
    });
    assert!(busy.unwrap_err().contains("Retry"));
    assert_eq!(state.snapshot().status.revision, snapshot.status.revision);
    assert_eq!(state.snapshot().status.phase, Phase::Ready);
    drop(lock);

    let checked = inspect_ready(&snapshot, cli.resolve_cached(&mut cache), |runtime| {
        i.inspect(runtime)
    })
    .unwrap();
    assert_eq!(checked, Ok(()));
    let (status, changed) = state.apply_inspection(snapshot.status.revision, checked);
    assert!(!changed);
    assert_eq!(status.phase, Phase::Ready);

    fs::write(i.skill_directory().join("SKILL.md"), b"User edit").unwrap();
    let checked = inspect_ready(&snapshot, cli.resolve_cached(&mut cache), |runtime| {
        i.inspect(runtime)
    })
    .unwrap();
    let (status, changed) = state.apply_inspection(snapshot.status.revision, checked);
    assert!(changed);
    assert_eq!(status.phase, Phase::Failed);
    assert_eq!(status.reason, Some(Reason::OwnedFileChanged));
}

#[test]
fn interrupted_upgrade_keeps_the_previous_pending_generation_authorized() {
    let installed = b"previous pending skill";
    let mut receipt = FileReceipt {
        active: None,
        pending: Some(installed.to_vec()),
    };
    receipt.prepare_replacement(Some(installed), b"new skill".to_vec());
    let bytes = serde_json::to_vec(&receipt).unwrap();
    let mut restored: FileReceipt = serde_json::from_slice(&bytes).unwrap();
    assert!(check_owned(Some(installed), &restored).is_ok());
    restored.prepare_replacement(Some(installed), b"later build skill".to_vec());
    assert!(check_owned(Some(installed), &restored).is_ok());
    assert_eq!(
        check_owned(Some(b"user edit"), &restored),
        Err(Reason::OwnedFileChanged)
    );
}

#[test]
fn interrupted_upgrade_preserves_unknown_bytes_and_the_pending_receipt() {
    for target in ["config", "skill"] {
        let fixture = Fixture::new();
        let i = &fixture.installation;
        i.reconcile(&fixture.runtime).unwrap();
        let (mut receipt, _) = i.receipt().unwrap().unwrap();
        receipt.config.pending = receipt.config.active.take();
        receipt.skill.pending = receipt.skill.active.take();
        let receipt_path = i.tools().join("receipt.json");
        let receipt_bytes = serde_json::to_vec(&receipt).unwrap();
        fs::write(&receipt_path, &receipt_bytes).unwrap();
        let config_path = i.tools().join("config.json");
        let skill_path = i.skill_directory().join("SKILL.md");
        fs::write(
            if target == "config" {
                &config_path
            } else {
                &skill_path
            },
            b"User edit",
        )
        .unwrap();
        let config_bytes = fs::read(&config_path).unwrap();
        let skill_bytes = fs::read(&skill_path).unwrap();

        assert_eq!(i.reconcile(&fixture.runtime), Err(Reason::OwnedFileChanged));
        assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);
        assert_eq!(fs::read(&config_path).unwrap(), config_bytes);
        assert_eq!(fs::read(&skill_path).unwrap(), skill_bytes);
    }
}

#[test]
fn bound_skill_command_preserves_exact_arguments_with_an_empty_path() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    i.reconcile(&fixture.runtime).unwrap();
    let executable = &fixture.runtime.executable;
    fs::create_dir(executable.parent().unwrap()).unwrap();
    fs::write(
        executable,
        "#!/bin/sh\n[ -z \"$PATH\" ] || exit 1\nprintf '%s\\n' \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
    let generated = fs::read_to_string(i.skill_directory().join("SKILL.md")).unwrap();
    let command = generated
        .lines()
        .find(|line| line.contains("--config"))
        .unwrap()
        .replace("<session_id>", "synthetic-session");
    let output = std::process::Command::new("/bin/sh")
        .args(["-c", &command])
        .env_clear()
        .env("PATH", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [
            "--config",
            i.tools().join("config.json").to_str().unwrap(),
            "checks",
            "synthetic-session"
        ],
    );
}

#[test]
fn installs_profile_bound_files_and_does_not_rewrite_healthy_setup() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    i.reconcile(&fixture.runtime).unwrap();
    i.inspect(&fixture.runtime).unwrap();
    let paths = [
        i.tools().join("config.json"),
        i.tools().join("receipt.json"),
        i.skill_directory().join("SKILL.md"),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|path| {
            (
                fs::read(path).unwrap(),
                fs::metadata(path).unwrap().modified().unwrap(),
            )
        })
        .collect();
    i.reconcile(&fixture.runtime).unwrap();
    for (path, (bytes, modified)) in paths.iter().zip(before) {
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
    }
    let config: serde_json::Value = serde_json::from_slice(&fs::read(&paths[0]).unwrap()).unwrap();
    assert_eq!(config["app_identifier"], DEBUG_IDENTIFIER);
    assert_eq!(config["database_filename"], "antiburn-debug.sqlite3");
    assert_eq!(
        config["data_root"],
        i.data.parent().unwrap().to_str().unwrap()
    );
    for key in ["state_file", "log_file"] {
        assert!(Path::new(config[key].as_str().unwrap()).starts_with(i.tools()));
    }
    assert_eq!(
        config["pricing_snapshot_path"],
        i.data.join("model-pricing.json").to_str().unwrap()
    );
}

#[test]
fn generated_skill_uses_the_canonical_body_and_an_absolute_bound_command() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    i.reconcile(&fixture.runtime).unwrap();
    let generated = fs::read_to_string(i.skill_directory().join("SKILL.md")).unwrap();
    let launcher = format!(
        "{} --config {} ",
        quote(&fixture.runtime.executable).unwrap(),
        quote(&i.tools().join("config.json")).unwrap()
    );
    assert!(generated.starts_with("---\nname: antiburn-debug\n"));
    assert!(generated.contains(&format!("{launcher}checks '<session_id>'")));
    assert_eq!(
        generated
            .replacen("name: antiburn-debug\n", "name: antiburn\n", 1)
            .replace(&launcher, "antiburn "),
        SKILL,
    );
    assert_eq!(i.skill_directory(), i.profile.join("skills/antiburn-debug"));
}

#[test]
fn binds_multiple_shell_examples_without_editing_prose_or_markdown() {
    let fixture = Fixture::new();
    let executable = &fixture.runtime.executable;
    fs::create_dir(executable.parent().unwrap()).unwrap();
    fs::write(executable, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
    fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
    let source = concat!(
        "Do not edit `antiburn` in prose.\n",
        "```sh\nantiburn checks 'session'\nantiburn capabilities --json\n```\n",
        "```markdown\nantiburn footer\n```\n",
        "```sh\nantiburn checks 'another session'\n```\n",
    );
    let config = fixture.installation.tools().join("config.json");
    for config in [None, Some(config.as_path())] {
        let rendered =
            String::from_utf8(render_skill(source, executable, config).unwrap()).unwrap();
        assert_eq!(
            rendered
                .lines()
                .filter(|line| line.starts_with("```"))
                .collect::<Vec<_>>(),
            ["```sh", "```", "```markdown", "```", "```sh", "```"]
        );
        assert!(rendered.contains("`antiburn` in prose."));
        assert!(rendered.contains("```markdown\nantiburn footer\n```"));
        let commands = rendered.lines().filter(|line| line.starts_with("'/"));
        let arguments = [
            ["checks", "session"],
            ["capabilities", "--json"],
            ["checks", "another session"],
        ];
        assert_eq!(commands.clone().count(), arguments.len());
        for (command, args) in commands.zip(arguments) {
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", command])
                .env_clear()
                .env("PATH", "")
                .output()
                .unwrap();
            assert!(output.status.success());
            assert!(output.stderr.is_empty());
            let mut expected = Vec::new();
            if let Some(config) = config {
                expected.extend(["--config", config.to_str().unwrap()]);
            }
            expected.extend(args);
            assert_eq!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .lines()
                    .collect::<Vec<_>>(),
                expected,
            );
        }
    }
}

#[test]
fn rejects_unbound_shell_examples_and_multiline_paths() {
    assert_eq!(
        render_skill(
            "```sh\ncommand antiburn checks id\n```\n",
            Path::new("/cli"),
            None
        ),
        Err(Reason::SetupUnavailable)
    );
    assert_eq!(
        render_skill(SKILL, Path::new("/home\n```\n/cli"), None),
        Err(Reason::SetupUnavailable)
    );
}

#[test]
fn preserves_unknown_skills_and_does_not_claim_identical_unowned_files() {
    for identical in [false, true] {
        let fixture = Fixture::new();
        let i = &fixture.installation;
        fs::create_dir(i.profile.join("skills")).unwrap();
        fs::create_dir(i.skill_directory()).unwrap();
        let path = i.skill_directory().join("SKILL.md");
        let bytes = if identical {
            i.skill(&fixture.runtime).unwrap()
        } else {
            b"User skill".to_vec()
        };
        fs::write(&path, &bytes).unwrap();
        assert_eq!(
            i.reconcile(&fixture.runtime),
            Err(Reason::OwnershipConflict)
        );
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert!(!i.tools().join("receipt.json").exists());
        assert!(!i.tools().join("config.json").exists());
    }
}

#[test]
fn preserves_release_tools_and_unrelated_skills() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    let release = i.data.with_file_name("ai.antiburn.desktop");
    fs::create_dir(&release).unwrap();
    fs::write(release.join("existing-cli"), b"Existing release CLI").unwrap();
    fs::create_dir(i.profile.join("skills")).unwrap();
    fs::create_dir(i.profile.join("skills/antiburn")).unwrap();
    let release_skill = i.profile.join("skills/antiburn/SKILL.md");
    fs::write(&release_skill, b"Existing release skill").unwrap();
    i.reconcile(&fixture.runtime).unwrap();
    assert_eq!(
        fs::read(release.join("existing-cli")).unwrap(),
        b"Existing release CLI"
    );
    assert_eq!(fs::read(release_skill).unwrap(), b"Existing release skill");
    assert!(!release.join("cli/tools").exists());
}

#[test]
fn refuses_unowned_debug_config_and_flat_skill_collisions() {
    for config_collision in [true, false] {
        let fixture = Fixture::new();
        let i = &fixture.installation;
        let path = if config_collision {
            fs::DirBuilder::new().mode(0o700).create(i.tools()).unwrap();
            i.tools().join("config.json")
        } else {
            fs::create_dir(i.profile.join("skills")).unwrap();
            i.profile.join("skills/antiburn-debug.md")
        };
        fs::write(&path, b"User file").unwrap();
        assert_eq!(
            i.reconcile(&fixture.runtime),
            Err(Reason::OwnershipConflict)
        );
        assert_eq!(fs::read(path).unwrap(), b"User file");
        assert!(!i.tools().join("receipt.json").exists());
    }
}

#[test]
fn preserves_modified_and_removed_owned_files() {
    for target in ["config", "skill", "removed_skill"] {
        let fixture = Fixture::new();
        let i = &fixture.installation;
        i.reconcile(&fixture.runtime).unwrap();
        let path = if target == "config" {
            i.tools().join("config.json")
        } else {
            i.skill_directory().join("SKILL.md")
        };
        if target == "removed_skill" {
            fs::remove_file(&path).unwrap();
        } else {
            fs::write(&path, b"User change").unwrap();
        }
        assert_eq!(i.reconcile(&fixture.runtime), Err(Reason::OwnedFileChanged));
        assert_eq!(i.inspect(&fixture.runtime), Err(Reason::OwnedFileChanged));
        if target == "removed_skill" {
            assert!(!path.exists());
        } else {
            assert_eq!(fs::read(path).unwrap(), b"User change");
        }
    }
}

#[test]
fn resumes_a_write_ahead_receipt_without_claiming_user_edits() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    i.reconcile(&fixture.runtime).unwrap();
    let (mut receipt, _) = i.receipt().unwrap().unwrap();
    receipt.config.pending = receipt.config.active.take();
    receipt.skill.pending = receipt.skill.active.take();
    fs::write(
        i.tools().join("receipt.json"),
        serde_json::to_vec(&receipt).unwrap(),
    )
    .unwrap();
    fs::remove_file(i.tools().join("config.json")).unwrap();
    i.reconcile(&fixture.runtime).unwrap();
    i.inspect(&fixture.runtime).unwrap();
}

#[test]
fn changed_profile_and_release_bindings_do_not_redirect_owned_setup() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    i.reconcile(&fixture.runtime).unwrap();
    let new_profile = i.profile.with_file_name("other opencode");
    fs::create_dir(&new_profile).unwrap();
    let changed = Installation {
        data: i.data.clone(),
        profile: new_profile.clone(),
    };
    assert_eq!(
        changed.reconcile(&fixture.runtime),
        Err(Reason::ProfileChanged)
    );
    assert!(!new_profile.join("skills").exists());
    let mut other_runtime = fixture.runtime.clone();
    other_runtime.executable = i
        .data
        .with_file_name("ai.antiburn.desktop")
        .join("cli/bin/antiburn-cli");
    assert_eq!(i.reconcile(&other_runtime), Err(Reason::SetupUnavailable));
    i.inspect(&fixture.runtime).unwrap();
}

#[test]
fn refuses_redirected_skill_directories_without_touching_the_target() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    let other = i.profile.with_file_name("other skills");
    fs::create_dir(&other).unwrap();
    symlink(&other, i.profile.join("skills")).unwrap();
    assert!(i.reconcile(&fixture.runtime).is_err());
    assert_eq!(fs::read_dir(other).unwrap().count(), 0);
}

#[test]
fn profile_selection_honors_explicit_and_xdg_roots_without_creating_profiles() {
    let fixture = Fixture::new();
    let home = fixture.installation.profile.parent().unwrap();
    let selected = fixture.installation.profile.clone();
    let env = BTreeMap::from([
        (
            "OPENCODE_CONFIG_DIR".into(),
            selected.to_str().unwrap().into(),
        ),
        (
            "XDG_CONFIG_HOME".into(),
            home.join("other").to_str().unwrap().into(),
        ),
    ]);
    let profile = discovery::profile(Vendor::OpenCode, home, &env, None).unwrap();
    assert_eq!(profile.root, selected);
    assert!(!home.join("other").exists());
    let env = BTreeMap::from([(
        "XDG_CONFIG_HOME".into(),
        home.join("other").to_str().unwrap().into(),
    )]);
    assert_eq!(
        discovery::profile(Vendor::OpenCode, home, &env, None)
            .unwrap()
            .root,
        home.join("other/opencode")
    );
}

#[test]
fn invalid_explicit_profile_environment_does_not_fall_back_to_global_setup() {
    assert_eq!(
        profile_environment(
            |key| (key == "OPENCODE_CONFIG_DIR").then(|| OsString::from_vec(vec![0xff]))
        ),
        Err(Reason::ProfileUnavailable),
    );
}

#[test]
fn refuses_a_shared_writable_profile_ancestor_without_repairing_permissions() {
    let mut fixture = Fixture::new();
    let parent = fixture
        .installation
        .profile
        .with_file_name("shared ancestor");
    fs::DirBuilder::new().mode(0o700).create(&parent).unwrap();
    fixture.installation.profile = parent.join("selected profile");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&fixture.installation.profile)
        .unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        fixture.installation.reconcile(&fixture.runtime),
        Err(Reason::ProfileUnavailable)
    );
    assert_eq!(
        fs::metadata(&parent).unwrap().permissions().mode() & 0o777,
        0o777
    );
    assert!(!fixture.installation.skill_directory().exists());
    assert!(!fixture.installation.tools().exists());
}

fn acl(path: &Path, operation: &str, entry: &str) {
    let status = std::process::Command::new("/bin/chmod")
        .arg(operation)
        .arg(entry)
        .arg(path)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "Could not prepare a synthetic ACL fixture"
    );
}

#[test]
fn refuses_unrelated_write_acls_on_profile_ancestors_without_removing_them() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    let ancestor = i.profile.parent().unwrap();
    let entry = "everyone allow search,add_file,delete_child";
    acl(ancestor, "+a", entry);
    let result = i.reconcile(&fixture.runtime);
    acl(ancestor, "-a", entry);
    assert!(result.is_err());
    assert!(!i.skill_directory().exists());
    assert!(!i.tools().exists());
}

#[test]
fn permits_the_default_home_deny_delete_acl_without_rewriting_it() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    let ancestor = i.profile.parent().unwrap();
    let entry = "everyone deny delete";
    acl(ancestor, "+a", entry);
    let result = i.reconcile(&fixture.runtime);
    acl(ancestor, "-a", entry);
    result.unwrap();
    i.inspect(&fixture.runtime).unwrap();
}

#[test]
fn rejects_an_unrelated_write_acl_added_to_an_owned_skill() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    i.reconcile(&fixture.runtime).unwrap();
    let skill = i.skill_directory().join("SKILL.md");
    let bytes = fs::read(&skill).unwrap();
    let entry = "everyone allow write";
    acl(&skill, "+a", entry);
    let inspected = i.inspect(&fixture.runtime);
    let repaired = i.reconcile(&fixture.runtime);
    acl(&skill, "-a", entry);
    assert!(inspected.is_err());
    assert!(repaired.is_err());
    assert_eq!(fs::read(skill).unwrap(), bytes);
}

#[test]
fn inherited_read_only_file_acl_blocks_success_until_the_owner_corrects_it() {
    let fixture = Fixture::new();
    let i = &fixture.installation;
    let skills = i.profile.join("skills");
    fs::DirBuilder::new().mode(0o700).create(&skills).unwrap();
    let entry =
        "everyone allow read,readattr,readextattr,readsecurity,file_inherit,directory_inherit";
    acl(&skills, "+a", entry);

    assert_eq!(i.reconcile(&fixture.runtime), Err(Reason::OwnedFileChanged));
    let skill = i.skill_directory().join("SKILL.md");
    assert!(
        antiburn_cli_runtime::validate_owned_file(&skill)
            .unwrap_err()
            .to_string()
            .contains("extended ACL")
    );
    let skill_bytes = fs::read(&skill).unwrap();
    let receipt_path = i.tools().join("receipt.json");
    let receipt_bytes = fs::read(&receipt_path).unwrap();
    assert_eq!(i.inspect(&fixture.runtime), Err(Reason::OwnedFileChanged));
    assert_eq!(i.reconcile(&fixture.runtime), Err(Reason::OwnedFileChanged));
    assert_eq!(fs::read(&skill).unwrap(), skill_bytes);
    assert_eq!(fs::read(&receipt_path).unwrap(), receipt_bytes);

    // Only the fixture owner corrects the unsupported file ACL before retry.
    acl(&skills, "-a", entry);
    assert!(
        antiburn_cli_runtime::validate_owned_file(&skill)
            .unwrap_err()
            .to_string()
            .contains("extended ACL")
    );
    let corrected = std::process::Command::new("/bin/chmod")
        .arg("-N")
        .arg(&skill)
        .status()
        .unwrap();
    assert!(corrected.success());
    i.reconcile(&fixture.runtime).unwrap();
    i.inspect(&fixture.runtime).unwrap();
    assert_eq!(fs::read(skill).unwrap(), skill_bytes);
    assert_eq!(fs::read(receipt_path).unwrap(), receipt_bytes);
}

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use super::IntegrationPhase;
use super::Vendor;
#[cfg(target_os = "macos")]
use super::discovery::Profile;
use super::discovery::Runtime;
use super::lifecycle::{Installation, accepted_route, render_config};
use super::process::NativeRunner;
use super::transaction;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    state: PathBuf,
    env: BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let state = root.join("private");
        transaction::private_directory(&state).unwrap();
        Self {
            _temp: temp,
            root,
            state,
            env: BTreeMap::new(),
        }
    }

    #[cfg(target_os = "macos")]
    fn profile(&self, vendor: Vendor) -> Profile {
        let root = self.root.join(format!("{vendor:?}"));
        fs::create_dir(&root).unwrap();
        Profile { vendor, root }
    }

    #[cfg(target_os = "macos")]
    fn save_profile(&self, profile: &Profile, slug: &str) {
        transaction::publish(
            &self.state.join(format!("{slug}-profile.json")),
            None,
            &serde_json::to_vec(profile).unwrap(),
        )
        .unwrap();
    }

    fn installation(&self) -> Installation<'_, NativeRunner> {
        Installation {
            state: &self.state,
            resources: &self.root,
            home: &self.root,
            env: &self.env,
            runner: &NativeRunner,
        }
    }
}

fn cli() -> &'static Path {
    Path::new("/internal runtime/antiburn-cli")
}

fn node() -> Runtime {
    Runtime {
        executable: "/runtime with spaces/node".into(),
        version: "v22.0.0".into(),
    }
}

#[test]
fn direct_routes_have_one_owned_callback_per_boundary_and_round_trip_exactly() {
    for vendor in [
        Vendor::Claude,
        Vendor::Codex,
        Vendor::Cursor,
        Vendor::OpenCode,
    ] {
        let original = br#"{ // keep this comment
  "unknown": { "a": true },
  "hooks": { "Unrelated": [{"command":"keep"}] },
}"#;
        let (installed, deltas) = render_config(
            vendor,
            original,
            Some(&node()),
            Path::new("/durable/artifact"),
            cli(),
        )
        .unwrap();
        let receipt = transaction::Receipt::prepare(
            "/unused".into(),
            Some(original.to_vec()),
            installed.clone(),
            deltas,
            "binding".into(),
        )
        .unwrap();
        assert_eq!(receipt.rollback(&installed).unwrap(), original);
        let value = super::document::value(&installed, &[]).unwrap().unwrap();
        assert_eq!(value["unknown"]["a"], true);
        assert_eq!(value["hooks"]["Unrelated"][0]["command"], "keep");
        match vendor {
            Vendor::Claude => {
                assert_eq!(
                    value["hooks"]["UserPromptSubmit"].as_array().unwrap().len(),
                    1
                );
                assert_eq!(value["hooks"]["PostToolUse"].as_array().unwrap().len(), 1);
            }
            Vendor::Codex => assert_eq!(
                value["hooks"]["UserPromptSubmit"].as_array().unwrap().len(),
                1
            ),
            Vendor::Cursor => {
                assert_eq!(value["version"], 1);
                assert_eq!(value["hooks"]["postToolUse"].as_array().unwrap().len(), 1);
            }
            Vendor::OpenCode => assert_eq!(
                value["plugin"][0],
                "file:///durable/artifact/opencode/runtime/antiburn.mjs"
            ),
            Vendor::Pi => unreachable!(),
        }
    }
}

#[test]
fn optional_help_and_bundled_plugin_routes_are_not_installed() {
    let (bytes, _) = render_config(
        Vendor::Codex,
        b"{}",
        Some(&node()),
        Path::new("/durable/artifact"),
        cli(),
    )
    .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(!text.contains("plugin.json") && !text.contains("SKILL.md"));
    assert!(text.contains("if [ -x '/runtime with spaces/node' ]"));
    assert!(text.contains("ANTIBURN_EXECUTABLE='/internal runtime/antiburn-cli'"));
}

#[test]
fn generated_command_hooks_inject_the_internal_cli_with_empty_path() {
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    let fixture = Fixture::new();
    let executable = fixture.root.join("internal 'runtime' é $cli");
    let node_path = fixture.root.join("node 'runtime' é $node");
    fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    fs::write(
        &node_path,
        "#!/bin/sh\nprintf '%s' \"$ANTIBURN_EXECUTABLE\"\n",
    )
    .unwrap();
    for path in [&executable, &node_path] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let runtime = Runtime {
        executable: node_path,
        version: "v22.0.0".into(),
    };
    for vendor in [Vendor::Claude, Vendor::Codex, Vendor::Cursor] {
        let (config, _) =
            render_config(vendor, b"{}", Some(&runtime), &fixture.root, &executable).unwrap();
        let value = super::document::value(&config, &[]).unwrap().unwrap();
        let command = if vendor == Vendor::Cursor {
            value["hooks"]["postToolUse"][0]["command"]
                .as_str()
                .unwrap()
        } else {
            value["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
        };
        let output = Command::new("/bin/sh")
            .args(["-c", command])
            .env_clear()
            .env("PATH", "")
            .env("HOME", &fixture.root)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, executable.to_str().unwrap().as_bytes());
    }
}

#[test]
fn missing_node_and_unknown_cursor_schemas_fail_before_any_registration() {
    assert!(render_config(Vendor::Claude, b"{}", None, Path::new("/artifact"), cli()).is_err());
    for original in [br#"{"version":2}"#.as_slice(), br#"{"version":"1"}"#] {
        assert!(
            render_config(
                Vendor::Cursor,
                original,
                Some(&node()),
                Path::new("/artifact"),
                cli()
            )
            .is_err()
        );
    }
}

#[test]
fn manifest_does_not_infer_parity_or_accept_unknown_builds() {
    for vendor in Vendor::ALL {
        assert!(accepted_route(vendor, "unknown-or-newer-build").is_none());
    }
}

#[test]
fn absent_managed_cli_does_not_discover_or_create_vendor_profiles() {
    let fixture = Fixture::new();
    let before = fs::read_dir(&fixture.root).unwrap().count();
    let result = fixture.installation().reconcile(None);
    assert_eq!(result.len(), 5);
    assert!(result.iter().all(|s| s.reason == "managed_cli_unavailable"));
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), before);
}

#[test]
fn absent_cli_keeps_receipt_ownership_visible_for_removal() {
    let fixture = Fixture::new();
    let receipt = transaction::Receipt::prepare(
        fixture.root.join("settings.json"),
        None,
        b"{}".to_vec(),
        Vec::new(),
        "fixture".into(),
    )
    .unwrap();
    transaction::save_receipt(&fixture.state.join("claude-receipt.json"), &receipt).unwrap();
    let before = fs::read_dir(&fixture.root).unwrap().count();
    let statuses = fixture.installation().reconcile(None);
    assert!(
        statuses
            .iter()
            .find(|status| status.vendor == Vendor::Claude)
            .unwrap()
            .owned
    );
    assert!(
        statuses
            .iter()
            .all(|status| status.reason == "managed_cli_unavailable")
    );
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), before);
}

#[cfg(target_os = "macos")]
#[test]
fn two_restarts_do_not_duplicate_or_write_and_update_then_remove_keeps_user_changes() {
    let fixture = Fixture::new();
    let profile = fixture.profile(Vendor::Codex);
    fixture.save_profile(&profile, "codex");
    let target = profile.root.join("hooks.json");
    let original = "{ // keep\n\"unrelated\":1,\"hooks\":{\"UserPromptSubmit\":[]}}";
    fs::write(&target, original).unwrap();
    let installation = fixture.installation();
    assert_eq!(
        installation
            .reconcile_config(
                &profile,
                Some(&node()),
                Path::new("/artifact/one"),
                "one",
                cli()
            )
            .unwrap(),
        IntegrationPhase::AwaitingTrust
    );
    let first = fs::read(&target).unwrap();
    let modified = fs::metadata(&target).unwrap().modified().unwrap();
    for _ in 0..2 {
        installation
            .reconcile_config(
                &profile,
                Some(&node()),
                Path::new("/artifact/one"),
                "one",
                cli(),
            )
            .unwrap();
    }
    assert_eq!(fs::read(&target).unwrap(), first);
    assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), modified);
    fs::write(
        &target,
        String::from_utf8(first)
            .unwrap()
            .replace("\"unrelated\":1", "\"unrelated\":2"),
    )
    .unwrap();
    installation
        .reconcile_config(
            &profile,
            Some(&node()),
            Path::new("/artifact/two"),
            "two",
            cli(),
        )
        .unwrap();
    let updated = fs::read_to_string(&target).unwrap();
    assert!(!updated.contains("/artifact/one"));
    assert_eq!(updated.matches("/artifact/two").count(), 1);
    installation.remove(Vendor::Codex).unwrap();
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        original.replace("\"unrelated\":1", "\"unrelated\":2")
    );
    assert_eq!(
        installation
            .reconcile_config(
                &profile,
                Some(&node()),
                Path::new("/artifact/two"),
                "two",
                cli()
            )
            .unwrap(),
        IntegrationPhase::Disabled
    );
}

#[cfg(target_os = "macos")]
#[test]
fn failed_upgrade_keeps_the_active_hook_removable_and_retryable() {
    use std::os::unix::fs::PermissionsExt;

    for retry in [false, true] {
        let fixture = Fixture::new();
        let profile = fixture.profile(Vendor::Codex);
        fixture.save_profile(&profile, "codex");
        let target = profile.root.join("hooks.json");
        let original = b"{\"unrelated\":1}";
        fs::write(&target, original).unwrap();
        let installation = fixture.installation();
        installation
            .reconcile_config(
                &profile,
                Some(&node()),
                Path::new("/artifact/one"),
                "one",
                cli(),
            )
            .unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(
            installation
                .reconcile_config(
                    &profile,
                    Some(&node()),
                    Path::new("/artifact/two"),
                    "two",
                    cli()
                )
                .is_err()
        );
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let current = fs::read_to_string(&target)
            .unwrap()
            .replace("\"unrelated\":1", "\"unrelated\":2");
        fs::write(&target, current).unwrap();
        if retry {
            installation
                .reconcile_config(
                    &profile,
                    Some(&node()),
                    Path::new("/artifact/two"),
                    "two",
                    cli(),
                )
                .unwrap();
            let updated = fs::read_to_string(&target).unwrap();
            assert!(updated.contains("/artifact/two"));
            assert!(!updated.contains("/artifact/one"));
        }
        installation.remove(Vendor::Codex).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"{\"unrelated\":2}");
        let receipt = transaction::load_receipt(&fixture.state.join("codex-receipt.json"))
            .unwrap()
            .unwrap();
        assert!(receipt.removed);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn committed_upgrade_recovers_its_receipt_and_preserves_later_changes() {
    for changed_owned in [false, true] {
        let fixture = Fixture::new();
        let profile = fixture.profile(Vendor::Codex);
        fixture.save_profile(&profile, "codex");
        let target = profile.root.join("hooks.json");
        fs::write(&target, b"{\"unrelated\":1}").unwrap();
        let installation = fixture.installation();
        installation
            .reconcile_config(
                &profile,
                Some(&node()),
                Path::new("/artifact/one"),
                "one",
                cli(),
            )
            .unwrap();
        let receipt_path = fixture.state.join("codex-receipt.json");
        let active = transaction::load_receipt(&receipt_path).unwrap().unwrap();
        let current = fs::read(&target).unwrap();
        let baseline = active.rollback(&current).unwrap();
        let (installed, deltas) = render_config(
            Vendor::Codex,
            &baseline,
            Some(&node()),
            Path::new("/artifact/two"),
            cli(),
        )
        .unwrap();
        let mut prepared = transaction::Receipt::prepare(
            target.clone(),
            Some(current),
            installed.clone(),
            deltas,
            "two".into(),
        )
        .unwrap();
        prepared.previous = Some(Box::new(active));
        transaction::save_receipt(&receipt_path, &prepared).unwrap();
        let mut current = String::from_utf8(installed)
            .unwrap()
            .replace("\"unrelated\":1", "\"unrelated\":2");
        if changed_owned {
            current = current.replace("/artifact/two", "/user-owned");
        }
        fs::write(&target, &current).unwrap();
        if changed_owned {
            assert!(installation.remove(Vendor::Codex).is_err());
            assert_eq!(fs::read_to_string(&target).unwrap(), current);
            assert!(
                transaction::load_receipt(&receipt_path)
                    .unwrap()
                    .unwrap()
                    .previous
                    .is_some()
            );
        } else {
            installation.remove(Vendor::Codex).unwrap();
            assert_eq!(fs::read(&target).unwrap(), b"{\"unrelated\":2}");
            let removed = transaction::load_receipt(&receipt_path).unwrap().unwrap();
            assert!(removed.removed && removed.published && removed.previous.is_none());
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn unchanged_config_binding_update_retains_removal_ownership() {
    let fixture = Fixture::new();
    let profile = fixture.profile(Vendor::OpenCode);
    fixture.save_profile(&profile, "opencode");
    let target = profile.root.join("opencode.json");
    fs::write(&target, b"{}").unwrap();
    let installation = fixture.installation();
    for binding in ["one", "two"] {
        installation
            .reconcile_config(&profile, None, Path::new("/artifact/same"), binding, cli())
            .unwrap();
    }
    installation.remove(Vendor::OpenCode).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"{}");
}

#[cfg(target_os = "macos")]
#[test]
fn vendor_removal_is_not_reinstalled_and_owned_edits_are_not_overwritten() {
    let fixture = Fixture::new();
    let profile = fixture.profile(Vendor::Claude);
    let target = profile.root.join("settings.json");
    fs::write(&target, b"{}").unwrap();
    let installation = fixture.installation();
    installation
        .reconcile_config(
            &profile,
            Some(&node()),
            Path::new("/artifact"),
            "one",
            cli(),
        )
        .unwrap();
    let installed = fs::read(&target).unwrap();
    fs::write(&target, b"{}").unwrap();
    assert_eq!(
        installation
            .reconcile_config(
                &profile,
                Some(&node()),
                Path::new("/artifact"),
                "one",
                cli()
            )
            .unwrap(),
        IntegrationPhase::UserChanged
    );
    assert_eq!(fs::read(&target).unwrap(), b"{}");
    let changed = String::from_utf8(installed)
        .unwrap()
        .replace("/artifact", "/user-owned");
    fs::write(&target, &changed).unwrap();
    assert_eq!(
        installation
            .reconcile_config(&profile, Some(&node()), Path::new("/new"), "two", cli())
            .unwrap(),
        IntegrationPhase::UserChanged
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), changed);
}

#[cfg(target_os = "macos")]
#[test]
fn direct_claude_hooks_coexist_with_public_usage_plugin_and_preserve_unknown_settings() {
    let fixture = Fixture::new();
    let profile = fixture.profile(Vendor::Claude);
    let target = profile.root.join("settings.json");
    let original = br#"{"enabledPlugins":{"antiburn@release-usage":true},"unknown":{"a":42}}"#;
    fs::write(&target, original).unwrap();
    fixture
        .installation()
        .reconcile_config(
            &profile,
            Some(&node()),
            Path::new("/artifact"),
            "one",
            cli(),
        )
        .unwrap();
    let installed = fs::read(&target).unwrap();
    let value = super::document::value(&installed, &[]).unwrap().unwrap();
    assert_eq!(value["enabledPlugins"]["antiburn@release-usage"], true);
    assert_eq!(value["unknown"]["a"], 42);
    assert!(
        String::from_utf8(installed)
            .unwrap()
            .contains(std::str::from_utf8(&original[1..original.len() - 1]).unwrap())
    );
}

#[cfg(target_os = "macos")]
#[test]
fn disabled_policies_duplicates_and_malformed_configs_are_unchanged() {
    for original in [
        "{bad}",
        r#"{"hooks":{},"hooks":{}}"#,
        r#"{"disableAllHooks":true}"#,
        r#"{"allowManagedHooksOnly":true}"#,
        r#"{"enabledPlugins":{"antiburn-session-checks@local":true}}"#,
    ] {
        let fixture = Fixture::new();
        let profile = fixture.profile(Vendor::Claude);
        let target = profile.root.join("settings.json");
        fs::write(&target, original).unwrap();
        let result = fixture.installation().reconcile_config(
            &profile,
            Some(&node()),
            Path::new("/artifact"),
            "one",
            cli(),
        );
        assert!(result.is_err() || result.unwrap() == IntegrationPhase::Disabled);
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
        assert!(!fixture.state.join("claude-receipt.json").exists());
    }
}

#[cfg(target_os = "macos")]
#[test]
fn registered_claude_hooks_report_disabled_after_either_policy_changes() {
    for policy in ["disableAllHooks", "allowManagedHooksOnly"] {
        let fixture = Fixture::new();
        let profile = fixture.profile(Vendor::Claude);
        let target = profile.root.join("settings.json");
        let original = format!(r#"{{"{policy}":false}}"#);
        fs::write(&target, original).expect("write the initial Claude policy fixture");
        let installation = fixture.installation();
        installation
            .reconcile_config(
                &profile,
                Some(&node()),
                Path::new("/artifact"),
                "one",
                cli(),
            )
            .expect("register the Claude hooks before disabling policy");
        let disabled = fs::read_to_string(&target)
            .expect("read the registered Claude hooks fixture")
            .replace(
                &format!(r#""{policy}":false"#),
                &format!(r#""{policy}":true"#),
            );
        fs::write(&target, &disabled).expect("enable the disabling Claude policy fixture");
        let receipt_path = fixture.state.join("claude-receipt.json");
        let receipt =
            fs::read(&receipt_path).expect("read the receipt before disabled-policy inspection");
        assert_eq!(
            installation
                .inspect_profile(&profile)
                .expect("inspect the disabled Claude policy"),
            IntegrationPhase::Disabled
        );
        for binding in ["one", "two"] {
            assert_eq!(
                installation
                    .reconcile_config(
                        &profile,
                        Some(&node()),
                        Path::new("/artifact"),
                        binding,
                        cli(),
                    )
                    .expect("reconcile the disabled Claude policy without writing"),
                IntegrationPhase::Disabled
            );
        }
        assert_eq!(
            fs::read_to_string(target).expect("read the unchanged disabled Claude config"),
            disabled
        );
        assert_eq!(
            fs::read(receipt_path).expect("read the unchanged disabled Claude receipt"),
            receipt
        );
    }
}

#[cfg(target_os = "macos")]
#[test]
fn prepared_but_unpublished_transaction_requires_repair_and_keeps_original() {
    let fixture = Fixture::new();
    let profile = fixture.profile(Vendor::Codex);
    let target = profile.root.join("hooks.json");
    fs::write(&target, b"{}").unwrap();
    let (installed, deltas) = render_config(
        Vendor::Codex,
        b"{}",
        Some(&node()),
        Path::new("/artifact"),
        cli(),
    )
    .unwrap();
    let receipt = transaction::Receipt::prepare(
        target.clone(),
        Some(b"{}".to_vec()),
        installed,
        deltas,
        "one".into(),
    )
    .unwrap();
    transaction::save_receipt(&fixture.state.join("codex-receipt.json"), &receipt).unwrap();
    assert_eq!(
        fixture.installation().reconcile_config(
            &profile,
            Some(&node()),
            Path::new("/artifact"),
            "one",
            cli()
        ),
        Err("interrupted_installation_requires_repair")
    );
    assert_eq!(fs::read(&target).unwrap(), b"{}");
}

#[cfg(target_os = "macos")]
#[test]
fn status_inspection_detects_changes_without_repair_or_receipt_writes() {
    let fixture = Fixture::new();
    let profile = fixture.profile(Vendor::Codex);
    fixture.save_profile(&profile, "codex");
    let installation = fixture.installation();
    let target = profile.root.join("hooks.json");
    fs::write(&target, b"{}").unwrap();
    installation
        .reconcile_config(
            &profile,
            Some(&node()),
            Path::new("/artifact/one"),
            "one",
            cli(),
        )
        .unwrap();
    let receipt_path = fixture.state.join("codex-receipt.json");
    let receipt = fs::read(&receipt_path).unwrap();
    let installed = fs::read(&target).unwrap();
    let modified = fs::metadata(&target).unwrap().modified().unwrap();
    assert_eq!(
        installation.inspect_profile(&profile).unwrap(),
        IntegrationPhase::AwaitingTrust
    );
    assert_eq!(fs::read(&target).unwrap(), installed);
    assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), modified);
    fs::write(&target, b"{}").unwrap();
    assert_eq!(
        installation.inspect_profile(&profile).unwrap(),
        IntegrationPhase::UserChanged
    );
    assert_eq!(fs::read(&target).unwrap(), b"{}");
    assert_eq!(fs::read(&receipt_path).unwrap(), receipt);
    installation.remove(Vendor::Codex).unwrap();
    let removed_receipt = fs::read(&receipt_path).unwrap();
    assert_eq!(
        installation.inspect_profile(&profile).unwrap(),
        IntegrationPhase::Disabled
    );
    assert_eq!(fs::read(&receipt_path).unwrap(), removed_receipt);
    assert_eq!(fs::read(&target).unwrap(), b"{}");
}

#[test]
fn inspection_without_cli_creates_no_state_or_profiles() {
    let fixture = Fixture::new();
    let state = fixture.root.join("absent-state");
    let installation = Installation {
        state: &state,
        resources: &fixture.root,
        home: &fixture.root,
        env: &fixture.env,
        runner: &NativeRunner,
    };
    let statuses = installation.inspect(None);
    assert!(
        statuses
            .iter()
            .all(|value| value.reason == "managed_cli_unavailable")
    );
    assert!(!state.exists());
}

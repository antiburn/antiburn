use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use antiburn_local::model::AgentKind;

#[cfg(unix)]
use super::filesystem::file_ownership;
use super::*;

fn roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let project = temporary.path().join("project");
    fs::create_dir(&home).unwrap();
    fs::create_dir(&project).unwrap();
    (temporary, home, project)
}

fn write(path: &Path, value: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

fn operation(setting: ConfigSetting, expected: &str, proposed: &str) -> ConfigOperation {
    ConfigOperation {
        setting,
        expected_value: expected.into(),
        proposed_value: proposed.into(),
    }
}

#[cfg(not(windows))]
#[test]
fn vendor_setting_scope_cases_prepare_apply_and_read_back() {
    struct Case {
        agent: AgentKind,
        setting: ConfigSetting,
        global_path: &'static str,
        global: &'static str,
        project_path: &'static str,
        project: &'static str,
        expected: &'static str,
        proposed: &'static str,
        selector: &'static str,
    }

    let cases = [
        Case {
            agent: AgentKind::Claude,
            setting: ConfigSetting::Reasoning,
            global_path: ".claude/settings.json",
            global: r#"{"model":"claude-opus-5","effortLevel":"low"}"#,
            project_path: ".claude/settings.local.json",
            project: r#"{"effortLevel":"high"}"#,
            expected: "high",
            proposed: "medium",
            selector: "effortLevel",
        },
        Case {
            agent: AgentKind::Codex,
            setting: ConfigSetting::Reasoning,
            global_path: ".codex/config.toml",
            global: "model_reasoning_effort = \"low\"\n",
            project_path: ".codex/config.toml",
            project: "model_reasoning_effort = \"high\"\n",
            expected: "high",
            proposed: "medium",
            selector: "model_reasoning_effort",
        },
        Case {
            agent: AgentKind::OpenCode,
            setting: ConfigSetting::Model,
            global_path: ".config/opencode/opencode.json",
            global: r#"{"model":"provider/global"}"#,
            project_path: "opencode.jsonc",
            project: "{\n  // keep this comment\n  \"model\": \"provider/old\",\n}\n",
            expected: "provider/old",
            proposed: "provider/new",
            selector: "model",
        },
        Case {
            agent: AgentKind::Pi,
            setting: ConfigSetting::Model,
            global_path: ".pi/agent/settings.json",
            global: r#"{"defaultProvider":"a","defaultModel":"global"}"#,
            project_path: ".pi/settings.json",
            project: r#"{"defaultProvider":"a","defaultModel":"old"}"#,
            expected: "a/old",
            proposed: "b/new",
            selector: "defaultProvider+defaultModel",
        },
        Case {
            agent: AgentKind::Pi,
            setting: ConfigSetting::Reasoning,
            global_path: ".pi/agent/settings.json",
            global: r#"{"defaultProvider":"a","defaultModel":"m","defaultThinkingLevel":"low"}"#,
            project_path: ".pi/settings.json",
            project: r#"{"modelThinkingLevels":{"a/m":"high"}}"#,
            expected: "high",
            proposed: "medium",
            selector: "modelThinkingLevels",
        },
    ];

    for case in cases {
        let (_temporary, home, project) = roots();
        write(&home.join(case.global_path), case.global);
        if case.agent == AgentKind::Codex {
            let project_key = project.canonicalize().unwrap();
            let global = format!(
                "{}[projects.{}]\ntrust_level = \"trusted\"\n",
                case.global,
                toml_edit::Value::from(project_key.to_string_lossy().as_ref())
            );
            write(&home.join(case.global_path), &global);
        }
        let path = project.join(case.project_path);
        write(&path, case.project);
        let context = ConfigContext::native(case.agent, &home, Some(project));
        let editor = AgentConfigEditor::new();
        let effective = editor.effective(&context, case.setting).unwrap();
        assert_eq!(effective.scope, ConfigScope::Project, "{:?}", case.agent);
        assert_eq!(effective.value, case.expected, "{:?}", case.agent);
        assert_eq!(effective.physical_identity().1, case.selector);

        let prepared = editor
            .prepare_operation(
                &context,
                &operation(case.setting, case.expected, case.proposed),
            )
            .unwrap();
        assert_eq!(prepared.physical_identity().1, case.selector);
        editor.apply(&prepared).unwrap();
        assert_eq!(
            editor.effective(&context, case.setting).unwrap().value,
            case.proposed
        );
        if case.agent == AgentKind::OpenCode {
            assert!(
                fs::read_to_string(path)
                    .unwrap()
                    .contains("// keep this comment")
            );
        }
    }
}

#[test]
fn inherited_settings_use_global_scope() {
    let cases = [
        (
            AgentKind::Claude,
            ConfigSetting::Reasoning,
            ".claude/settings.json",
            r#"{"effortLevel":"high"}"#,
            ".claude/settings.json",
            r#"{"model":"project"}"#,
        ),
        (
            AgentKind::Pi,
            ConfigSetting::Reasoning,
            ".pi/agent/settings.json",
            r#"{"defaultProvider":"a","defaultModel":"m","defaultThinkingLevel":"high"}"#,
            ".pi/settings.json",
            r#"{"theme":"dark"}"#,
        ),
    ];
    for (agent, setting, global_path, global, project_path, project_value) in cases {
        let (_temporary, home, project) = roots();
        write(&home.join(global_path), global);
        write(&project.join(project_path), project_value);
        let effective = AgentConfigEditor::new()
            .effective(&ConfigContext::native(agent, &home, Some(project)), setting)
            .unwrap();
        assert_eq!(effective.scope, ConfigScope::Global, "{agent:?}");
        assert_eq!(effective.value, "high", "{agent:?}");
    }
}

#[test]
fn codex_profiles_fail_closed_for_each_setting() {
    for setting in [ConfigSetting::Model, ConfigSetting::Reasoning] {
        let (_temporary, home, project) = roots();
        write(
            &home.join(".codex/config.toml"),
            "model = \"old\"\nmodel_reasoning_effort = \"high\"\nprofile = \"work\"\n",
        );
        assert_eq!(
            AgentConfigEditor::new().effective(
                &ConfigContext::native(AgentKind::Codex, home, Some(project)),
                setting,
            ),
            Err(ConfigUnavailableReason::RuntimeOverride)
        );
    }
}

#[cfg(not(windows))]
#[test]
fn claude_edits_an_exact_saved_model_effort() {
    let (_temporary, home, project) = roots();
    let path = home.join(".claude/settings.json");
    write(
        &path,
        r#"{"model":"claude-opus-5","effortLevel":"low","modelSettings":{"claude-opus-5":{"effortLevel":"high"}}}"#,
    );
    let context = ConfigContext::native(AgentKind::Claude, &home, Some(project));
    let editor = AgentConfigEditor::new();
    let effective = editor
        .effective(&context, ConfigSetting::Reasoning)
        .unwrap();
    assert_eq!(effective.value, "high");
    assert_eq!(effective.physical_identity().1, "modelSettings.effortLevel");
    let prepared = editor
        .prepare_operation(
            &context,
            &operation(ConfigSetting::Reasoning, "high", "medium"),
        )
        .unwrap();
    editor.apply(&prepared).unwrap();
    let document: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(
        document["modelSettings"]["claude-opus-5"]["effortLevel"],
        "medium"
    );
    assert_eq!(document["effortLevel"], "low");
}

#[test]
fn claude_allows_unrelated_saved_model_effort() {
    let (_temporary, home, project) = roots();
    write(
        &home.join(".claude/settings.json"),
        r#"{"model":"opus","effortLevel":"low","modelSettings":{"claude-opus-5":{"effortLevel":"high"}}}"#,
    );
    assert_eq!(
        AgentConfigEditor::new()
            .effective(
                &ConfigContext::native(AgentKind::Claude, home, Some(project)),
                ConfigSetting::Reasoning,
            )
            .unwrap()
            .value,
        "low"
    );
}

#[test]
fn vendor_precedence_cases_are_table_driven() {
    struct Case {
        name: &'static str,
        agent: AgentKind,
        global_path: &'static str,
        global: &'static str,
        cwd_path: &'static str,
        cwd: &'static str,
        expected: Result<&'static str, ConfigUnavailableReason>,
    }

    let cases = [
        Case {
            name: "pi same project values",
            agent: AgentKind::Pi,
            global_path: ".pi/agent/settings.json",
            global: r#"{"defaultProvider":"a","defaultModel":"m"}"#,
            cwd_path: ".pi/settings.json",
            cwd: r#"{"defaultProvider":"a","defaultModel":"m"}"#,
            expected: Ok("a/m"),
        },
        Case {
            name: "pi different project values",
            agent: AgentKind::Pi,
            global_path: ".pi/agent/settings.json",
            global: r#"{"defaultProvider":"a","defaultModel":"m"}"#,
            cwd_path: ".pi/settings.json",
            cwd: r#"{"defaultProvider":"b","defaultModel":"n"}"#,
            expected: Ok("b/n"),
        },
        Case {
            name: "pi provider spans files",
            agent: AgentKind::Pi,
            global_path: ".pi/agent/settings.json",
            global: r#"{"defaultProvider":"a","defaultModel":"m"}"#,
            cwd_path: ".pi/settings.json",
            cwd: r#"{"defaultProvider":"a"}"#,
            expected: Err(ConfigUnavailableReason::SplitModelRoute),
        },
        Case {
            name: "pi model spans files",
            agent: AgentKind::Pi,
            global_path: ".pi/agent/settings.json",
            global: r#"{"defaultProvider":"a","defaultModel":"m"}"#,
            cwd_path: ".pi/settings.json",
            cwd: r#"{"defaultModel":"m"}"#,
            expected: Err(ConfigUnavailableReason::SplitModelRoute),
        },
        Case {
            name: "claude exact active model override",
            agent: AgentKind::Claude,
            global_path: ".claude/settings.json",
            global: r#"{"model":"claude-opus-5","effortLevel":"low"}"#,
            cwd_path: ".claude/settings.json",
            cwd: r#"{"modelSettings":{"other":{"effortLevel":"medium"},"claude-opus-5":{"effortLevel":"high"}}}"#,
            expected: Ok("high"),
        },
        Case {
            name: "claude unrelated model override",
            agent: AgentKind::Claude,
            global_path: ".claude/settings.json",
            global: r#"{"model":"claude-opus-5","effortLevel":"low"}"#,
            cwd_path: ".claude/settings.json",
            cwd: r#"{"modelSettings":{"other":{"effortLevel":"medium"}}}"#,
            expected: Ok("low"),
        },
    ];

    for case in cases {
        let (_temporary, home, cwd) = roots();
        write(&home.join(case.global_path), case.global);
        write(&cwd.join(case.cwd_path), case.cwd);
        let setting = if case.agent == AgentKind::Claude {
            ConfigSetting::Reasoning
        } else {
            ConfigSetting::Model
        };
        let actual = AgentConfigEditor::new()
            .effective(&ConfigContext::native(case.agent, home, Some(cwd)), setting)
            .map(|effective| effective.value);
        match case.expected {
            Ok(expected) => assert_eq!(actual.as_deref(), Ok(expected), "{}", case.name),
            Err(expected) => assert_eq!(actual, Err(expected), "{}", case.name),
        }
    }
}

#[test]
fn cwd_local_vendor_cases_use_the_exact_working_directory() {
    struct Case {
        agent: AgentKind,
        global_path: &'static str,
        global: &'static str,
        root_path: &'static str,
        root: &'static str,
        nested_path: &'static str,
        nested: &'static str,
        expected: &'static str,
    }
    let cases = [
        Case {
            agent: AgentKind::OpenCode,
            global_path: ".config/opencode/opencode.json",
            global: r#"{"model":"a/global"}"#,
            root_path: "opencode.json",
            root: r#"{"model":"a/root"}"#,
            nested_path: ".opencode/opencode.jsonc",
            nested: "{\n  // nested wins\n  \"model\": \"a/nested\"\n}",
            expected: "a/nested",
        },
        Case {
            agent: AgentKind::Pi,
            global_path: ".pi/agent/settings.json",
            global: r#"{"defaultProvider":"a","defaultModel":"global"}"#,
            root_path: ".pi/settings.json",
            root: r#"{"defaultProvider":"a","defaultModel":"root"}"#,
            nested_path: ".pi/settings.json",
            nested: r#"{"defaultProvider":"a","defaultModel":"nested"}"#,
            expected: "a/nested",
        },
    ];

    for case in cases {
        let (_temporary, home, root) = roots();
        let nested = root.join("packages/app");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        write(&home.join(case.global_path), case.global);
        write(&root.join(case.root_path), case.root);
        write(&nested.join(case.nested_path), case.nested);
        let effective = AgentConfigEditor::new()
            .effective_model(&ConfigContext::native_workspace(
                case.agent, home, nested, root,
            ))
            .unwrap();
        assert_eq!(effective.value, case.expected, "{:?}", case.agent);
    }
}

#[test]
fn nested_workspace_resolution_is_table_driven() {
    struct Case {
        agent: AgentKind,
        global_path: &'static str,
        global: &'static str,
        nested_path: &'static str,
        nested: &'static str,
        project_value: &'static str,
        global_value: &'static str,
    }
    let cases = [
        Case {
            agent: AgentKind::OpenCode,
            global_path: ".config/opencode/opencode.json",
            global: r#"{"model":"a/global"}"#,
            nested_path: "opencode.json",
            nested: r#"{"model":"a/project"}"#,
            project_value: "a/project",
            global_value: "a/global",
        },
        Case {
            agent: AgentKind::Pi,
            global_path: ".pi/agent/settings.json",
            global: r#"{"defaultProvider":"a","defaultModel":"global"}"#,
            nested_path: ".pi/settings.json",
            nested: r#"{"defaultProvider":"a","defaultModel":"project"}"#,
            project_value: "a/project",
            global_value: "a/global",
        },
    ];

    for case in cases {
        let (_temporary, home, root) = roots();
        let nested = root.join("packages/app");
        fs::create_dir_all(&nested).unwrap();
        write(&home.join(case.global_path), case.global);
        let context = ConfigContext::native_workspace(case.agent, &home, &nested, &root);
        let inherited = AgentConfigEditor::new().effective_model(&context).unwrap();
        assert_eq!(inherited.scope, ConfigScope::Global, "{:?}", case.agent);
        assert_eq!(inherited.value, case.global_value, "{:?}", case.agent);

        write(&nested.join(case.nested_path), case.nested);
        let project = AgentConfigEditor::new().effective_model(&context).unwrap();
        assert_eq!(project.scope, ConfigScope::Project, "{:?}", case.agent);
        assert_eq!(project.value, case.project_value, "{:?}", case.agent);
        assert!(
            project
                .physical_identity()
                .0
                .starts_with(nested.canonicalize().unwrap())
        );
    }
}

#[test]
fn equal_root_and_nested_values_keep_distinct_physical_identity() {
    let cases = [
        (
            AgentKind::OpenCode,
            "opencode.json",
            r#"{"model":"a/same"}"#,
        ),
        (
            AgentKind::Pi,
            ".pi/settings.json",
            r#"{"defaultProvider":"a","defaultModel":"same"}"#,
        ),
    ];
    for (agent, relative, contents) in cases {
        let (_temporary, home, root) = roots();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        write(&root.join(relative), contents);
        let editor = AgentConfigEditor::new();
        let root_effective = editor
            .effective_model(&ConfigContext::native(agent, &home, Some(root.clone())))
            .unwrap();
        write(&nested.join(relative), contents);
        let nested_effective = editor
            .effective_model(&ConfigContext::native_workspace(
                agent, &home, &nested, &root,
            ))
            .unwrap();
        assert_eq!(root_effective.value, nested_effective.value, "{agent:?}");
        assert_ne!(
            root_effective.physical_identity(),
            nested_effective.physical_identity(),
            "{agent:?}"
        );
    }
}

#[test]
fn untrusted_workspace_context_fails_closed() {
    let (_temporary, home, trusted) = roots();
    let outside = trusted.parent().unwrap().join("outside");
    fs::create_dir(&outside).unwrap();
    write(
        &home.join(".config/opencode/opencode.json"),
        r#"{"model":"a/global"}"#,
    );
    let context = ConfigContext::native_workspace(AgentKind::OpenCode, home, outside, trusted);
    assert_eq!(
        AgentConfigEditor::new().effective_model(&context),
        Err(ConfigUnavailableReason::UnsafePath)
    );
}

#[test]
fn codex_nested_workspace_is_ambiguous() {
    let (_temporary, home, root) = roots();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    write(&home.join(".codex/config.toml"), "model = \"global\"\n");
    assert_eq!(
        AgentConfigEditor::new().effective_model(&ConfigContext::native_workspace(
            AgentKind::Codex,
            home,
            nested,
            root,
        )),
        Err(ConfigUnavailableReason::InvalidPrecedence)
    );
}

#[cfg(not(windows))]
#[test]
fn apply_rejects_new_nested_precedence_for_cwd_local_vendors() {
    let cases = [
        (
            AgentKind::OpenCode,
            ".config/opencode/opencode.json",
            r#"{"model":"a/old"}"#,
            "opencode.json",
            r#"{"model":"a/other"}"#,
            "a/old",
            "a/new",
        ),
        (
            AgentKind::Pi,
            ".pi/agent/settings.json",
            r#"{"defaultProvider":"a","defaultModel":"old"}"#,
            ".pi/settings.json",
            r#"{"defaultProvider":"a","defaultModel":"other"}"#,
            "a/old",
            "a/new",
        ),
    ];
    for (agent, global_path, global, new_path, new_config, expected, proposed) in cases {
        let (_temporary, home, root) = roots();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        write(&home.join(global_path), global);
        let context = ConfigContext::native_workspace(agent, &home, &nested, &root);
        let editor = AgentConfigEditor::new();
        let prepared = editor
            .prepare(
                &context,
                &ConfigChange {
                    expected_value: expected.into(),
                    proposed_value: proposed.into(),
                },
            )
            .unwrap();
        let canonical_nested = nested.canonicalize().unwrap();
        let canonical_root = root.canonicalize().unwrap();
        assert_eq!(
            prepared.workspace_cwd.as_deref(),
            Some(canonical_nested.as_path())
        );
        assert_eq!(
            prepared.trusted_workspace_root.as_deref(),
            Some(canonical_root.as_path())
        );

        write(&nested.join(new_path), new_config);
        assert_eq!(
            editor.apply(&prepared),
            Err(ApplyError::Conflict(ApplyConflict::ChangedIdentity)),
            "{agent:?}"
        );
    }
}

#[test]
fn claude_settings_environment_overrides_fail_closed() {
    let cases = [
        (
            ConfigSetting::Model,
            r#"{"model":"claude-opus-5","env":{"ANTHROPIC_MODEL":"other"}}"#,
        ),
        (
            ConfigSetting::Reasoning,
            r#"{"model":"claude-opus-5","effortLevel":"high","env":{"CLAUDE_CODE_EFFORT_LEVEL":"low"}}"#,
        ),
    ];
    for (setting, contents) in cases {
        let (_temporary, home, project) = roots();
        write(&home.join(".claude/settings.json"), contents);
        assert_eq!(
            AgentConfigEditor::new().effective(
                &ConfigContext::native(AgentKind::Claude, home, Some(project)),
                setting,
            ),
            Err(ConfigUnavailableReason::RuntimeOverride)
        );
    }
}

#[test]
fn opencode_unsafe_precedence_cases_fail_closed() {
    enum UnsafeCase {
        Dynamic,
        Agent,
    }
    for case in [UnsafeCase::Dynamic, UnsafeCase::Agent] {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let parent = temporary.path().join("parent");
        let project = parent.join("project");
        fs::create_dir(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        write(
            &home.join(".config/opencode/opencode.json"),
            r#"{"model":"provider/global"}"#,
        );
        match case {
            UnsafeCase::Dynamic => {
                write(&project.join("opencode.json"), r#"{"model":"{env:MODEL}"}"#)
            }
            UnsafeCase::Agent => write(
                &project.join("opencode.json"),
                r#"{"model":"a/b","agent":{"build":{"model":"c/d"}}}"#,
            ),
        }
        assert!(matches!(
            AgentConfigEditor::new().effective(
                &ConfigContext::native(AgentKind::OpenCode, home, Some(project)),
                ConfigSetting::Model,
            ),
            Err(ConfigUnavailableReason::InvalidPrecedence)
                | Err(ConfigUnavailableReason::DynamicValue)
        ));
    }
}

#[test]
fn opencode_file_precedence_is_table_driven() {
    struct Case {
        name: &'static str,
        direct_root: Option<&'static str>,
        direct_nested: Option<&'static str>,
        dot_root: Option<&'static str>,
        dot_nested: Option<&'static str>,
        expected: &'static str,
    }
    let cases = [
        Case {
            name: "jsonc overrides json in one directory",
            direct_root: Some("a/json"),
            direct_nested: Some("a/jsonc"),
            dot_root: None,
            dot_nested: None,
            expected: "a/jsonc",
        },
        Case {
            name: "nested direct config overrides root direct config",
            direct_root: Some("a/root"),
            direct_nested: Some("a/nested"),
            dot_root: None,
            dot_nested: None,
            expected: "a/nested",
        },
        Case {
            name: "root dot config overrides nested dot config",
            direct_root: None,
            direct_nested: None,
            dot_root: Some("a/root-dot"),
            dot_nested: Some("a/nested-dot"),
            expected: "a/root-dot",
        },
        Case {
            name: "dot config overrides direct config",
            direct_root: None,
            direct_nested: Some("a/direct"),
            dot_root: Some("a/dot"),
            dot_nested: None,
            expected: "a/dot",
        },
    ];

    for case in cases {
        let (_temporary, home, root) = roots();
        let nested = root.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        write(
            &home.join(".config/opencode/opencode.json"),
            r#"{"model":"a/global"}"#,
        );
        if let Some(model) = case.direct_root {
            write(
                &root.join("opencode.json"),
                &format!(r#"{{"model":"{model}"}}"#),
            );
        }
        if let Some(model) = case.direct_nested {
            if case.name.starts_with("jsonc") {
                write(
                    &root.join("opencode.jsonc"),
                    &format!(r#"{{"model":"{model}"}}"#),
                );
            } else {
                write(
                    &nested.join("opencode.json"),
                    &format!(r#"{{"model":"{model}"}}"#),
                );
            }
        }
        if let Some(model) = case.dot_root {
            write(
                &root.join(".opencode/opencode.json"),
                &format!(r#"{{"model":"{model}"}}"#),
            );
        }
        if let Some(model) = case.dot_nested {
            write(
                &nested.join(".opencode/opencode.json"),
                &format!(r#"{{"model":"{model}"}}"#),
            );
        }
        let actual = AgentConfigEditor::new()
            .effective_model(&ConfigContext::native_workspace(
                AgentKind::OpenCode,
                home,
                nested,
                root,
            ))
            .unwrap();
        assert_eq!(actual.value, case.expected, "{}", case.name);
    }
}

#[cfg(not(windows))]
#[test]
fn apply_rejects_a_setting_precedence_change() {
    let (_temporary, home, project) = roots();
    write(
        &home.join(".pi/agent/settings.json"),
        r#"{"defaultProvider":"a","defaultModel":"m","defaultThinkingLevel":"high"}"#,
    );
    let context = ConfigContext::native(AgentKind::Pi, &home, Some(project.clone()));
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_operation(
            &context,
            &operation(ConfigSetting::Reasoning, "high", "medium"),
        )
        .unwrap();
    write(
        &project.join(".pi/settings.json"),
        r#"{"defaultThinkingLevel":"low"}"#,
    );
    assert_eq!(
        editor.apply(&prepared),
        Err(ApplyError::Conflict(ApplyConflict::ChangedIdentity))
    );
}

#[cfg(not(windows))]
#[test]
fn apply_rejects_a_content_conflict() {
    let (_temporary, home, project) = roots();
    let path = home.join(".claude/settings.json");
    write(&path, r#"{"model":"old"}"#);
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare(
            &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
            &ConfigChange {
                expected_value: "old".into(),
                proposed_value: "new".into(),
            },
        )
        .unwrap();
    write(&path, r#"{"model":"other"}"#);
    assert_eq!(
        editor.apply(&prepared),
        Err(ApplyError::Conflict(ApplyConflict::ChangedContent))
    );
}

#[cfg(unix)]
#[test]
fn apply_preserves_mode_owner_and_group() {
    let (_temporary, home, project) = roots();
    let path = home.join(".claude/settings.json");
    write(&path, r#"{"model":"old"}"#);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o764)).unwrap();
    let ownership = file_ownership(&fs::metadata(&path).unwrap());
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare(
            &ConfigContext::native(AgentKind::Claude, &home, Some(project)),
            &ConfigChange {
                expected_value: "old".into(),
                proposed_value: "new".into(),
            },
        )
        .unwrap();
    editor.apply(&prepared).unwrap();
    let metadata = fs::metadata(path).unwrap();
    assert_eq!(metadata.permissions().mode() & 0o777, 0o764);
    assert_eq!(file_ownership(&metadata), ownership);
}

#[cfg(unix)]
#[test]
fn a_symlinked_config_directory_is_rejected() {
    use std::os::unix::fs::symlink;

    let (_temporary, home, project) = roots();
    let actual = home.join("actual-claude");
    fs::create_dir(&actual).unwrap();
    write(&actual.join("settings.json"), r#"{"model":"old"}"#);
    symlink(&actual, home.join(".claude")).unwrap();
    assert_eq!(
        AgentConfigEditor::new().effective_model(&ConfigContext::native(
            AgentKind::Claude,
            &home,
            Some(project)
        )),
        Err(ConfigUnavailableReason::SymlinkTarget)
    );
}

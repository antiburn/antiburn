use super::super::{AgentConfigEditor, ConfigChange, ConfigOperation, ConfigOperationValue};
use super::*;
use serde_json::{Value, json};
#[cfg(not(windows))]
use std::path::Path;
use std::{fs, path::PathBuf};

struct Fixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
    global: PathBuf,
    context: ConfigContext,
    entries: Vec<Value>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let home = temporary.path().join("home");
        let cwd = temporary.path().join("repo/package");
        let global = home.join(".config/opencode");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let context = ConfigContext::native_workspace(
            antiburn_local::model::AgentKind::OpenCode,
            &home,
            &cwd,
            temporary.path().join("repo"),
        );
        Self {
            _temporary: temporary,
            home,
            cwd,
            global,
            context,
            entries: Vec::new(),
        }
    }
    fn document(&mut self, path: PathBuf, raw: Value, canonical: Value) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        self.entries
            .push(json!({"type":"document","path":path,"info":canonical}));
    }
    fn directory(&mut self, path: PathBuf) {
        fs::create_dir_all(&path).unwrap();
        self.entries.push(json!({"type":"directory","path":path}));
    }
    fn authority(&self) -> CurrentConfigAuthority {
        CurrentConfigAuthority::fixture(&self.cwd, Value::Array(self.entries.clone()))
    }
    fn resolve(&self, subject: OpenCodeConfigSubject) -> OpenCodeResolvedTarget {
        AgentConfigEditor::new()
            .resolve_opencode(&self.context, &self.authority(), subject)
            .unwrap()
    }
    #[cfg(not(windows))]
    fn refresh_document(&mut self, path: &Path, canonical: Value) {
        self.entries
            .iter_mut()
            .find(|entry| entry.get("path") == Some(&json!(path)))
            .unwrap()["info"] = canonical;
    }
}

#[test]
fn api_order_selects_outer_direct_then_inner_dot_config_even_above_repository() {
    let mut fixture = Fixture::new();
    let outside = fixture._temporary.path().join("opencode.json");
    fixture.document(
        outside,
        json!({"model":"openai/outside"}),
        json!({"model":{"providerID":"openai","model":"outside"}}),
    );
    let direct = fixture.cwd.join("opencode.json");
    fixture.document(
        direct,
        json!({"model":"openai/direct"}),
        json!({"model":{"providerID":"openai","model":"direct"}}),
    );
    let outer_dot = fixture
        .cwd
        .parent()
        .unwrap()
        .join(".opencode/opencode.json");
    fixture.document(
        outer_dot,
        json!({"model":"openai/outer-dot"}),
        json!({"model":{"providerID":"openai","model":"outer-dot"}}),
    );
    let inner_dot = fixture.cwd.join(".opencode/opencode.jsonc");
    fixture.document(
        inner_dot.clone(),
        json!({"model":"openai/inner-dot"}),
        json!({"model":{"providerID":"openai","model":"inner-dot"}}),
    );
    let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
    assert_eq!(target.physical_path(), inner_dot);
    assert_eq!(target.current_value()["model"], "inner-dot");
    assert_eq!(target.scope(), ConfigScope::Project);
    assert_eq!(
        target.resolution_digest(),
        fixture
            .resolve(OpenCodeConfigSubject::FutureDefaultModel)
            .resolution_digest()
    );
    assert_eq!(
        target.raw_selector(),
        super::super::PhysicalSelector::Jsonc(vec!["model".into()])
    );
}

#[test]
fn missing_consumer_authority_blocks_legacy_findings_without_writing() {
    let fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fs::write(&path, r#"{"model":"openai/old"}"#).unwrap();
    let mut context = fixture.context.clone();
    context.source_format = Some(antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1);
    assert!(matches!(
        AgentConfigEditor::new().prepare(
            &context,
            &ConfigChange {
                expected_value: "openai/old".into(),
                proposed_value: "openai/new".into()
            }
        ),
        Err(ConfigUnavailableReason::RuntimeOverride)
    ));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        r#"{"model":"openai/old"}"#
    );
    assert!(!path.with_extension("json.bak").exists());
}

#[cfg(windows)]
#[test]
fn windows_preparation_rejects_current_config_writes_without_backup() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(
        path.clone(),
        json!({"model":"openai/old"}),
        json!({"model":{"providerID":"openai","model":"old"}}),
    );
    let original = fs::read(&path).unwrap();
    let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
    assert!(matches!(
        AgentConfigEditor::new().prepare_opencode_operation(&target, json!("openai/new")),
        Err(ConfigUnavailableReason::AutomaticApplyUnsupported)
    ));
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(!path.with_extension("json.bak").exists());
}

#[cfg(not(windows))]
#[test]
fn runtime_route_changes_block_prepared_model_edits_before_backup() {
    for override_info in [
        json!({"providers":{"openai":{"settings":{"baseURL":"https://invalid.example"}}}}),
        json!({"providers":{"openai":{"models":{"new":{"modelID":"other"}}}}}),
        json!({"providers":{"openai":{"body":{"baseURL":"https://invalid.example"}}}}),
        json!({"providers":{"openai":{"canonical":"other-provider"}}}),
        json!({"providers":{"openai":{"package":"custom-provider-package"}}}),
        json!({"providers":{"openai":{"headers":{"x-route":"alternate"}}}}),
        json!({"agents":{"build":{"model":{"providerID":"openai","model":"old"},"mode":"primary"}}}),
    ] {
        let mut fixture = Fixture::new();
        let path = fixture.global.join("opencode.json");
        fixture.document(
            path.clone(),
            json!({"model":"openai/old"}),
            json!({"model":{"providerID":"openai","model":"old"}}),
        );
        let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
        let editor = AgentConfigEditor::new();
        let prepared = editor
            .prepare_opencode_operation(&target, json!("openai/new"))
            .unwrap();
        fixture
            .entries
            .push(json!({"type":"document","info":override_info}));
        assert!(write::apply_fixture(&editor, &prepared, &fixture.authority()).is_err());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            r#"{"model":"openai/old"}"#
        );
        assert!(!path.with_extension("json.bak").exists());
    }
}

#[cfg(not(windows))]
#[test]
fn runtime_route_changes_block_model_readback() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(
        path.clone(),
        json!({"model":"openai/old"}),
        json!({"model":{"providerID":"openai","model":"old"}}),
    );
    let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_opencode_operation(&target, json!("openai/new"))
        .unwrap();
    write::apply_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    fixture.refresh_document(
        &path,
        json!({"model":{"providerID":"openai","model":"new"}}),
    );
    write::readback_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    fixture.entries.push(json!({"type":"document","info":{"providers":{"openai":{"settings":{"baseURL":"https://invalid.example"}}}}}));
    assert!(matches!(
        write::readback_fixture(&editor, &prepared, &fixture.authority()),
        Err(super::super::ApplyError::Readback(
            super::super::ApplyReadbackError::SemanticMismatch
        ))
    ));
}

#[cfg(not(windows))]
#[test]
fn native_model_prepare_apply_and_readback_keep_exact_backup() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(path.clone(), json!({"model":{"providerID":"openai","model":"old"},"compaction":{"reserved":40000,"tail_turns":8}}), json!({"model":{"providerID":"openai","model":"old"},"compaction":{"buffer":40000}}));
    let original = fs::read(&path).unwrap();
    let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_opencode_operation(&target, json!("openai/new"))
        .unwrap();
    assert!(prepared.retained_bytes() < 4 * 1024 * 1024);
    write::apply_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    assert_eq!(fs::read(path.with_extension("json.bak")).unwrap(), original);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["compaction"]["tail_turns"],
        8
    );
    fixture.refresh_document(
        &path,
        json!({"model":{"providerID":"openai","model":"new"},"compaction":{"buffer":40000}}),
    );
    write::readback_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    assert!(editor.apply(&prepared.operation).is_err());
}

#[cfg(not(windows))]
#[test]
fn compaction_only_enables_existing_false_control() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(
        path,
        json!({"compaction":{"auto":false,"reserved":40000,"preserve_recent_tokens":15000}}),
        json!({"compaction":{"auto":false,"buffer":40000,"keep":{"tokens":15000}}}),
    );
    let target = fixture.resolve(OpenCodeConfigSubject::AutomaticCompaction);
    let editor = AgentConfigEditor::new();
    assert!(
        editor
            .prepare_opencode_operation(&target, json!(20000))
            .is_err()
    );
    let prepared = editor
        .prepare_opencode_operation(&target, json!(true))
        .unwrap();
    write::apply_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    let value: Value = serde_json::from_slice(&fs::read(&target.path).unwrap()).unwrap();
    assert_eq!(
        value["compaction"],
        json!({"auto":true,"reserved":40000,"preserve_recent_tokens":15000})
    );
}

#[test]
fn named_agent_uses_native_entry_instead_of_another_matching_model() {
    let mut fixture = Fixture::new();
    let path = fixture.cwd.join("opencode.json");
    fixture.document(path, json!({"agent":{"review":{"model":"openai/legacy"}},"agents":{"review":{"model":"openai/native"},"other":{"model":"openai/native"}}}), json!({"agents":{"review":{"model":{"providerID":"openai","model":"native"}},"other":{"model":{"providerID":"openai","model":"native"}}}}));
    let target = fixture.resolve(OpenCodeConfigSubject::AgentModel("review".into()));
    assert_eq!(
        target.raw_selector(),
        super::super::PhysicalSelector::Json(vec![
            "agents".into(),
            "review".into(),
            "model".into()
        ])
    );
}

#[test]
fn model_less_higher_priority_agent_keeps_the_lower_layer_model() {
    let mut fixture = Fixture::new();
    let lower = fixture.global.join("opencode.json");
    fixture.document(
        lower,
        json!({"agents":{"review":{"model":"openai/lower"}}}),
        json!({"agents":{"review":{"model":{"providerID":"openai","model":"lower"}}}}),
    );
    let higher = fixture.cwd.join("opencode.json");
    fixture.document(
        higher,
        json!({"agents":{"review":{"system":"Use the local review rules."}}}),
        json!({"agents":{"review":{"system":"Use the local review rules."}}}),
    );
    assert_eq!(
        fixture
            .resolve(OpenCodeConfigSubject::AgentModel("review".into()))
            .current_value()["model"],
        "lower"
    );
}

#[test]
fn model_less_higher_priority_agent_document_keeps_lower_layer_model() {
    let mut fixture = Fixture::new();
    fixture.directory(fixture.global.clone());
    let lower = fixture.global.join("agents/review.md");
    fs::create_dir_all(lower.parent().unwrap()).unwrap();
    fs::write(&lower, "---\nmodel: openai/lower\n---\n").unwrap();
    let lower_agents = fixture.global.join("agents");
    fixture.directory(lower_agents);

    let higher_agents = fixture.cwd.join(".opencode/agents");
    fs::create_dir_all(&higher_agents).unwrap();
    fs::write(
        higher_agents.join("review.md"),
        "---\ndescription: Local review rules\n---\n",
    )
    .unwrap();
    fixture.directory(fixture.cwd.join(".opencode"));

    assert_eq!(
        fixture
            .resolve(OpenCodeConfigSubject::AgentModel("review".into()))
            .current_value()["model"],
        "lower"
    );
}

#[cfg(not(windows))]
#[test]
fn mcp_edits_complete_native_or_compatible_winner_without_overlay() {
    for native in [false, true] {
        let mut fixture = Fixture::new();
        let path = fixture.global.join("opencode.json");
        let raw = if native {
            json!({"mcp":{"docs":{"type":"local","command":["losing"],"enabled":true},"servers":{"docs":{"type":"local","command":["winning"]}}}})
        } else {
            json!({"mcp":{"docs":{"type":"local","command":["winning"],"enabled":true}}})
        };
        fixture.document(path.clone(), raw, json!({"mcp":{"servers":{"docs":{"type":"local","command":["winning"],"disabled":false}}}}));
        if native {
            fixture.entries[0]["info"]["mcp"]["servers"]["docs"]
                .as_object_mut()
                .unwrap()
                .remove("disabled");
        }
        let target = fixture.resolve(OpenCodeConfigSubject::McpServer("docs".into()));
        let editor = AgentConfigEditor::new();
        let prepared = editor
            .prepare_opencode_operation(&target, json!(false))
            .unwrap();
        write::apply_fixture(&editor, &prepared, &fixture.authority()).unwrap();
        let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let server = if native {
            &value["mcp"]["servers"]["docs"]
        } else {
            &value["mcp"]["docs"]
        };
        assert_eq!(server["command"], json!(["winning"]));
        assert_eq!(
            server[if native { "disabled" } else { "enabled" }],
            json!(native)
        );
    }
}

#[test]
fn pathless_winner_policy_dynamic_model_and_wrong_location_fail_closed() {
    for info in [
        json!({"model":{"providerID":"openai","model":"inline"}}),
        json!({"policies":[{"action":"provider.use","resource":"*","effect":"deny"}]}),
    ] {
        let mut fixture = Fixture::new();
        fixture.entries.push(json!({"type":"document","info":info}));
        assert!(
            resolve(
                &fixture.context,
                &fixture.authority(),
                OpenCodeConfigSubject::FutureDefaultModel
            )
            .is_err()
        );
    }
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(
        path,
        json!({"model":"{env:MODEL}"}),
        json!({"model":{"providerID":"openai","model":"old"}}),
    );
    assert!(
        resolve(
            &fixture.context,
            &fixture.authority(),
            OpenCodeConfigSubject::FutureDefaultModel
        )
        .is_err()
    );
    let wrong = CurrentConfigAuthority::fixture(&fixture.home, json!([]));
    assert!(
        resolve(
            &fixture.context,
            &wrong,
            OpenCodeConfigSubject::FutureDefaultModel
        )
        .is_err()
    );
}

#[cfg(not(windows))]
#[test]
fn new_higher_priority_config_blocks_prepared_edit_and_readback_recovery() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(
        path.clone(),
        json!({"model":"openai/old"}),
        json!({"model":{"providerID":"openai","model":"old"}}),
    );
    let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_opencode_operation(&target, json!("openai/new"))
        .unwrap();
    fs::write(
        fixture.cwd.join("opencode.json"),
        r#"{"model":"openai/other"}"#,
    )
    .unwrap();
    assert!(write::apply_fixture(&editor, &prepared, &fixture.authority()).is_err());
    assert!(!path.with_extension("json.bak").exists());
    assert!(write::readback_fixture(&editor, &prepared, &fixture.authority()).is_err());
}

#[cfg(not(windows))]
#[test]
fn skill_advertising_edits_winning_file_and_preserves_explicit_use() {
    let mut fixture = Fixture::new();
    fixture.directory(fixture.global.clone());
    let directory = fixture.global.join("skills/review");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("SKILL.md");
    fs::write(&path, "---\nname: Review display\ndescription: Review changes\nmetadata:\n  unrelated: keep\n---\nProcedure body.\n").unwrap();
    let target = fixture.resolve(OpenCodeConfigSubject::SkillAdvertising("review".into()));
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_opencode_operation(&target, json!(false))
        .unwrap();
    write::apply_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    let bytes = fs::read(&path).unwrap();
    let (header, body) = inputs::markdown(&bytes).unwrap();
    assert_eq!(
        header["metadata"]["opencode/autoinvoke"],
        serde_yaml_ng::Value::Bool(false)
    );
    assert_eq!(header["metadata"]["unrelated"].as_str(), Some("keep"));
    assert_eq!(body, "Procedure body.\n");
    assert!(!String::from_utf8(bytes).unwrap().contains("permissions"));
    write::readback_fixture(&editor, &prepared, &fixture.authority()).unwrap();
}

#[test]
fn v2_source_does_not_authorize_the_legacy_editor() {
    let fixture = Fixture::new();
    let mut context = fixture.context.clone();
    context.source_format = Some(antiburn_local::analysis::SourceFormat::OpenCodeSqliteV2);
    assert!(
        AgentConfigEditor::new()
            .prepare_operation(
                &context,
                &ConfigOperation {
                    setting: ConfigSetting::Compaction,
                    expected_value: ConfigOperationValue::Boolean(false),
                    proposed_value: ConfigOperationValue::Boolean(true)
                }
            )
            .is_err()
    );
}

#[cfg(not(windows))]
#[test]
fn compatible_named_agent_edit_keeps_separate_variant_and_other_agent() {
    let mut fixture = Fixture::new();
    let path = fixture.cwd.join("opencode.json");
    fixture.document(path.clone(), json!({"agent":{"review":{"model":"openai/old","variant":"high"},"other":{"model":"openai/old"}}}), json!({"agents":{"review":{"model":{"providerID":"openai","model":"old","variant":"high"}},"other":{"model":{"providerID":"openai","model":"old"}}}}));
    let target = fixture.resolve(OpenCodeConfigSubject::AgentModel("review".into()));
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_opencode_operation(
            &target,
            json!({"providerID":"openai","model":"new","variant":"high"}),
        )
        .unwrap();
    write::apply_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    let result: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(result["agent"]["review"]["model"], "openai/new");
    assert_eq!(result["agent"]["review"]["variant"], "high");
    assert_eq!(result["agent"]["other"]["model"], "openai/old");
    fixture.refresh_document(&path, json!({"agents":{"review":{"model":{"providerID":"openai","model":"new","variant":"high"}},"other":{"model":{"providerID":"openai","model":"old"}}}}));
    write::readback_fixture(&editor, &prepared, &fixture.authority()).unwrap();
}

#[test]
fn disabled_only_mcp_layer_cannot_borrow_a_lower_complete_entry() {
    let mut fixture = Fixture::new();
    let global = fixture.global.join("opencode.json");
    fixture.document(
        global,
        json!({"mcp":{"servers":{"docs":{"type":"local","command":["docs"]}}}}),
        json!({"mcp":{"servers":{"docs":{"type":"local","command":["docs"]}}}}),
    );
    let project = fixture.cwd.join("opencode.json");
    fixture.document(
        project,
        json!({"mcp":{"servers":{"docs":{"disabled":false}}}}),
        json!({"mcp":{}}),
    );
    assert!(
        resolve(
            &fixture.context,
            &fixture.authority(),
            OpenCodeConfigSubject::McpServer("docs".into())
        )
        .is_err()
    );
}

#[cfg(not(windows))]
#[test]
fn unsafe_backup_keeps_v2_config_unchanged() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(
        path.clone(),
        json!({"model":"openai/old"}),
        json!({"model":{"providerID":"openai","model":"old"}}),
    );
    let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_opencode_operation(&target, json!("openai/new"))
        .unwrap();
    #[cfg(unix)]
    symlink(&path, path.with_extension("json.bak")).unwrap();
    assert!(write::apply_fixture(&editor, &prepared, &fixture.authority()).is_err());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["model"],
        "openai/old"
    );
}

#[test]
fn authoritative_order_can_select_explicit_file_but_not_a_managed_target() {
    let mut fixture = Fixture::new();
    let global = fixture.global.join("opencode.json");
    fixture.document(
        global,
        json!({"model":"openai/global"}),
        json!({"model":{"providerID":"openai","model":"global"}}),
    );
    let explicit = fixture.home.join("explicit.json");
    fixture.document(
        explicit.clone(),
        json!({"model":"openai/explicit"}),
        json!({"model":{"providerID":"openai","model":"explicit"}}),
    );
    assert_eq!(
        fixture
            .resolve(OpenCodeConfigSubject::FutureDefaultModel)
            .physical_path(),
        explicit
    );
    let system = fixture._temporary.path().join("system/opencode.json");
    fixture.document(
        system,
        json!({"model":"openai/system"}),
        json!({"model":{"providerID":"openai","model":"system"}}),
    );
    assert!(matches!(
        resolve(
            &fixture.context,
            &fixture.authority(),
            OpenCodeConfigSubject::FutureDefaultModel
        ),
        Err(ConfigUnavailableReason::UnsafePath)
    ));
}

#[test]
fn consumer_reported_global_directory_does_not_use_antiburn_xdg_environment() {
    let mut fixture = Fixture::new();
    let global = fixture._temporary.path().join("actual-global");
    fixture.document(
        global.join("opencode.json"),
        json!({"model":"openai/current"}),
        json!({"model":{"providerID":"openai","model":"current"}}),
    );
    fixture.directory(global.clone());
    let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
    assert_eq!(target.scope(), ConfigScope::Global);
    assert_eq!(target.physical_path(), global.join("opencode.json"));
}

#[cfg(not(windows))]
#[test]
fn applied_receipt_cannot_recover_without_a_running_verified_consumer() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(
        path.clone(),
        json!({"model":"openai/old"}),
        json!({"model":{"providerID":"openai","model":"old"}}),
    );
    let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_opencode_operation(&target, json!("openai/new"))
        .unwrap();
    assert!(editor.apply_opencode_operation(&prepared).is_err());
    assert!(editor.readback_opencode_operation(&prepared).is_err());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap()["model"],
        "openai/old"
    );
}

#[cfg(not(windows))]
#[test]
fn jsonc_edits_keep_comments_and_insert_only_the_mcp_control() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.jsonc");
    let original = "{\n // Keep setup notes.\n \"mcp\": {\"servers\": {\"docs\": {\"type\": \"local\", \"command\": [\"docs\"]}}},\n \"compaction\": {\"auto\": false /* Keep this note. */},\n}\n";
    fs::write(&path, original).unwrap();
    fixture.entries.push(json!({"type":"document","path":path,"info":{"mcp":{"servers":{"docs":{"type":"local","command":["docs"]}}},"compaction":{"auto":false}}}));
    let editor = AgentConfigEditor::new();
    let target = fixture.resolve(OpenCodeConfigSubject::McpServer("docs".into()));
    assert_eq!(target.current_value(), json!(true));
    let prepared = editor
        .prepare_opencode_operation(&target, json!(false))
        .unwrap();
    write::apply_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    let changed = fs::read_to_string(&path).unwrap();
    assert!(changed.contains("// Keep setup notes."));
    assert!(changed.contains("false /* Keep this note. */"));
    assert!(changed.contains("\"disabled\":true,"));
    assert_eq!(
        fs::read_to_string(path.with_extension("jsonc.bak")).unwrap(),
        original
    );
}

#[cfg(not(windows))]
#[test]
fn native_markdown_agent_winner_keeps_body_and_embedded_variant() {
    let mut fixture = Fixture::new();
    fixture.directory(fixture.global.clone());
    let directory = fixture.global.join("agents");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("review.md");
    fs::write(&path, "---\nmodel: openai/old#high\ndescription: Review changes\n---\nKeep the review procedure.\n").unwrap();
    let target = fixture.resolve(OpenCodeConfigSubject::AgentModel("review".into()));
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_opencode_operation(&target, json!("openai/new#high"))
        .unwrap();
    write::apply_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    let bytes = fs::read(&path).unwrap();
    let (header, body) = inputs::markdown(&bytes).unwrap();
    assert_eq!(header["model"].as_str(), Some("openai/new#high"));
    assert_eq!(body, "Keep the review procedure.\n");
    write::readback_fixture(&editor, &prepared, &fixture.authority()).unwrap();
}

#[cfg(not(windows))]
#[test]
fn compatible_mcp_oauth_and_numeric_timeout_keep_their_raw_shape() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(path.clone(), json!({"mcp":{"docs":{"type":"remote","url":"https://example.invalid/mcp","enabled":true,"timeout":2000,"oauth":{"clientId":"fixture-client","clientSecret":"fixture-private-key"},"unrelated":"keep"}}}), json!({"mcp":{"servers":{"docs":{"type":"remote","url":"https://example.invalid/mcp","disabled":false,"timeout":{"startup":2000,"catalog":2000,"execution":2000},"oauth":{"client_id":"fixture-client","client_secret":"fixture-private-key"}}}}}));
    let target = fixture.resolve(OpenCodeConfigSubject::McpServer("docs".into()));
    assert_eq!(target.current_value(), json!(true));
    assert!(!format!("{target:?}").contains("fixture-private-key"));
    let editor = AgentConfigEditor::new();
    let prepared = editor
        .prepare_opencode_operation(&target, json!(false))
        .unwrap();
    write::apply_fixture(&editor, &prepared, &fixture.authority()).unwrap();
    let changed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(changed["mcp"]["docs"]["timeout"], 2000);
    assert_eq!(
        changed["mcp"]["docs"]["oauth"]["clientId"],
        "fixture-client"
    );
    assert_eq!(changed["mcp"]["docs"]["unrelated"], "keep");
    assert_eq!(changed["mcp"]["docs"]["enabled"], false);
}

#[test]
fn current_inventory_selects_v2_controls_without_inventing_schema_cost() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(path, json!({"mcp":{"servers":{"docs":{"type":"local","command":["docs"]},"off":{"type":"local","command":["off"],"disabled":true}}},"permissions":[{"action":"webfetch","resource":"*","effect":"deny"},{"action":"webfetch","resource":"*","effect":"allow"}]}), json!({"mcp":{"servers":{"docs":{"type":"local","command":["docs"]},"off":{"type":"local","command":["off"],"disabled":true}}},"permissions":[{"action":"webfetch","resource":"*","effect":"deny"},{"action":"webfetch","resource":"*","effect":"allow"}]}));
    fixture.directory(fixture.global.clone());
    let skill = fixture.global.join("skills/review/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(
        &skill,
        "---\nname: Review display\ndescription: Review changes\n---\nProcedure.\n",
    )
    .unwrap();
    let inventory = current_inventory(&fixture.context, &fixture.authority()).unwrap();
    let find = |name: &str| {
        inventory
            .resources
            .iter()
            .find(|resource| resource.canonical_name == name)
            .unwrap()
    };
    assert_eq!(find("docs").enabled, super::super::EnabledState::Enabled);
    assert_eq!(find("off").enabled, super::super::EnabledState::Disabled);
    assert_eq!(
        find("webfetch").enabled,
        super::super::EnabledState::Enabled
    );
    assert_eq!(find("review").kind, super::super::ResourceKind::Skill);
    assert!(
        inventory
            .resources
            .iter()
            .all(|resource| resource.definition_tokens.is_none())
    );
    assert!(!inventory.issues.is_empty());
    fixture.entries.push(json!({"type":"document","info":{"policies":[{"action":"tool.use","resource":"*","effect":"deny"}]}}));
    assert!(current_inventory(&fixture.context, &fixture.authority()).is_err());
}

#[test]
fn a_later_agent_definition_can_replace_a_disabled_agent() {
    let mut fixture = Fixture::new();
    fixture.document(
        fixture.global.join("opencode.json"),
        json!({"agents":{"review":{"disabled":true}}}),
        json!({"agents":{"review":{"disabled":true}}}),
    );
    fixture.document(
        fixture.cwd.join("opencode.json"),
        json!({"agents":{"review":{"model":"openai/current"}}}),
        json!({"agents":{"review":{"model":{"providerID":"openai","model":"current"}}}}),
    );
    let target = fixture.resolve(OpenCodeConfigSubject::AgentModel("review".into()));
    assert_eq!(target.current_value()["model"], "current");
    assert_eq!(target.scope(), ConfigScope::Project);
}

#[cfg(not(windows))]
#[test]
fn grouped_contexts_bind_one_file_and_reject_a_new_override_before_backup() {
    let mut fixture = Fixture::new();
    let path = fixture.global.join("opencode.json");
    fixture.document(
        path.clone(),
        json!({"model":"openai/old"}),
        json!({"model":{"providerID":"openai","model":"old"}}),
    );
    let editor = AgentConfigEditor::new();
    let target = fixture.resolve(OpenCodeConfigSubject::FutureDefaultModel);
    let mut prepared = editor
        .prepare_opencode_operation(&target, json!("openai/new"))
        .unwrap();
    let nested = fixture.cwd.join("nested");
    fs::create_dir(&nested).unwrap();
    let context = ConfigContext::native_workspace(
        antiburn_local::model::AgentKind::OpenCode,
        &fixture.home,
        &nested,
        fixture.cwd.parent().unwrap(),
    );
    let authority = CurrentConfigAuthority::fixture(&nested, Value::Array(fixture.entries.clone()));
    editor
        .add_opencode_context(&mut prepared, &context, &authority)
        .unwrap();
    fs::write(nested.join("opencode.json"), r#"{"model":"openai/other"}"#).unwrap();
    assert!(write::apply_fixture(&editor, &prepared, &fixture.authority()).is_err());
    assert!(!path.with_extension("json.bak").exists());
    assert_eq!(
        fs::read_to_string(path).unwrap(),
        r#"{"model":"openai/old"}"#
    );
}

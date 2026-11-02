use std::io::Write;

use antiburn_local::analysis::{
    CheckToolCategory, NativeOperationPaths, NativeRequestAttribution, NormalizedRecord, RawSource,
    RecordSink, SessionInput, SessionSummary, SourceFormat, ToolResultObservation,
    file_live::{NativeFileCursor, read_native_file_page},
    opencode_v2_events::OpenCodeV2EventJoiner,
    read_opencode_live_tool_result_page, read_opencode_tool_result_page, reader_for,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use tempfile::TempDir;

#[path = "support/jsonl_fixture.rs"]
mod jsonl_fixture;

const CLAUDE: &str = include_str!("fixtures/native_tool_facts/claude.jsonl");
const CODEX: &str = include_str!("fixtures/native_tool_facts/codex.jsonl");
const CODEX_COMPLETED: &str = include_str!("fixtures/native_tool_facts/codex_completed.jsonl");
const CODEX_MODE_COMPLETED: &str =
    include_str!("fixtures/native_tool_facts/codex_requested_mode_completed.jsonl");
const CODEX_MODE_TRANSITIONS: &str =
    include_str!("fixtures/native_tool_facts/codex_mode_transitions.jsonl");
const PI: &str = include_str!("fixtures/native_tool_facts/pi.jsonl");
const PI_SSH: &str = include_str!("fixtures/native_tool_facts/pi_ssh_override.jsonl");
const PI_MODE: &str = include_str!("fixtures/native_tool_facts/pi_mode_hint.jsonl");
const V1: &str = include_str!("fixtures/native_tool_facts/opencode_v1.jsonl");
const V1_CUSTOM_OVERRIDE: &str =
    include_str!("fixtures/native_tool_facts/opencode_v1_custom_override.jsonl");
const V1_USER_SHELL: &str = include_str!("fixtures/native_tool_facts/opencode_v1_user_shell.jsonl");
const V1_DIRECT_SUBTASK: &str =
    include_str!("fixtures/native_tool_facts/opencode_v1_direct_subtask.jsonl");
const V2: &str = include_str!("fixtures/native_tool_facts/opencode_v2.jsonl");
const V2_FAST: &str = include_str!("fixtures/native_tool_facts/opencode_v2_fast.jsonl");

#[derive(Default)]
struct Results(Vec<ToolResultObservation>);
impl RecordSink for Results {
    fn record(&mut self, record: NormalizedRecord) {
        if let NormalizedRecord::ToolResult(result) = record {
            self.0.push(*result);
        }
    }
    fn finish(&mut self, _: SessionSummary) {}
}

fn input(agent: &str, source_format: SourceFormat, source: RawSource) -> SessionInput {
    SessionInput {
        agent: agent.into(),
        session_id: "facts".into(),
        source,
        source_format,
        fork_parent_session_id: None,
    }
}

fn parse(agent: &str, format: SourceFormat, source: &str) -> Vec<ToolResultObservation> {
    let mut results = Results::default();
    reader_for(agent)
        .visit(
            &input(agent, format, RawSource::Jsonl(source.into())),
            &mut results,
        )
        .unwrap();
    results.0
}

fn change(source: &str, mut edit: impl FnMut(&mut Value)) -> String {
    source
        .lines()
        .map(|line| {
            let mut value: Value = serde_json::from_str(line).unwrap();
            edit(&mut value);
            format!("{value}\n")
        })
        .collect()
}

fn rows(source: &str) -> Vec<Value> {
    source
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn encode_rows(records: &[Value]) -> String {
    let paginated = records[0]["payload"]["history_mode"] == "paginated";
    records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            let mut record = record.clone();
            if paginated {
                record["ordinal"] = json!(index);
            }
            format!("{record}\n")
        })
        .collect()
}

fn assert_live_facts_at_every_split(agent: &str, format: SourceFormat, source: &str) {
    for line_ending in ["\n", "\r\n"] {
        let source = jsonl_fixture::with_line_ending(source, line_ending);
        let expected: Vec<_> = parse(agent, format, &source)
            .into_iter()
            .map(|result| result.facts)
            .collect();
        assert!(!expected.is_empty());
        let lines: Vec<_> = source.split_inclusive('\n').collect();
        for boundary in 1..lines.len() {
            let directory = TempDir::new().unwrap();
            let path = directory.path().join("session.jsonl");
            std::fs::write(&path, lines[..boundary].concat()).unwrap();
            let input = input(agent, format, RawSource::File(path.clone()));
            let first = read_native_file_page(&input, None, &|| false).unwrap();
            let cursor = NativeFileCursor::decode(&first.cursor.encode()).unwrap();
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(lines[boundary..].concat().as_bytes())
                .unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
            let second = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
            let actual: Vec<_> = first
                .results
                .into_iter()
                .chain(second.results)
                .map(|result| result.observation.facts)
                .collect();
            assert_eq!(
                actual, expected,
                "{agent} at boundary {boundary}, line ending: {line_ending:?}"
            );
        }
    }
}

fn codex_modes(source: &str) -> Vec<Option<bool>> {
    parse("codex", SourceFormat::CodexRolloutJsonl, source)
        .iter()
        .map(|result| {
            result
                .facts
                .as_ref()
                .and_then(|facts| facts.requester.fast_mode)
        })
        .collect()
}

#[test]
fn codex_owned_requested_tiers_freeze_at_exact_calls_across_later_selections() {
    assert_eq!(codex_modes(CODEX), vec![Some(true)]);
    assert_eq!(codex_modes(CODEX_MODE_COMPLETED), vec![Some(true)]);
    assert_eq!(
        codex_modes(CODEX_MODE_TRANSITIONS),
        vec![Some(false), Some(true), Some(false)]
    );
    let results = parse(
        "codex",
        SourceFormat::CodexRolloutJsonl,
        CODEX_MODE_TRANSITIONS,
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.call_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("standard-1"), Some("fast-2"), Some("standard-3")]
    );
    for result in results {
        assert_eq!(
            result.facts.unwrap().requester.model_key().as_deref(),
            Some("openai/gpt-test")
        );
    }
    assert_live_facts_at_every_split(
        "codex",
        SourceFormat::CodexRolloutJsonl,
        CODEX_MODE_TRANSITIONS,
    );
}

#[test]
fn codex_snapshot_replacement_clears_missing_null_and_unreviewed_preferences() {
    for tier in [
        None,
        Some(Value::Null),
        Some(json!(false)),
        Some(json!("auto")),
        Some(json!("flex")),
        Some(json!("fast")),
        Some(json!("PRIORITY")),
        Some(json!("other")),
    ] {
        let mut records = rows(CODEX);
        let mut replacement = records[1].clone();
        if let Some(tier) = tier {
            replacement["payload"]["thread_settings"]["service_tier"] = tier;
        } else {
            replacement["payload"]["thread_settings"]
                .as_object_mut()
                .unwrap()
                .remove("service_tier");
        }
        records.insert(2, replacement);
        let source = encode_rows(&records);
        assert_eq!(codex_modes(&source), vec![None]);
        assert_live_facts_at_every_split("codex", SourceFormat::CodexRolloutJsonl, &source);
    }
}

#[test]
fn codex_request_mode_needs_owned_settings_and_matching_bounded_identity() {
    for case in [
        "missing-owner",
        "foreign-owner",
        "malformed-owner",
        "missing-model",
        "different-model",
        "missing-provider",
        "malformed-provider",
        "oversized-model",
        "oversized-provider",
        "no-context",
        "fake-context-tier",
        "settings-after-context",
    ] {
        let mut records = rows(CODEX);
        match case {
            "missing-owner" => {
                records[1]["payload"]
                    .as_object_mut()
                    .unwrap()
                    .remove("thread_id");
            }
            "foreign-owner" => records[1]["payload"]["thread_id"] = json!("parent-thread"),
            "malformed-owner" => records[1]["payload"]["thread_id"] = json!(42),
            "missing-model" => {
                records[1]["payload"]["thread_settings"]
                    .as_object_mut()
                    .unwrap()
                    .remove("model");
            }
            "different-model" => {
                records[1]["payload"]["thread_settings"]["model"] = json!("other-model")
            }
            "missing-provider" => {
                records[1]["payload"]["thread_settings"]
                    .as_object_mut()
                    .unwrap()
                    .remove("model_provider_id");
            }
            "malformed-provider" => {
                records[1]["payload"]["thread_settings"]["model_provider_id"] = json!(false)
            }
            "oversized-model" => {
                records[1]["payload"]["thread_settings"]["model"] = json!("x".repeat(4097))
            }
            "oversized-provider" => {
                records[1]["payload"]["thread_settings"]["model_provider_id"] =
                    json!("x".repeat(4097))
            }
            "no-context" => {
                records.remove(2);
            }
            "fake-context-tier" => {
                records.remove(1);
                records[1]["payload"]["service_tier"] = json!("priority");
            }
            "settings-after-context" => records.swap(1, 2),
            _ => unreachable!(),
        }
        let source = encode_rows(&records);
        assert_eq!(codex_modes(&source), vec![None], "{case}");
        assert_live_facts_at_every_split("codex", SourceFormat::CodexRolloutJsonl, &source);
    }
}

#[test]
fn codex_foreign_snapshots_cannot_replace_owned_preferences_or_requester_identity() {
    let mut records = rows(CODEX);
    let mut foreign = records[1].clone();
    foreign["payload"]["thread_id"] = json!("parent-thread");
    foreign["payload"]["thread_settings"] = json!({"model":"other-model","model_provider_id":"other-provider","service_tier":"default"});
    records.insert(2, foreign);
    let source = encode_rows(&records);
    let facts = parse("codex", SourceFormat::CodexRolloutJsonl, &source)
        .remove(0)
        .facts
        .unwrap();
    assert_eq!(
        facts.requester.model_key().as_deref(),
        Some("openai/gpt-test")
    );
    assert_eq!(facts.requester.fast_mode, Some(true));
    assert_live_facts_at_every_split("codex", SourceFormat::CodexRolloutJsonl, &source);
}

#[test]
fn codex_later_owned_preferences_cannot_relabel_prior_unknown_standard_or_fast_calls() {
    for (tier, expected) in [
        (Value::Null, None),
        (json!("default"), Some(false)),
        (json!("priority"), Some(true)),
    ] {
        let mut records = rows(CODEX);
        records[1]["payload"]["thread_settings"]["service_tier"] = tier;
        let mut later = records[1].clone();
        later["payload"]["thread_settings"] = json!({
            "model":"other-model","model_provider_id":"other-provider",
            "service_tier":if expected == Some(true) { "default" } else { "priority" }
        });
        records.insert(4, later);
        let source = encode_rows(&records);
        let facts = parse("codex", SourceFormat::CodexRolloutJsonl, &source)
            .remove(0)
            .facts
            .unwrap();
        assert_eq!(facts.requester.fast_mode, expected);
        assert_eq!(
            facts.requester.model_key().as_deref(),
            Some("openai/gpt-test")
        );
        assert_live_facts_at_every_split("codex", SourceFormat::CodexRolloutJsonl, &source);
    }
}

#[test]
fn codex_duplicate_call_ids_cannot_rebind_requested_mode() {
    let mut records = rows(CODEX);
    records.insert(5, records[3].clone());
    let source = encode_rows(&records);
    let results = parse("codex", SourceFormat::CodexRolloutJsonl, &source);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].facts, None);
    assert_live_facts_at_every_split("codex", SourceFormat::CodexRolloutJsonl, &source);
}

#[test]
fn codex_completed_only_mode_requires_an_unchanged_same_turn_context() {
    for case in [
        "same-context",
        "changed-cwd",
        "changed-model",
        "changed-preference",
        "changed-then-restored",
        "settings-after-context",
        "different-turn",
    ] {
        let mut records = rows(CODEX_MODE_COMPLETED);
        let mut context = records[2].clone();
        match case {
            "changed-cwd" | "changed-then-restored" => {
                context["payload"]["cwd"] = json!("/other/directory")
            }
            "changed-model" => context["payload"]["model"] = json!("different-model"),
            "changed-preference" => {
                let mut settings = records[1].clone();
                settings["payload"]["thread_settings"]["service_tier"] = json!("default");
                records.insert(3, settings);
            }
            "settings-after-context" => {
                records.insert(3, records[1].clone());
            }
            "different-turn" => context["payload"]["turn_id"] = json!("different-turn"),
            _ => {}
        }
        if case != "settings-after-context" {
            let index = records.len() - 1;
            records.insert(index, context);
        }
        if case == "changed-then-restored" {
            let index = records.len() - 1;
            records.insert(index, records[2].clone());
        }
        let source = encode_rows(&records);
        assert_eq!(
            codex_modes(&source),
            vec![if case == "same-context" {
                Some(true)
            } else {
                None
            }],
            "{case}"
        );
        assert_live_facts_at_every_split("codex", SourceFormat::CodexRolloutJsonl, &source);
    }
}

#[test]
fn pi_recorded_custom_model_suffixes_supply_name_hints_without_inventing_model_keys() {
    for model in [
        "example-fast",
        "example-Fast",
        "example_ULTRAFAST",
        "Example Ultrafast",
    ] {
        let source = change(PI_MODE, |record| {
            if record["message"]["role"] == "assistant" {
                record["message"]["model"] = json!(model);
            }
        });
        let result = parse("pi", SourceFormat::PiV3Jsonl, &source).remove(0);
        let requester = result.facts.unwrap().requester;
        assert_eq!(requester.fast_mode, Some(true), "{model}");
        assert_eq!(
            requester.model_key(),
            if model.contains(' ') {
                None
            } else {
                Some(format!("example-provider/{model}"))
            }
        );
        assert_live_facts_at_every_split("pi", SourceFormat::PiV3Jsonl, &source);
    }
}

#[test]
fn pi_suffix_hints_ignore_other_fields_malformed_ids_and_interior_tokens() {
    for model in [
        Value::Null,
        json!(false),
        json!("example"),
        json!("fast"),
        json!("breakfast"),
        json!("example.fast"),
        json!("fast-example"),
        json!("example-fast-preview"),
        json!("example-fast "),
        json!("Example  Fast"),
        json!("example\tFast"),
        json!("-fast"),
        json!(format!("{}-fast", "x".repeat(4096))),
    ] {
        let source = change(PI_MODE, |record| {
            if record["type"] == "model_change" {
                record["modelId"] = json!("selected-fast");
            }
            if record["message"]["role"] == "assistant" {
                record["message"]["model"] = model.clone();
                record["message"]["responseModel"] = json!("response-fast");
                record["message"]["name"] = json!("Example Fast");
                record["message"]["serviceTier"] = json!("priority");
            }
            if record["message"]["role"] == "toolResult" {
                record["message"]["model"] = json!("result-fast");
                record["message"]["content"][0]["text"] = json!("Using fast mode");
            }
        });
        let results = parse("pi", SourceFormat::PiV3Jsonl, &source);
        assert_eq!(results.len(), 1, "{model}");
        assert_eq!(
            results[0].facts.as_ref().unwrap().requester.fast_mode,
            None,
            "{model}"
        );
    }
}

#[test]
fn pi_model_hints_keep_exact_requesters_and_do_not_survive_broken_result_joins() {
    let mut records = rows(PI_MODE);
    records.push(json!({"type":"model_change","id":"later-selection","parentId":"result","timestamp":"2026-01-01T00:00:04Z","provider":"other-provider","modelId":"other-model"}));
    let source = encode_rows(&records);
    let results = parse("pi", SourceFormat::PiV3Jsonl, &source);
    assert_eq!(
        results[0]
            .facts
            .as_ref()
            .unwrap()
            .requester
            .model_key()
            .as_deref(),
        Some("example-provider/example-fast")
    );
    assert_eq!(
        results[0].facts.as_ref().unwrap().requester.fast_mode,
        Some(true)
    );
    assert_live_facts_at_every_split("pi", SourceFormat::PiV3Jsonl, &source);
    for field in ["parentId", "toolCallId", "toolName"] {
        let source = change(PI_MODE, |record| {
            if record["message"]["role"] == "toolResult" {
                if field == "parentId" {
                    record[field] = json!("unjoined");
                } else {
                    record["message"][field] = json!("unjoined");
                }
            }
        });
        for result in parse("pi", SourceFormat::PiV3Jsonl, &source) {
            assert!(
                result
                    .facts
                    .as_ref()
                    .is_none_or(|facts| facts.requester.fast_mode.is_none()),
                "{field}"
            );
        }
    }
}

#[test]
fn native_request_facts_use_exact_calls_not_result_or_selected_models() {
    let cases = [
        (
            "claude",
            SourceFormat::ClaudeJsonl,
            CLAUDE,
            None,
            "claude-opus-test",
            Some(true),
            CheckToolCategory::Read,
        ),
        (
            "codex",
            SourceFormat::CodexRolloutJsonl,
            CODEX,
            Some("openai"),
            "gpt-test",
            Some(true),
            CheckToolCategory::Delegate,
        ),
        (
            "pi",
            SourceFormat::PiV3Jsonl,
            PI,
            Some("anthropic"),
            "claude-test",
            None,
            CheckToolCategory::Write,
        ),
    ];
    for (agent, format, source, provider, model, mode, category) in cases {
        let results = parse(agent, format, source);
        assert_eq!(results.len(), 1, "{agent}");
        let facts = results[0].facts.as_ref().unwrap();
        assert_eq!(facts.requester.provider.as_deref(), provider, "{agent}");
        assert_eq!(facts.requester.model.as_deref(), Some(model), "{agent}");
        assert_eq!(facts.requester.fast_mode, mode, "{agent}");
        assert_eq!(facts.category, Some(category), "{agent}");
        if agent == "claude" {
            assert_eq!(
                facts
                    .operation_paths
                    .as_ref()
                    .unwrap()
                    .repo_relative_paths("/synthetic/project"),
                Some(vec!["src/api.rs".into()])
            );
        } else {
            assert_eq!(facts.operation_paths, None, "{agent}");
        }
    }
}

#[test]
fn native_file_facts_survive_every_call_result_cursor_boundary() {
    for (agent, format, source) in [
        ("claude", SourceFormat::ClaudeJsonl, CLAUDE),
        ("codex", SourceFormat::CodexRolloutJsonl, CODEX),
        ("codex", SourceFormat::CodexRolloutJsonl, CODEX_COMPLETED),
        (
            "codex",
            SourceFormat::CodexRolloutJsonl,
            CODEX_MODE_COMPLETED,
        ),
        ("pi", SourceFormat::PiV3Jsonl, PI),
        ("pi", SourceFormat::PiV3Jsonl, PI_SSH),
        ("pi", SourceFormat::PiV3Jsonl, PI_MODE),
    ] {
        for line_ending in ["\n", "\r\n"] {
            let source = jsonl_fixture::with_line_ending(source, line_ending);
            let expected = parse(agent, format, &source).remove(0).facts;
            let lines: Vec<_> = source.split_inclusive('\n').collect();
            for boundary in 1..lines.len() {
                let directory = TempDir::new().unwrap();
                let path = directory.path().join("source.jsonl");
                std::fs::write(&path, lines[..boundary].concat()).unwrap();
                let input = input(agent, format, RawSource::File(path.clone()));
                let first = read_native_file_page(&input, None, &|| false).unwrap();
                assert!(first.results.is_empty(), "{agent} at {boundary}");
                let cursor = NativeFileCursor::decode(&first.cursor.encode()).unwrap();
                let mut file = std::fs::OpenOptions::new()
                    .append(true)
                    .open(&path)
                    .unwrap();
                file.write_all(lines[boundary..].concat().as_bytes())
                    .unwrap();
                assert_eq!(std::fs::read(&path).unwrap(), source.as_bytes());
                let second = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
                assert_eq!(second.results.len(), 1, "{agent} at {boundary}");
                assert_eq!(
                    second.results[0].observation.facts, expected,
                    "{agent} at {boundary}, line ending: {line_ending:?}"
                );
            }
        }
    }
}

#[test]
fn missing_or_unproved_modes_and_providers_remain_unknown() {
    for speed in [Value::Null, json!("priority"), json!(false), json!("FAST")] {
        let source = change(CLAUDE, |record| {
            if record["type"] == "assistant" {
                record["message"]["usage"]["speed"] = speed.clone();
                record["speed"] = json!("fast");
            }
        });
        let result = parse("claude", SourceFormat::ClaudeJsonl, &source).remove(0);
        let facts = result.facts.unwrap();
        assert_eq!(facts.requester.fast_mode, None);
        assert_eq!(facts.requester.model_key(), None);
    }
    let source = change(CLAUDE, |record| {
        if record["type"] == "assistant" {
            record["message"]["usage"]["speed"] = json!("standard");
        }
    });
    assert_eq!(
        parse("claude", SourceFormat::ClaudeJsonl, &source)[0]
            .facts
            .as_ref()
            .unwrap()
            .requester
            .fast_mode,
        Some(false)
    );
    let source = change(CODEX, |record| {
        if record["type"] == "session_meta" {
            record["payload"]
                .as_object_mut()
                .unwrap()
                .remove("model_provider");
        }
        if record["payload"]["type"] == "thread_settings_applied" {
            record["payload"]["thread_settings"]
                .as_object_mut()
                .unwrap()
                .remove("model_provider_id");
        }
    });
    let facts = parse("codex", SourceFormat::CodexRolloutJsonl, &source)
        .remove(0)
        .facts
        .unwrap();
    assert_eq!(facts.requester.model_key(), None);
    assert_eq!(facts.requester.fast_mode, None);
    let source = change(PI, |record| {
        if record["message"]["role"] == "assistant" {
            record["message"]
                .as_object_mut()
                .unwrap()
                .remove("provider");
        }
    });
    let facts = parse("pi", SourceFormat::PiV3Jsonl, &source)
        .remove(0)
        .facts
        .unwrap();
    assert_eq!(facts.requester.provider, None);
    assert_eq!(facts.requester.model.as_deref(), Some("claude-test"));
}

#[test]
fn native_paths_need_recorded_cwd_and_supported_fields() {
    let source = change(CLAUDE, |record| {
        record.as_object_mut().unwrap().remove("cwd");
    });
    assert!(
        parse("claude", SourceFormat::ClaudeJsonl, &source)[0]
            .facts
            .as_ref()
            .unwrap()
            .operation_paths
            .is_none()
    );
    let source = change(PI, |record| {
        if record["message"]["role"] == "assistant" {
            record["message"]["content"][0]["name"] = json!("bash");
            record["message"]["content"][0]["arguments"] =
                json!({"command":"cat src/api.rs","path":"fake.rs"});
        }
        if record["message"]["role"] == "toolResult" {
            record["message"]["toolName"] = json!("bash");
        }
    });
    assert!(
        parse("pi", SourceFormat::PiV3Jsonl, &source)[0]
            .facts
            .as_ref()
            .unwrap()
            .operation_paths
            .is_none()
    );
    for path in ["../secret", "C:\\work\\file.rs", "file:///work/file.rs", ""] {
        let facts = NativeOperationPaths {
            cwd: "/synthetic/project".into(),
            paths: vec![path.into()],
        };
        assert_eq!(facts.repo_relative_paths("/synthetic/project"), None);
    }
    let facts = NativeOperationPaths {
        cwd: "/synthetic/project/src".into(),
        paths: vec!["./api.rs".into(), "/outside/file".into()],
    };
    assert_eq!(
        facts.repo_relative_paths("/synthetic/project"),
        Some(vec!["src/api.rs".into()])
    );
    assert_eq!(
        facts.repo_relative_paths("/different/project"),
        Some(Vec::new())
    );
    let root_target = NativeOperationPaths {
        cwd: "/synthetic/project".into(),
        paths: vec![".".into()],
    };
    assert_eq!(root_target.repo_relative_paths("/synthetic/project"), None);
}

#[test]
fn pi_core_and_ssh_shaped_calls_keep_requesters_but_withhold_local_paths() {
    for source in [PI, PI_SSH] {
        for (name, category) in [
            ("read", CheckToolCategory::Read),
            ("edit", CheckToolCategory::Edit),
            ("write", CheckToolCategory::Write),
        ] {
            for path in [
                "api.rs",
                "@src/api.rs",
                "@/outside/file.rs",
                "src/name\u{00a0}file.rs",
                "src/name\u{202f}file.rs",
                "src/name\u{3000}file.rs",
            ] {
                let source = change(source, |record| {
                    if record["message"]["role"] == "assistant" {
                        record["message"]["content"][0]["name"] = json!(name);
                        record["message"]["content"][0]["arguments"] = match name {
                            "read" => json!({"path":path}),
                            "edit" => json!({"path":path,"oldText":"before","newText":"after"}),
                            _ => json!({"path":path,"content":"text"}),
                        };
                    }
                    if record["message"]["role"] == "toolResult" {
                        record["message"]["toolName"] = json!(name);
                    }
                });
                let results = parse("pi", SourceFormat::PiV3Jsonl, &source);
                assert_eq!(results.len(), 1, "{name}: {path}");
                let facts = results[0].facts.as_ref().unwrap();
                assert_eq!(
                    facts.requester.model_key().as_deref(),
                    Some("anthropic/claude-test")
                );
                assert_eq!(facts.requester.fast_mode, None);
                assert_eq!(facts.category, Some(category));
                assert_eq!(facts.operation_paths, None, "{name}: {path}");
            }
        }
    }
}

#[test]
fn claude_paths_keep_literal_at_prefixes_and_unicode_spaces() {
    for path in [
        "@src/api.rs",
        "@/outside/file.rs",
        "src/name\u{00a0}file.rs",
        "src/name\u{202f}file.rs",
    ] {
        let source = change(CLAUDE, |record| {
            if record["type"] == "assistant" {
                record["message"]["content"][0]["input"]["file_path"] = json!(path);
            }
        });
        let result = parse("claude", SourceFormat::ClaudeJsonl, &source).remove(0);
        let paths = result.facts.unwrap().operation_paths.unwrap();
        assert_eq!(paths.paths, vec![path]);
        assert_eq!(
            paths.repo_relative_paths("/synthetic/project"),
            Some(vec![format!("src/{path}")])
        );
    }
}

#[test]
fn codex_completed_file_paths_include_rename_targets_only_in_the_exact_turn() {
    let result = parse("codex", SourceFormat::CodexRolloutJsonl, CODEX_COMPLETED).remove(0);
    let facts = result.facts.unwrap();
    assert_eq!(
        facts.requester.model_key().as_deref(),
        Some("openai/gpt-test")
    );
    assert_eq!(facts.category, Some(CheckToolCategory::Edit));
    assert_eq!(
        facts
            .operation_paths
            .unwrap()
            .repo_relative_paths("/synthetic/project"),
        Some(vec!["src/api.rs".into(), "src/renamed.rs".into()])
    );
    let source = change(CODEX_COMPLETED, |record| {
        if record["type"] == "event_msg" {
            record["payload"]["turn_id"] = json!("unrelated-turn");
        }
    });
    let facts = parse("codex", SourceFormat::CodexRolloutJsonl, &source)
        .remove(0)
        .facts
        .unwrap();
    assert_eq!(facts.requester.model_key(), None);
    assert_eq!(facts.operation_paths, None);
    let source = change(CODEX_COMPLETED, |record| {
        if record["type"] == "event_msg" {
            record["payload"]["item"]["changes"]["api.rs"]["move_path"] = json!({"bad":"path"});
        }
    });
    assert!(parse("codex", SourceFormat::CodexRolloutJsonl, &source).is_empty());
}

#[test]
fn broken_native_joins_do_not_supply_requester_or_operation_facts() {
    for (agent, format, source) in [
        ("claude", SourceFormat::ClaudeJsonl, CLAUDE),
        ("pi", SourceFormat::PiV3Jsonl, PI),
    ] {
        let source = change(source, |record| {
            if record["message"]["role"] == "toolResult" {
                record["message"]["toolCallId"] = json!("unjoined");
            }
            if record["message"]["role"] == "user" && record["message"]["content"].is_array() {
                record["message"]["content"][0]["tool_use_id"] = json!("unjoined");
            }
        });
        assert_eq!(parse(agent, format, &source)[0].facts, None, "{agent}");
    }
    let source = change(CODEX, |record| {
        if record["payload"]["type"] == "function_call_output" {
            record["payload"]["call_id"] = json!("unjoined");
        }
    });
    assert_eq!(
        parse("codex", SourceFormat::CodexRolloutJsonl, &source)[0].facts,
        None
    );
}

#[test]
fn source_specific_categories_do_not_change_metric_categories_or_guess_extensions() {
    for (name, expected) in [
        ("Write", CheckToolCategory::Write),
        ("Agent", CheckToolCategory::Delegate),
        ("Task", CheckToolCategory::Delegate),
        ("Skill", CheckToolCategory::Skill),
        ("mcp__server__tool", CheckToolCategory::Mcp),
        ("custom_write", CheckToolCategory::Other),
    ] {
        let source = change(CLAUDE, |record| {
            if record["type"] == "assistant" {
                record["message"]["content"][0]["name"] = json!(name);
            }
        });
        let result = parse("claude", SourceFormat::ClaudeJsonl, &source).remove(0);
        assert_eq!(result.facts.unwrap().category, Some(expected));
        assert_eq!(
            result.tool_category,
            Some(antiburn_local::analysis::ToolCall::new(name).category)
        );
    }
    for name in ["skill", "subagent", "mcp__server__tool"] {
        let source = change(PI, |record| {
            if record["message"]["role"] == "assistant" {
                record["message"]["content"][0]["name"] = json!(name);
            }
            if record["message"]["role"] == "toolResult" {
                record["message"]["toolName"] = json!(name);
            }
        });
        assert_eq!(
            parse("pi", SourceFormat::PiV3Jsonl, &source)[0]
                .facts
                .as_ref()
                .unwrap()
                .category,
            Some(CheckToolCategory::Other)
        );
    }
}

fn journal() -> (TempDir, std::path::PathBuf) {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE session(id TEXT PRIMARY KEY, parent_id TEXT);
        INSERT INTO session VALUES ('facts', NULL);
        CREATE TABLE message(id TEXT, session_id TEXT, data TEXT);
        CREATE TABLE event_sequence(aggregate_id TEXT PRIMARY KEY, seq INTEGER, owner_id TEXT);
        INSERT INTO event_sequence VALUES ('facts', 0, 'owner');
        CREATE TABLE event(id TEXT, aggregate_id TEXT, seq INTEGER, type TEXT, data TEXT);
        CREATE UNIQUE INDEX event_aggregate_seq_idx ON event(aggregate_id, seq);",
        )
        .unwrap();
    (directory, path)
}

fn append_journal(path: &std::path::Path, first: usize, records: &str) {
    let connection = Connection::open(path).unwrap();
    for (index, line) in records.lines().enumerate() {
        let record: Value = serde_json::from_str(line).unwrap();
        let seq = first + index;
        connection
            .execute(
                "INSERT INTO event VALUES (?1,'facts',?2,?3,?4)",
                params![
                    format!("event-{seq}"),
                    seq,
                    record["type"].as_str().unwrap(),
                    record["data"].to_string()
                ],
            )
            .unwrap();
        connection
            .execute("UPDATE event_sequence SET seq=?1", [seq])
            .unwrap();
    }
}

fn assert_v1_requester(source: &str, expected: &NativeRequestAttribution) {
    let (_directory, path) = journal();
    append_journal(&path, 0, source);
    let strict = read_opencode_tool_result_page(&path, "facts", None, &|| false).unwrap();
    let cold = read_opencode_live_tool_result_page(&path, "facts", None, None, &|| false).unwrap();
    for results in [&strict.results, &cold.results] {
        assert!(results.iter().any(|result| result.finality));
        for result in results.iter().filter(|result| result.finality) {
            assert_eq!(&result.facts.as_ref().unwrap().requester, expected);
            assert_eq!(result.facts.as_ref().unwrap().operation_paths, None);
        }
    }
    let lines: Vec<_> = source.lines().collect();
    for boundary in 1..lines.len() {
        let (_directory, path) = journal();
        append_journal(&path, 0, &lines[..boundary].join("\n"));
        let first =
            read_opencode_live_tool_result_page(&path, "facts", None, None, &|| false).unwrap();
        let cursor = serde_json::from_value(serde_json::to_value(&first.cursor).unwrap()).unwrap();
        append_journal(&path, boundary, &lines[boundary..].join("\n"));
        let second =
            read_opencode_live_tool_result_page(&path, "facts", Some(&cursor), None, &|| false)
                .unwrap();
        let terminals: Vec<_> = first
            .results
            .iter()
            .chain(&second.results)
            .filter(|result| result.finality)
            .collect();
        assert!(!terminals.is_empty(), "boundary {boundary}");
        for result in terminals {
            assert_eq!(
                &result.facts.as_ref().unwrap().requester,
                expected,
                "boundary {boundary}"
            );
            assert_eq!(result.facts.as_ref().unwrap().operation_paths, None);
        }
    }
}

#[test]
fn opencode_v1_shell_and_direct_subtask_do_not_attribute_selected_or_worker_models() {
    for (source, category) in [
        (V1_USER_SHELL, CheckToolCategory::Shell),
        (V1_DIRECT_SUBTASK, CheckToolCategory::Delegate),
    ] {
        assert_v1_requester(source, &NativeRequestAttribution::default());
        let (_directory, path) = journal();
        append_journal(&path, 0, source);
        let page = read_opencode_tool_result_page(&path, "facts", None, &|| false).unwrap();
        let result = page.results.last().unwrap();
        assert!(result.context.as_ref().unwrap().main_session);
        assert_eq!(result.facts.as_ref().unwrap().category, Some(category));
        let source = change(source, |record| {
            if record["data"]["info"]["role"] == "assistant" {
                record["data"]["info"]["tokens"] = json!({"input":100,"output":20});
            }
        });
        assert_v1_requester(&source, &NativeRequestAttribution::default());
    }
}

#[test]
fn opencode_v1_request_proof_needs_an_exact_retained_step_start() {
    assert_v1_requester(
        V1,
        &NativeRequestAttribution {
            provider: Some("anthropic".into()),
            model: Some("claude-test".into()),
            fast_mode: None,
        },
    );
    for case in [
        "missing",
        "other-message",
        "other-session",
        "missing-id",
        "missing-owner-session",
        "step-finish",
        "removed",
        "changed-owner",
        "changed-type",
        "too-late",
    ] {
        let mut records: Vec<Value> = V1
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        match case {
            "missing" => { records.remove(3); }
            "other-message" => records[3]["data"]["part"]["messageID"] = json!("unrelated-assistant"),
            "other-session" => records[3]["data"]["part"]["sessionID"] = json!("other-session"),
            "missing-id" => { records[3]["data"]["part"].as_object_mut().unwrap().remove("id"); }
            "missing-owner-session" => { records[2]["data"]["info"].as_object_mut().unwrap().remove("sessionID"); }
            "step-finish" => records[3]["data"]["part"]["type"] = json!("step-finish"),
            "removed" => records.insert(4, json!({"type":"message.part.removed.1","data":{"sessionID":"facts","messageID":"assistant-1","partID":"step-1"}})),
            "changed-owner" | "changed-type" => {
                let mut replacement = records[3].clone();
                if case == "changed-owner" { replacement["data"]["part"]["messageID"] = json!("unrelated-assistant"); }
                else { replacement["data"]["part"]["type"] = json!("text"); }
                records.insert(4, replacement);
            }
            "too-late" => { let proof = records.remove(3); records.push(proof); }
            _ => unreachable!(),
        }
        let source = records
            .iter()
            .map(|record| format!("{record}\n"))
            .collect::<String>();
        assert_v1_requester(&source, &NativeRequestAttribution::default());
    }
    let source = V1
        .lines()
        .filter(|line| !line.contains("step-start"))
        .collect::<Vec<_>>()
        .join("\n");
    let (_directory, path) = journal();
    append_journal(&path, 0, &source);
    let page = read_opencode_tool_result_page(&path, "facts", None, &|| false).unwrap();
    let facts = page.results.last().unwrap().facts.as_ref().unwrap();
    assert_eq!(facts.category, Some(CheckToolCategory::Write));
    assert_eq!(facts.operation_paths, None);
}

#[test]
fn opencode_v1_journal_freezes_requester_but_withholds_paths_across_restart_and_correction() {
    let (_directory, path) = journal();
    let lines: Vec<_> = V1.lines().collect();
    let terminal = lines.len() - 1;
    append_journal(&path, 0, &lines[..terminal].join("\n"));
    let first = read_opencode_live_tool_result_page(&path, "facts", None, None, &|| false).unwrap();
    let cursor = serde_json::from_value(serde_json::to_value(first.cursor).unwrap()).unwrap();
    append_journal(&path, terminal, lines[terminal]);
    let second =
        read_opencode_live_tool_result_page(&path, "facts", Some(&cursor), None, &|| false)
            .unwrap();
    assert_eq!(second.results.len(), 1);
    let facts = second.results[0].facts.as_ref().unwrap();
    assert_eq!(
        facts.requester.model_key().as_deref(),
        Some("anthropic/claude-test")
    );
    assert_eq!(facts.requester.fast_mode, None);
    assert_eq!(facts.category, Some(CheckToolCategory::Write));
    assert_eq!(facts.operation_paths, None);
    let strict = read_opencode_tool_result_page(&path, "facts", None, &|| false).unwrap();
    assert_eq!(
        strict.results.last().unwrap().facts,
        second.results[0].facts
    );
    let mut retained = serde_json::to_value(&second.cursor).unwrap();
    *retained
        .pointer_mut("/live/parts/part-1/state/facts/operation_paths")
        .unwrap() = json!({"cwd":"/synthetic/project/src","paths":["api.rs"]});
    let cursor = serde_json::from_value(retained).unwrap();
    let correction = change(&format!("{}\n{}", lines[2], lines[terminal]), |record| {
        if record["type"] == "message.updated.1" {
            record["data"]["info"]["providerID"] = json!("other-provider");
            record["data"]["info"]["path"]["cwd"] = json!("/wrong/current");
        } else {
            record["data"]["part"]["state"]["input"]["filePath"] = json!("corrected.rs");
        }
    });
    append_journal(&path, lines.len(), &correction);
    let third = read_opencode_live_tool_result_page(&path, "facts", Some(&cursor), None, &|| false)
        .unwrap();
    let facts = third.results[0].facts.as_ref().unwrap();
    assert_eq!(
        facts.requester.model_key().as_deref(),
        Some("anthropic/claude-test")
    );
    assert_eq!(facts.operation_paths, None);
    assert_eq!(
        third.results[0].arguments.as_ref().unwrap()["filePath"],
        "corrected.rs"
    );
}

#[test]
fn opencode_v1_same_name_custom_tools_keep_requesters_and_categories_without_local_paths() {
    for (name, category) in [
        ("read", CheckToolCategory::Read),
        ("edit", CheckToolCategory::Edit),
        ("write", CheckToolCategory::Write),
    ] {
        for path in [
            "api.rs",
            "@src/api.rs",
            "@/outside/file.rs",
            "src/name\u{00a0}file.rs",
        ] {
            let source = change(V1_CUSTOM_OVERRIDE, |record| {
                if record["data"]["part"]["type"] == "tool" {
                    record["data"]["part"]["tool"] = json!(name);
                    record["data"]["part"]["state"]["input"]["filePath"] = json!(path);
                }
            });
            assert_v1_requester(
                &source,
                &NativeRequestAttribution {
                    provider: Some("anthropic".into()),
                    model: Some("claude-test".into()),
                    fast_mode: None,
                },
            );
            let (_directory, database) = journal();
            append_journal(&database, 0, &source);
            let page = read_opencode_tool_result_page(&database, "facts", None, &|| false).unwrap();
            let result = page.results.last().unwrap();
            let facts = result.facts.as_ref().unwrap();
            assert_eq!(facts.category, Some(category));
            assert_eq!(
                result.tool_category,
                Some(antiburn_local::analysis::ToolCall::new(name).category)
            );
            assert_eq!(facts.operation_paths, None);
            assert_eq!(result.arguments.as_ref().unwrap()["filePath"], path);
        }
    }
}

#[test]
fn opencode_v1_categories_and_optional_attribution_keep_native_field_limits() {
    for (name, category) in [
        ("read", CheckToolCategory::Read),
        ("edit", CheckToolCategory::Edit),
        ("task", CheckToolCategory::Delegate),
        ("skill", CheckToolCategory::Skill),
    ] {
        let source = change(V1, |record| {
            if record["data"]["part"]["type"] == "tool" {
                record["data"]["part"]["tool"] = json!(name);
            }
        });
        let (_directory, path) = journal();
        append_journal(&path, 0, &source);
        let page = read_opencode_tool_result_page(&path, "facts", None, &|| false).unwrap();
        let facts = page.results.last().unwrap().facts.as_ref().unwrap();
        assert_eq!(facts.category, Some(category));
    }
    let source = change(V1, |record| {
        if record["data"]["info"]["role"] == "assistant" {
            record["data"]["info"]["providerID"] = json!(42);
            record["data"]["info"]["modelID"] = json!(false);
        }
    });
    let (_directory, path) = journal();
    append_journal(&path, 0, &source);
    let page = read_opencode_tool_result_page(&path, "facts", None, &|| false).unwrap();
    assert_eq!(
        page.results
            .last()
            .unwrap()
            .facts
            .as_ref()
            .unwrap()
            .requester,
        NativeRequestAttribution::default()
    );
    let source = change(V1, |record| {
        if record["data"]["part"]["type"] == "tool" {
            record["data"]["part"]["metadata"] = json!({"providerExecuted":true});
        }
    });
    let (_directory, path) = journal();
    append_journal(&path, 0, &source);
    let page = read_opencode_tool_result_page(&path, "facts", None, &|| false).unwrap();
    let facts = page.results.last().unwrap().facts.as_ref().unwrap();
    assert_eq!(facts.category, Some(CheckToolCategory::Other));
    assert_eq!(facts.operation_paths, None);
}

#[test]
fn old_json_lacks_check_facts_and_new_fact_payloads_roundtrip_without_false_modes() {
    let result = parse("claude", SourceFormat::ClaudeJsonl, CLAUDE).remove(0);
    let mut json = serde_json::to_value(&result).unwrap();
    json.as_object_mut().unwrap().remove("facts");
    assert!(
        serde_json::from_value::<ToolResultObservation>(json)
            .unwrap()
            .facts
            .is_none()
    );
    let mut facts = result.facts.unwrap();
    facts.requester.fast_mode = Some(false);
    let encoded = postcard::to_allocvec(&facts).unwrap();
    let restored =
        postcard::from_bytes::<antiburn_local::analysis::NativeToolFacts>(&encoded).unwrap();
    assert_eq!(restored, facts);
}

#[test]
fn opencode_v2_exact_step_join_keeps_requester_without_variant_or_location_guesses() {
    for executed in [Some(false), Some(true), None] {
        let source = change(V2, |record| {
            if record["data"].get("executed").is_some() {
                if let Some(executed) = executed {
                    record["data"]["executed"] = json!(executed);
                } else {
                    record["data"].as_object_mut().unwrap().remove("executed");
                }
            }
        });
        let mut joiner = OpenCodeV2EventJoiner::new();
        let results: Vec<_> = source
            .lines()
            .filter_map(|line| joiner.observe(&serde_json::from_str(line).unwrap()))
            .collect();
        assert_eq!(results.len(), 1);
        let result = results
            .into_iter()
            .next()
            .unwrap()
            .into_observation(false, true);
        let facts = result.facts.unwrap();
        assert_eq!(
            facts.requester.model_key().as_deref(),
            Some("anthropic/claude-test")
        );
        assert_eq!(facts.requester.fast_mode, None);
        assert_eq!(
            facts.category,
            Some(if executed == Some(false) {
                CheckToolCategory::Skill
            } else {
                CheckToolCategory::Other
            })
        );
        assert_eq!(facts.operation_paths, None);
    }
    let source = change(V2, |record| {
        if record["type"] == "session.tool.called" {
            record["data"]["assistantMessageID"] = json!("unjoined");
        }
    });
    let mut joiner = OpenCodeV2EventJoiner::new();
    let result = source
        .lines()
        .filter_map(|line| joiner.observe(&serde_json::from_str(line).unwrap()))
        .next()
        .unwrap();
    assert_eq!(result.requester, NativeRequestAttribution::default());
}

fn v2_results(source: &str) -> Vec<antiburn_local::analysis::opencode_v2_events::JoinedToolResult> {
    let mut joiner = OpenCodeV2EventJoiner::new();
    source
        .lines()
        .filter_map(|line| joiner.observe(&serde_json::from_str(line).unwrap()))
        .collect()
}

#[test]
fn opencode_v2_exact_fast_catalog_ids_record_selected_mode_not_served_tier() {
    for id in [
        "gpt-5.5-fast",
        "gpt-5.6-luna-fast",
        "gpt-5.6-sol-fast",
        "gpt-5.6-terra-fast",
        "gpt-6-astra-fast",
        "gpt-6-astra-ultrafast",
        "gpt-6-luna-fast",
        "gpt-6-sol-fast",
        "gpt-6.1-sol-fast",
        "gpt-6.1-sol-ultrafast",
    ] {
        let source = change(V2_FAST, |record| {
            if record["type"] == "session.step.started" {
                record["data"]["model"]["id"] = json!(id);
            }
        });
        for source in [
            source.clone(),
            source.lines().skip(2).collect::<Vec<_>>().join("\n"),
        ] {
            let mut results = v2_results(&source);
            assert_eq!(results.len(), 1, "{id}");
            let result = results.remove(0);
            assert_eq!(result.requester.model_key(), Some(format!("openai/{id}")));
            assert_eq!(result.requester.fast_mode, Some(true), "{id}");
            let facts = result.into_observation(true, true).facts.unwrap();
            assert_eq!(facts.requester.fast_mode, Some(true));
            assert_eq!(facts.category, Some(CheckToolCategory::Skill));
            assert_eq!(facts.operation_paths, None);
        }
    }
}

#[test]
fn opencode_v2_base_ids_variants_labels_and_unknown_providers_do_not_prove_mode() {
    let mut models = vec![
        json!({"providerID":"proxy","id":"gpt-6.1-sol-fast"}),
        json!({"providerID":"OpenAI","id":"gpt-6.1-sol-fast"}),
        json!({"providerID":"chatgpt","id":"gpt-6.1-sol-fast"}),
        json!({"id":"gpt-6.1-sol-fast"}),
        json!({"providerID":"openai","id":"gpt-6.1-sol","variant":"fast"}),
        json!({"providerID":"openai","id":"gpt-6.1-sol","variant":"ultrafast"}),
        json!({"providerID":"openai","id":"gpt-6.1-sol","name":"GPT 6.1 Sol (Fast)"}),
        json!({"providerID":"openai","id":"gpt-6.1-sol (Fast)"}),
        json!({"providerID":"openai","id":"gpt-6.1-sol-fast-custom"}),
        json!({"providerID":"openai","id":"gpt-5.5-ultrafast"}),
        json!({"providerID":"openai","id":"gpt-custom-fast"}),
        json!({"providerID":"openai","modelID":"gpt-6.1-sol-fast"}),
        Value::Null,
    ];
    for id in [
        "gpt-5.5",
        "gpt-5.6-luna",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-6-astra",
        "gpt-6-luna",
        "gpt-6-sol",
        "gpt-6.1-sol",
    ] {
        models.push(json!({"providerID":"openai","id":id}));
    }
    for model in models {
        let source = change(V2_FAST, |record| {
            if record["type"] == "session.step.started" {
                record["data"]["model"] = model.clone();
            }
            if record["type"] == "session.model.selected" {
                record["data"]["model"] = json!({"providerID":"openai","id":"gpt-6.1-sol-fast"});
            }
            if record["type"] == "session.tool.success" {
                record["data"]["content"][0]["text"] = json!("Fast mode: openai/gpt-6.1-sol-fast");
            }
        });
        let mut results = v2_results(&source);
        assert_eq!(results.len(), 1, "{model}");
        let result = results.remove(0);
        assert_eq!(result.requester.fast_mode, None, "{model}");
        assert_eq!(
            result
                .into_observation(true, true)
                .facts
                .unwrap()
                .requester
                .fast_mode,
            None
        );
    }
}

#[test]
fn opencode_v2_fast_mode_requires_the_exact_step_and_call_join() {
    for case in [
        "missing-step",
        "other-assistant",
        "other-session",
        "other-call",
    ] {
        let source = change(V2_FAST, |record| {
            if record["type"] == "session.step.started" {
                match case {
                    "missing-step" => record["type"] = json!("session.model.selected"),
                    "other-assistant" => {
                        record["data"]["assistantMessageID"] = json!("other-assistant")
                    }
                    "other-session" => record["data"]["sessionID"] = json!("other-session"),
                    _ => {}
                }
            }
            if case == "other-call" && record["type"] == "session.tool.success" {
                record["data"]["id"] = json!("unjoined-call");
            }
        });
        let results = v2_results(&source);
        if case == "other-call" {
            assert!(results.is_empty());
            continue;
        }
        assert!(!results.is_empty(), "{case}");
        for result in results {
            assert_eq!(
                result.requester,
                NativeRequestAttribution::default(),
                "{case}"
            );
        }
    }
}

#[test]
fn old_cursor_revision_is_rejected_before_changed_postcard_layout() {
    for revision in [1_u32, 2, 3] {
        let error =
            NativeFileCursor::decode(&postcard::to_allocvec(&revision).unwrap()).unwrap_err();
        assert_eq!(
            error.downcast_ref::<antiburn_local::analysis::file_live::NativeFileReadError>(),
            Some(&antiburn_local::analysis::file_live::NativeFileReadError::InvalidCursor)
        );
    }
}

#[test]
fn pre_review_fact_cursors_require_explicit_recovery_instead_of_replay() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("pi.jsonl");
    std::fs::write(&path, PI).unwrap();
    let input = input("pi", SourceFormat::PiV3Jsonl, RawSource::File(path));
    let page = read_native_file_page(&input, None, &|| false).unwrap();
    let mut cursor = serde_json::to_value(page.cursor).unwrap();
    cursor["revision"] = json!(2);
    let cursor: NativeFileCursor = serde_json::from_value(cursor).unwrap();
    let error = read_native_file_page(&input, Some(&cursor), &|| false).unwrap_err();
    assert_eq!(
        error.downcast_ref::<antiburn_local::analysis::file_live::NativeFileReadError>(),
        Some(&antiburn_local::analysis::file_live::NativeFileReadError::InvalidCursor)
    );
    let (_directory, path) = journal();
    append_journal(&path, 0, V1);
    let page = read_opencode_live_tool_result_page(&path, "facts", None, None, &|| false).unwrap();
    let mut cursor = serde_json::to_value(page.cursor).unwrap();
    cursor["live"]["revision"] = json!(3);
    let cursor = serde_json::from_value(cursor).unwrap();
    let error = read_opencode_live_tool_result_page(&path, "facts", Some(&cursor), None, &|| false)
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<antiburn_local::analysis::OpenCodeJournalReadError>(),
        Some(&antiburn_local::analysis::OpenCodeJournalReadError::SourceChanged)
    );
}

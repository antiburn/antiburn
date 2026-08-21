// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use crate::analysis::engine::analyze_session;
use crate::analysis::merge::merge_subagent_events;
use crate::analysis::model::{EventSource, ModelRun, Role, ToolCategory};
use crate::analysis::{RawSource, SessionInput, analyze_sources, normalize_source};

fn jsonl_input(agent: &str, jsonl: &str) -> SessionInput {
    SessionInput {
        agent: agent.into(),
        session_id: "s".into(),
        source: RawSource::Jsonl(jsonl.into()),
    }
}

/// A small but representative Claude transcript: one edit turn, one error
/// (tool_result), one thinking turn, one grep turn.
const CLAUDE_FIXTURE: &str = concat!(
    r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":5000},"content":[{"type":"text","text":"editing"},{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
    "\n",
    r#"{"type":"user","timestamp":"2024-06-01T12:01:00Z","message":{"role":"user","content":[{"type":"tool_result","is_error":true,"content":"boom"}]}}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T12:02:00Z","message":{"role":"assistant","usage":{"input_tokens":1200,"output_tokens":50},"content":[{"type":"thinking","thinking":"hmm"}]}}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T12:03:00Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Grep","input":{"pattern":"foo"}}]}}"#,
);

#[test]
fn claude_tokens_tools_and_timing() {
    let session = normalize_source(&jsonl_input("claude", CLAUDE_FIXTURE)).unwrap();
    assert_eq!(session.events.len(), 4);

    let m = analyze_session(&session);
    // tokens_in counts fresh input plus cache writes, but excludes cache reads;
    // the cached prefix instead shows up as occupancy in peak_context_tokens.
    assert_eq!(m.tokens_in, 1000 + 1200);
    assert_eq!(m.tokens_out, 200 + 50);
    assert_eq!(m.peak_context_tokens, 6000);
    assert_eq!(m.tool_mix.edit, 1);
    assert_eq!(m.tool_mix.search, 1);
    assert_eq!(m.grep_count, 1);
    // Duration spans 12:00 → 12:03 = 180s.
    assert_eq!(m.duration_secs, 180);
    // Every gap is 60s (< idle threshold), so active time == wall-clock span.
    assert_eq!(m.active_secs, 180);
}

#[test]
fn effective_input_includes_cache_writes_in_totals_and_buckets() {
    let fixture = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":5000,"cache_creation_input_tokens":500},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","usage":{"input_tokens":200,"output_tokens":50,"cache_creation_input_tokens":300},"content":[{"type":"tool_use","name":"Grep","input":{"pattern":"foo"}}]}}"#,
    );
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    let m = analyze_session(&session);

    // Header total: effective input is fresh input + cache writes, never reads.
    assert_eq!(m.tokens_in, 2000);

    // Chart buckets independently preserve the same effective-input total.
    assert_eq!(
        m.buckets.iter().map(|bucket| bucket.tokens_in).sum::<u64>(),
        2000
    );

    // Cost retains disjoint per-rate inputs, and occupancy still includes reads.
    assert_eq!(m.billable_input_tokens, 1200);
    assert_eq!(m.billable_cache_creation_tokens, 800);
    assert_eq!(m.billable_cache_read_tokens, 5000);
    assert_eq!(m.peak_context_tokens, 6500);
}

/// Two short bursts separated by an ~8h idle gap: wall-clock is huge but active
/// time should be only the in-burst seconds plus one capped idle gap.
const IDLE_GAP_FIXTURE: &str = concat!(
    r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T12:00:30Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"b.rs"}}]}}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T20:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"c.rs"}}]}}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T20:00:30Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"d.rs"}}]}}"#,
);

#[test]
fn active_time_excludes_idle_gaps() {
    let session = normalize_source(&jsonl_input("claude", IDLE_GAP_FIXTURE)).unwrap();
    let m = analyze_session(&session);
    // Wall-clock: 12:00:00 → 20:00:30 = 8h 30s = 28830s.
    assert_eq!(m.duration_secs, 28830);
    // Active: 30s + capped(8h → 300s) + 30s = 360s.
    assert_eq!(m.active_secs, 360);
    assert!(m.active_secs < m.duration_secs);
}

#[test]
fn pi_jsonl_tool_calls_feed_tool_mix() {
    let pi_fixture = concat!(
        r#"{"type":"message","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"I'll inspect first."},{"type":"toolCall","id":"call_read","name":"read","arguments":{"path":"src/lib.rs"}}]}}"#,
        "\n",
        r#"{"type":"message","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"call_edit","name":"edit","arguments":{"path":"src/lib.rs","old":"a","new":"b"}}]}}"#,
        "\n",
        r#"{"type":"message","timestamp":"2024-06-01T12:02:00Z","message":{"role":"assistant","content":[{"type":"toolCall","id":"call_test","name":"bash","arguments":{"command":"cargo test"}}]}}"#,
        "\n",
        r#"{"type":"message","timestamp":"2024-06-01T12:03:00Z","message":{"role":"toolResult","toolCallId":"call_test","toolName":"bash","isError":true,"content":[{"type":"text","text":"FAILED"}]}}"#,
        "\n",
        r#"{"type":"message","timestamp":"2024-06-01T12:04:00Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"Need another approach."}]}}"#,
    );
    let summary = analyze_sources(vec![jsonl_input("pi", pi_fixture)]);
    assert_eq!(summary.tool_mix.read, 1);
    assert_eq!(summary.tool_mix.edit, 1);
    assert_eq!(summary.tool_mix.test, 1);
}

#[test]
fn test_command_detection_is_token_aware() {
    use crate::analysis::model::is_test_command;
    assert!(is_test_command("cargo test --workspace"));
    assert!(is_test_command("pytest -q"));
    assert!(is_test_command("npm run test"));
    assert!(is_test_command("go test ./..."));
    assert!(is_test_command("./node_modules/.bin/jest"));
    // Must not fire on words that merely contain "test".
    assert!(!is_test_command("git checkout latest"));
    assert!(!is_test_command("echo contest"));
    assert!(!is_test_command("ls -la"));
}

#[test]
fn generic_openai_shape_is_normalized() {
    // No "message" wrapper, string content, OpenAI usage + tool_calls.
    let raw = r#"{"role":"assistant","usage":{"prompt_tokens":500,"completion_tokens":100},"content":"done","tool_calls":[{"function":{"name":"write_file"}}]}"#;
    let session = normalize_source(&jsonl_input("amp", raw)).unwrap();
    assert_eq!(session.events.len(), 1);
    let ev = &session.events[0];
    assert_eq!(ev.role, Role::Assistant);
    assert_eq!(ev.usage.input_tokens, 500);
    assert_eq!(ev.usage.output_tokens, 100);
    assert_eq!(ev.tools[0].category, ToolCategory::Edit);
}

#[test]
fn openai_cached_tokens_no_longer_double_count_downstream() {
    let raw = r#"{"role":"assistant","model":"gpt-5.4","timestamp":"2024-06-01T12:00:00Z","usage":{"prompt_tokens":1000000,"completion_tokens":100000,"cached_tokens":400000},"content":"done"}"#;
    let session = normalize_source(&jsonl_input("amp", raw)).unwrap();
    let m = analyze_session(&session);

    assert_eq!(m.billable_input_tokens, 600_000);
    assert_eq!(m.billable_output_tokens, 100_000);
    assert_eq!(m.billable_cache_read_tokens, 400_000);
    assert_eq!(m.billable_cache_creation_tokens, 0);
    assert_eq!(m.tokens_in, 600_000);
    assert_eq!(m.peak_context_tokens, 1_000_000);
    assert_eq!(
        m.buckets.iter().map(|bucket| bucket.tokens_in).sum::<u64>(),
        600_000
    );
    let model_tokens = &m.model_breakdown["gpt-5.4"];
    assert_eq!(model_tokens.input_tokens, 600_000);
    assert_eq!(model_tokens.cache_read_tokens, 400_000);

    let cost = m.cost.expect("gpt-5.4 is priceable");
    assert!((cost.input_usd - 1.50).abs() < 1e-9);
    assert!((cost.output_usd - 1.50).abs() < 1e-9);
    assert!((cost.cache_read_usd - 0.10).abs() < 1e-9);
    assert_eq!(cost.cache_write_usd, 0.0);
    assert!((cost.total_usd - 3.10).abs() < 1e-9);

    let summary = analyze_sources(vec![jsonl_input("amp", raw)]);
    assert_eq!(summary.tokens_in_total, 600_000);
}

#[test]
fn pi_sessions_report_real_local_usage_downstream() {
    let raw = r#"{"type":"message","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-opus-4-6","usage":{"input":2,"output":8,"cacheRead":0,"cacheWrite":14792,"totalTokens":14802},"content":[{"type":"text","text":"done"}]}}"#;
    let session = normalize_source(&jsonl_input("pi", raw)).unwrap();
    let m = analyze_session(&session);

    assert_eq!(m.billable_input_tokens, 2);
    assert_eq!(m.billable_output_tokens, 8);
    assert_eq!(m.billable_cache_read_tokens, 0);
    assert_eq!(m.billable_cache_creation_tokens, 14_792);
    // Sibling PR #1054 makes tokens_in effective input (2 -> 14,794). Keep this
    // assertion merge-order-independent while pinning the stable buckets below.
    assert!(m.tokens_in > 0);
    assert_eq!(m.peak_context_tokens, 14_794);

    let model_tokens = &m.model_breakdown["claude-opus-4-6"];
    assert_eq!(model_tokens.input_tokens, 2);
    assert_eq!(model_tokens.cache_creation_tokens, 14_792);

    let cost = m.cost.expect("claude-opus-4-6 is priceable");
    assert!((cost.input_usd - 0.00001).abs() < 1e-9);
    assert!((cost.output_usd - 0.0002).abs() < 1e-9);
    assert!((cost.cache_read_usd - 0.0).abs() < 1e-9);
    assert!((cost.cache_write_usd - 0.14792).abs() < 1e-9);
    assert!((cost.total_usd - 0.14813).abs() < 1e-9);
}

#[test]
fn sqlite_vendor_is_extracted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("opencode.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute("CREATE TABLE messages (id INTEGER, data TEXT)", [])
            .unwrap();
        let rec = r#"{"role":"assistant","usage":{"input_tokens":300,"output_tokens":40},"content":[{"type":"tool_use","name":"Read","input":{}}]}"#;
        conn.execute("INSERT INTO messages (id, data) VALUES (1, ?1)", [rec])
            .unwrap();
    }
    let input = SessionInput {
        agent: "opencode".into(),
        session_id: "x".into(),
        source: RawSource::Sqlite(path),
    };
    let session = normalize_source(&input).unwrap();
    assert_eq!(session.events.len(), 1);
    assert_eq!(session.events[0].usage.input_tokens, 300);
    assert_eq!(session.events[0].tools[0].category, ToolCategory::Read);
}

/// A Codex "rollout" transcript: the `{timestamp, type, payload}` envelope with
/// a user turn, an (encrypted) reasoning turn, a shell call that runs tests, a
/// failed tool result, a patch, a per-turn token_count, and the duplicate
/// `agent_message` UI echo that must be ignored.
const CODEX_FIXTURE: &str = concat!(
    r#"{"timestamp":"2024-06-01T12:00:00Z","type":"session_meta","payload":{"id":"x","cwd":"/tmp"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:05Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"review changelist"}]}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:10Z","type":"response_item","payload":{"type":"reasoning","summary":[],"encrypted_content":"zzz"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:15Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test\"}","call_id":"c1"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:20Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"Process exited with code 1\nFAILED"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:25Z","type":"response_item","payload":{"type":"function_call","name":"apply_patch","arguments":"{}","call_id":"c2"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:30Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":50},"model_context_window":258400}}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:35Z","type":"event_msg","payload":{"type":"agent_message","message":"done"}}"#,
);

#[test]
fn codex_rollout_envelope_is_normalized() {
    let session = normalize_source(&jsonl_input("codex", CODEX_FIXTURE)).unwrap();
    // session_meta and the agent_message echo are dropped; the six signal-bearing
    // records (user msg, reasoning, two calls, one output, one token_count) remain.
    assert_eq!(session.events.len(), 6);

    let m = analyze_session(&session);
    // last_token_usage: input 1000 (200 cached) → 800 fresh + 200 cache_read.
    // tokens_in excludes the cache read; occupancy is the full 1000. Codex reports
    // no cache writes, so effective input remains the 800 fresh tokens.
    assert_eq!(m.tokens_in, 800);
    assert_eq!(m.tokens_out, 50);
    // Occupancy is the live prompt size (last_token_usage), NOT the unbounded
    // lifetime total_token_usage — so it stays within the window.
    assert_eq!(m.peak_context_tokens, 1000);
    // The model's real context window is captured from model_context_window.
    assert_eq!(m.context_window, 258400);
    assert!(m.buckets.iter().any(|b| b.tokens_in > 0));
    assert!(m.buckets.iter().any(|b| b.tokens_out > 0));
    assert!(m.buckets.iter().any(|b| b.context_tokens > 0));
    // exec_command running `cargo test` is reclassified Bash → Test.
    assert_eq!(m.tool_mix.test, 1);
    assert_eq!(m.tool_mix.edit, 1); // apply_patch
}

#[test]
fn codex_exec_wrapper_recovers_current_tool_categories() {
    let records = [
        serde_json::json!({
            "timestamp": "2026-08-05T12:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call",
                "name": "exec",
                "call_id": "explore",
                "input": r#"const r = await tools.exec_command({"cmd":"rg src"}); text(r.output);"#,
            },
        }),
        serde_json::json!({
            "timestamp": "2026-08-05T12:00:10Z",
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call",
                "name": "exec",
                "call_id": "test",
                "input": r#"const r = await tools.exec_command({cmd:"cargo test -p antiburn-local", yield_time_ms:30000}); text(r.output);"#,
            },
        }),
        serde_json::json!({
            "timestamp": "2026-08-05T12:00:20Z",
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call",
                "name": "exec",
                "call_id": "implement",
                "input": r#"const [test, patch] = await Promise.all([tools.exec_command({"cmd":"cargo test"}), tools.apply_patch("*** Begin Patch\n*** End Patch")]); text(test.output); text(patch);"#,
            },
        }),
        serde_json::json!({
            "timestamp": "2026-08-05T12:00:30Z",
            "type": "response_item",
            "payload": {
                "type": "custom_tool_call_output",
                "call_id": "implement",
                "output": [{"type": "input_text", "text": "Script failed"}],
            },
        }),
    ];
    let fixture = records
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    let session = normalize_source(&jsonl_input("codex", &fixture)).unwrap();
    assert_eq!(session.events[0].tools.len(), 1);
    assert_eq!(session.events[0].tools[0].name, "exec_command");
    assert_eq!(session.events[0].tools[0].category, ToolCategory::Bash);
    assert_eq!(session.events[1].tools[0].category, ToolCategory::Test);
    assert_eq!(session.events[2].tools.len(), 2);
    assert_eq!(session.events[2].tools[0].category, ToolCategory::Test);
    assert_eq!(session.events[2].tools[1].category, ToolCategory::Edit);

    let metrics = analyze_session(&session);
    assert_eq!(metrics.tool_mix.bash, 1);
    assert_eq!(metrics.tool_mix.test, 2);
    assert_eq!(metrics.tool_mix.edit, 1);
}

#[test]
fn codex_exec_wrapper_ignores_tool_syntax_in_javascript_text() {
    let fixture = serde_json::json!({
        "timestamp": "2026-08-05T12:00:00Z",
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call",
            "name": "exec",
            "call_id": "text-only",
            "input": concat!(
                "const quoted = \"tools.apply_patch({})\";\n",
                "const templated = `tools.exec_command({\\\"cmd\\\":\\\"cargo test\\\"})`;\n",
                "// tools.apply_patch({})\n",
                "/* tools.exec_command({\\\"cmd\\\":\\\"cargo test\\\"}) */\n",
                "text(quoted + templated);",
            ),
        },
    })
    .to_string();

    let session = normalize_source(&jsonl_input("codex", &fixture)).unwrap();
    assert_eq!(session.events[0].tools.len(), 1);
    assert_eq!(session.events[0].tools[0].name, "exec");
    assert_eq!(session.events[0].tools[0].category, ToolCategory::Bash);
}

#[test]
fn codex_exec_wrapper_keeps_fallback_for_unrecognized_scripts() {
    let fixture = serde_json::json!({
        "timestamp": "2026-08-05T12:00:00Z",
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call",
            "name": "exec",
            "call_id": "unknown",
            "input": "const value = 1; text(value);",
        },
    })
    .to_string();

    let session = normalize_source(&jsonl_input("codex", &fixture)).unwrap();
    assert_eq!(session.events[0].tools.len(), 1);
    assert_eq!(session.events[0].tools[0].name, "exec");
    assert_eq!(session.events[0].tools[0].category, ToolCategory::Bash);
}

#[test]
fn write_stdin_is_process_control_not_an_edit() {
    let fixture = serde_json::json!({
        "timestamp": "2026-08-05T12:00:00Z",
        "type": "response_item",
        "payload": {
            "type": "custom_tool_call",
            "name": "exec",
            "call_id": "poll",
            "input": "const r = await tools.write_stdin({\"session_id\": 7}); text(r.output);",
        },
    })
    .to_string();

    let session = normalize_source(&jsonl_input("codex", &fixture)).unwrap();
    assert_eq!(session.events[0].tools.len(), 1);
    assert_eq!(session.events[0].tools[0].name, "write_stdin");
    assert_eq!(session.events[0].tools[0].category, ToolCategory::Bash);
}

#[test]
fn codex_fork_bills_only_child_owned_requests() {
    let fixture = concat!(
        r#"{"timestamp":"2026-06-20T09:00:00Z","type":"session_meta","payload":{"id":"thread-child","thread_source":"subagent","agent_path":"/root/investigate"}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T09:00:01Z","type":"session_meta","payload":{"id":"thread-parent","thread_source":"user"}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T09:00:02Z","type":"turn_context","payload":{"model":"gpt-5.5-codex"}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T09:00:03Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Replayed parent prompt"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T09:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":9000000,"cached_input_tokens":8000000,"output_tokens":500000},"total_token_usage":{"input_tokens":9000000,"cached_input_tokens":8000000,"output_tokens":500000}}}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T10:01:01Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"Child instructions"}]}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T10:01:02Z","type":"event_msg","payload":{"type":"task_started"}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T10:01:03Z","type":"turn_context","payload":{"model":"gpt-5.6-codex"}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T10:01:04Z","type":"response_item","payload":{"type":"agent_message","author":"/root","recipient":"/root/investigate"}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T10:01:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":900,"output_tokens":50},"total_token_usage":{"input_tokens":9001000,"cached_input_tokens":8000900,"output_tokens":500050},"model_context_window":258400}}}"#,
        "\n",
        r#"{"timestamp":"2026-06-20T10:01:06Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":2000,"cached_input_tokens":1800,"output_tokens":25},"total_token_usage":{"input_tokens":9003000,"cached_input_tokens":8002700,"output_tokens":500075},"model_context_window":258400}}}"#,
    );

    let session = normalize_source(&jsonl_input("codex", fixture)).unwrap();
    let metrics = analyze_session(&session);

    // Only the two requests made by the child contribute usage and cost.
    assert_eq!(metrics.billable_input_tokens, 300);
    assert_eq!(metrics.billable_output_tokens, 75);
    assert_eq!(metrics.billable_cache_read_tokens, 2700);
    assert_eq!(metrics.peak_context_tokens, 2000);
    assert_eq!(metrics.context_window, 258400);
    assert_eq!(metrics.model.as_deref(), Some("gpt-5.6-codex"));
}

/// Two turns report separate live-context readings.
const CODEX_COMPACTION_FIXTURE: &str = concat!(
    r#"{"timestamp":"2024-06-01T12:00:00Z","type":"response_item","payload":{"type":"function_call","name":"read_file","arguments":"{}","call_id":"c1"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":200000,"output_tokens":100},"model_context_window":258400}}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:01:00Z","type":"response_item","payload":{"type":"function_call","name":"read_file","arguments":"{}","call_id":"c2"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:01:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":30000,"output_tokens":80},"model_context_window":258400}}}"#,
);

#[test]
fn codex_buckets_keep_observed_context_readings() {
    let session = normalize_source(&jsonl_input("codex", CODEX_COMPACTION_FIXTURE)).unwrap();
    let m = analyze_session(&session);
    // Peak occupancy is the largest live prompt (200k), well within the 258k
    // window — not the lifetime sum.
    assert_eq!(m.peak_context_tokens, 200000);
    assert_eq!(m.context_window, 258400);

    let ctx: Vec<u64> = m
        .buckets
        .iter()
        .filter(|b| b.context_tokens > 0)
        .map(|b| b.context_tokens)
        .collect();
    assert_eq!(ctx, vec![200_000, 30_000]);
}

/// A Claude session with a real, *marked* compaction: a high-occupancy turn
/// (~200k), the `compact_boundary` system record, then a low-occupancy turn
/// (~10k). Unlike the marker-less sampling-gap fixtures above, the drift curve
/// must reset at the boundary and show the real post-compaction drop.
const CLAUDE_MARKED_COMPACTION_FIXTURE: &str = concat!(
    r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":100,"cache_read_input_tokens":199000},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
    "\n",
    r#"{"type":"system","subtype":"compact_boundary","timestamp":"2024-06-01T12:00:30Z","content":"Compacted conversation"}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","usage":{"input_tokens":500,"output_tokens":80,"cache_read_input_tokens":9500},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"b.rs"}}]}}"#,
);

#[test]
fn claude_marked_compaction_resets_context_occupancy() {
    let session =
        normalize_source(&jsonl_input("claude", CLAUDE_MARKED_COMPACTION_FIXTURE)).unwrap();
    let m = analyze_session(&session);

    // The session-wide peak is unaffected by the fix — only the bucket curve.
    assert_eq!(m.peak_context_tokens, 200_000);

    // Exactly one bucket carries the boundary marker.
    let boundary_indices: Vec<usize> = m
        .buckets
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_compaction_boundary)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(boundary_indices.len(), 1, "got {boundary_indices:?}");
    let boundary = boundary_indices[0];

    let before = m.buckets[..boundary]
        .iter()
        .rev()
        .find(|b| b.context_tokens > 0)
        .expect("a context reading before the boundary");
    assert_eq!(before.context_tokens, 200_000);

    let after = m.buckets[boundary..]
        .iter()
        .find(|b| b.context_tokens > 0)
        .expect("a context reading at or after the boundary");
    assert_eq!(after.context_tokens, 10_000);
}

/// The marked variant of `CODEX_COMPACTION_FIXTURE`: same two `token_count`
/// occupancy readings (200k then 30k), but with the explicit
/// `context_compacted` event between them — so the drop is a real compaction,
/// not a sampling gap, and the drift curve must come back down.
const CODEX_MARKED_COMPACTION_FIXTURE: &str = concat!(
    r#"{"timestamp":"2024-06-01T12:00:00Z","type":"response_item","payload":{"type":"function_call","name":"read_file","arguments":"{}","call_id":"c1"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":200000,"output_tokens":100},"model_context_window":258400}}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:30Z","type":"event_msg","payload":{"type":"context_compacted"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:01:00Z","type":"response_item","payload":{"type":"function_call","name":"read_file","arguments":"{}","call_id":"c2"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:01:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":30000,"output_tokens":80},"model_context_window":258400}}}"#,
);

#[test]
fn codex_marked_compaction_resets_context_occupancy() {
    let session = normalize_source(&jsonl_input("codex", CODEX_MARKED_COMPACTION_FIXTURE)).unwrap();
    let m = analyze_session(&session);
    assert_eq!(m.peak_context_tokens, 200_000);

    // The context_compacted event survives parsing and marks its bucket.
    assert!(
        m.buckets.iter().any(|b| b.is_compaction_boundary),
        "expected a compaction-boundary bucket"
    );

    let last_context = m
        .buckets
        .iter()
        .rev()
        .find(|b| b.context_tokens > 0)
        .expect("a context reading");
    assert_eq!(last_context.context_tokens, 30_000);
}

/// Newer Codex rollouts mark a finished compaction with a top-level
/// `{"type":"compacted", ...}` record instead of the `context_compacted`
/// event_msg. Two real compactions, well separated in time with ordinary
/// turns between them, must each produce their own boundary event — not
/// zero (the historical bug) and not one merged marker.
const CODEX_COMPACTED_RECORD_FIXTURE: &str = concat!(
    r#"{"timestamp":"2024-06-01T12:00:00Z","type":"response_item","payload":{"type":"function_call","name":"read_file","arguments":"{}","call_id":"c1"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":200000,"output_tokens":100},"model_context_window":258400}}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:30Z","type":"compacted","payload":{"message":"","window_number":1,"replacement_history":[]}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:01:00Z","type":"response_item","payload":{"type":"function_call","name":"read_file","arguments":"{}","call_id":"c2"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:01:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":30000,"output_tokens":80},"model_context_window":258400}}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:05:00Z","type":"compacted","payload":{"message":"","window_number":2,"replacement_history":[]}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:05:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":50000,"output_tokens":60},"model_context_window":258400}}}"#,
);

#[test]
fn codex_top_level_compacted_records_mark_boundaries() {
    let session = normalize_source(&jsonl_input("codex", CODEX_COMPACTED_RECORD_FIXTURE)).unwrap();

    // Both `compacted` records survive parsing as their own boundary event, at
    // their own timestamp.
    let boundaries: Vec<Option<i64>> = session
        .events
        .iter()
        .filter(|ev| ev.is_compaction_boundary)
        .map(|ev| ev.ts_ms)
        .collect();
    assert_eq!(
        boundaries,
        vec![Some(1_717_243_230_000), Some(1_717_243_500_000)],
        "expected one boundary event per compacted record, at its own timestamp"
    );

    let m = analyze_session(&session);
    assert_eq!(m.peak_context_tokens, 200_000);
    let boundary_buckets = m
        .buckets
        .iter()
        .filter(|b| b.is_compaction_boundary)
        .count();
    assert_eq!(
        boundary_buckets, 2,
        "expected two distinct compaction-boundary buckets"
    );
}

/// Some Codex rollouts write both sibling records for the same compaction:
/// the top-level `compacted` record and the `context_compacted` event_msg,
/// back-to-back with no turn between them. That must still produce exactly
/// one boundary, not two.
const CODEX_COMPACTED_AND_EVENT_MSG_FIXTURE: &str = concat!(
    r#"{"timestamp":"2024-06-01T12:00:00Z","type":"response_item","payload":{"type":"function_call","name":"read_file","arguments":"{}","call_id":"c1"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":200000,"output_tokens":100},"model_context_window":258400}}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:30Z","type":"compacted","payload":{"message":"","window_number":1,"replacement_history":[]}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:30Z","type":"event_msg","payload":{"type":"context_compacted"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:01:00Z","type":"response_item","payload":{"type":"function_call","name":"read_file","arguments":"{}","call_id":"c2"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:01:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":30000,"output_tokens":80},"model_context_window":258400}}}"#,
);

#[test]
fn codex_compacted_record_and_event_msg_sibling_dedupe_to_one_boundary() {
    let session =
        normalize_source(&jsonl_input("codex", CODEX_COMPACTED_AND_EVENT_MSG_FIXTURE)).unwrap();

    let boundaries: Vec<Option<i64>> = session
        .events
        .iter()
        .filter(|ev| ev.is_compaction_boundary)
        .map(|ev| ev.ts_ms)
        .collect();
    assert_eq!(
        boundaries,
        vec![Some(1_717_243_230_000)],
        "the second sibling record must be deduped, not counted as a new compaction"
    );

    let m = analyze_session(&session);
    assert_eq!(m.peak_context_tokens, 200_000);
    let last_context = m
        .buckets
        .iter()
        .rev()
        .find(|b| b.context_tokens > 0)
        .expect("a context reading");
    assert_eq!(last_context.context_tokens, 30_000);
}

/// The pre-compaction turn and boundary share one progress bucket.
const CLAUDE_COMPACTION_SHARES_BUCKET_FIXTURE: &str = concat!(
    r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":100,"cache_read_input_tokens":199000},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
    "\n",
    r#"{"type":"system","subtype":"compact_boundary","timestamp":"2024-06-01T12:00:00Z","content":"Compacted conversation"}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","usage":{"input_tokens":500,"output_tokens":80,"cache_read_input_tokens":9500},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"b.rs"}}]}}"#,
);

#[test]
fn claude_compaction_sharing_bucket_with_pre_compaction_turn_still_resets() {
    let session = normalize_source(&jsonl_input(
        "claude",
        CLAUDE_COMPACTION_SHARES_BUCKET_FIXTURE,
    ))
    .unwrap();
    let m = analyze_session(&session);
    assert_eq!(m.peak_context_tokens, 200_000);

    let boundary_indices: Vec<usize> = m
        .buckets
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_compaction_boundary)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(boundary_indices.len(), 1, "got {boundary_indices:?}");
    let boundary = boundary_indices[0];

    assert!(m.buckets[boundary].tokens_in > 0);
    assert_eq!(m.buckets[boundary].context_tokens, 0);

    let after = m.buckets[boundary + 1..]
        .iter()
        .find(|b| b.context_tokens > 0)
        .expect("a context reading after the boundary");
    assert_eq!(after.context_tokens, 10_000);
}

/// A Claude session where a cached turn (large cache-read ratio) is followed
/// by a turn that rewrites almost the whole context back to the cache — the
/// TTL-lapse pattern a cache rehydration should catch.
const CLAUDE_CACHE_REHYDRATION_FIXTURE: &str = concat!(
    r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":50,"cache_read_input_tokens":25000},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T12:05:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":50,"cache_creation_input_tokens":25000},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"b.rs"}}]}}"#,
);

#[test]
fn cache_rehydration_is_detected_after_a_ttl_lapse() {
    let session =
        normalize_source(&jsonl_input("claude", CLAUDE_CACHE_REHYDRATION_FIXTURE)).unwrap();
    let m = analyze_session(&session);

    let rehydrated: Vec<usize> = m
        .buckets
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_cache_rehydration)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(rehydrated.len(), 1, "got {rehydrated:?}");
}

/// Real numbers from a Claude session after a 155-minute idle gap. The
/// system prompt and tools (24,682 tokens) stay cached; only the conversation
/// re-writes, so the write share is 76%, not close to 100%.
const CLAUDE_CACHE_REHYDRATION_WITH_CACHED_PREFIX_FIXTURE: &str = concat!(
    r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":101,"output_tokens":50,"cache_read_input_tokens":99584,"cache_creation_input_tokens":1879},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T14:35:00Z","message":{"role":"assistant","usage":{"input_tokens":2,"output_tokens":50,"cache_read_input_tokens":24682,"cache_creation_input_tokens":77040},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"b.rs"}}]}}"#,
);

#[test]
fn cache_rehydration_is_detected_when_the_prefix_stays_cached() {
    let session = normalize_source(&jsonl_input(
        "claude",
        CLAUDE_CACHE_REHYDRATION_WITH_CACHED_PREFIX_FIXTURE,
    ))
    .unwrap();
    let m = analyze_session(&session);

    let count = m.buckets.iter().filter(|b| b.is_cache_rehydration).count();
    assert_eq!(count, 1, "a 76% rewrite after a long gap is a rehydration");
}

#[test]
fn first_turn_after_compaction_is_not_flagged_as_rehydration() {
    // Same shape as the rehydration fixture, but a compaction boundary sits
    // between the cached turn and the full-rewrite turn — the rewrite is
    // explained by the compaction, not a cache TTL lapse.
    let fixture = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":50,"cache_read_input_tokens":25000},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
        "\n",
        r#"{"type":"system","subtype":"compact_boundary","timestamp":"2024-06-01T12:00:30Z","content":"Compacted conversation"}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:05:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":50,"cache_creation_input_tokens":25000},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"b.rs"}}]}}"#,
    );
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    let m = analyze_session(&session);

    assert!(
        m.buckets.iter().all(|b| !b.is_cache_rehydration),
        "the first turn after a compaction must not be flagged as a rehydration"
    );
}

#[test]
fn small_contexts_are_not_flagged_as_rehydration() {
    // Same write/read ratios as the rehydration fixture, but scaled down so
    // both turns sit well under the 20k-token rehydration floor.
    let fixture = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":5000},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:05:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":50,"cache_creation_input_tokens":8000},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"b.rs"}}]}}"#,
    );
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    let m = analyze_session(&session);

    assert!(
        m.buckets.iter().all(|b| !b.is_cache_rehydration),
        "a small context rewrite must not be flagged as a rehydration"
    );
}

#[test]
fn cache_read_and_write_tokens_are_summed_per_bucket() {
    let fixture = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":200,"cache_read_input_tokens":5000,"cache_creation_input_tokens":500},"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","usage":{"input_tokens":200,"output_tokens":50,"cache_read_input_tokens":1000,"cache_creation_input_tokens":300},"content":[{"type":"tool_use","name":"Grep","input":{"pattern":"foo"}}]}}"#,
    );
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    let m = analyze_session(&session);

    assert_eq!(
        m.buckets
            .iter()
            .map(|bucket| bucket.cache_read_tokens)
            .sum::<u64>(),
        6000
    );
    assert_eq!(
        m.buckets
            .iter()
            .map(|bucket| bucket.cache_write_tokens)
            .sum::<u64>(),
        800
    );
}

/// A Codex session carries `cached_input_tokens` (cache reads) but has no
/// separate cache-write signal in its `token_count` usage payload — Codex
/// folds any cache write into `input_tokens` with no distinct field. The
/// bucket's `cache_read_tokens` must still carry the observed reads through;
/// `cache_write_tokens` stays zero since the format has nothing to report.
const CODEX_CACHE_FIXTURE: &str = concat!(
    r#"{"timestamp":"2024-06-01T12:00:00Z","type":"response_item","payload":{"type":"function_call","name":"read_file","arguments":"{}","call_id":"c1"}}"#,
    "\n",
    r#"{"timestamp":"2024-06-01T12:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":30000,"cached_input_tokens":25000,"output_tokens":80},"model_context_window":258400}}}"#,
);

#[test]
fn codex_cache_read_tokens_are_carried_into_buckets() {
    let session = normalize_source(&jsonl_input("codex", CODEX_CACHE_FIXTURE)).unwrap();
    let m = analyze_session(&session);

    assert_eq!(
        m.buckets
            .iter()
            .map(|bucket| bucket.cache_read_tokens)
            .sum::<u64>(),
        25_000
    );
    assert_eq!(
        m.buckets
            .iter()
            .map(|bucket| bucket.cache_write_tokens)
            .sum::<u64>(),
        0
    );
}

/// An OpenCode session as the discovery layer emits it: `message` rows (role +
/// `payload.tokens`) and `part` rows (text / tool / patch) tagged by `messageID`.
/// Messages come first, then parts — the adapter joins parts onto their message.
const OPENCODE_FIXTURE: &str = concat!(
    r#"{"type":"message","role":"user","time":{"created":1717243100000},"messageID":"m0","sessionID":"s1","payload":{"role":"user"}}"#,
    "\n",
    r#"{"type":"message","role":"assistant","time":{"created":1717243200000},"messageID":"m1","sessionID":"s1","payload":{"role":"assistant","tokens":{"input":1000,"output":50,"reasoning":0,"cache":{"write":500,"read":0}}}}"#,
    "\n",
    r#"{"type":"part","partType":"text","time":{"created":1717243100000},"messageID":"m0","sessionID":"s1","payload":{"type":"text","text":"review changelist"}}"#,
    "\n",
    r#"{"type":"part","partType":"text","time":{"created":1717243200000},"messageID":"m1","sessionID":"s1","payload":{"type":"text","text":"working on it"}}"#,
    "\n",
    r#"{"type":"part","partType":"tool","time":{"created":1717243201000},"messageID":"m1","sessionID":"s1","payload":{"type":"tool","tool":"bash","callID":"t1","state":{"status":"completed","input":{"command":"cargo test"}}}}"#,
    "\n",
    r#"{"type":"part","partType":"patch","time":{"created":1717243202000},"messageID":"m1","sessionID":"s1","payload":{"type":"patch","files":["a.rs"]}}"#,
);

#[test]
fn opencode_message_part_stream_is_joined() {
    let session = normalize_source(&jsonl_input("opencode", OPENCODE_FIXTURE)).unwrap();
    // Two messages → two events; the four parts merge onto them.
    assert_eq!(session.events.len(), 2);

    // m0 is the user prompt.
    assert_eq!(session.events[0].role, Role::User);

    // m1 gets usage from the message row and tools from its parts.
    let a = &session.events[1];
    assert_eq!(a.role, Role::Assistant);
    assert_eq!(a.usage.input_tokens, 1000);
    assert_eq!(a.usage.output_tokens, 50);
    assert_eq!(a.usage.cache_creation_tokens, 500);
    assert_eq!(a.tools.len(), 2); // bash + patch

    let m = analyze_session(&session);
    assert_eq!(m.tokens_in, 1500); // 1000 fresh input + 500 cache write
    assert_eq!(m.tokens_out, 50);
    assert_eq!(m.tool_mix.test, 1); // bash running `cargo test`
    assert_eq!(m.tool_mix.edit, 1); // patch
}

/// OpenCode reports two distinct per-turn context readings.
const OPENCODE_DRIFT_FIXTURE: &str = concat!(
    r#"{"type":"message","role":"assistant","time":{"created":1717243200000},"messageID":"m1","sessionID":"s2","payload":{"role":"assistant","tokens":{"input":8000,"output":50,"reasoning":0,"cache":{"write":0,"read":0}}}}"#,
    "\n",
    r#"{"type":"message","role":"assistant","time":{"created":1717243260000},"messageID":"m2","sessionID":"s2","payload":{"role":"assistant","tokens":{"input":200,"output":20,"reasoning":0,"cache":{"write":0,"read":0}}}}"#,
    "\n",
    r#"{"type":"part","partType":"tool","time":{"created":1717243200000},"messageID":"m1","sessionID":"s2","payload":{"type":"tool","tool":"read","callID":"r1","state":{"status":"completed","input":{"filePath":"a.rs"}}}}"#,
    "\n",
    r#"{"type":"part","partType":"tool","time":{"created":1717243260000},"messageID":"m2","sessionID":"s2","payload":{"type":"tool","tool":"read","callID":"r2","state":{"status":"completed","input":{"filePath":"b.rs"}}}}"#,
);

#[test]
fn opencode_buckets_keep_observed_context_readings() {
    let session = normalize_source(&jsonl_input("opencode", OPENCODE_DRIFT_FIXTURE)).unwrap();
    let m = analyze_session(&session);
    // Throughput is per-turn: 8000 then 200 (input, no cache here).
    assert_eq!(m.tokens_in, 8000 + 200);

    let ctx: Vec<u64> = m
        .buckets
        .iter()
        .filter(|b| b.context_tokens > 0)
        .map(|b| b.context_tokens)
        .collect();
    assert_eq!(ctx, vec![8_000, 200]);
}

/// Claude reports each turn's full prompt.
#[test]
fn claude_buckets_do_not_exceed_real_context_peak() {
    let session = normalize_source(&jsonl_input("claude", CLAUDE_FIXTURE)).unwrap();
    let m = analyze_session(&session);
    assert_eq!(m.peak_context_tokens, 6000);
    assert_eq!(m.context_window, crate::analysis::engine::CONTEXT_WINDOW);
    let ctx: Vec<u64> = m
        .buckets
        .iter()
        .filter(|b| b.context_tokens > 0)
        .map(|b| b.context_tokens)
        .collect();
    assert_eq!(ctx, vec![6_000, 1_200]);
    assert!(ctx.iter().all(|&c| c <= 6000));
}

/// A 1M-context Opus session: the window is inferred from `message.model`, so a
/// 300k prompt reads as 30% of 1M rather than 150% of the 200k default.
#[test]
fn claude_infers_1m_window_from_model() {
    let fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-opus-4-7","usage":{"input_tokens":300000,"output_tokens":200},"content":[{"type":"text","text":"hi"}]}}"#;
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    assert_eq!(session.context_window, Some(1_000_000));
    assert_eq!(session.model.as_deref(), Some("claude-opus-4-7"));
    let m = analyze_session(&session);
    assert_eq!(m.peak_context_tokens, 300_000);
    assert_eq!(m.context_window, 1_000_000);
    assert!(m.peak_context_tokens * 2 < m.context_window);
    // Model is captured and priced; billable components are tracked separately
    // from the occupancy-based `tokens_in`.
    assert_eq!(m.model.as_deref(), Some("claude-opus-4-7"));
    assert_eq!(m.billable_input_tokens, 300_000);
    assert_eq!(m.billable_output_tokens, 200);
    let cost = m.cost.expect("opus-4-7 is priceable");
    // 300k input @ $5/1M + 200 output @ $25/1M.
    assert!((cost.input_usd - 1.5).abs() < 1e-9);
    assert!((cost.output_usd - 0.005).abs() < 1e-9);
    assert!((cost.total_usd - 1.505).abs() < 1e-9);
}

#[test]
fn claude_fable_5_has_1m_context() {
    let fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-fable-5","usage":{"input_tokens":300000,"output_tokens":200},"content":[{"type":"text","text":"hi"}]}}"#;
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    assert_eq!(session.context_window, Some(1_000_000));
    let metrics = analyze_session(&session);
    assert!(metrics.context_available);
    assert_eq!(metrics.context_window, 1_000_000);
}

#[test]
fn claude_sonnet_5_has_1m_context() {
    let fixture = r#"{"type":"assistant","timestamp":"2026-07-22T10:00:00Z","message":{"role":"assistant","model":"claude-sonnet-5","usage":{"input_tokens":300000,"output_tokens":200},"content":[{"type":"text","text":"hi"}]}}"#;
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    assert_eq!(session.context_window, Some(1_000_000));
    let metrics = analyze_session(&session);
    assert!(metrics.context_available);
    assert_eq!(metrics.context_window, 1_000_000);
}

#[test]
fn unknown_claude_context_is_unavailable() {
    let fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-unknown-99","usage":{"input_tokens":1000,"output_tokens":50},"content":[{"type":"text","text":"x"}]}}"#;
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    assert_eq!(session.context_window, None);
    let metrics = analyze_session(&session);
    assert!(!metrics.context_available);
    let summary = analyze_sources(vec![jsonl_input("claude", fixture)]);
    assert!(!summary.context_available);
}

/// A session whose model isn't recorded (or isn't in the pricing table) yields no
/// cost estimate — the UI then shows no badge rather than a misleading `$0`.
#[test]
fn cost_is_none_when_model_unknown() {
    let fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":50},"content":[{"type":"text","text":"x"}]}}"#;
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    assert_eq!(session.model, None);
    let m = analyze_session(&session);
    assert_eq!(m.model, None);
    assert_eq!(m.cost, None);
    // Billable components are still tracked even without a price.
    assert_eq!(m.billable_input_tokens, 1000);
    assert_eq!(m.billable_output_tokens, 50);
}

/// `cost_total_usd` sums the priceable sessions; a session with an unknown model
/// contributes nothing rather than forcing the total to `None`.
#[test]
fn aggregate_sums_cost_total() {
    let priced = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":1000000,"output_tokens":0},"content":[{"type":"text","text":"x"}]}}"#;
    let unpriced = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":50},"content":[{"type":"text","text":"y"}]}}"#;
    let summary = analyze_sources(vec![
        jsonl_input("claude", priced),
        jsonl_input("claude", unpriced),
    ]);
    // Only the priced session contributes: 1M input @ $5/1M = $5.
    let total = summary
        .cost_total_usd
        .expect("at least one priceable session");
    assert!((total - 5.0).abs() < 1e-9, "got {total}");
}

/// A session that mixes models (Opus on the main thread, Haiku on a subagent) is
/// priced per-model from each turn's own `message.model` — not entirely at the most
/// expensive model. This is the fix for the cost-overstatement bug.
#[test]
fn mixed_model_session_prices_each_model_at_its_own_rate() {
    let fixture = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":0,"output_tokens":1000000},"content":[{"type":"text","text":"main"}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","model":"claude-haiku-4-5","usage":{"input_tokens":0,"output_tokens":1000000},"content":[{"type":"text","text":"subagent"}]}}"#,
    );
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    // Display headline stays the most expensive priceable model seen.
    assert_eq!(session.model.as_deref(), Some("claude-opus-4-6"));
    let m = analyze_session(&session);
    assert_eq!(m.model.as_deref(), Some("claude-opus-4-6"));
    let cost = m.cost.expect("at least one model is priceable");
    // Per-model: Opus 1M out @ $25/1M + Haiku 1M out @ $5/1M = $30.
    assert!(
        (cost.total_usd - 30.0).abs() < 1e-9,
        "got {}",
        cost.total_usd
    );
    // Pricing both buckets at Opus (the old behavior) would have been 2M @ $25/1M
    // = $50; per-model must be strictly cheaper.
    assert!(cost.total_usd < 50.0);
}

#[test]
fn claude_records_distinct_model_thinking_modes() {
    let fixture = concat!(
        r#"{"type":"assistant","effort":"high","message":{"role":"assistant","model":"claude-fable-5","usage":{"input_tokens":10,"output_tokens":2},"content":[{"type":"text","text":"one"}]}}"#,
        "\n",
        r#"{"type":"assistant","effort":"low","message":{"role":"assistant","model":"claude-fable-5","usage":{"input_tokens":20,"output_tokens":3},"content":[{"type":"text","text":"two"}]}}"#,
    );
    let metrics = analyze_session(&normalize_source(&jsonl_input("claude", fixture)).unwrap());

    assert_eq!(
        metrics.model_runs,
        vec![
            ModelRun {
                model: "claude-fable-5".to_string(),
                thinking_mode: Some("high".to_string()),
            },
            ModelRun {
                model: "claude-fable-5".to_string(),
                thinking_mode: Some("low".to_string()),
            },
        ]
    );
}

#[test]
fn codex_applies_turn_effort_to_the_following_usage() {
    let fixture = concat!(
        r#"{"type":"turn_context","payload":{"model":"gpt-5.6-sol","effort":"xhigh"}}"#,
        "\n",
        r#"{"type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"output_tokens":10}}}}"#,
    );
    let session = normalize_source(&jsonl_input("codex", fixture)).unwrap();
    assert_eq!(session.events[0].thinking_mode.as_deref(), Some("xhigh"));

    let metrics = analyze_session(&session);
    assert_eq!(
        metrics.model_runs,
        vec![ModelRun {
            model: "gpt-5.6-sol".to_string(),
            thinking_mode: Some("xhigh".to_string()),
        }]
    );
}

/// A model id carrying a context-window tag (`claude-opus-4-7[1m]`) still prices —
/// the engine strips the `[..]` tag before the table lookup the crate doesn't.
#[test]
fn windowed_model_tag_still_prices() {
    let fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-opus-4-7[1m]","usage":{"input_tokens":1000000,"output_tokens":0},"content":[{"type":"text","text":"x"}]}}"#;
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    let m = analyze_session(&session);
    let cost = m.cost.expect("opus-4-7[1m] strips to a priceable key");
    // 4-7 shares the 4-6 tier: 1M input @ $5/1M = $5.
    assert!(
        (cost.total_usd - 5.0).abs() < 1e-9,
        "got {}",
        cost.total_usd
    );
}

/// Claude re-logs the same assistant message (same `message.id`) multiple times,
/// each copy carrying the full usage. The adapter de-duplicates by message id so
/// token counts and cost are counted once — not multiplied by the copy count.
/// (Real transcripts do this heavily, e.g. one assistant message logged 5×, which
/// is the root cause of the on-device cost overstatement vs the backend.)
#[test]
fn duplicate_claude_message_ids_are_deduplicated() {
    // msg_a logged 3×, msg_b once. A naive per-line sum would triple msg_a — and
    // its 1M cache-read in particular, the dominant cost component.
    let line_a = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","id":"msg_a","model":"claude-opus-4-6","usage":{"input_tokens":1000,"output_tokens":100,"cache_read_input_tokens":1000000},"content":[{"type":"text","text":"a"}]}}"#;
    let line_b = r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","id":"msg_b","model":"claude-opus-4-6","usage":{"input_tokens":2000,"output_tokens":200,"cache_read_input_tokens":0},"content":[{"type":"text","text":"b"}]}}"#;
    let fixture = [line_a, line_a, line_a, line_b].join("\n");
    let session = normalize_source(&jsonl_input("claude", &fixture)).unwrap();
    let m = analyze_session(&session);
    // Each distinct message id contributes once: input 1000+2000, output 100+200,
    // cache_read 1_000_000 (msg_a counted once, not 3×).
    assert_eq!(m.billable_input_tokens, 3000);
    assert_eq!(m.billable_output_tokens, 300);
    assert_eq!(m.billable_cache_read_tokens, 1_000_000);
    // Opus rates: 3000·5e-06 + 300·2.5e-05 + 1e6·5e-07 = 0.015 + 0.0075 + 0.5.
    let cost = m.cost.expect("priceable");
    assert!(
        (cost.total_usd - 0.5225).abs() < 1e-9,
        "got {}",
        cost.total_usd
    );
}

/// When the model isn't recognized (no `model`, or a model we mapped to 200k) but
/// the session demonstrably held more, the window snaps up to the tier that fits
/// — occupancy can't exceed the real window, so the chart never tops 100%.
#[test]
fn context_window_floors_to_observed_peak() {
    let fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":250000,"output_tokens":10},"content":[{"type":"text","text":"x"}]}}"#;
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    assert_eq!(session.context_window, None); // no model recorded
    let m = analyze_session(&session);
    assert_eq!(m.peak_context_tokens, 250_000);
    // 250k exceeds the 200k default, so the window uses the 1M tier.
    assert_eq!(m.context_window, 1_000_000);
    assert!(m.peak_context_tokens <= m.context_window);
}

#[test]
fn aggregate_blends_multiple_vendors() {
    let inputs = vec![
        jsonl_input("claude", CLAUDE_FIXTURE),
        jsonl_input(
            "amp",
            r#"{"role":"assistant","usage":{"prompt_tokens":500,"completion_tokens":100},"content":"x","tool_calls":[{"function":{"name":"write_file"}}]}"#,
        ),
    ];
    let summary = analyze_sources(inputs);
    assert_eq!(summary.session_count, 2);
    assert_eq!(summary.tool_mix.edit, 2); // Edit + write_file
    assert_eq!(summary.tool_mix.search, 1); // Grep
    assert_eq!(summary.grep_total, 1);
    assert_eq!(summary.buckets.len(), crate::analysis::BUCKETS);
    assert!(summary.tokens_out_total > 0);
}

/// The initial-context graft keys on `(agent, session_id)`. Two same-agent
/// sessions must each keep their own breakdown — which only holds when their ids
/// are distinct. (`discover_active_session_inputs` now falls back to the full
/// path instead of an empty id so File-sourced sessions never collide here — F12.)
#[test]
fn initial_context_grafts_to_each_session_by_distinct_id() {
    // Skill listing precedes the assistant so attribution is independent of the
    // post-assistant-ack handling; the assistant supplies the analyzable event.
    let session = |skill: &str| {
        format!(
            concat!(
                r#"{{"type":"attachment","attachment":{{"type":"skill_listing","content":"- {skill}: a skill."}}}}"#,
                "\n",
                r#"{{"type":"assistant","message":{{"role":"assistant","usage":{{"input_tokens":1000}},"content":[{{"type":"text","text":"hi"}}]}}}}"#,
            ),
            skill = skill
        )
    };
    let inputs = vec![
        SessionInput {
            agent: "claude".into(),
            session_id: "sess-a".into(),
            source: RawSource::Jsonl(session("skill-a")),
        },
        SessionInput {
            agent: "claude".into(),
            session_id: "sess-b".into(),
            source: RawSource::Jsonl(session("skill-b")),
        },
    ];
    let summary = analyze_sources(inputs);
    assert_eq!(summary.session_count, 2);

    let skill_names = |id: &str| -> Vec<String> {
        summary
            .sessions
            .iter()
            .find(|m| m.session_id == id)
            .and_then(|m| m.initial_context.as_ref())
            .map(|ic| {
                ic.sources
                    .iter()
                    .filter_map(|s| s.source_name.clone())
                    .collect()
            })
            .unwrap_or_default()
    };

    // Each session's breakdown reflects its own skill — neither overwrote the
    // other, which is exactly the collision an empty shared id would have caused.
    assert!(skill_names("sess-a").iter().any(|n| n == "skill-a"));
    assert!(!skill_names("sess-a").iter().any(|n| n == "skill-b"));
    assert!(skill_names("sess-b").iter().any(|n| n == "skill-b"));
    assert!(!skill_names("sess-b").iter().any(|n| n == "skill-a"));
}

#[test]
fn empty_inputs_give_empty_summary() {
    let summary = analyze_sources(vec![]);
    assert_eq!(summary.session_count, 0);
    assert_eq!(summary.buckets.len(), crate::analysis::BUCKETS);
}

#[test]
fn serialized_metrics_exclude_removed_phase_and_health_data() {
    let session = normalize_source(&jsonl_input("claude", CLAUDE_FIXTURE)).unwrap();
    let value = serde_json::to_value(analyze_session(&session)).unwrap();

    for field in [
        "disruptionCount",
        "phaseDistribution",
        "patternScore",
        "signals",
        "segments",
        "skillUses",
    ] {
        assert!(value.get(field).is_none(), "unexpected field {field}");
    }

    let bucket = value["buckets"][0].as_object().expect("a bucket object");
    assert!(!bucket.contains_key("dominantPhase"));
    assert!(!bucket.contains_key("distribution"));
}

#[test]
fn cursor_transcript_uses_embedded_time_model_and_structured_tools() {
    let session = normalize_source(&jsonl_input(
        "cursor",
        concat!(
            r#"{"sessionId":"cursor-1","cursor_source":"agent_transcript","model":"cursor-small"}"#,
            "\n",
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<timestamp>Tuesday, Jun 9, 2026, 6:32 PM (UTC+10)</timestamp>\n<user_query>update the code</user_query>"}]}}"#,
            "\n",
            r#"{"role":"assistant","timestamp":"2026-06-09T08:33:00Z","content":"done","tool_calls":[{"function":{"name":"apply_patch","arguments":"{}"}}]}"#,
        ),
    ))
    .unwrap();

    assert_eq!(session.events.len(), 2);
    assert_eq!(session.model.as_deref(), Some("cursor-small"));
    assert_eq!(session.events[0].model.as_deref(), Some("cursor-small"));
    assert_eq!(session.events[1].tools[0].category, ToolCategory::Edit);
    let metrics = analyze_session(&session);
    assert_eq!(metrics.duration_secs, 60);
    assert_eq!(metrics.tool_mix.edit, 1);
}

/// A brain-transcript payload: the synthesized metadata header line followed
/// by step-based JSONL (user input, a planner/thinking turn, an Edit tool step,
/// and a failed tool response).
const ANTIGRAVITY_BRAIN_FIXTURE: &str = concat!(
    r#"{"sessionId":"abc","source":"antigravity_brain","cwd":"/tmp/foo","title":"help"}"#,
    "\n",
    r#"{"step_index":0,"type":"USER_INPUT","status":"DONE","created_at":"2024-06-01T12:00:00Z","content":"<USER_REQUEST>\nedit a file\n</USER_REQUEST>"}"#,
    "\n",
    r#"{"type":"PLANNER_RESPONSE","status":"DONE","created_at":"2024-06-01T12:00:30Z","content":"planning the edit","tool_calls":[{"name":"grep_search","args":{}},{"name":"write_to_file","args":{}}]}"#,
    "\n",
    r#"{"type":"GREP_SEARCH","status":"DONE","created_at":"2024-06-01T12:00:45Z","content":"results"}"#,
    "\n",
    r#"{"type":"WRITE_TO_FILE","status":"DONE","created_at":"2024-06-01T12:01:00Z","content":"written"}"#,
    "\n",
    r#"{"type":"ERROR_MESSAGE","status":"failed","created_at":"2024-06-01T12:01:30Z","content":"boom"}"#,
);

#[test]
fn antigravity_brain_transcript_normalizes_steps() {
    let session = normalize_source(&jsonl_input("antigravity", ANTIGRAVITY_BRAIN_FIXTURE)).unwrap();
    // Header line is skipped; the five steps become events.
    assert_eq!(session.events.len(), 5);

    assert_eq!(session.events[0].role, Role::User);

    // The planner step carries the tool calls.
    assert_eq!(session.events[1].role, Role::Assistant);
    assert_eq!(session.events[1].tools.len(), 2);
    assert_eq!(session.events[1].tools[0].category, ToolCategory::Search);
    assert_eq!(session.events[1].tools[1].category, ToolCategory::Edit);

    // Action result steps do not count the planner's tool calls again.
    assert_eq!(session.events[2].role, Role::Tool);
    assert!(session.events[2].tools.is_empty());

    assert_eq!(session.events[4].role, Role::Tool);

    let m = analyze_session(&session);
    assert_eq!(m.tool_mix.search, 1);
    assert_eq!(m.tool_mix.edit, 1);
    assert_eq!(m.grep_count, 1);
    // Span 12:00 → 12:01:30 = 90s.
    assert_eq!(m.duration_secs, 90);
}

/// An API-cascade payload: a single JSON document whose steps live under
/// `/steps/steps`, using the `CORTEX_STEP_TYPE_*` namings.
const ANTIGRAVITY_CASCADE_FIXTURE: &str = concat!(
    r#"{"sessionId":"c1","source":"antigravity_api","baseUri":{"path":"/tmp"},"steps":{"steps":["#,
    r#"{"type":"CORTEX_STEP_TYPE_USER_INPUT","status":"ok","userInput":{"userResponse":"hello world"}},"#,
    r#"{"type":"CORTEX_STEP_TYPE_TOOL","status":"ok","toolName":"read_file"}"#,
    r#"]}}"#,
);

#[test]
fn antigravity_cascade_document_normalizes_steps() {
    let session =
        normalize_source(&jsonl_input("antigravity", ANTIGRAVITY_CASCADE_FIXTURE)).unwrap();
    // The wrapping document is not itself an event; only its two steps are.
    assert_eq!(session.events.len(), 2);

    assert_eq!(session.events[0].role, Role::User);

    assert_eq!(session.events[1].role, Role::Tool);
    assert_eq!(session.events[1].tools.len(), 1);
    assert_eq!(session.events[1].tools[0].category, ToolCategory::Read);
}

/// API-cascade steps carry their timestamp under `metadata.createdAt`, not at
/// the top level. Without reading it, every event parses with no time and the
/// session shows "0 mins active" (regression that hid Agent-Manager sessions'
/// real duration).
const ANTIGRAVITY_CASCADE_TS_FIXTURE: &str = concat!(
    r#"{"sessionId":"c2","source":"antigravity_api","baseUri":{"path":"/tmp"},"steps":{"steps":["#,
    r#"{"type":"CORTEX_STEP_TYPE_USER_INPUT","status":"ok","userInput":{"userResponse":"hi"},"metadata":{"createdAt":"2026-06-09T11:00:00Z"}},"#,
    r#"{"type":"CORTEX_STEP_TYPE_PLANNER_RESPONSE","status":"ok","content":"ok","metadata":{"createdAt":"2026-06-09T11:01:00Z"}}"#,
    r#"]}}"#,
);

#[test]
fn antigravity_cascade_reads_timestamp_from_metadata() {
    let session =
        normalize_source(&jsonl_input("antigravity", ANTIGRAVITY_CASCADE_TS_FIXTURE)).unwrap();
    assert_eq!(session.events.len(), 2);
    let t0 = session.events[0]
        .ts_ms
        .expect("step 0 timestamp from metadata.createdAt");
    let t1 = session.events[1]
        .ts_ms
        .expect("step 1 timestamp from metadata.createdAt");
    assert_eq!(t1 - t0, 60_000, "events are 60s apart");
}

#[test]
fn antigravity_aggregates_alongside_claude() {
    let summary = analyze_sources(vec![
        jsonl_input("antigravity", ANTIGRAVITY_BRAIN_FIXTURE),
        jsonl_input("claude", CLAUDE_FIXTURE),
    ]);
    assert_eq!(summary.session_count, 2);
    // The Antigravity session contributes a non-empty, analyzed session.
    assert!(summary.sessions.iter().any(|s| s.tool_mix.edit >= 1));
}

#[test]
fn claude_skill_use_is_captured_with_position_tokens_and_duration() {
    // The invoking turn supplies usage. The next event supplies duration.
    let fixture = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":40},"content":[{"type":"tool_use","name":"Skill","input":{"skill":"deep-research"}}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#,
    );
    let m = analyze_session(&normalize_source(&jsonl_input("claude", fixture)).unwrap());

    assert_eq!(m.skill_uses.len(), 1);
    let su = &m.skill_uses[0];
    assert_eq!(su.name, "deep-research");
    assert_eq!(su.tokens_out, 40);
    assert_eq!(su.context_tokens, 1000);
    // Idle-capped gap to the next event (60s < idle threshold).
    assert_eq!(su.duration_ms, Some(60_000));
    // No description until a skill listing grafts one (see analyze_sources test).
    assert_eq!(su.description, None);
    // The skill remains in `tool_mix.other`.
    assert_eq!(m.tool_mix.other, 1);
    assert_eq!(m.tool_mix.edit, 1);
}

#[test]
fn codex_skill_use_is_captured_from_function_call() {
    // Codex emits skills as a `function_call` named `skill`, args a JSON string.
    let fixture = concat!(
        r#"{"timestamp":"2024-06-01T12:00:00Z","type":"response_item","payload":{"type":"function_call","name":"skill","arguments":"{\"skill\":\"verify\"}"}}"#,
        "\n",
        r#"{"timestamp":"2024-06-01T12:00:30Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":2000,"output_tokens":15},"model_context_window":272000}}}"#,
    );
    let m = analyze_session(&normalize_source(&jsonl_input("codex", fixture)).unwrap());

    assert_eq!(m.skill_uses.len(), 1);
    assert_eq!(m.skill_uses[0].name, "verify");
    // Gap to the next (token_count) event.
    assert_eq!(m.skill_uses[0].duration_ms, Some(30_000));
}

#[test]
fn claude_skill_use_is_captured_from_user_slash_command() {
    // A user-typed `/deep-research` is a command-name tag, NOT a `Skill` tool_use.
    // Paired with the skill's base-directory marker (proving it ran), it still
    // yields a marker. The trailing `/clear` command must not — it never ran.
    let fixture = concat!(
        r#"{"type":"user","timestamp":"2024-06-01T12:00:00Z","message":{"role":"user","content":[{"type":"text","text":"Base directory for this skill: /home/avery/.claude/skills/deep-research\n\nResearch harness."}]}}"#,
        "\n",
        r#"{"type":"user","timestamp":"2024-06-01T12:00:01Z","message":{"role":"user","content":"<command-name>/deep-research</command-name>"}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
        "\n",
        r#"{"type":"user","timestamp":"2024-06-01T12:02:00Z","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
    );
    let m = analyze_session(&normalize_source(&jsonl_input("claude", fixture)).unwrap());

    assert_eq!(m.skill_uses.len(), 1);
    assert_eq!(m.skill_uses[0].name, "deep-research");
}

#[test]
fn skill_descriptions_are_grafted_from_the_listing() {
    // The one-liner lives only in the raw transcript's skill listing, grafted onto
    // the marker by name during the `analyze_sources` raw-payload pass.
    let fixture = concat!(
        r#"{"type":"attachment","attachment":{"type":"skill_listing","content":"- deep-research: Fan-out web research harness."}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"output_tokens":40},"content":[{"type":"tool_use","name":"Skill","input":{"skill":"deep-research"}}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#,
    );
    let summary = analyze_sources(vec![jsonl_input("claude", fixture)]);

    assert_eq!(summary.sessions.len(), 1);
    let skills = &summary.sessions[0].skill_uses;
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "deep-research");
    assert_eq!(
        skills[0].description.as_deref(),
        Some("Fan-out web research harness.")
    );
}

/// The detail view summarizes one session. Its context must stay in real
/// tokens against its own window, not shrink into a fixed reference tier.
#[test]
fn single_session_summary_keeps_its_own_window_and_raw_context() {
    let fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-fable-5","usage":{"input_tokens":100,"cache_read_input_tokens":130000,"output_tokens":50},"content":[{"type":"text","text":"hi"}]}}"#;
    let summary = analyze_sources(vec![jsonl_input("claude", fixture)]);
    assert_eq!(summary.context_window, 1_000_000);
    assert_eq!(summary.peak_context_tokens, 130_100);
    let peak_bucket = summary
        .buckets
        .iter()
        .map(|b| b.context_tokens)
        .max()
        .unwrap_or(0);
    assert_eq!(peak_bucket, 130_100);
}

/// A mixed summary lands on the largest contributing window and scales the
/// smaller-window session up to it, so occupancy stays comparable.
#[test]
fn mixed_summary_scales_to_the_largest_window() {
    let small = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-3-5-haiku","usage":{"input_tokens":100000,"output_tokens":50},"content":[{"type":"text","text":"hi"}]}}"#;
    let large = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-fable-5","usage":{"input_tokens":200000,"output_tokens":50},"content":[{"type":"text","text":"hi"}]}}"#;
    let summary = analyze_sources(vec![
        jsonl_input("claude", small),
        jsonl_input("claude", large),
    ]);
    assert_eq!(summary.context_window, 1_000_000);
    // 100k of a 200k window is 50%, which is 500k of the 1M reference.
    assert_eq!(summary.peak_context_tokens, 500_000);
}

/* -------------------------------------------------------------------------
 * Sub-agent merge — `merge_subagent_events` + `analyze_session` together.
 * Ordering itself is covered by the unit tests in `analysis::merge`; these
 * exercise the product rule end to end: a sub-agent's tokens are an
 * implementation detail that sums into the parent, while context occupancy
 * and idle-gap timing stay governed by the parent's own turns (plus every
 * stream, for the idle rule).
 * ---------------------------------------------------------------------- */

/// Two parent turns ten minutes apart, with one sub-agent turn at the
/// halfway point.
const MERGE_PARENT_FIXTURE: &str = concat!(
    r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":100},"content":[{"type":"text","text":"start"}]}}"#,
    "\n",
    r#"{"type":"assistant","timestamp":"2024-06-01T12:10:00Z","message":{"role":"assistant","usage":{"input_tokens":2000,"output_tokens":200},"content":[{"type":"text","text":"end"}]}}"#,
);
const MERGE_SUBAGENT_FIXTURE: &str = r#"{"type":"assistant","timestamp":"2024-06-01T12:05:00Z","message":{"role":"assistant","usage":{"input_tokens":500,"output_tokens":50},"content":[{"type":"text","text":"delegated"}]}}"#;

#[test]
fn subagent_tokens_sum_into_parent_buckets_time_aligned() {
    let parent = normalize_source(&jsonl_input("claude", MERGE_PARENT_FIXTURE)).unwrap();
    let subagent = normalize_source(&jsonl_input("claude", MERGE_SUBAGENT_FIXTURE)).unwrap();
    let merged = merge_subagent_events(parent, vec![subagent]);

    // Every event is tagged with the stream it came from.
    let sources: Vec<EventSource> = merged.events.iter().map(|e| e.source).collect();
    assert_eq!(
        sources,
        vec![
            EventSource::Parent,
            EventSource::Subagent,
            EventSource::Parent
        ]
    );

    let m = analyze_session(&merged);
    // The session total sums every stream: subagents are an implementation
    // detail of the same session.
    assert_eq!(m.tokens_in, 1000 + 500 + 2000);
    assert_eq!(m.tokens_out, 100 + 50 + 200);

    // Both parent turns are 5 minutes apart from the sub-agent turn, well
    // under the idle threshold, so active time == wall-clock time and the
    // sub-agent turn lands exactly halfway through the progress grid.
    assert_eq!(m.buckets[0].tokens_in, 1000, "parent's first turn");
    assert_eq!(
        m.buckets[90].tokens_in, 500,
        "sub-agent's turn, time-aligned to the midpoint"
    );
    assert_eq!(m.buckets[179].tokens_in, 2000, "parent's last turn");
    assert_eq!(m.buckets.iter().map(|b| b.tokens_in).sum::<u64>(), 3500);
}

#[test]
fn peak_context_and_context_window_stay_parent_only_after_merge() {
    // The sub-agent's own context is far larger than anything the parent
    // ever sees. If it leaked into the parent's occupancy, the merged
    // session's peak and window would both jump.
    let parent_fixture = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","usage":{"input_tokens":1000,"output_tokens":50},"content":[{"type":"text","text":"a"}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:05:00Z","message":{"role":"assistant","usage":{"input_tokens":1500,"output_tokens":50},"content":[{"type":"text","text":"b"}]}}"#,
    );
    let subagent_fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:02:30Z","message":{"role":"assistant","usage":{"input_tokens":900000,"output_tokens":50},"content":[{"type":"text","text":"delegated"}]}}"#;

    let parent_only = normalize_source(&jsonl_input("claude", parent_fixture)).unwrap();
    let parent_only_metrics = analyze_session(&parent_only);

    let parent = normalize_source(&jsonl_input("claude", parent_fixture)).unwrap();
    let subagent = normalize_source(&jsonl_input("claude", subagent_fixture)).unwrap();
    let merged = merge_subagent_events(parent, vec![subagent]);
    let m = analyze_session(&merged);

    assert_eq!(
        m.peak_context_tokens,
        parent_only_metrics.peak_context_tokens
    );
    assert_eq!(m.peak_context_tokens, 1500);
    assert_eq!(m.context_window, parent_only_metrics.context_window);
    // No bucket's `context_tokens` (the parent-only occupancy reading) ever
    // reflects the sub-agent's 900k-token turn.
    assert!(m.buckets.iter().all(|b| b.context_tokens <= 1500));
    // The sub-agent's own huge input still lands in `tokens_in`, though —
    // that side of the merge is inclusive.
    assert_eq!(m.tokens_in, 1000 + 900_000 + 1500);
}

#[test]
fn idle_gap_counts_only_when_every_stream_is_idle() {
    // Same 8h parent gap as `active_time_excludes_idle_gaps`, but a
    // sub-agent turn lands in the middle of it. The gap is idle only when
    // the parent *and* every sub-agent are idle, so one event splits the
    // single 8h gap into two, each capped independently.
    let parent = normalize_source(&jsonl_input("claude", IDLE_GAP_FIXTURE)).unwrap();
    let subagent_fixture = r#"{"type":"assistant","timestamp":"2024-06-01T16:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Edit","input":{"file_path":"sub.rs"}}]}}"#;
    let subagent = normalize_source(&jsonl_input("claude", subagent_fixture)).unwrap();
    let merged = merge_subagent_events(parent, vec![subagent]);
    let m = analyze_session(&merged);

    // Wall-clock span is unchanged: 12:00:00 → 20:00:30 = 28830s.
    assert_eq!(m.duration_secs, 28830);
    // Active: 30s + capped(≈4h → 300s) + capped(≈4h → 300s) + 30s = 660s,
    // more than the 360s a lone parent gap gives — the sub-agent turn kept
    // both halves of the gap "awake" up to the cap on each side.
    assert_eq!(m.active_secs, 660);
}

#[test]
fn subagent_launch_marker_counts_only_parent_task_calls() {
    // The parent launches one sub-agent (a `Task` tool call) alongside an
    // ordinary edit; a same-named tool from a sub-agent's own transcript
    // must not count as a further launch.
    let parent_fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Task","input":{"prompt":"go look"}},{"type":"tool_use","name":"Edit","input":{"file_path":"a.rs"}}]}}"#;
    let subagent_fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:01:00Z","message":{"role":"assistant","content":[{"type":"tool_use","name":"Task","input":{"prompt":"nested"}}]}}"#;

    let parent = normalize_source(&jsonl_input("claude", parent_fixture)).unwrap();
    let subagent = normalize_source(&jsonl_input("claude", subagent_fixture)).unwrap();
    let merged = merge_subagent_events(parent, vec![subagent]);
    let m = analyze_session(&merged);

    let total_launches: u32 = m.buckets.iter().map(|b| b.subagent_launches).sum();
    assert_eq!(total_launches, 1, "only the parent's own Task call counts");
}

#[test]
fn rehydration_detection_ignores_subagent_turns() {
    // Same two parent turns as `cache_rehydration_is_detected_after_a_ttl_lapse`,
    // with a sub-agent turn carrying its own large cache-write in between. If
    // that turn fed the parent-only rehydration state machine, it would
    // either wrongly flag itself or reset the state so the real parent
    // rehydration is missed.
    let subagent_fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:02:30Z","message":{"role":"assistant","usage":{"input_tokens":10000,"output_tokens":50,"cache_creation_input_tokens":40000},"content":[{"type":"text","text":"delegated"}]}}"#;

    let parent =
        normalize_source(&jsonl_input("claude", CLAUDE_CACHE_REHYDRATION_FIXTURE)).unwrap();
    let subagent = normalize_source(&jsonl_input("claude", subagent_fixture)).unwrap();
    let merged = merge_subagent_events(parent, vec![subagent]);
    let m = analyze_session(&merged);

    let rehydrated: Vec<usize> = m
        .buckets
        .iter()
        .enumerate()
        .filter(|(_, b)| b.is_cache_rehydration)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        rehydrated.len(),
        1,
        "the parent's own rehydration must still be detected, got {rehydrated:?}"
    );
}

/* -------------------------------------------------------------------------
 * Mode signals — Bucket.model / thinking_mode / speed / has_thinking.
 * ---------------------------------------------------------------------- */

#[test]
fn mode_signals_land_in_the_bucket_of_the_event_that_carried_them() {
    let fixture = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":10,"output_tokens":5,"speed":"standard"},"content":[{"type":"text","text":"a"}]},"effort":"high"}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:10:00Z","message":{"role":"assistant","model":"claude-fable-5","usage":{"input_tokens":10,"output_tokens":5,"speed":"fast"},"content":[{"type":"thinking","thinking":"hmm"}]},"effort":"low"}"#,
    );
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    let m = analyze_session(&session);

    let first = &m.buckets[0];
    assert_eq!(first.model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(first.thinking_mode.as_deref(), Some("high"));
    assert_eq!(first.speed.as_deref(), Some("standard"));
    assert!(!first.has_thinking);

    let second = &m.buckets[179];
    assert_eq!(second.model.as_deref(), Some("claude-fable-5"));
    assert_eq!(second.thinking_mode.as_deref(), Some("low"));
    assert_eq!(second.speed.as_deref(), Some("fast"));
    assert!(second.has_thinking);
}

#[test]
fn mode_signal_bucket_keeps_the_last_value_seen_in_it() {
    // The first two events share a timestamp, so both land in bucket 0; the
    // third gives the session a real active-time span. The bucket must keep
    // the second event's mode, not the first.
    let fixture = concat!(
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":10,"speed":"standard"},"content":[{"type":"text","text":"a"}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-fable-5","usage":{"input_tokens":10,"speed":"fast"},"content":[{"type":"text","text":"b"}]}}"#,
        "\n",
        r#"{"type":"assistant","timestamp":"2024-06-01T12:10:00Z","message":{"role":"assistant","usage":{"input_tokens":10},"content":[{"type":"text","text":"c"}]}}"#,
    );
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    let m = analyze_session(&session);

    let bucket = &m.buckets[0];
    assert_eq!(bucket.model.as_deref(), Some("claude-fable-5"));
    assert_eq!(bucket.speed.as_deref(), Some("fast"));
}

#[test]
fn subagent_mode_signals_never_override_the_parent_buckets() {
    // The sub-agent runs a different model/effort/speed than the parent; none
    // of it should leak into the parent's own bucket mode signals.
    let parent_fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":10,"speed":"standard"},"content":[{"type":"text","text":"a"}]},"effort":"high"}"#;
    let subagent_fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:05:00Z","message":{"role":"assistant","model":"claude-haiku","usage":{"input_tokens":10,"speed":"fast"},"content":[{"type":"thinking","thinking":"delegated"}]},"effort":"low"}"#;

    let parent = normalize_source(&jsonl_input("claude", parent_fixture)).unwrap();
    let subagent = normalize_source(&jsonl_input("claude", subagent_fixture)).unwrap();
    let merged = merge_subagent_events(parent, vec![subagent]);
    let m = analyze_session(&merged);

    for bucket in &m.buckets {
        assert_ne!(bucket.model.as_deref(), Some("claude-haiku"));
        assert_ne!(bucket.thinking_mode.as_deref(), Some("low"));
        assert_ne!(bucket.speed.as_deref(), Some("fast"));
        assert!(
            !bucket.has_thinking,
            "the sub-agent's thinking must not set the parent bucket"
        );
    }
    assert_eq!(m.buckets[0].model.as_deref(), Some("claude-opus-4-6"));
    assert_eq!(m.buckets[0].thinking_mode.as_deref(), Some("high"));
    assert_eq!(m.buckets[0].speed.as_deref(), Some("standard"));
}

#[test]
fn buckets_with_no_mode_signal_stay_none() {
    let fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","content":[{"type":"text","text":"no model or usage here"}]}}"#;
    let session = normalize_source(&jsonl_input("claude", fixture)).unwrap();
    let m = analyze_session(&session);

    assert!(m.buckets.iter().all(|b| b.model.is_none()));
    assert!(m.buckets.iter().all(|b| b.thinking_mode.is_none()));
    assert!(m.buckets.iter().all(|b| b.speed.is_none()));
    assert!(m.buckets.iter().all(|b| !b.has_thinking));
}

#[test]
fn aggregate_metrics_leaves_mode_signals_at_default() {
    let fixture = r#"{"type":"assistant","timestamp":"2024-06-01T12:00:00Z","message":{"role":"assistant","model":"claude-opus-4-6","usage":{"input_tokens":10,"speed":"fast"},"content":[{"type":"thinking","thinking":"x"}]},"effort":"high"}"#;
    let summary = analyze_sources(vec![jsonl_input("claude", fixture)]);

    // A multi-session summary cannot carry one session's mode signals, since
    // each contributing session can run a different agent/model.
    assert!(summary.buckets.iter().all(|b| b.model.is_none()));
    assert!(summary.buckets.iter().all(|b| b.thinking_mode.is_none()));
    assert!(summary.buckets.iter().all(|b| b.speed.is_none()));
    assert!(summary.buckets.iter().all(|b| !b.has_thinking));
}

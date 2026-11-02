use super::*;
use std::io::Write;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/native_receipt_compatibility/decode.rs"
));

fn fixture(agent: &str) -> (tempfile::TempDir, SessionInput, Value, NativeFileCursor) {
    let text = match agent {
        "claude" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/native_receipt_compatibility/file-claude.json"
        )),
        "codex" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/native_receipt_compatibility/file-codex.json"
        )),
        "pi" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/native_receipt_compatibility/file-pi.json"
        )),
        "pending" => include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/native_receipt_compatibility/file-claude-pending.json"
        )),
        _ => unreachable!(),
    };
    let mut fixture: Value = serde_json::from_str(text).unwrap();
    expand_sparse_arrays(&mut fixture);
    assert_eq!(
        fixture["cursor_sha256"],
        crate::digest::sha256_hex(&serde_json::to_vec(&fixture["cursor"]).unwrap())
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.jsonl");
    std::fs::write(&path, fixture["source_prefix"].as_str().unwrap()).unwrap();
    let identity = NativeFileSource::open(&path, None)
        .unwrap()
        .anchor(0, 0)
        .unwrap()
        .identity;
    fixture["cursor"]["anchor"]["identity"] = serde_json::json!(identity);
    let input = SessionInput {
        agent: fixture["cursor"]["agent"].as_str().unwrap().into(),
        session_id: fixture["cursor"]["session_id"].as_str().unwrap().into(),
        source_format: serde_json::from_value(fixture["cursor"]["source_format"].clone()).unwrap(),
        source: RawSource::File(path),
        fork_parent_session_id: None,
    };
    let cursor = convert(&serde_json::to_vec(&fixture["cursor"]).unwrap()).unwrap();
    (directory, input, fixture, cursor)
}

fn append(input: &SessionInput, text: &str) {
    let RawSource::File(path) = &input.source else {
        unreachable!()
    };
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

#[test]
fn main_64_actual_file_bodies_keep_offsets_joins_and_complete_each_append_once() {
    for agent in ["claude", "codex", "pi"] {
        let (_directory, input, fixture, cursor) = fixture(agent);
        assert_eq!(
            cursor.offset(),
            fixture["cursor"]["anchor"]["tail"]["offset"]
                .as_u64()
                .unwrap()
        );
        let idle = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
        assert_eq!(idle.bytes_read, 0);
        assert!(idle.results.is_empty() && idle.evidence.events.is_empty());
        assert_eq!(idle.cursor.offset(), cursor.offset());
        assert_eq!(idle.baseline_offset, cursor.baseline_offset());
        assert_eq!(
            idle.cursor.evidence_baseline_offset(),
            fixture["source_prefix"].as_str().unwrap().len() as u64
        );
        assert!(!idle.history_complete);
        let retry: NativeFileCursor =
            serde_json::from_str(&cursor.clone().retry_json_after_page(&idle.cursor).unwrap())
                .unwrap();
        assert_eq!(retry.offset(), cursor.offset());
        assert_eq!(
            retry.evidence_baseline_offset(),
            idle.cursor.evidence_baseline_offset()
        );
        if agent == "claude" {
            assert!(idle.requests.is_empty());
        }
        let mut suffix = fixture["source_suffix"].as_str().unwrap().to_owned();
        if agent == "codex" {
            // Complete the old partial row with its required timestamp. Keep the saved prefix.
            suffix = format!(
                "{},\"timestamp\":\"2026-01-01T00:00:05Z\"}}\n",
                suffix.strip_suffix("}\n").unwrap()
            );
        }
        append(&input, &suffix);
        let restored = NativeFileCursor::decode(&idle.cursor.encode()).unwrap();
        let next = read_native_file_page(&input, Some(&restored), &|| false).unwrap();
        assert_eq!(next.results.len(), 1, "{agent}");
        assert_eq!(
            next.results[0].observation.call_id.as_deref(),
            Some("call-1")
        );
        assert!(next.results[0].live_eligible);
        assert!(next.results[0].observation.arguments.is_some());
        let final_page = read_native_file_page(&input, Some(&next.cursor), &|| false).unwrap();
        assert!(final_page.results.is_empty() && final_page.evidence.events.is_empty());
    }
}

#[test]
fn main_64_actual_pending_output_drains_without_source_reads_or_completed_replay() {
    let (_directory, input, fixture, cursor) = fixture("pending");
    assert_eq!(fixture["completed_count"], 256);
    let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
    assert_eq!(page.bytes_read, 0);
    assert_eq!(page.results.len(), 1);
    assert_eq!(
        page.results[0].observation.call_id.as_deref(),
        Some("queued-255")
    );
    assert_eq!(page.cursor.offset(), cursor.offset());
    assert!(
        page.results[0]
            .observation
            .context
            .as_ref()
            .unwrap()
            .user_messages
            .is_empty()
    );
    assert!(
        read_native_file_page(&input, Some(&page.cursor), &|| false)
            .unwrap()
            .results
            .is_empty()
    );
}

#[test]
fn main_64_conversion_freezes_current_public_frontier_and_retains_it_on_retry() {
    let (_directory, input, fixture, cursor) = fixture("claude");
    append(&input, fixture["source_suffix"].as_str().unwrap());
    let page = read_native_file_page(&input, Some(&cursor), &|| false).unwrap();
    assert_eq!(page.results.len(), 1);
    assert!(page.evidence.events.iter().all(|event| matches!(
        event.fact,
        NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(_))
    ) || !event.live_eligible));
    let retry: NativeFileCursor =
        serde_json::from_str(&cursor.clone().retry_json_after_page(&page.cursor).unwrap()).unwrap();
    assert_eq!(retry.offset(), cursor.offset());
    assert_eq!(
        retry.evidence_baseline_offset(),
        page.cursor.evidence_baseline_offset()
    );
    assert_eq!(retry.baseline_offset(), cursor.baseline_offset());
}

#[test]
fn main_64_malformed_pairs_and_changed_sources_still_fail_closed() {
    let (_directory, input, fixture, cursor) = fixture("codex");
    for mutation in [
        "revision",
        "agent",
        "session",
        "nested",
        "extra",
        "parser-field",
        "source-format",
    ] {
        let mut value = fixture["cursor"].clone();
        match mutation {
            "revision" => value["revision"] = serde_json::json!(2),
            "agent" => value["agent"] = serde_json::json!("pi"),
            "session" => value["session_id"] = serde_json::json!("other"),
            "nested" => {
                value["adapter"]["Codex"]["parser"]["native_calls"] =
                    serde_json::json!("{\"call\":{}}")
            }
            "extra" => value["pending_evidence"] = serde_json::json!("[]"),
            "parser-field" => {
                value["adapter"]["Codex"]["parser"]["future_field"] = serde_json::json!(true)
            }
            "source-format" => {
                value["adapter"]["Codex"]["parser"]["source_format"] =
                    serde_json::json!("pi_v3_jsonl")
            }
            _ => unreachable!(),
        }
        assert!(
            convert(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{mutation}"
        );
    }
    let RawSource::File(path) = &input.source else {
        unreachable!()
    };
    let mut bytes = std::fs::read(path).unwrap();
    bytes[0] = b' ';
    std::fs::write(path, bytes).unwrap();
    assert_eq!(
        read_native_file_page(&input, Some(&cursor), &|| false)
            .unwrap_err()
            .downcast_ref::<NativeFileReadError>(),
        Some(&NativeFileReadError::SourceChanged)
    );
}

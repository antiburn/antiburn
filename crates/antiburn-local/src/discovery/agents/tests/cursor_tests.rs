fn assert_cursor_scope_ordering_gap(content: &str) {
    let header_line = content.lines().next().expect("session has a header");
    let header: Value = serde_json::from_str(header_line).expect("session header is valid JSON");
    assert_eq!(header["cursor_scope_ordering"], "unproven");
    let input = crate::analysis::SessionInput {
        agent: "cursor".into(),
        session_id: "synthetic".into(),
        source: crate::analysis::RawSource::Jsonl(content.to_owned()),
        fork_parent_session_id: None,
        source_format: Default::default(),
    };
    let mut collector = crate::analysis::SessionCollector::new("cursor", "synthetic");
    crate::analysis::reader_for("cursor")
        .visit(&input, &mut collector)
        .expect("Cursor reader accepts the synthetic session");
    assert!(
        collector
            .partial_reasons()
            .contains(&crate::analysis::PartialReason::AttributionIncomplete)
    );
}

#[test]
fn cursor_discovery_tool_aliases_and_unknown_wrappers_never_promote_nested_users() {
    let values: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/cursor_characterization/discovery_boundaries.json"
    ))
    .unwrap();
    let metadata =
        parse_cursor_store_db_metadata(&[json!({"agentId":"synthetic"}).to_string()]).unwrap();
    for value in values {
        assert!(
            extract_cursor_message_candidates(&value).is_empty(),
            "{value}"
        );
        let content = build_cursor_store_db_content(
            &metadata,
            &[("unknown".into(), value.to_string())],
            None,
        )
        .unwrap();
        let raw: Value = serde_json::from_str(content.lines().nth(1).unwrap()).unwrap();
        assert_eq!(raw["type"], "__unrecognized_store_blob__");
        assert_eq!(raw["native"], value);
    }
    let messages = extract_cursor_message_candidates(&json!({"messages":[
        {"role":"user","content":"Recorded task"},
        {"role":"assistant","content":"Recorded proposal"}
    ]}));
    assert_eq!(messages.len(), 2);
}

#[test]
fn cursor_store_row_order_and_timestamps_do_not_prove_scope_chronology() {
    let values: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/cursor_characterization/ordering_corrections.json"
    ))
    .unwrap();
    for order in [[0, 1], [1, 0]] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("store.db");
        let db = create_cursor_store_db(&path);
        db.execute(
            "INSERT INTO meta VALUES ('session', ?1)",
            params![json!({"agentId":"synthetic"}).to_string()],
        )
        .unwrap();
        for index in order {
            db.execute(
                "INSERT INTO blobs VALUES (?1, ?2)",
                params![
                    values[index]["id"].as_str().unwrap(),
                    values[index].to_string()
                ],
            )
            .unwrap();
        }
        drop(db);
        let snapshot = read_cursor_store_db_snapshot(&path).unwrap();
        let content =
            build_cursor_store_db_content(&snapshot.metadata, &snapshot.records, None).unwrap();
        assert_cursor_scope_ordering_gap(&content);
        let turns = content
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0]["content"], values[order[0]]["content"]);
        assert_eq!(turns[1]["content"], values[order[1]]["content"]);
    }
}

#[test]
fn cursor_native_id_fallback_and_tied_timestamps_do_not_prove_scope_chronology() {
    let values: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/cursor_characterization/ordering_corrections.json"
    ))
    .unwrap();
    for order in [[0, 1], [1, 0]] {
        let temp = TempDir::new().unwrap();
        let db = create_cursor_db(&temp.path().join("state.vscdb"));
        db.execute(
            "INSERT INTO cursorDiskKV VALUES ('composerData:synthetic', '{}')",
            [],
        )
        .unwrap();
        for index in order {
            db.execute(
                "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
                params![
                    format!(
                        "bubbleId:synthetic:{}",
                        values[index]["id"].as_str().unwrap()
                    ),
                    values[index].to_string()
                ],
            )
            .unwrap();
        }
        let content = build_desktop_cursor_session_content(&db, "synthetic", None, None).unwrap();
        assert_cursor_scope_ordering_gap(&content);
        let turns = content
            .lines()
            .skip(1)
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(turns[0]["bubbleId"], "a-correction");
        assert_eq!(turns[1]["bubbleId"], "z-first");
        assert_eq!(turns[0]["content"], values[1]["content"]);
        assert_eq!(turns[1]["content"], values[0]["content"]);
    }
}

#[test]
fn cursor_native_scope_preserves_text_and_does_not_make_tool_fallbacks_user_scope() {
    let bubbles: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/cursor_characterization/native_scope_bubbles.json"
    ))
    .unwrap();
    assert_eq!(
        extract_cursor_bubble_text(&bubbles[0]).as_deref(),
        bubbles[0]["text"].as_str()
    );
    assert_eq!(
        extract_cursor_bubble_text(&bubbles[1]).as_deref(),
        bubbles[1]["richText"].as_str()
    );
    assert_eq!(extract_cursor_bubble_text(&bubbles[2]), None);
    assert_eq!(
        extract_cursor_bubble_text(&bubbles[3]).as_deref(),
        Some("approved")
    );
    assert_eq!(cursor_role_for_value(&bubbles[3]), Some("assistant"));
}

#[test]
fn native_read_projection_preserves_conflicts_and_bounds_params() {
    let fixture = include_str!(
        "../../../../tests/fixtures/cursor_characterization/native_read_grep_binary.jsonl"
    );
    let row: Value = fixture
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|row| {
            row.pointer("/cursor_tool_result/name")
                .and_then(Value::as_str)
                == Some("read_file_v2")
        })
        .unwrap();
    let original = row["cursor_tool_result"].clone();
    let mut conflicting = original.clone();
    conflicting["toolCallId"] = json!("different-call");
    let projected = cursor_native_tool_fields(&json!({"toolFormerData": conflicting}));
    assert_eq!(projected["native_result_binding_error"], "unsafe_identity");
    assert!(projected.get("native_result_binding").is_none());
    let mut oversized = original;
    oversized["params"] = json!(" ".repeat(crate::analysis::MAX_CONTENT_PART_BYTES + 1));
    let projected = cursor_native_tool_fields(&json!({"toolFormerData": oversized}));
    assert_eq!(projected["native_result_binding_error"], "malformed");
    assert!(projected.get("params").is_none());
    assert!(projected.get("native_result_binding").is_none());
}

#[test]
fn native_read_grep_projection_keeps_only_verified_bounded_binding() {
    let fixture = include_str!(
        "../../../../tests/fixtures/cursor_characterization/native_read_grep_binary.jsonl"
    );
    let mut rows = Vec::new();
    let mut expected = Vec::new();
    for mut line in fixture
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        let Some(tool) = line.get("cursor_tool_result") else {
            rows.push(line.to_string());
            continue;
        };
        let projected = cursor_native_tool_fields(&json!({"toolFormerData": tool}));
        assert!(projected.get("toolCallBinary").is_none());
        if matches!(
            tool.get("name").and_then(Value::as_str),
            Some("read_file_v2" | "ripgrep_raw_search")
        ) {
            assert!(
                projected["native_result_binding"]["args_verified"]
                    .as_bool()
                    .unwrap_or(false)
            );
            assert!(
                projected["native_result_binding"]["completed"]
                    .as_bool()
                    .unwrap_or(false)
            );
            assert_eq!(projected["native_result_binding"]["status"], "success");
            expected.push((
                tool["toolCallId"].as_str().unwrap().to_owned(),
                serde_json::from_str::<Value>(tool["params"].as_str().unwrap()).unwrap(),
            ));
        }
        line["cursor_tool_result"] = projected;
        rows.push(line.to_string());
    }
    let input = crate::analysis::SessionInput {
        agent: "cursor".into(),
        session_id: "cursor-native-probe".into(),
        source: crate::analysis::RawSource::Jsonl(rows.join("\n")),
        fork_parent_session_id: None,
        source_format: crate::analysis::SourceFormat::CursorIdeComposer,
    };
    let reader = crate::analysis::reader_for("cursor");
    let mut sink = crate::analysis::CompositeSink::new(
        crate::analysis::SessionMetricsAccumulator::new("cursor", "cursor-native-probe"),
        crate::analysis::SessionEvidenceAccumulator::new(crate::analysis::EvidenceSource {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            kind: crate::analysis::SourceKind::Jsonl,
            capabilities: reader.capabilities(&input),
        }),
    );
    reader.visit(&input, &mut sink).unwrap();
    let results = sink.tool_result_observations();
    assert_eq!(results.len(), expected.len());
    for (result, (call_id, arguments)) in results.iter().zip(expected) {
        assert_eq!(result.call_id.as_deref(), Some(call_id.as_str()));
        assert_eq!(result.arguments.as_ref(), Some(&arguments));
        assert_eq!(
            result.admission(),
            crate::analysis::ToolResultAdmission::Admitted
        );
    }
}

#[test]
fn desktop_native_tool_bubbles_keep_full_ids_and_bounded_result_fields() {
    let temp = TempDir::new().unwrap();
    let db = create_cursor_db(&temp.path().join("state.vscdb"));
    let bubble_id = "bubble/t:call\nresult";
    db.execute(
        "INSERT INTO cursorDiskKV VALUES ('composerData:synthetic', ?1)",
        params![
            json!({"fullConversationHeadersOnly":[{"bubbleId":bubble_id}], "conversationMap":{}})
                .to_string()
        ],
    )
    .unwrap();
    let tool = json!({"name":"run_terminal_command_v2", "toolCallId":"call\nresult", "status":"completed", "params":json!({"command":"cargo test parser"}).to_string(), "result":json!({"output":"Parser failed", "exitCode":1}).to_string(), "toolCallBinary":"excluded"});
    db.execute(
        "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
        params![
            format!("bubbleId:synthetic:{bubble_id}"),
            json!({"type":2,"bubbleId":bubble_id,"toolFormerData":tool}).to_string()
        ],
    )
    .unwrap();
    let content = build_desktop_cursor_session_content(&db, "synthetic", None, None).unwrap();
    let records = content
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records[0]["cursor_scope_ordering"], "recorded_headers");
    assert_eq!(records[1]["bubbleId"], bubble_id);
    assert_eq!(records[1]["cursor_tool_result"]["result"], tool["result"]);
    assert!(
        records[1]["cursor_tool_result"]
            .get("toolCallBinary")
            .is_none()
    );
    let oversized =
        json!({"toolFormerData":{"result":"x".repeat(crate::analysis::MAX_CONTENT_PART_BYTES+1)}});
    assert!(
        cursor_native_tool_fields(&oversized)
            .get("result")
            .is_none()
    );
}

#[test]
fn cursor_native_missing_user_and_missing_bubble_prevent_complete_history() {
    let temp = TempDir::new().unwrap();
    let db = create_cursor_db(&temp.path().join("state.vscdb"));
    let bubbles: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/cursor_characterization/native_scope_bubbles.json"
    ))
    .unwrap();
    let mut headers = bubbles
        .iter()
        .map(|bubble| json!({"bubbleId":bubble["bubbleId"]}))
        .collect::<Vec<_>>();
    headers.push(json!({"bubbleId":"missing-bubble"}));
    db.execute(
        "INSERT INTO cursorDiskKV VALUES ('composerData:synthetic', ?1)",
        params![json!({"fullConversationHeadersOnly":headers}).to_string()],
    )
    .unwrap();
    for bubble in &bubbles {
        db.execute(
            "INSERT INTO cursorDiskKV VALUES (?1, ?2)",
            params![
                format!(
                    "bubbleId:synthetic:{}",
                    bubble["bubbleId"].as_str().unwrap()
                ),
                bubble.to_string()
            ],
        )
        .unwrap();
    }
    let content = build_desktop_cursor_session_content(&db, "synthetic", None, None).unwrap();
    let records = content
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records[1]["bubbleId"], "user-1");
    assert_eq!(records[1]["content"], bubbles[0]["text"]);
    assert_eq!(records[2]["content"], bubbles[1]["richText"]);
    assert_eq!(records[3]["type"], "__unavailable_user_content__");
    assert_eq!(records[4]["role"], "assistant");
    assert_eq!(records[5]["type"], "__missing_bubble__");
    let mut collector = crate::analysis::SessionCollector::new("cursor", "synthetic");
    let input = crate::analysis::SessionInput {
        agent: "cursor".into(),
        session_id: "synthetic".into(),
        source: crate::analysis::RawSource::Jsonl(content),
        fork_parent_session_id: None,
        source_format: Default::default(),
    };
    crate::analysis::reader_for("cursor")
        .visit(&input, &mut collector)
        .unwrap();
    assert!(
        collector
            .partial_reasons()
            .contains(&crate::analysis::PartialReason::UnrecognizedRecordType)
    );
}

#[test]
fn cursor_store_generic_blobs_preserve_structure_order_and_identity() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("store.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT); CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);").unwrap();
    db.execute(
        "INSERT INTO meta VALUES ('session', ?1)",
        params![json!({"agentId":"synthetic-session"}).to_string()],
    )
    .unwrap();
    let source = include_str!(
        "../../../../tests/fixtures/cursor_characterization/store_reader_blocks.jsonl"
    );
    for (index, line) in source.lines().enumerate() {
        db.execute(
            "INSERT INTO blobs VALUES (?1, ?2)",
            params![format!("blob-{index}"), line.as_bytes()],
        )
        .unwrap();
    }
    let (rows, unreadable) = sqlite_table_string_rows(&db, "blobs");
    assert!(!unreadable);
    assert_eq!(rows.len(), 4);
    drop(db);
    let candidate = build_cursor_store_db_session(&path).unwrap();
    let SessionSource::Inline { content, .. } = candidate.log.source else {
        panic!("expected inline store")
    };
    let messages = content
        .lines()
        .skip(1)
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 4);
    for (message, original) in messages.iter().zip(source.lines()) {
        let original: Value = serde_json::from_str(original).unwrap();
        assert_eq!(message["content"], original["content"]);
        assert_eq!(message["role"], original["role"]);
        assert_eq!(message["bubbleId"], original["id"]);
        assert_eq!(message["providerOptions"], original["providerOptions"]);
    }
    assert_eq!(messages[0]["content"], messages[3]["content"]);
}

#[test]
fn cursor_store_does_not_extract_user_turns_from_tool_arguments() {
    let messages = extract_cursor_message_candidates(&json!({
        "role":"assistant",
        "content":[{"type":"tool-call","toolName":"Unknown","toolCallId":"call-1","args":{
            "role":"user","content":"This is not user authorization"
        }}]
    }));
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, "assistant");
    assert!(
        extract_cursor_message_candidates(&json!({
            "type":"tool-call","toolName":"Unknown","toolCallId":"call-1",
            "args":{"role":"user","content":"Not a user turn"}
        }))
        .is_empty()
    );
}

#[test]
fn cursor_store_unreadable_blob_remains_a_parser_gap() {
    let metadata =
        parse_cursor_store_db_metadata(&[json!({"agentId":"synthetic"}).to_string()]).unwrap();
    let content = build_cursor_store_db_content(
        &metadata,
        &[
            (
                "user-1".into(),
                json!({"role":"user","content":"Task"}).to_string(),
            ),
            ("bad".into(), r#"{"type":"__unreadable_blob__"}"#.into()),
        ],
        None,
    )
    .unwrap();
    let mut collector = crate::analysis::SessionCollector::new("cursor", "synthetic");
    let input = crate::analysis::SessionInput {
        agent: "cursor".into(),
        session_id: "synthetic".into(),
        source: crate::analysis::RawSource::Jsonl(content),
        fork_parent_session_id: None,
        source_format: Default::default(),
    };
    crate::analysis::reader_for("cursor")
        .visit(&input, &mut collector)
        .unwrap();
    assert!(
        collector
            .partial_reasons()
            .contains(&crate::analysis::PartialReason::UnrecognizedRecordType)
    );
}

#[test]
fn cursor_store_generic_schema_rejects_missing_metadata_and_marks_invalid_utf8() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("store.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT); CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);").unwrap();
    db.execute("INSERT INTO blobs VALUES ('call', ?1)", params![json!({"role":"assistant","content":[{"type":"tool-call","toolCallId":"call-1","toolName":"Read","args":{}}]}).to_string().as_bytes()]).unwrap();
    assert!(read_cursor_store_db_snapshot(&path).is_none());
    db.execute(
        "INSERT INTO meta VALUES ('session', ?1)",
        params![json!({"agentId":"synthetic"}).to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO blobs VALUES ('bad', ?1)",
        params![vec![0xff_u8, 0xfe]],
    )
    .unwrap();
    let (rows, unreadable) = sqlite_table_string_rows(&db, "blobs");
    assert!(unreadable);
    assert_eq!(rows.len(), 1);
    drop(db);
    let snapshot = read_cursor_store_db_snapshot(&path).unwrap();
    assert!(
        snapshot
            .records
            .iter()
            .any(|(id, _)| id == "cursor-unreadable-blob")
    );
}

#[tokio::test]
async fn cursor_store_discovery_does_not_scan_acp_or_unrelated_plans() {
    let home = TempDir::new().unwrap();
    let acp = home.path().join(".cursor/acp-sessions/synthetic-session");
    std::fs::create_dir_all(&acp).unwrap();
    seed_cursor_store_db(&acp.join("store.db"), "synthetic-session");
    let plans = home.path().join(".cursor/plans");
    std::fs::create_dir_all(&plans).unwrap();
    std::fs::write(plans.join("unrelated.plan.md"), "# Unrelated plan").unwrap();
    assert!(
        discover_store_db_sessions(home.path(), current_unix_epoch_for_tests(), 3600)
            .await
            .is_empty()
    );
}

/// Read back the [`ForkObservation`] an adapter embedded in a synthetic
/// metadata header. Mirrors what a consumer of the rendered content does.
fn embedded_fork_observation(content: &str) -> Option<ForkObservation> {
    let first = content.lines().next()?;
    let value: Value = serde_json::from_str(first).ok()?;
    serde_json::from_value(value.get(FORK_OBSERVATION_KEY)?.clone()).ok()
}

/// Stand-in for the duplicate detector an embedding application supplies:
/// accepts a parent whose conversation items are a structural prefix of the
/// child's, ignoring per-item ids. That copied prefix is the only evidence
/// Cursor's desktop composers leave behind.
fn prefix_duplicate_detector(parent_store: &str, child_store: &str) -> Option<ForkObservation> {
    fn structural(mut value: Value) -> Value {
        if let Value::Object(object) = &mut value {
            object.retain(|key, _| key != "bubbleId");
        }
        value
    }
    fn parts(store: &str) -> Option<(String, Vec<Value>)> {
        let value: Value = serde_json::from_str(store).ok()?;
        let id = value
            .pointer("/composer_header/composerId")?
            .as_str()?
            .to_string();
        let items = value
            .pointer("/composer_data/fullConversationHeadersOnly")?
            .as_array()?
            .iter()
            .cloned()
            .map(structural)
            .collect();
        Some((id, items))
    }
    let (parent_id, parent_items) = parts(parent_store)?;
    let (child_id, child_items) = parts(child_store)?;
    if parent_id == child_id
        || parent_items.is_empty()
        || child_items.len() <= parent_items.len()
        || !child_items.starts_with(&parent_items)
    {
        return None;
    }
    Some(ForkObservation {
        parent_agent: "cursor".to_string(),
        parent_agent_session_id: parent_id,
        fork_kind: "duplicate".to_string(),
        provider_fork_point_id: None,
        detection_source: "stable_id_prefix".to_string(),
        confidence: 100,
        inherited_item_count: parent_items.len().try_into().ok(),
        extractor_version: "test".to_string(),
    })
}
use super::app_config_dir_in;
use super::*;
use crate::discovery::scanner;
use tempfile::TempDir;

#[test]
fn cursor_user_data_override_keeps_default_and_deduplicates_canonical_roots() {
    let home = TempDir::new().unwrap();
    let explicit = TempDir::new().unwrap();
    let roots =
        cursor_user_data_roots_for_home(home.path(), Some(home.path()), Some(explicit.path()));
    assert_eq!(
        roots,
        vec![
            app_config_dir_in("Cursor", home.path()),
            explicit.path().to_path_buf()
        ]
    );

    let default = app_config_dir_in("Cursor", home.path());
    std::fs::create_dir_all(&default).unwrap();
    let alias = home.path().join("cursor-profile-alias");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&default, &alias).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&default, &alias).unwrap();
    let roots = cursor_user_data_roots_for_home(home.path(), Some(home.path()), Some(&alias));
    assert_eq!(roots, vec![default]);
}

#[test]
fn cursor_user_data_override_ignores_blank_relative_and_synthetic_home_values() {
    let home = TempDir::new().unwrap();
    for value in [
        Path::new(""),
        Path::new("   "),
        Path::new("relative/cursor"),
    ] {
        let roots = cursor_user_data_roots_for_home(home.path(), Some(home.path()), Some(value));
        assert_eq!(roots, vec![app_config_dir_in("Cursor", home.path())]);
    }

    let explicit = TempDir::new().unwrap();
    let other_home = TempDir::new().unwrap();
    let roots = cursor_user_data_roots_for_home(
        home.path(),
        Some(other_home.path()),
        Some(explicit.path()),
    );
    assert_eq!(roots, vec![app_config_dir_in("Cursor", home.path())]);
}

#[test]
fn cursor_owns_path_keeps_canonical_user_roots_without_profile_environment() {
    assert!(cursor_owns_path(
        "/users/a/library/application support/cursor/user/workspacestorage/workspace/state.vscdb",
        &[]
    ));
    assert!(cursor_owns_path(
        "/tmp/home/.cursor/projects/session.jsonl",
        &[]
    ));
}

#[tokio::test]
async fn cursor_user_data_override_discovers_and_classifies_native_desktop_state() {
    let home = TempDir::new().unwrap();
    let explicit = TempDir::new().unwrap();
    let global_dir = explicit.path().join("User/globalStorage");
    tokio::fs::create_dir_all(&global_dir).await.unwrap();
    seed_global_cursor_session(&global_dir.join("state.vscdb"), "override-composer");
    let workspace_dir = explicit
        .path()
        .join("User/workspaceStorage/workspace-override");
    write_workspace(
        &workspace_dir,
        if cfg!(windows) {
            "file:///C:/cursor-test-workspace"
        } else {
            "file:///tmp/cursor-test-workspace"
        },
        "override-composer",
        1_775_083_382_256i64,
    )
    .await;

    let roots =
        cursor_user_data_roots_for_home(home.path(), Some(home.path()), Some(explicit.path()));
    let logs = discover_recent_in_roots(home.path(), &roots, 1_775_083_400, 3600, None).await;
    assert_eq!(logs.len(), 1);
    let workspace_paths = collect_workspace_paths_in_roots(&roots).await;
    assert_eq!(workspace_paths.len(), 1);
    let surface_paths = cursor_surface_paths(home.path(), &roots);
    let watch_roots = cursor_watch_roots(home.path(), &roots);
    let owned = cursor_owns_path(
        &crate::discovery::normalize_for_matching(
            &explicit
                .path()
                .join("User/workspaceStorage/workspace-override/state.vscdb")
                .to_string_lossy(),
        ),
        &roots,
    );
    let default_ws = app_config_dir_in("Cursor", home.path()).join("User/workspaceStorage");
    let explicit_ws = explicit.path().join("User/workspaceStorage");
    let explicit_global = explicit.path().join("User/globalStorage");
    assert!(surface_paths.ide_desktop.contains(&default_ws));
    assert!(surface_paths.ide_desktop.contains(&explicit_ws));
    assert!(surface_paths.ide_desktop.contains(&explicit_global));
    assert!(watch_roots.iter().any(|root| root.path == explicit_ws));
    assert!(watch_roots.iter().any(|root| root.path == explicit_global));
    assert!(
        owned,
        "override paths should be owned by Cursor IDE discovery"
    );
}

#[tokio::test]
async fn cursor_copied_profiles_keep_distinct_refresh_identities_and_legacy_aliases() {
    let home = TempDir::new().unwrap();
    let explicit = TempDir::new().unwrap();
    let default = app_config_dir_in("Cursor", home.path());
    let roots = vec![default.clone(), explicit.path().to_path_buf()];

    for (index, root) in roots.iter().enumerate() {
        let global_dir = root.join("User/globalStorage");
        tokio::fs::create_dir_all(&global_dir).await.unwrap();
        let title = if index == 0 {
            "newer short title".to_owned()
        } else {
            "older but longer title ".repeat(32)
        };
        seed_global_cursor_session_with_name(
            &global_dir.join("state.vscdb"),
            "copied-profile-session",
            &title,
        );
        write_workspace(
            &root.join("User/workspaceStorage/workspace-copy"),
            "file:///tmp/copied-profile-workspace",
            "copied-profile-session",
            1_775_083_382_256 - (index as i64 * 10_000),
        )
        .await;
    }

    let candidates =
        discover_recent_candidates_in_roots(home.path(), &roots, 1_775_083_400, 3600, None).await;
    let labels = candidates
        .iter()
        .filter_map(|candidate| match &candidate.log.source {
            SessionSource::Inline { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(labels.len(), 2);
    assert!(
        labels
            .iter()
            .all(|label| label.starts_with("cursor-desktop:profile:"))
    );
    let explicit_label = cursor_desktop_source_label(
        &cursor_profile_id(explicit.path()),
        "workspace-copy",
        "copied-profile-session",
    );
    assert!(labels.contains(&explicit_label));
    assert_ne!(labels[0], labels[1]);
    assert!(
        labels
            .iter()
            .all(|label| legacy_cursor_source_label(label).as_deref()
                == Some("cursor-desktop:workspace-copy:copied-profile-session"))
    );

    let display = discover_recent_in_roots(home.path(), &roots, 1_775_083_400, 3600, None).await;
    assert_eq!(
        display.len(),
        1,
        "display discovery still deduplicates copies"
    );
    assert_eq!(
        display[0].source_label(),
        explicit_label,
        "the longer older copy still wins the display merge, but exact source lookup can retain both identities"
    );
}

#[tokio::test]
async fn cursor_unmerged_sources_retain_cli_identity_when_desktop_reuses_its_id() {
    let home = TempDir::new().unwrap();
    let default = app_config_dir_in("Cursor", home.path());
    let global_dir = default.join("User/globalStorage");
    tokio::fs::create_dir_all(&global_dir).await.unwrap();
    seed_global_cursor_session(&global_dir.join("state.vscdb"), "shared-session");
    write_workspace(
        &default.join("User/workspaceStorage/workspace-desktop"),
        "file:///tmp/desktop-workspace",
        "shared-session",
        1_775_083_382_256,
    )
    .await;

    let transcript = home.path().join(
        ".cursor/projects/workspace-cli/agent-transcripts/shared-session/shared-session.jsonl",
    );
    tokio::fs::create_dir_all(transcript.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &transcript,
        "{\"role\":\"user\",\"message\":\"CLI session\"}\n",
    )
    .await
    .unwrap();

    let roots = vec![default];
    let candidates =
        discover_recent_candidates_in_roots(home.path(), &roots, 1_775_083_400, 100_000_000, None)
            .await;
    assert!(candidates.iter().any(|candidate| matches!(
        &candidate.log.source,
        SessionSource::Inline { label, .. }
            if crate::discovery::normalize_for_matching(label)
                == crate::discovery::normalize_for_matching(&transcript.to_string_lossy())
    )));
    assert!(candidates.iter().any(|candidate| matches!(
        &candidate.log.source,
        SessionSource::Inline { label, .. }
            if label.starts_with("cursor-desktop:profile:")
                && legacy_cursor_source_label(label).as_deref()
                    == Some("cursor-desktop:workspace-desktop:shared-session")
    )));

    let merged =
        discover_recent_in_roots(home.path(), &roots, 1_775_083_400, 100_000_000, None).await;
    assert_eq!(merged.len(), 1);
    assert!(matches!(merged[0].source, SessionSource::Inline { .. }));
}

#[test]
fn cursor_default_profile_receives_a_scoped_identity() {
    let home = TempDir::new().unwrap();
    let default = app_config_dir_in("Cursor", home.path());
    let explicit = home.path().join("Cursor Profile");

    let default_id = cursor_profile_id(&default);
    let explicit_id = cursor_profile_id(&explicit);

    assert_eq!(default_id.len(), 16);
    assert_eq!(explicit_id.len(), 16);
    assert_ne!(default_id, explicit_id);
    assert_eq!(
        cursor_desktop_source_label(&default_id, "workspace", "same-session"),
        format!("cursor-desktop:profile:{default_id}:workspace:same-session")
    );
}

#[test]
fn cursor_user_data_override_matches_normalized_windows_paths() {
    let roots = vec![PathBuf::from(r"C:\Audit\Cursor Profile")];
    for (path, expected) in [
        (
            r"C:\Audit\Cursor Profile\User\globalStorage\state.vscdb",
            true,
        ),
        (
            r"C:\Audit\Cursor Profile\User\workspaceStorage\one\state.vscdb",
            true,
        ),
        (
            r"C:\Audit\Cursor Profile Other\User\globalStorage\state.vscdb",
            false,
        ),
        (r"C:\Audit\Cursor Profile\UserOther\state.vscdb", false),
    ] {
        assert_eq!(
            cursor_owns_path(&crate::discovery::normalize_for_matching(path), &roots),
            expected,
            "{path}"
        );
    }
}

#[test]
fn cursor_desktop_profile_environment_does_not_rewrite_cli_roots() {
    let home = TempDir::new().unwrap();
    let explicit = TempDir::new().unwrap();
    let roots =
        cursor_user_data_roots_for_home(home.path(), Some(home.path()), Some(explicit.path()));
    let paths = cursor_surface_paths(home.path(), &roots);
    assert_eq!(
        paths.cli,
        vec![
            home.path().join(".cursor/projects"),
            home.path().join(".cursor/chats")
        ]
    );
}

#[test]
fn cursor_desktop_priority_keeps_claimed_source_when_cli_reuses_id_at_newer_workspace() {
    let session_id = "shared-session";
    let desktop_label = "cursor-desktop:composer-hash:shared-session";
    let desktop = CursorDiscoveredSession {
        canonical_id: Some(session_id.to_string()),
        workspace_hint: Some("/workspace/desktop".to_string()),
        content_len: 50,
        priority: CursorSourcePriority::DesktopComposer,
        log: SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::Cursor,
            source: SessionSource::Inline {
                label: desktop_label.to_string(),
                content: format!(
                    "{}\n{{}}",
                    json!({"session_id": session_id, "cwd": "/workspace/desktop"})
                ),
                source_format: None,
            },
            updated_at: Some(100),
        },
    };
    let cli = CursorDiscoveredSession {
        canonical_id: Some(session_id.to_string()),
        workspace_hint: Some("/workspace/cli".to_string()),
        content_len: 500,
        priority: CursorSourcePriority::AgentTranscript,
        log: SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::Cursor,
            source: SessionSource::File(PathBuf::from(format!(
                "/home/tester/.cursor/projects/cli/agent-transcripts/{session_id}/{session_id}.jsonl"
            ))),
            updated_at: Some(200),
        },
    };
    let mut discovered = HashMap::new();
    merge_cursor_session(&mut discovered, cli);
    merge_cursor_session(&mut discovered, desktop);

    assert_eq!(discovered.len(), 1);
    let selected = discovered.into_values().next().unwrap().log;
    assert_eq!(selected.source_label(), desktop_label);
    let SessionSource::Inline { content, .. } = &selected.source else {
        panic!("Desktop composer must win over the newer CLI transcript");
    };
    assert!(content.contains("/workspace/desktop"));
    assert!(
        crate::discovery::Explorers::DISK
            .find_source_log_by_label(
                std::slice::from_ref(&selected),
                &AgentKind::Cursor,
                session_id,
                desktop_label,
                None,
            )
            .is_some(),
        "the source refresh candidate must satisfy the exact persisted label"
    );
}

#[test]
fn cursor_candidates_do_not_merge_metadata_across_workspaces() {
    let session_id = "shared-metadata-session";
    let make_inline = |label: &str, cwd: &str, priority, metadata: Value| {
        let content = format!("{metadata}\n{{}}",);
        CursorDiscoveredSession {
            canonical_id: Some(session_id.to_string()),
            workspace_hint: Some(cwd.to_string()),
            content_len: content.len(),
            priority,
            log: SessionLog {
                environment: Default::default(),
                agent_type: AgentKind::Cursor,
                source: SessionSource::Inline {
                    label: label.to_string(),
                    content,
                    source_format: None,
                },
                updated_at: Some(100),
            },
        }
    };
    let desktop = make_inline(
        "cursor-desktop:desktop:shared-metadata-session",
        "/workspace/desktop",
        CursorSourcePriority::DesktopComposer,
        json!({"session_id": session_id, "cwd": "/workspace/desktop"}),
    );
    let cli = make_inline(
        "cursor-store:cli:shared-metadata-session",
        "/workspace/cli",
        CursorSourcePriority::AgentTranscript,
        json!({
            "session_id": session_id,
            "cwd": "/workspace/cli",
            "title": "CLI title",
            "model": "cli-model",
            "createdAt": 50,
        }),
    );
    let mut discovered = HashMap::new();
    merge_cursor_session(&mut discovered, desktop);
    merge_cursor_session(&mut discovered, cli);

    assert_eq!(discovered.len(), 1);
    let selected = discovered.into_values().next().unwrap();
    assert_eq!(
        selected.log.source_label(),
        "cursor-desktop:desktop:shared-metadata-session"
    );
    let SessionSource::Inline { content, .. } = selected.log.source else {
        panic!("the desktop composer source must remain inline");
    };
    let metadata: Value = serde_json::from_str(content.lines().next().unwrap()).unwrap();
    assert_eq!(metadata["cwd"], "/workspace/desktop");
    assert!(metadata.get("title").is_none());
    assert!(metadata.get("model").is_none());
    assert!(metadata.get("createdAt").is_none());
}

#[test]
fn cursor_equal_candidates_use_a_stable_source_label_tiebreak() {
    let make_candidate = |label: &str| CursorDiscoveredSession {
        canonical_id: Some("stable-session".to_string()),
        workspace_hint: Some("/workspace/stable".to_string()),
        content_len: 10,
        priority: CursorSourcePriority::DesktopComposer,
        log: SessionLog {
            environment: Default::default(),
            agent_type: AgentKind::Cursor,
            source: SessionSource::Inline {
                label: label.to_string(),
                content: "{\"session_id\":\"stable-session\"}\n{}".to_string(),
                source_format: None,
            },
            updated_at: Some(100),
        },
    };
    let mut discovered = HashMap::new();
    merge_cursor_session(&mut discovered, make_candidate("cursor-z"));
    merge_cursor_session(&mut discovered, make_candidate("cursor-a"));

    assert_eq!(discovered.len(), 1);
    assert_eq!(
        discovered.into_values().next().unwrap().log.source_label(),
        "cursor-a"
    );
}

fn create_cursor_db(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("test Cursor database should open");
    conn.execute_batch(
        "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value BLOB);
         CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value BLOB);",
    )
    .expect("test Cursor database schema should initialize");
    conn
}

fn create_cursor_store_db(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("test Cursor store database should open");
    conn.execute_batch(
        "CREATE TABLE meta (key TEXT PRIMARY KEY, value BLOB);
         CREATE TABLE blobs (key TEXT PRIMARY KEY, value BLOB);",
    )
    .expect("test Cursor store database schema should initialize");
    conn
}

fn synthetic_cursor_blob_id(index: usize) -> String {
    format!("{index:064x}")
}

fn seed_cursor_store_db_with_blobs(
    chats_root: &Path,
    workspace: &str,
    session_id: &str,
    created_at: i64,
    blob_indexes: impl IntoIterator<Item = usize>,
) {
    let path = chats_root.join(workspace).join(session_id).join("store.db");
    std::fs::create_dir_all(
        path.parent()
            .expect("test Cursor store path should have a parent"),
    )
    .expect("test Cursor store directory should be created");
    let db = Connection::open(path).expect("test Cursor store should open");
    db.execute_batch(
        "CREATE TABLE meta (key TEXT, value TEXT);
         CREATE TABLE blobs (id TEXT PRIMARY KEY, data BLOB);",
    )
    .expect("test Cursor store schema should initialize");
    let blob_ids = blob_indexes
        .into_iter()
        .map(synthetic_cursor_blob_id)
        .collect::<Vec<_>>();
    let meta = json!({
        "agentId": session_id,
        "latestRootBlobId": blob_ids.last(),
        "name": format!("Synthetic {session_id}"),
        "createdAt": created_at,
        "mode": "agent",
        "lastUsedModel": "synthetic-model",
    })
    .to_string();
    let hex = meta
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    db.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)",
        params!["0", hex],
    )
    .expect("test Cursor store metadata should be inserted");
    for (index, blob_id) in blob_ids.iter().enumerate() {
        let data = if index == 0 {
            json!({ "role": "user", "content": "Synthetic prompt" }).to_string()
        } else {
            "{}".to_string()
        };
        db.execute(
            "INSERT INTO blobs (id, data) VALUES (?1, ?2)",
            params![blob_id, data.as_bytes()],
        )
        .expect("test Cursor store blob should be inserted");
    }
}

fn cursor_store_observation(session: &CursorDiscoveredSession) -> Option<ForkObservation> {
    let SessionSource::Inline { content, .. } = &session.log.source else {
        panic!("expected inline Cursor store session")
    };
    serde_json::from_str::<Value>(
        content
            .lines()
            .next()
            .expect("inline Cursor store session should contain metadata"),
    )
    .expect("inline Cursor store metadata should be valid JSON")
    .get(FORK_OBSERVATION_KEY)
    .cloned()
    .filter(|value| !value.is_null())
    .and_then(|value| serde_json::from_value(value).ok())
}

#[test]
fn cursor_store_db_reader_detects_the_19_of_27_fork_boundary() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/cursor-cli-store-fork.json")).unwrap();
    let parent_blob_ids = fixture["parent"]["blob_ids"].as_array().unwrap();
    let child_blob_ids = fixture["child"]["blob_ids"].as_array().unwrap();
    assert_eq!(parent_blob_ids.len(), 19);
    assert_eq!(child_blob_ids.len(), 27);
    assert!(parent_blob_ids.iter().all(|id| child_blob_ids.contains(id)));

    let temp = TempDir::new().unwrap();
    let chats_root = temp.path().join("chats");
    seed_cursor_store_db_with_blobs(
        &chats_root,
        "workspace-a",
        "parent-session",
        1_000,
        0..parent_blob_ids.len(),
    );
    seed_cursor_store_db_with_blobs(
        &chats_root,
        "workspace-a",
        "child-session",
        2_000,
        0..child_blob_ids.len(),
    );

    let sessions = query_cursor_store_db_sessions(&chats_root, i64::MIN);
    let child = sessions
        .iter()
        .find(|session| session.canonical_id.as_deref() == Some("child-session"))
        .unwrap();
    let observation = cursor_store_observation(child).expect("strict blob subset lineage");
    assert_eq!(observation.parent_agent, "cursor");
    assert_eq!(observation.parent_agent_session_id, "parent-session");
    assert_eq!(observation.fork_kind, "fork");
    assert_eq!(observation.detection_source, "stable_id_prefix");
    assert_eq!(observation.inherited_item_count, Some(19));
    assert_eq!(observation.confidence, 100);
    assert_eq!(observation.extractor_version, "cursor-store-db-v1");
}

#[test]
fn cursor_store_db_reader_rejects_independent_partial_overlap() {
    let temp = TempDir::new().unwrap();
    let chats_root = temp.path().join("chats");
    seed_cursor_store_db_with_blobs(&chats_root, "workspace-a", "independent-a", 1_000, 0..19);
    seed_cursor_store_db_with_blobs(&chats_root, "workspace-a", "independent-b", 2_000, 10..37);

    let sessions = query_cursor_store_db_sessions(&chats_root, i64::MIN);
    assert!(
        sessions
            .iter()
            .all(|session| cursor_store_observation(session).is_none())
    );
}

#[test]
fn cursor_store_db_reader_rejects_ambiguous_maximal_parents() {
    let temp = TempDir::new().unwrap();
    let chats_root = temp.path().join("chats");
    seed_cursor_store_db_with_blobs(&chats_root, "workspace-a", "parent-a", 1_000, 0..19);
    seed_cursor_store_db_with_blobs(
        &chats_root,
        "workspace-a",
        "parent-b",
        1_000,
        (0..18).chain(std::iter::once(19)),
    );
    seed_cursor_store_db_with_blobs(&chats_root, "workspace-a", "child-session", 2_000, 0..27);

    let sessions = query_cursor_store_db_sessions(&chats_root, i64::MIN);
    let child = sessions
        .iter()
        .find(|session| session.canonical_id.as_deref() == Some("child-session"))
        .unwrap();
    assert!(cursor_store_observation(child).is_none());
}

#[test]
fn cursor_store_db_reader_never_links_same_session_id_to_itself() {
    let temp = TempDir::new().unwrap();
    let chats_root = temp.path().join("chats");
    seed_cursor_store_db_with_blobs(&chats_root, "workspace-a", "resumed-session", 1_000, 0..19);
    seed_cursor_store_db_with_blobs(
        &chats_root,
        "workspace-a",
        "resumed-session-copy",
        2_000,
        0..27,
    );
    let copied_path = chats_root
        .join("workspace-a")
        .join("resumed-session-copy")
        .join("store.db");
    let db = Connection::open(copied_path).unwrap();
    let meta = json!({
        "agentId": "resumed-session",
        "createdAt": 2_000,
        "name": "Synthetic resume",
    })
    .to_string();
    let hex = meta
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    db.execute("UPDATE meta SET value = ?1 WHERE key = '0'", params![hex])
        .unwrap();

    let sessions = query_cursor_store_db_sessions(&chats_root, i64::MIN);
    assert!(
        sessions
            .iter()
            .all(|session| cursor_store_observation(session).is_none())
    );
}

#[test]
#[ignore = "requires a recent local Cursor Agent /fork"]
fn live_cursor_cli_fork_store_matches_reader_contract() {
    let home = crate::paths::home_dir().expect("home directory");
    let sessions = query_cursor_store_db_sessions(&home.join(".cursor/chats"), i64::MIN);
    let observation = sessions
        .iter()
        .find_map(cursor_store_observation)
        .expect("a fixture-proven Cursor CLI fork");
    assert_eq!(observation.parent_agent, "cursor");
    assert_eq!(observation.fork_kind, "fork");
    assert_eq!(observation.inherited_item_count, Some(19));
}

async fn write_workspace(path: &Path, folder: &str, composer_id: &str, updated_at: i64) {
    tokio::fs::create_dir_all(path)
        .await
        .expect("test Cursor workspace directory should be created");
    tokio::fs::write(
        path.join("workspace.json"),
        json!({ "folder": folder }).to_string(),
    )
    .await
    .expect("test Cursor workspace metadata should be written");
    let db = create_cursor_db(&path.join("state.vscdb"));
    db.execute(
        "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
        params![
            "composer.composerData",
            json!({
                "allComposers": [{
                    "composerId": composer_id,
                    "lastUpdatedAt": updated_at,
                    "createdAt": updated_at - 1000,
                }]
            })
            .to_string()
        ],
    )
    .expect("test Cursor workspace composer data should be inserted");
}

fn seed_global_cursor_session(path: &Path, composer_id: &str) {
    let db = create_cursor_db(path);
    db.execute(
        "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
        params![
            format!("composerData:{composer_id}"),
            json!({
                "composerId": composer_id,
                "createdAt": 1_775_083_355_225i64,
                "lastUpdatedAt": 1_775_083_382_256i64,
                "modelConfig": { "modelName": "composer-2" },
                "fullConversationHeadersOnly": [
                    { "bubbleId": "u1", "type": 1 },
                    { "bubbleId": "a1", "type": 2 }
                ]
            })
            .to_string()
        ],
    )
    .expect("test Cursor composer data should be inserted");
    db.execute(
        "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
        params![
            format!("bubbleId:{composer_id}:u1"),
            json!({
                "type": 1,
                "text": "Explain the architecture",
                "createdAt": "2026-04-01T22:42:55.658Z",
                "modelInfo": { "modelName": "composer-2" }
            })
            .to_string()
        ],
    )
    .expect("test Cursor user bubble should be inserted");
    db.execute(
        "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
        params![
            format!("bubbleId:{composer_id}:a1"),
            json!({
                "type": 2,
                "markdown": "Here is the architecture.",
                "createdAt": "2026-04-01T22:42:57.468Z"
            })
            .to_string()
        ],
    )
    .expect("test Cursor assistant bubble should be inserted");
}

#[test]
fn cursor_desktop_reader_embeds_only_verified_duplicate_lineage() {
    let temp = TempDir::new().unwrap();
    let ws_root = temp.path().join("workspaceStorage");
    let workspace = ws_root.join("workspace-1");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(
        workspace.join("workspace.json"),
        json!({ "folder": "/tmp/repo" }).to_string(),
    )
    .unwrap();
    let workspace_db = create_cursor_db(&workspace.join("state.vscdb"));
    workspace_db
        .execute(
            "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
            params![
                "composer.composerData",
                json!({
                    "allComposers": [
                        { "composerId": "parent", "lastUpdatedAt": 2_000_000, "isSubagent": 0 },
                        { "composerId": "child", "lastUpdatedAt": 3_000_000, "isSubagent": 0 }
                    ]
                })
                .to_string()
            ],
        )
        .unwrap();
    let global_path = temp.path().join("global.vscdb");
    let global = create_cursor_db(&global_path);
    for (id, headers) in [
        (
            "parent",
            json!([
                { "bubbleId": "parent-u", "type": 1, "grouping": { "hasText": true } },
                { "bubbleId": "parent-a", "type": 2, "grouping": { "hasText": true } }
            ]),
        ),
        (
            "child",
            json!([
                { "bubbleId": "child-u", "type": 1, "grouping": { "hasText": true } },
                { "bubbleId": "child-a", "type": 2, "grouping": { "hasText": true } },
                { "bubbleId": "child-u2", "type": 1, "grouping": { "hasText": true } }
            ]),
        ),
    ] {
        global
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                params![
                    format!("composerData:{id}"),
                    json!({
                        "composerId": id,
                        "fullConversationHeadersOnly": headers,
                        "isBestOfNSubcomposer": false
                    })
                    .to_string()
                ],
            )
            .unwrap();
    }
    for (id, bubble, kind, text) in [
        ("parent", "parent-u", 1, "question"),
        ("parent", "parent-a", 2, "answer"),
        ("child", "child-u", 1, "question"),
        ("child", "child-a", 2, "answer"),
        ("child", "child-u2", 1, "follow up"),
    ] {
        global
            .execute(
                "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
                params![
                    format!("bubbleId:{id}:{bubble}"),
                    json!({ "type": kind, "text": text }).to_string()
                ],
            )
            .unwrap();
    }
    drop(global);

    let sessions = query_cursor_desktop_sessions(
        &ws_root,
        &global_path,
        &cursor_profile_id(temp.path()),
        4_000,
        4_000,
        Some(prefix_duplicate_detector),
    );
    let child = sessions
        .iter()
        .find(|session| session.canonical_id.as_deref() == Some("child"))
        .unwrap();
    let SessionSource::Inline { content, .. } = &child.log.source else {
        panic!("expected inline Cursor session")
    };
    let observation = embedded_fork_observation(content).expect("duplicate lineage");
    assert_eq!(observation.parent_agent_session_id, "parent");
    assert_eq!(observation.inherited_item_count, Some(2));
}

fn seed_cursor_store_db(path: &Path, session_id: &str) {
    let db = create_cursor_store_db(path);
    db.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)",
        params![
            "sessionMeta",
            json!({
                "agentId": session_id,
                "workspacePath": "/Users/avery/dev/demo-cli",
                "lastUsedModel": "claude-sonnet-4",
                "createdAt": 1_775_083_355_225i64,
                "lastUpdatedAt": 1_775_083_382_256i64,
            })
            .to_string()
        ],
    )
    .expect("test Cursor store session metadata should be inserted");
    db.execute(
        "INSERT INTO blobs (key, value) VALUES (?1, ?2)",
        params![
            "message-1",
            json!({
                "messages": [
                    {"role": "user", "content": "Investigate the outage", "createdAt": 1_775_083_355_225i64},
                    {"role": "assistant", "content": "Here is the outage analysis.", "createdAt": 1_775_083_382_256i64}
                ]
            })
            .to_string()
        ],
    )
    .expect("test Cursor store messages should be inserted");
}

#[tokio::test]
async fn test_cursor_log_dirs_collects_chat_sessions_and_agent_transcripts() {
    let home = TempDir::new().unwrap();

    let ws_root = app_config_dir_in("Cursor", home.path())
        .join("User")
        .join("workspaceStorage")
        .join("abc")
        .join("chatSessions");
    tokio::fs::create_dir_all(&ws_root).await.unwrap();

    let transcripts_dir = home
        .path()
        .join(".cursor")
        .join("projects")
        .join("Users-avery-dev-demo-cli")
        .join("agent-transcripts")
        .join("conversation");
    tokio::fs::create_dir_all(&transcripts_dir).await.unwrap();
    tokio::fs::write(transcripts_dir.join("conversation.jsonl"), "{}\n")
        .await
        .unwrap();

    let dirs = log_dirs_in(home.path()).await;

    assert!(dirs.contains(&ws_root));
    assert!(dirs.iter().any(|dir| dir.ends_with("agent-transcripts")));
}

#[tokio::test]
async fn test_cursor_log_dirs_ignore_non_session_project_dirs() {
    let home = TempDir::new().unwrap();
    let agent_tools_dir = home
        .path()
        .join(".cursor")
        .join("projects")
        .join("p1")
        .join("agent-tools");
    tokio::fs::create_dir_all(&agent_tools_dir).await.unwrap();
    tokio::fs::write(agent_tools_dir.join("tool.txt"), "content")
        .await
        .unwrap();

    let terminals_dir = home
        .path()
        .join(".cursor")
        .join("projects")
        .join("p1")
        .join("terminals");
    tokio::fs::create_dir_all(&terminals_dir).await.unwrap();
    tokio::fs::write(terminals_dir.join("1.txt"), "content")
        .await
        .unwrap();

    let dirs = log_dirs_in(home.path()).await;
    assert!(dirs.is_empty());
}

// F2: the walk never enters a directory past the documented depth, so a
// project's other subdirectories (`canvases`, and anything else it
// accumulates) are never read.
#[tokio::test]
async fn test_collect_agent_transcript_dirs_ignores_agent_transcripts_one_level_too_deep() {
    let home = TempDir::new().unwrap();
    let projects_dir = home.path().join(".cursor").join("projects");

    let too_deep = projects_dir
        .join("some-project")
        .join("canvases")
        .join("agent-transcripts");
    tokio::fs::create_dir_all(&too_deep).await.unwrap();

    let dirs = collect_agent_transcript_dirs(&projects_dir).await;

    assert!(!dirs.contains(&too_deep));
}

// Regression guard: an mtime gate on the project directory is wrong here
// (see `MAX_TRANSCRIPT_WALK_DEPTH`'s doc comment) because a new session
// file never touches it — only `agent-transcripts/` itself. A project
// last used outside any recency window must still surface a session
// added to it since.
#[tokio::test]
async fn test_collect_agent_transcript_dirs_finds_transcripts_under_an_old_project_dir() {
    let home = TempDir::new().unwrap();
    let projects_dir = home.path().join(".cursor").join("projects");

    let project_dir = projects_dir.join("old-project");
    let transcripts = project_dir.join("agent-transcripts");
    tokio::fs::create_dir_all(&transcripts).await.unwrap();
    // 30 days old — well outside any recency window this agent uses.
    crate::discovery::set_file_mtime(&project_dir, 1_700_000_000 - 30 * 86_400);

    let dirs = collect_agent_transcript_dirs(&projects_dir).await;

    assert!(dirs.contains(&transcripts));
}

#[tokio::test]
async fn test_cursor_discovers_agent_transcript_and_recovers_metadata() {
    let home = TempDir::new().unwrap();
    let repo_root = home
        .path()
        .join("Users")
        .join("zack")
        .join("dev")
        .join("demo-cli");
    tokio::fs::create_dir_all(&repo_root).await.unwrap();

    let transcript = home
        .path()
        .join(".cursor")
        .join("projects")
        .join(encode_cursor_project_workspace_key(&repo_root))
        .join("agent-transcripts")
        .join("abc")
        .join("abc.jsonl");
    tokio::fs::create_dir_all(transcript.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &transcript,
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
    )
    .await
    .unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let logs = discover_recent_in(home.path(), now, 3600, None).await;
    assert_eq!(logs.len(), 1);

    let content = match &logs[0].source {
        SessionSource::Inline { content, .. } => content,
        SessionSource::File(_) => panic!("expected inline transcript"),
        SessionSource::ProviderDb { .. } => panic!("expected inline transcript"),
    };
    let metadata = scanner::parse_session_metadata_str(content);
    assert_eq!(metadata.session_id.as_deref(), Some("abc"));
    assert_eq!(
        metadata.cwd.as_deref(),
        Some(repo_root.to_string_lossy().as_ref())
    );
}

#[test]
fn cursor_agent_subagent_path_proves_parent_session() {
    let child = Path::new(
        "/home/.cursor/projects/workspace/agent-transcripts/parent-123/subagents/child-456.jsonl",
    );
    assert_eq!(
        cursor_agent_subagent_parent(child).as_deref(),
        Some("parent-123")
    );

    let top_level =
        Path::new("/home/.cursor/projects/workspace/agent-transcripts/parent-123/parent-123.jsonl");
    assert!(cursor_agent_subagent_parent(top_level).is_none());

    let unsafe_parent =
        Path::new("/home/.cursor/projects/workspace/agent-transcripts/../subagents/child.jsonl");
    assert!(cursor_agent_subagent_parent(unsafe_parent).is_none());

    let nested = Path::new(
        "/home/.cursor/projects/workspace/agent-transcripts/parent-123/artifacts/subagents/child.jsonl",
    );
    assert!(cursor_agent_subagent_parent(nested).is_none());
}

#[tokio::test]
async fn cursor_agent_discovery_embeds_subagent_parent_observation() {
    let home = TempDir::new().unwrap();
    let transcripts = home
        .path()
        .join(".cursor/projects/workspace/agent-transcripts");
    let parent_id = "parent-123";
    let child_id = "child-456";
    let parent = transcripts
        .join(parent_id)
        .join(format!("{parent_id}.jsonl"));
    let child = transcripts
        .join(parent_id)
        .join("subagents")
        .join(format!("{child_id}.jsonl"));
    tokio::fs::create_dir_all(parent.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::create_dir_all(child.parent().unwrap())
        .await
        .unwrap();
    let body =
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n";
    tokio::fs::write(&parent, body).await.unwrap();
    tokio::fs::write(&child, body).await.unwrap();

    let logs = discover_recent_in(home.path(), current_unix_epoch_for_tests(), 3600, None).await;
    let child_content = logs
        .iter()
        .find_map(|log| {
            let SessionSource::Inline { content, .. } = &log.source else {
                return None;
            };
            (scanner::parse_session_metadata_str(content)
                .session_id
                .as_deref()
                == Some(child_id))
            .then_some(content.as_str())
        })
        .expect("child transcript should be discovered");
    let observation = embedded_fork_observation(child_content).expect("parent observation");
    assert_eq!(observation.parent_agent_session_id, parent_id);
    assert_eq!(observation.fork_kind, "subagent");
    assert_eq!(observation.detection_source, "subagent_path");
    assert_eq!(observation.confidence, 100);
}

#[tokio::test]
async fn test_cursor_discovers_desktop_state_vscdb_sessions() {
    let home = TempDir::new().unwrap();
    let global_dir = app_config_dir_in("Cursor", home.path())
        .join("User")
        .join("globalStorage");
    tokio::fs::create_dir_all(&global_dir).await.unwrap();
    seed_global_cursor_session(&global_dir.join("state.vscdb"), "composer-1");

    let workspace_dir = app_config_dir_in("Cursor", home.path())
        .join("User")
        .join("workspaceStorage")
        .join("workspace-1");
    write_workspace(
        &workspace_dir,
        if cfg!(windows) {
            "file:///C:/cursor-test-workspace"
        } else {
            "file:///tmp/cursor-test-workspace"
        },
        "composer-1",
        1_775_083_382_256i64,
    )
    .await;

    let logs = discover_recent_in(home.path(), 1_775_083_400, 3600, None).await;
    assert_eq!(logs.len(), 1);
    let content = match &logs[0].source {
        SessionSource::Inline { content, .. } => content,
        SessionSource::File(_) => panic!("expected inline desktop log"),
        SessionSource::ProviderDb { .. } => panic!("expected inline desktop log"),
    };
    let metadata = scanner::parse_session_metadata_str(content);
    assert_eq!(metadata.session_id.as_deref(), Some("composer-1"));
    assert_eq!(
        metadata.cwd.as_deref(),
        Some(if cfg!(windows) {
            "C:/cursor-test-workspace"
        } else {
            "/tmp/cursor-test-workspace"
        })
    );
    assert!(content.contains("Explain the architecture"));
    assert!(content.contains("Here is the architecture."));
}

/// Helper for the migrated-blob test. Mirrors `write_workspace` but seeds
/// `composer.composerData` in the post-migration shape
/// (`selectedComposerIds[]` + `hasMigratedComposerData: true`), which is
/// what Cursor writes today.
async fn write_workspace_migrated(path: &Path, folder: &str, composer_ids: &[&str]) {
    tokio::fs::create_dir_all(path).await.unwrap();
    tokio::fs::write(
        path.join("workspace.json"),
        json!({ "folder": folder }).to_string(),
    )
    .await
    .expect("migrated test Cursor workspace metadata should be written");
    let db = create_cursor_db(&path.join("state.vscdb"));
    db.execute(
        "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
        params![
            "composer.composerData",
            json!({
                "selectedComposerIds": composer_ids,
                "lastFocusedComposerIds": composer_ids,
                "hasMigratedComposerData": true,
                "hasMigratedMultipleComposers": true,
            })
            .to_string()
        ],
    )
    .expect("migrated test Cursor composer data should be inserted");
}

/// Seed a composer with an explicit `name` field (Cursor's title slot,
/// used by both AI-set initial names and user renames).
fn seed_global_cursor_session_with_name(path: &Path, composer_id: &str, name: &str) {
    let db = Connection::open(path).unwrap();
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS ItemTable (key TEXT PRIMARY KEY, value BLOB);
         CREATE TABLE IF NOT EXISTS cursorDiskKV (key TEXT PRIMARY KEY, value BLOB);",
    )
    .expect("named test Cursor database schema should initialize");
    db.execute(
        "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
        params![
            format!("composerData:{composer_id}"),
            json!({
                "composerId": composer_id,
                "name": name,
                "createdAt": 1_775_083_355_225i64,
                "lastUpdatedAt": 1_775_083_382_256i64,
                "fullConversationHeadersOnly": [
                    { "bubbleId": "u1", "type": 1 }
                ]
            })
            .to_string()
        ],
    )
    .expect("named test Cursor composer data should be inserted");
    db.execute(
        "INSERT INTO cursorDiskKV (key, value) VALUES (?1, ?2)",
        params![
            format!("bubbleId:{composer_id}:u1"),
            json!({
                "type": 1,
                "text": "any prompt",
                "createdAt": "2026-04-01T22:42:55.658Z",
            })
            .to_string()
        ],
    )
    .expect("named test Cursor user bubble should be inserted");
}

#[tokio::test]
async fn test_cursor_desktop_session_uses_composer_name_as_title() {
    // Composer's `name` field is Cursor IDE's title slot. AI-generated
    // initial names and user-renamed titles both land here. Assert the
    // scanner picks it up via Tier 2 (ExplicitField) so it beats the
    // first-message fallback, and the provenance is `Explicit`.
    use crate::discovery::scanner::TitleSource;

    let home = TempDir::new().unwrap();
    let global_dir = app_config_dir_in("Cursor", home.path())
        .join("User")
        .join("globalStorage");
    tokio::fs::create_dir_all(&global_dir).await.unwrap();
    seed_global_cursor_session_with_name(
        &global_dir.join("state.vscdb"),
        "composer-1",
        "(RENAME) Hello world test from Cursor IDE",
    );

    let workspace_dir = app_config_dir_in("Cursor", home.path())
        .join("User")
        .join("workspaceStorage")
        .join("workspace-1");
    write_workspace_migrated(
        &workspace_dir,
        if cfg!(windows) {
            "file:///C:/cursor-test-workspace"
        } else {
            "file:///tmp/cursor-test-workspace"
        },
        &["composer-1"],
    )
    .await;

    let logs = discover_recent_in(home.path(), 1_775_083_400, 3600, None).await;
    assert_eq!(logs.len(), 1);
    let content = match &logs[0].source {
        SessionSource::Inline { content, .. } => content,
        SessionSource::File(_) => panic!("expected inline desktop log"),
        SessionSource::ProviderDb { .. } => panic!("expected inline desktop log"),
    };
    let metadata = scanner::parse_session_metadata_str(content);
    assert_eq!(
        metadata.title.as_deref(),
        Some("(RENAME) Hello world test from Cursor IDE")
    );
    assert_eq!(metadata.title_source, Some(TitleSource::Explicit));
}

#[tokio::test]
async fn test_cursor_discovers_desktop_sessions_with_migrated_workspace_blob() {
    // Real on-disk shape from a current Cursor install: the workspace
    // `composer.composerData` blob only carries id lists; the per-composer
    // `lastUpdatedAt` lives in the global `cursorDiskKV.composerData:<id>`
    // row. Verifies the fallback path through
    // `fetch_composer_last_updated_at_ms`.
    let home = TempDir::new().unwrap();
    let global_dir = app_config_dir_in("Cursor", home.path())
        .join("User")
        .join("globalStorage");
    tokio::fs::create_dir_all(&global_dir).await.unwrap();
    seed_global_cursor_session(&global_dir.join("state.vscdb"), "composer-1");

    let workspace_dir = app_config_dir_in("Cursor", home.path())
        .join("User")
        .join("workspaceStorage")
        .join("workspace-1");
    write_workspace_migrated(
        &workspace_dir,
        if cfg!(windows) {
            "file:///C:/cursor-test-workspace"
        } else {
            "file:///tmp/cursor-test-workspace"
        },
        &["composer-1"],
    )
    .await;

    let logs = discover_recent_in(home.path(), 1_775_083_400, 3600, None).await;
    assert_eq!(
        logs.len(),
        1,
        "expected one Cursor IDE session from migrated workspace blob"
    );
    let content = match &logs[0].source {
        SessionSource::Inline { content, .. } => content,
        SessionSource::File(_) => panic!("expected inline desktop log"),
        SessionSource::ProviderDb { .. } => panic!("expected inline desktop log"),
    };
    let metadata = scanner::parse_session_metadata_str(content);
    assert_eq!(metadata.session_id.as_deref(), Some("composer-1"));
    assert!(content.contains("Explain the architecture"));
}

#[test]
fn test_cursor_dedupe_preserves_bubble_order() {
    let mut ids = vec![
        "bubble-2".to_string(),
        "bubble-10".to_string(),
        "bubble-2".to_string(),
        "bubble-1".to_string(),
    ];

    dedupe_preserving_order(&mut ids);

    assert_eq!(ids, vec!["bubble-2", "bubble-10", "bubble-1"]);
}

#[test]
fn cursor_store_db_metadata_proves_subagent_parent() {
    let metadata = parse_cursor_store_db_metadata(&[json!({
        "agentId": "child-456",
        "subagentInfo": {"parentAgentId": " parent-123 "},
    })
    .to_string()])
    .expect("session metadata");
    let observation = cursor_store_db_subagent_observation(&metadata).expect("parent link");

    assert_eq!(observation.parent_agent_session_id, "parent-123");
    assert_eq!(observation.fork_kind, "subagent");
    assert_eq!(observation.detection_source, "store_db_subagent_info");
    assert_eq!(observation.confidence, 100);

    let invalid = parse_cursor_store_db_metadata(&[json!({
        "agentId": "child-456",
        "subagentInfo": {"parentAgentId": "../parent-123"},
    })
    .to_string()])
    .expect("session metadata");
    assert!(cursor_store_db_subagent_observation(&invalid).is_none());
}

#[tokio::test]
async fn test_cursor_discovers_store_db_sessions() {
    let home = TempDir::new().unwrap();
    let store_dir = home
        .path()
        .join(".cursor")
        .join("chats")
        .join("a")
        .join("b");
    tokio::fs::create_dir_all(&store_dir).await.unwrap();
    seed_cursor_store_db(&store_dir.join("store.db"), "store-session-1");

    let logs = discover_store_db_sessions(home.path(), current_unix_epoch_for_tests(), 3600).await;
    assert_eq!(logs.len(), 1);
    let content = match &logs[0].log.source {
        SessionSource::Inline { content, .. } => content,
        SessionSource::File(_) => panic!("expected inline store db session"),
        SessionSource::ProviderDb { .. } => panic!("expected inline store db session"),
    };
    let metadata = scanner::parse_session_metadata_str(content);
    assert_eq!(metadata.session_id.as_deref(), Some("store-session-1"));
    assert_eq!(metadata.cwd.as_deref(), Some("/Users/avery/dev/demo-cli"));
    assert!(content.contains("Investigate the outage"));
    assert!(content.contains("Here is the outage analysis."));
}

/// Seed a `store.db` with a `meta` blob carrying a `name` field but no
/// workspace path — matches the actual on-disk shape of
/// `~/.cursor/chats/<ws>/<agent>/store.db` written by `cursor-agent`
/// (workspace path is recoverable only via the agent-transcripts side).
fn seed_cursor_store_db_named_no_cwd(path: &Path, session_id: &str, name: &str) {
    let db = create_cursor_store_db(path);
    db.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)",
        params![
            "sessionMeta",
            json!({
                "agentId": session_id,
                "name": name,
                "lastUsedModel": "composer-2.5",
                "createdAt": current_unix_epoch_for_tests() * 1000 - 60_000,
                "lastUpdatedAt": current_unix_epoch_for_tests() * 1000,
            })
            .to_string()
        ],
    )
    .expect("named test Cursor store metadata should be inserted");
    db.execute(
        "INSERT INTO blobs (key, value) VALUES (?1, ?2)",
        params![
            "message-1",
            json!({
                "messages": [
                    {"role": "user", "content": "doesn't matter — store.db `name` should win", "createdAt": 1i64},
                ]
            })
            .to_string()
        ],
    )
    .expect("named test Cursor store message should be inserted");
}

#[test]
fn test_decode_hex_jsonish_decodes_real_meta_blob_shape() {
    // Mirrors the exact bytes Cursor writes into
    // `~/.cursor/chats/*/store.db#meta.value`: literal ASCII hex of a
    // JSON document, stored in a TEXT column.
    let json = r#"{"agentId":"x","name":"renamed"}"#;
    let hex: String = json.bytes().map(|b| format!("{:02x}", b)).collect();
    let decoded = decode_hex_jsonish(&hex).expect("hex-encoded json must decode");
    assert_eq!(decoded, json);
}

#[test]
fn test_decode_hex_jsonish_rejects_plain_json_and_garbage() {
    assert!(decode_hex_jsonish("{\"already\":\"json\"}").is_none());
    assert!(decode_hex_jsonish("not-hex").is_none());
    // First byte decodes to 'x' (`78`), not `{` / `[`.
    assert!(decode_hex_jsonish("78797a").is_none());
    // Odd length.
    assert!(decode_hex_jsonish("7b22").is_some()); // even
    assert!(decode_hex_jsonish("7b223").is_none()); // odd
}

/// Seed a store.db whose `meta.value` is hex-encoded JSON, matching the
/// real on-disk shape Cursor writes (see `decode_hex_jsonish`).
fn seed_cursor_store_db_hex_meta(path: &Path, session_id: &str, name: &str) {
    let db = create_cursor_store_db(path);
    let meta_json = json!({
        "agentId": session_id,
        "name": name,
        "createdAt": current_unix_epoch_for_tests() * 1000 - 60_000,
        "lastUpdatedAt": current_unix_epoch_for_tests() * 1000,
    })
    .to_string();
    let hex: String = meta_json.bytes().map(|b| format!("{:02x}", b)).collect();
    db.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)",
        params!["sessionMeta", hex],
    )
    .unwrap();
    db.execute(
        "INSERT INTO blobs (key, value) VALUES (?1, ?2)",
        params![
            "msg-1",
            json!({"messages":[{"role":"user","content":"first prompt","createdAt":1i64}]})
                .to_string()
        ],
    )
    .unwrap();
}

#[tokio::test]
async fn test_cursor_cli_store_db_with_hex_encoded_meta_reads_rename_name() {
    // Production Cursor stores `meta.value` as a TEXT column whose content
    // is hex-encoded JSON. Verifies the reader's hex-decode fallback
    // surfaces the `name` field (the `/rename` slot) so it lands as the
    // tray title.
    use crate::discovery::scanner::TitleSource;

    let home = TempDir::new().unwrap();
    let session_id = "abc12345-6789-0000-aaaa-cccccccccccc";
    let store_dir = home
        .path()
        .join(".cursor")
        .join("chats")
        .join("workspacehash")
        .join(session_id);
    tokio::fs::create_dir_all(&store_dir).await.unwrap();
    seed_cursor_store_db_hex_meta(
        &store_dir.join("store.db"),
        session_id,
        "(RENAME) hello from real-shape store.db",
    );

    let logs = discover_recent_in(home.path(), current_unix_epoch_for_tests(), 3600, None).await;
    assert_eq!(logs.len(), 1);
    let content = match &logs[0].source {
        SessionSource::Inline { content, .. } => content,
        SessionSource::File(_) => panic!("expected inline log"),
        SessionSource::ProviderDb { .. } => panic!("expected inline log"),
    };
    let metadata = scanner::parse_session_metadata_str(content);
    assert_eq!(
        metadata.title.as_deref(),
        Some("(RENAME) hello from real-shape store.db"),
    );
    assert_eq!(metadata.title_source, Some(TitleSource::Explicit));
}

#[test]
fn cursor_cli_store_metadata_ignores_nested_blob_names() {
    let temp = TempDir::new().unwrap();
    let db = create_cursor_store_db(&temp.path().join("store.db"));
    db.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)",
        params!["session", json!({ "agentId": "session-1" }).to_string()],
    )
    .unwrap();
    db.execute(
        "INSERT INTO blobs (key, value) VALUES (?1, ?2)",
        params![
            "tool-result",
            json!({
                "name": "Nested tool blob must not become title",
                "role": "user",
                "content": "Actual first prompt"
            })
            .to_string()
        ],
    )
    .unwrap();
    drop(db);

    let candidate = build_cursor_store_db_session(&temp.path().join("store.db")).unwrap();
    let SessionSource::Inline { content, .. } = candidate.log.source else {
        panic!("expected inline store session")
    };
    let metadata = scanner::parse_session_metadata_str(&content);
    assert_eq!(metadata.title.as_deref(), Some("Actual first prompt"));
}

#[tokio::test]
async fn test_cursor_cli_session_uses_store_db_name_and_backfills_cwd() {
    // Real on-disk shape: `cursor-agent` writes its session metadata to
    // `~/.cursor/chats/<ws-hash>/<agent_id>/store.db` and its transcript
    // to `~/.cursor/projects/<ws>/agent-transcripts/<id>/<id>.jsonl`.
    // The two stores share the agent UUID; only the agent-transcripts
    // side knows the workspace path. Asserts:
    // 1. `meta.value.name` becomes the title.
    // 2. cwd is backfilled from the agent-transcripts project dir, so
    //    the StoreDb + AgentTranscript candidates merge to a single row.
    let home = TempDir::new().unwrap();
    let repo_root = home
        .path()
        .join("Users")
        .join("zack")
        .join("dev")
        .join("demo-cli");
    tokio::fs::create_dir_all(&repo_root).await.unwrap();
    let session_id = "abc12345-6789-0000-aaaa-bbbbbbbbbbbb";

    // Agent-transcripts side: gives cwd via the workspace-key dir name.
    let transcript = home
        .path()
        .join(".cursor")
        .join("projects")
        .join(encode_cursor_project_workspace_key(&repo_root))
        .join("agent-transcripts")
        .join(session_id)
        .join(format!("{session_id}.jsonl"));
    tokio::fs::create_dir_all(transcript.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &transcript,
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"<user_query>fallback prompt</user_query>\"}]}}\n",
    )
    .await
    .unwrap();

    // store.db side: same UUID, has the title, has no cwd.
    let store_dir = home
        .path()
        .join(".cursor")
        .join("chats")
        .join("workspacehash")
        .join(session_id);
    tokio::fs::create_dir_all(&store_dir).await.unwrap();
    seed_cursor_store_db_named_no_cwd(
        &store_dir.join("store.db"),
        session_id,
        "(RENAME) hello world from Cursor CLI",
    );

    let logs = discover_recent_in(home.path(), current_unix_epoch_for_tests(), 3600, None).await;
    assert_eq!(
        logs.len(),
        1,
        "expected the two candidates (transcript + store.db) to merge"
    );
    let content = match &logs[0].source {
        SessionSource::Inline { content, .. } => content,
        SessionSource::File(_) => panic!("expected inline log"),
        SessionSource::ProviderDb { .. } => panic!("expected inline log"),
    };
    let metadata = scanner::parse_session_metadata_str(content);
    assert_eq!(metadata.session_id.as_deref(), Some(session_id));
    assert_eq!(
        metadata.title.as_deref(),
        Some("(RENAME) hello world from Cursor CLI"),
        "title must come from store.db `name`, not the transcript first-message"
    );
    assert_eq!(
        metadata.cwd.as_deref(),
        Some(repo_root.to_string_lossy().as_ref()),
        "cwd must be backfilled from the agent-transcripts side"
    );
}

#[tokio::test]
async fn agent_fixture_keeps_distinct_sessions_and_detects_marked_fork() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/cursor-cli-agent-fork.json")).unwrap();
    let home = TempDir::new().unwrap();
    let repo_root = home.path().join("home/avery/projects/demo-app");
    tokio::fs::create_dir_all(&repo_root).await.unwrap();
    let project_dir = home
        .path()
        .join(".cursor/projects")
        .join(encode_cursor_project_workspace_key(&repo_root));

    for session in fixture["sessions"].as_array().unwrap() {
        let session_id = session["id"].as_str().unwrap();
        let transcript_path = project_dir
            .join("agent-transcripts")
            .join(session_id)
            .join(format!("{session_id}.jsonl"));
        tokio::fs::create_dir_all(transcript_path.parent().unwrap())
            .await
            .unwrap();
        let transcript = session["transcript"]
            .as_array()
            .unwrap()
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(&transcript_path, format!("{transcript}\n"))
            .await
            .unwrap();

        let chat_dir = home
            .path()
            .join(".cursor/chats/fixture-workspace")
            .join(session_id);
        tokio::fs::create_dir_all(&chat_dir).await.unwrap();
        tokio::fs::write(chat_dir.join("meta.json"), session["meta"].to_string())
            .await
            .unwrap();
        seed_cursor_store_db_named_no_cwd(
            &chat_dir.join("store.db"),
            session_id,
            session["meta"]["title"].as_str().unwrap(),
        );
    }

    let logs = discover_recent_in(home.path(), current_unix_epoch_for_tests(), 3600, None).await;
    assert_eq!(logs.len(), 3, "all three Cursor Agent IDs stay distinct");
    let content_by_id = logs
        .iter()
        .filter_map(|log| {
            let SessionSource::Inline { content, .. } = &log.source else {
                return None;
            };
            let id = scanner::parse_session_metadata_str(content).session_id?;
            Some((id, content))
        })
        .collect::<HashMap<_, _>>();
    let child_id = "00000000-0000-4000-8000-000000000102";
    assert_eq!(
        embedded_fork_observation(content_by_id[child_id])
            .expect("fork observation")
            .parent_agent_session_id,
        "00000000-0000-4000-8000-000000000101"
    );
    let unrelated_id = "00000000-0000-4000-8000-000000000103";
    assert!(
        embedded_fork_observation(content_by_id[unrelated_id]).is_none(),
        "a fork title without an exact copied prefix must fail closed"
    );
}

#[test]
fn title_fork_annotation_preserves_an_existing_observation() {
    let existing = ForkObservation {
        parent_agent: "cursor".to_owned(),
        parent_agent_session_id: "declared-parent".to_owned(),
        fork_kind: "subagent".to_owned(),
        provider_fork_point_id: None,
        detection_source: "store_db_subagent_info".to_owned(),
        confidence: 100,
        inherited_item_count: None,
        extractor_version: "cursor-store-db-v1".to_owned(),
    };
    let replacement = ForkObservation {
        parent_agent_session_id: "title-parent".to_owned(),
        ..existing.clone()
    };
    let mut content = format!("{}\n{{}}", json!({FORK_OBSERVATION_KEY: existing}));

    set_embedded_fork_observation(&mut content, &replacement);

    assert_eq!(embedded_fork_observation(&content), Some(existing));
}

#[tokio::test]
async fn test_cursor_dedupes_duplicate_transcript_and_desktop_session_ids() {
    let home = TempDir::new().unwrap();
    let repo_root = PathBuf::from("/Users/avery/dev/demo-cli");

    let global_dir = app_config_dir_in("Cursor", home.path())
        .join("User")
        .join("globalStorage");
    tokio::fs::create_dir_all(&global_dir).await.unwrap();
    seed_global_cursor_session(&global_dir.join("state.vscdb"), "shared-session");

    let workspace_dir = app_config_dir_in("Cursor", home.path())
        .join("User")
        .join("workspaceStorage")
        .join("workspace-1");
    write_workspace(
        &workspace_dir,
        &format!("file://{}", repo_root.display()),
        "shared-session",
        1_775_083_382_256i64,
    )
    .await;

    let encoded = encode_cursor_project_workspace_key(&repo_root);
    let transcript = home
        .path()
        .join(".cursor")
        .join("projects")
        .join(encoded)
        .join("agent-transcripts")
        .join("shared-session")
        .join("shared-session.jsonl");
    tokio::fs::create_dir_all(transcript.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(
        &transcript,
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"short transcript\"}]}}\n",
    )
    .await
    .unwrap();

    let logs = discover_recent_in(home.path(), 1_775_083_400, 3600, None).await;
    assert_eq!(logs.len(), 1);
    let content = match &logs[0].source {
        SessionSource::Inline { content, .. } => content,
        SessionSource::File(_) => panic!("expected inline session"),
        SessionSource::ProviderDb { .. } => panic!("expected inline session"),
    };
    assert!(content.contains("Explain the architecture"));
    assert!(!content.contains("short transcript"));
}

fn current_unix_epoch_for_tests() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn test_encode_cursor_project_workspace_key_windows_drive_format() {
    let encoded = encode_cursor_project_workspace_key(Path::new(r"C:\Users\avery\dev\demo-cli"));

    if cfg!(windows) {
        assert_eq!(encoded, "C--Users-avery-dev-demo-cli");
    }
}

use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::fs;

fn chunked_source() -> (tempfile::TempDir, IdeSource, ProjectSelection, Connection) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let project = root.join("project");
    let user = root.join("Cursor/User");
    let workspace = user.join("workspaceStorage/synthetic-workspace");
    let global_root = user.join("globalStorage");
    for directory in [&project, &workspace, &global_root] {
        fs::create_dir_all(directory).unwrap();
    }
    fs::write(
        workspace.join("workspace.json"),
        serde_json::json!({ "folder": format!("file://{}", project.display()) }).to_string(),
    )
    .unwrap();
    Connection::open(workspace.join("state.vscdb"))
        .unwrap()
        .execute_batch("CREATE TABLE ItemTable(key TEXT PRIMARY KEY, value BLOB);")
        .unwrap();
    let writer = Connection::open(global_root.join("state.vscdb")).unwrap();
    writer
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE cursorDiskKV(key TEXT PRIMARY KEY, value BLOB);",
        )
        .unwrap();
    writer
        .execute_batch(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/cloud/cursor/ide-chunked.sql"
        )))
        .unwrap();
    let source = reopen(&workspace, "synthetic-chunked").unwrap();
    let selection = ProjectSelection::new(vec![project]).unwrap();
    (temporary, source, selection, writer)
}

#[test]
fn chunked_export_rejects_workspace_index_changed_during_its_snapshot() {
    let (_temporary, source, selection, writer) = chunked_source();
    let mut changed = false;
    let result = source.export(
        &Limits::default(),
        &selection,
        Instant::now() + SOURCE_TIME,
        |_| {
            if !changed {
                writer
                    .execute(
                        "UPDATE composerHeaders SET workspaceId='moved-workspace'
                         WHERE composerId='synthetic-chunked'",
                        [],
                    )
                    .unwrap();
                changed = true;
            }
            Ok(())
        },
    );
    assert!(changed);
    assert!(matches!(result, Err(Error::SourceChanged)));
}

#[test]
fn chunked_export_keeps_its_wal_snapshot_across_an_atomic_header_update() {
    let (_temporary, source, selection, mut writer) = chunked_source();
    let replacement = r#"{"bubbleId":"new-assistant","type":2,"text":"synthetic replacement"}"#;
    // Cursor commits bubble payloads before its root and header transaction.
    writer
        .execute(
            "INSERT INTO cursorDiskKV VALUES ('bubbleId:synthetic-chunked:new-assistant', ?1)",
            [replacement],
        )
        .unwrap();

    let limits = Limits::default();
    let mut active = Vec::new();
    let mut committed = false;
    let new_headers = r#"[{"bubbleId":"new-assistant","type":2}]"#;
    source
        .export(&limits, &selection, Instant::now() + SOURCE_TIME, |bytes| {
            if !committed {
                let transaction = writer.transaction().unwrap();
                transaction
                    .execute(
                        "UPDATE cursorDiskKV SET value=json_set(value,'$.headerLog.generation','new-generation')
                         WHERE key='composerData:synthetic-chunked'",
                        [],
                    )
                    .unwrap();
                transaction
                    .execute(
                        "UPDATE cursorDiskKV SET value=?1
                         WHERE key='composerConvHeaders:synthetic-chunked:000001'",
                        [new_headers],
                    )
                    .unwrap();
                transaction.commit().unwrap();
                committed = true;
            }
            active.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
    assert!(committed);

    let mut next = Vec::new();
    source
        .export(&limits, &selection, Instant::now() + SOURCE_TIME, |bytes| {
            next.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();

    let active = captured_rows(&active);
    let next = captured_rows(&next);
    let shared = [
        "composerData:synthetic-chunked",
        "composerConvHeaders:synthetic-chunked:000000",
        "composerConvHeaders:synthetic-chunked:000001",
        "bubbleId:synthetic-chunked:z-user",
    ];
    for (rows, last_key, generation, headers, bubble) in [
        (
            &active,
            "bubbleId:synthetic-chunked:a-assistant",
            "synthetic-generation",
            r#"[{"bubbleId":"a-assistant","type":2}]"#,
            r#"{"bubbleId":"a-assistant","type":2,"text":"synthetic reply"}"#,
        ),
        (
            &next,
            "bubbleId:synthetic-chunked:new-assistant",
            "new-generation",
            new_headers,
            replacement,
        ),
    ] {
        assert!(
            rows.iter()
                .map(|(key, _)| key.as_str())
                .eq(shared.into_iter().chain([last_key]))
        );
        let composer: Value = serde_json::from_str(&rows[0].1).unwrap();
        assert_eq!(composer["headerLog"]["generation"], generation);
        assert_eq!(rows[1].1, r#"[ {"bubbleId":"z-user","type":1} ]"#);
        assert_eq!(rows[2].1, headers);
        assert_eq!(
            rows[3].1,
            r#"{"bubbleId":"z-user","type":1,"text":"synthetic prompt"}"#
        );
        assert_eq!(rows[4].1, bubble);
    }
}

fn captured_rows(bytes: &[u8]) -> Vec<(String, String)> {
    let records: Vec<Value> = std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.last().unwrap()["kind"], "end");
    let data: Vec<_> = records
        .iter()
        .filter_map(|record| record["base64"].as_str())
        .map(|encoded| String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap())
        .collect();
    assert_eq!(data.len() % 2, 0);
    data.chunks_exact(2)
        .map(|pair| (pair[0].clone(), pair[1].clone()))
        .collect()
}

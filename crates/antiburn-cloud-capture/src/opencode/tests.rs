use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::cell::Cell;
use std::collections::BTreeMap;

const CURRENT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/cloud/opencode/current.sql"
));
const EARLIER: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/cloud/opencode/earlier.sql"
));

fn database(sql: &str) -> (tempfile::TempDir, PathBuf, Connection) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(sql).unwrap();
    (directory, path, connection)
}

fn source(path: &Path, id: &str) -> OpenCodeSource {
    discover(path, &Limits::default())
        .unwrap()
        .into_iter()
        .find(|source| source.native_session_id() == id)
        .unwrap()
}

fn exported(source: &OpenCodeSource) -> (Vec<Value>, OpenCodeExport) {
    let mut records = Vec::new();
    let result = export(source, &Limits::default(), |bytes| {
        records.push(serde_json::from_slice(bytes).unwrap());
        Ok(())
    })
    .unwrap();
    (records, result)
}

/// Provider selection assertions only. Generic format fidelity has its own oracle.
fn row_counts(records: &[Value]) -> BTreeMap<String, usize> {
    let mut table = "";
    let mut counts = BTreeMap::new();
    for record in records {
        match record["kind"].as_str().unwrap() {
            "table" => {
                table = record["name"].as_str().unwrap();
                counts.insert(table.to_owned(), 0);
            }
            "row" => *counts.get_mut(table).unwrap() += 1,
            _ => {}
        }
    }
    counts
}

fn has_native_text(records: &[Value], expected: &str) -> bool {
    let encoded = STANDARD.encode(expected);
    records
        .iter()
        .any(|record| record["base64"].as_str() == Some(encoded.as_str()))
}

#[test]
fn discovers_all_native_identities_without_grouping_children_or_forks() {
    let (_directory, path, _connection) = database(CURRENT);
    let sources = discover(&path, &Limits::default()).unwrap();
    assert_eq!(
        sources
            .iter()
            .map(OpenCodeSource::native_session_id)
            .collect::<Vec<_>>(),
        ["ses_child", "ses_detached", "ses_fork", "ses_main"]
    );
    assert_ne!(sources[0].native_key(), sources[1].native_key());
    assert_eq!(
        sources[0].key_for_session("ses_detached").unwrap(),
        sources[1].native_key()
    );
}

#[test]
fn current_snapshot_keeps_all_owned_source_and_scoped_dependencies() {
    let (_directory, path, _connection) = database(CURRENT);
    let (records, relationship) = exported(&source(&path, "ses_main"));
    assert!(relationship.parent_session_id.is_none());
    assert!(relationship.fork_session_id.is_none());
    assert_eq!(
        row_counts(&records),
        BTreeMap::from([
            ("session_v2".into(), 1),
            ("session_message".into(), 4),
            ("session_part".into(), 2),
            ("event_sequence".into(), 1),
            ("event".into(), 4),
            ("session_inbox".into(), 1),
            ("instruction_entry".into(), 2),
            ("instruction_state".into(), 1),
            ("job".into(), 1),
            ("blob_ref".into(), 1),
            ("instruction_blob".into(), 3),
            ("blob".into(), 4),
        ])
    );
    for value in [
        "parent-owned child summary",
        "retained job error",
        "\"retired\"",
        "\"removed entry source\"",
        "not JSON: preserve this unknown native record",
        " { \"id\": \"Exact Model ID\", \"providerID\": \"synthetic\", \"future\": 1 } ",
    ] {
        assert!(
            has_native_text(&records, value),
            "missing synthetic source value"
        );
    }
    for value in [
        "CHILD-ONLY-CONTENT",
        "UNRELATED-JOB-CONTENT",
        "EXCLUDED-GLOBAL-VALUE",
        "EXCLUDED-SCHEDULE-VALUE",
        "\"fork baseline\"",
        "\"unrelated\"",
    ] {
        assert!(!has_native_text(&records, value));
    }
}

#[test]
fn earlier_v2_and_additive_columns_do_not_need_current_optional_tables() {
    let (_directory, path, connection) = database(EARLIER);
    connection
        .execute_batch(
            "ALTER TABLE session_message ADD COLUMN future_payload BLOB;
         UPDATE session_message SET future_payload = X'ff00fe';",
        )
        .unwrap();
    let (records, _) = exported(&source(&path, "ses_main"));
    assert_eq!(
        row_counts(&records),
        BTreeMap::from([("session_v2".into(), 1), ("session_message".into(), 1)])
    );
    assert!(records.iter().any(
        |record| record["columns"].as_array().is_some_and(|columns| {
            columns
                .iter()
                .any(|column| column["name"] == "future_payload")
        })
    ));
    assert!(records.iter().any(|record| record["base64"] == "/wD+"));
}

#[test]
fn detached_child_and_fork_never_need_parent_payload() {
    let (_directory, path, connection) = database(CURRENT);
    connection.execute("UPDATE session_message SET data = 'parent payload is not JSON' WHERE session_id = 'ses_main'", []).unwrap();
    let (child, relation) = exported(&source(&path, "ses_detached"));
    assert_eq!(relation.parent_session_id.as_deref(), Some("ses_absent"));
    assert_eq!(row_counts(&child)["session_message"], 1);
    let (fork, relation) = exported(&source(&path, "ses_fork"));
    assert_eq!(relation.fork_session_id.as_deref(), Some("ses_main"));
    assert!(has_native_text(&fork, "\"fork baseline\""));
    assert!(!has_native_text(&fork, "parent payload is not JSON"));
}

#[test]
fn discovery_does_not_parse_any_payload_or_dependency() {
    let (_directory, path, connection) = database(CURRENT);
    connection
        .execute_batch(
            "UPDATE session_message SET data = 'not JSON';
         UPDATE instruction_state SET initial_values = 'not JSON';
         UPDATE event SET data = 'not JSON';",
        )
        .unwrap();
    assert_eq!(discover(&path, &Limits::default()).unwrap().len(), 4);
    assert!(matches!(
        export(&source(&path, "ses_main"), &Limits::default(), |_| Ok(())),
        Err(Error::InvalidSource)
    ));
}

#[test]
fn optional_broken_and_unknown_owned_tables_fail_without_v1_fallback() {
    for sql in [
        "CREATE TABLE session_part(message_id TEXT, session_id TEXT)",
        "CREATE TABLE future_payload(id TEXT PRIMARY KEY, session_id TEXT, data TEXT)",
        "CREATE TABLE future_payload(id TEXT PRIMARY KEY, owner TEXT REFERENCES session_v2(id))",
        "CREATE TABLE session_future(id TEXT PRIMARY KEY, data TEXT)",
        "DROP INDEX session_message_session_seq_idx",
        "CREATE TABLE blob(hash TEXT, data BLOB)",
        "CREATE VIEW session_part AS SELECT id FROM session_message",
    ] {
        let (_directory, path, connection) = database(EARLIER);
        connection.execute_batch(sql).unwrap();
        assert!(
            matches!(
                discover(&path, &Limits::default()),
                Err(Error::UnsupportedSource)
            ),
            "{sql}"
        );
    }
    let (_directory, path, _connection) = database("CREATE TABLE session(id TEXT PRIMARY KEY);");
    assert!(matches!(
        discover(&path, &Limits::default()),
        Err(Error::UnsupportedSource)
    ));
}

#[test]
fn unknown_tables_with_case_insensitive_owned_links_fail_before_export() {
    for target in [
        "SESSION_V2",
        "SeSsIoN_v2",
        "SESSION_MESSAGE",
        "SeSsIoN_MeSsAgE",
        "BLOB",
        "BlOb",
        "INSTRUCTION_BLOB",
        "InStRuCtIoN_bLoB",
    ] {
        let (_directory, path, connection) = database(CURRENT);
        let admitted = source(&path, "ses_main");
        connection
            .execute_batch(&format!(
                "PRAGMA foreign_keys = OFF;
                 CREATE TABLE notes(owner TEXT REFERENCES {target}(id), body TEXT);
                 INSERT INTO notes VALUES ('ses_main', 'uncaptured synthetic note');"
            ))
            .unwrap();
        assert!(
            matches!(
                discover(&path, &Limits::default()),
                Err(Error::UnsupportedSource)
            ),
            "{target}"
        );
        let mut emitted = Vec::new();
        assert!(
            matches!(
                export(&admitted, &Limits::default(), |bytes| {
                    emitted.extend_from_slice(bytes);
                    Ok(())
                }),
                Err(Error::UnsupportedSource)
            ),
            "{target}"
        );
        assert!(emitted.is_empty(), "{target}");
    }
}

#[test]
fn exact_binary_ownership_does_not_select_case_aliases() {
    let (_directory, path, connection) = database(EARLIER);
    connection.execute_batch(
        "INSERT INTO session_v2(id) VALUES ('SES_MAIN');
         INSERT INTO session_message VALUES ('msg_other', 'SES_MAIN', 'user', 1, '{\"text\":\"case alias\"}');",
    ).unwrap();
    let (records, _) = exported(&source(&path, "ses_main"));
    assert_eq!(row_counts(&records)["session_message"], 1);
}

#[test]
fn part_message_ownership_mismatch_is_not_silently_dropped() {
    let (_directory, path, connection) = database(CURRENT);
    connection
        .execute(
            "UPDATE session_part SET message_id = 'msg_child' WHERE ordinal = 0",
            [],
        )
        .unwrap();
    assert!(matches!(
        export(&source(&path, "ses_main"), &Limits::default(), |_| Ok(())),
        Err(Error::InvalidSource)
    ));
}

#[test]
fn missing_native_dependencies_are_absence_not_invented_values() {
    let (_directory, path, connection) = database(CURRENT);
    connection.execute("DELETE FROM blob WHERE hash = 'dd2bf3282325d2e67286772e2f8119ec7a19f73a416f1a41e4b0c4c514d7acd5'", []).unwrap();
    connection
        .execute(
            "DELETE FROM instruction_blob WHERE value = '\"retired\"'",
            [],
        )
        .unwrap();
    let (records, _) = exported(&source(&path, "ses_main"));
    assert_eq!(row_counts(&records)["blob"], 3);
    assert_eq!(row_counts(&records)["instruction_blob"], 2);
    assert_eq!(row_counts(&records)["event"], 4);
}

#[test]
fn committed_wal_and_later_concurrent_writes_use_one_snapshot() {
    let (_directory, path, connection) = database(EARLIER);
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         INSERT INTO session_message VALUES ('msg_wal', 'ses_main', 'user', 2, '{\"text\":\"committed WAL value\"}');",
    ).unwrap();
    let source = source(&path, "ses_main");
    let changed = Cell::new(false);
    let mut records = Vec::<Value>::new();
    export(&source, &Limits::default(), |bytes| {
        if !changed.replace(true) {
            connection
                .execute_batch(
                    "UPDATE session_message SET data = '{\"text\":\"later writer value\"}';
                 UPDATE session_v2 SET parent_id = 'ses_later_parent';",
                )
                .unwrap();
        }
        records.push(serde_json::from_slice(bytes).unwrap());
        Ok(())
    })
    .unwrap();
    assert!(has_native_text(
        &records,
        "{\"text\":\"committed WAL value\"}"
    ));
    assert!(!has_native_text(
        &records,
        "{\"text\":\"later writer value\"}"
    ));
    assert_eq!(
        exported(&source).1.parent_session_id.as_deref(),
        Some("ses_later_parent")
    );
}

#[test]
fn replacement_and_renamed_ancestor_fail_but_normal_checkpoint_does_not() {
    let (directory, path, connection) = database(EARLIER);
    let selected = source(&path, "ses_main");
    connection
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_checkpoint(PASSIVE);")
        .unwrap();
    exported(&selected);
    let saved = directory.path().join("old.db");
    std::fs::rename(&path, &saved).unwrap();
    let replacement = Connection::open(&path).unwrap();
    replacement.execute_batch(EARLIER).unwrap();
    assert!(matches!(
        export(&selected, &Limits::default(), |_| Ok(())),
        Err(Error::SourceChanged)
    ));
}

#[test]
fn database_replaced_after_sqlite_open_cannot_publish_a_stream() {
    let (_directory, path, connection) = database(EARLIER);
    let source = source(&path, "ses_main");
    let changed = Cell::new(false);
    let old = path.with_file_name("old.db");
    let result = export(&source, &Limits::default(), |_| {
        if !changed.replace(true) {
            std::fs::rename(&path, &old).unwrap();
            let replacement = Connection::open(&path).unwrap();
            replacement.execute_batch(EARLIER).unwrap();
        }
        Ok(())
    });
    drop(connection);
    assert!(matches!(result, Err(Error::SourceChanged)));
}

#[test]
fn busy_deadline_discovery_and_expanded_byte_limits_are_closed_errors() {
    let (_directory, path, connection) = database(EARLIER);
    let selected = source(&path, "ses_main");
    assert!(matches!(
        export_before(&selected, &Limits::default(), |_| Ok(()), Instant::now()),
        Err(Error::Limit)
    ));
    assert!(matches!(
        export(
            &selected,
            &Limits {
                source_bytes: 64,
                ..Limits::default()
            },
            |_| Ok(())
        ),
        Err(Error::Limit)
    ));
    connection.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert!(matches!(
        discover(&path, &Limits::default()),
        Err(Error::Busy)
    ));
    connection.execute_batch("ROLLBACK").unwrap();
    connection
        .execute("INSERT INTO session_v2(id) VALUES ('ses_second')", [])
        .unwrap();
    assert!(matches!(
        discover(
            &path,
            &Limits {
                discovery_sessions: 1,
                ..Limits::default()
            }
        ),
        Err(Error::Limit)
    ));
}

#[test]
fn sink_interruption_stops_native_reads_without_changing_its_error() {
    let (_directory, path, _connection) = database(CURRENT);
    let result = export(&source(&path, "ses_main"), &Limits::default(), |_| {
        Err(Error::InterruptedCapture)
    });
    assert!(matches!(result, Err(Error::InterruptedCapture)));
}

#[test]
fn hardlinks_and_symlinked_sidecars_are_rejected() {
    use std::os::unix::fs::symlink;
    let (_directory, path, _connection) = database(EARLIER);
    std::fs::hard_link(&path, path.with_file_name("alias.db")).unwrap();
    assert!(matches!(
        discover(&path, &Limits::default()),
        Err(Error::InvalidSource)
    ));
    std::fs::remove_file(path.with_file_name("alias.db")).unwrap();
    let target = path.with_file_name("sidecar-target");
    std::fs::write(&target, []).unwrap();
    symlink(&target, path.with_file_name("opencode.db-wal")).unwrap();
    assert!(matches!(
        discover(&path, &Limits::default()),
        Err(Error::InvalidSource)
    ));
}

#[test]
fn selected_row_and_reference_limits_never_return_partial_exports() {
    let (_directory, path, connection) = database(EARLIER);
    connection
        .execute_batch(
            "WITH RECURSIVE n(x) AS (VALUES(2) UNION ALL SELECT x + 1 FROM n WHERE x <= 100000)
         INSERT INTO session_message
         SELECT 'msg_' || x, 'ses_main', 'user', x, '{}' FROM n;",
        )
        .unwrap();
    assert!(matches!(
        export(&source(&path, "ses_main"), &Limits::default(), |_| Ok(())),
        Err(Error::Limit)
    ));

    let (_directory, path, connection) = database(EARLIER);
    connection.execute_batch(
        "CREATE TABLE instruction_state(session_id TEXT PRIMARY KEY, initial_values TEXT, current_values TEXT);
         CREATE TABLE instruction_blob(hash TEXT PRIMARY KEY, value TEXT);",
    ).unwrap();
    let values = (0..=MAX_REFERENCES)
        .map(|number| format!("\"api/{number}\":\"{number:064x}\""))
        .collect::<Vec<_>>()
        .join(",");
    connection
        .execute(
            "INSERT INTO instruction_state VALUES ('ses_main', ?1, '{}')",
            [format!("{{{values}}}")],
        )
        .unwrap();
    assert!(matches!(
        export(&source(&path, "ses_main"), &Limits::default(), |_| Ok(())),
        Err(Error::Limit)
    ));
}

#[test]
fn old_pending_inputs_are_owned_source_and_can_prove_in_store_refs() {
    let (_directory, path, connection) = database(EARLIER);
    connection.execute_batch(
        "CREATE TABLE session_pending (
             id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
             admitted_seq INTEGER NOT NULL, data TEXT NOT NULL);
         CREATE TABLE blob(hash TEXT PRIMARY KEY, data BLOB);
         INSERT INTO session_pending VALUES ('pending', 'ses_main', 'user', 3,
             '{\"files\":[{\"mime\":\"text/plain\",\"uri\":\"media:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824\"}]}');
         INSERT INTO blob VALUES ('2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824', X'68656c6c6f');",
    ).unwrap();
    let (records, _) = exported(&source(&path, "ses_main"));
    assert_eq!(row_counts(&records)["session_pending"], 1);
    assert_eq!(row_counts(&records)["blob"], 1);
}

#[test]
fn event_reference_owner_must_match_its_selected_native_aggregate() {
    let (_directory, path, connection) = database(CURRENT);
    connection.execute(
        "UPDATE event SET data = '{\"sessionID\":\"ses_child\",\"delta\":{}}' WHERE id = 'evt_instruction'",
        [],
    ).unwrap();
    assert!(matches!(
        export(&source(&path, "ses_main"), &Limits::default(), |_| Ok(())),
        Err(Error::InvalidSource)
    ));
}

#[test]
fn physical_rowids_do_not_change_the_native_stream() {
    let (_directory, path, connection) = database(EARLIER);
    let selected = source(&path, "ses_main");
    let (before, _) = exported(&selected);
    connection
        .execute_batch(
            "UPDATE session_message SET rowid = 500;
         UPDATE session_v2 SET rowid = 700;
         VACUUM;",
        )
        .unwrap();
    let (after, _) = exported(&selected);
    assert_eq!(before, after);
}

#[test]
fn ancestor_replacement_cannot_redirect_a_saved_source() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let original = root.join("store");
    std::fs::create_dir(&original).unwrap();
    let path = original.join("opencode.db");
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch(EARLIER).unwrap();
    let selected = source(&path, "ses_main");
    std::fs::rename(&original, root.join("saved")).unwrap();
    std::fs::create_dir(&original).unwrap();
    let replacement = Connection::open(&path).unwrap();
    replacement.execute_batch(EARLIER).unwrap();
    assert!(matches!(
        export(&selected, &Limits::default(), |_| Ok(())),
        Err(Error::SourceChanged)
    ));
}

#[test]
fn present_oversized_in_store_blob_is_an_error_not_missing_source() {
    let (_directory, path, connection) = database(CURRENT);
    connection
        .execute(
            "UPDATE blob SET data = zeroblob(?1)
         WHERE hash = '2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824'",
            [64 * 1024 * 1024 + 1],
        )
        .unwrap();
    let emitted = Cell::new(false);
    let result = export(&source(&path, "ses_main"), &Limits::default(), |_| {
        emitted.set(true);
        Ok(())
    });
    assert!(matches!(result, Err(Error::Limit)));
    assert!(!emitted.get());
}

#[test]
fn native_event_schema_suffixes_select_dependencies_without_rewriting_source() {
    let (_directory, path, connection) = database(CURRENT);
    connection.execute(
        "UPDATE event SET type = 'session.instructions.updated.31' WHERE id = 'evt_instruction'",
        [],
    ).unwrap();
    let (records, _) = exported(&source(&path, "ses_main"));
    assert_eq!(row_counts(&records)["instruction_blob"], 3);
    assert!(has_native_text(&records, "session.instructions.updated.31"));
}

#[test]
fn shadowed_rowid_cannot_redirect_selected_source_to_a_child_row() {
    for table in ["session_v2", "session_message", "blob", "instruction_blob"] {
        let (_directory, path, connection) = database(CURRENT);
        let selected = source(&path, "ses_main");
        connection
            .execute_batch(&format!("ALTER TABLE {table} ADD COLUMN RoWiD INTEGER;"))
            .unwrap();
        if table == "session_message" {
            connection
                .execute_batch(
                    "UPDATE session_message SET RoWiD = _rowid_;
                     UPDATE session_message SET RoWiD =
                       (SELECT _rowid_ FROM session_message WHERE id = 'msg_child')
                     WHERE id = 'msg_user';",
                )
                .unwrap();
        }
        let emitted = Cell::new(false);
        assert!(matches!(
            export(&selected, &Limits::default(), |_| {
                emitted.set(true);
                Ok(())
            }),
            Err(Error::UnsupportedSource)
        ));
        assert!(!emitted.get());
    }
}

#[test]
fn unshadowed_physical_rowid_keeps_other_additive_alias_columns() {
    let (_directory, path, connection) = database(EARLIER);
    connection
        .execute_batch(
            "ALTER TABLE session_message ADD COLUMN _rowid_ TEXT;
             ALTER TABLE session_message ADD COLUMN oid TEXT;
             UPDATE session_message SET _rowid_ = 'ordinary alias value', oid = 'ordinary oid value';",
        )
        .unwrap();
    let (records, _) = exported(&source(&path, "ses_main"));
    assert!(has_native_text(&records, "ordinary alias value"));
    assert!(has_native_text(&records, "ordinary oid value"));
    assert!(has_native_text(
        &records,
        "{\"text\":\"earlier native input\"}"
    ));
}

#[test]
fn blob_refs_cannot_assign_existing_foreign_messages_or_inputs_to_selected_session() {
    for target in ["msg_child", "inbox_foreign", "pending_foreign"] {
        let (_directory, path, connection) = database(CURRENT);
        connection
            .execute_batch(
                "INSERT INTO session_inbox VALUES
                   ('inbox_foreign', 'ses_child', 'user', '{}', 'queued', 1, 1);
                 CREATE TABLE session_pending (
                   id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
                   admitted_seq INTEGER NOT NULL, data TEXT NOT NULL);
                 INSERT INTO session_pending VALUES
                   ('pending_foreign', 'ses_child', 'user', 1, '{}');",
            )
            .unwrap();
        connection
            .execute(
                "UPDATE blob_ref SET message_id = ?1,
                 hash = '34dd830a7088d2242b635c33961974c34d0b3f8398d12f4d50a8d97cde9c5996'
                 WHERE session_id = 'ses_main'",
                [target],
            )
            .unwrap();
        let emitted = Cell::new(false);
        assert!(matches!(
            export(&source(&path, "ses_main"), &Limits::default(), |_| {
                emitted.set(true);
                Ok(())
            }),
            Err(Error::InvalidSource)
        ));
        assert!(!emitted.get());
    }
}

#[test]
fn blob_refs_retain_absent_targets_and_accept_owned_queued_inputs() {
    for target in ["msg_absent", "inbox_current"] {
        let (_directory, path, connection) = database(CURRENT);
        connection
            .execute(
                "UPDATE blob_ref SET message_id = ?1,
                 hash = '34dd830a7088d2242b635c33961974c34d0b3f8398d12f4d50a8d97cde9c5996'
                 WHERE session_id = 'ses_main'",
                [target],
            )
            .unwrap();
        let (records, _) = exported(&source(&path, "ses_main"));
        assert_eq!(row_counts(&records)["blob_ref"], 1);
        assert_eq!(row_counts(&records)["blob"], 5);
        assert!(has_native_text(&records, target));
    }
}

#[test]
fn oversized_identity_discriminator_and_key_guards_do_not_materialize_text() {
    let oversized = "x".repeat(2 * 1024 * 1024);
    let (_directory, path, connection) = database(EARLIER);
    connection
        .execute("UPDATE session_v2 SET id = ?1", [&oversized])
        .unwrap();
    assert!(matches!(
        discover(&path, &Limits::default()),
        Err(Error::InvalidSource)
    ));
    let deadline = Instant::now() + EXPORT_TIME;
    let schema = selection::Schema::read(&connection, &Limits::default(), deadline).unwrap();
    connection.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 8192);
    assert!(matches!(
        schema.validate_identities(&connection),
        Err(Error::InvalidSource)
    ));
    assert!(matches!(
        bounded_text(&connection, "session_v2", "id", 1),
        Err(Error::InvalidSource)
    ));

    for sql in [
        "UPDATE session_message SET type = ?1",
        "UPDATE session_message SET id = ?1 WHERE id = 'msg_user'",
        "UPDATE instruction_entry SET key = ?1 WHERE key = 'api/current'",
    ] {
        let (_directory, _path, connection) = database(CURRENT);
        // These synthetic mutations can leave foreign-key targets absent.
        connection.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        connection.execute(sql, [&oversized]).unwrap();
        let deadline = Instant::now() + EXPORT_TIME;
        let schema = selection::Schema::read(&connection, &Limits::default(), deadline).unwrap();
        // CAST would load the entire value and return SQLITE_TOOBIG (Limit).
        // octet_length reads its stored byte count and rejects it as invalid.
        connection.set_limit(rusqlite::limits::Limit::SQLITE_LIMIT_LENGTH, 8192);
        let result = selection::select(
            &connection,
            &schema,
            "ses_main",
            &Limits::default(),
            deadline,
        )
        .map(|_| ());
        assert!(
            matches!(result, Err(Error::InvalidSource)),
            "{sql}: {result:?}"
        );
    }
}

/// Run a separate test process with captured pipes, then reap it before returning.
fn lock_process(path: &Path, operation: &str) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "opencode::tests::separate_process_lock_worker",
            "--ignored",
            "--nocapture",
        ])
        .env("ANTIBURN_OPENCODE_LOCK_TEST_DATABASE", path)
        .env("ANTIBURN_OPENCODE_LOCK_TEST_OPERATION", operation)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "lock worker failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("lock check complete"),
        "lock worker did not run"
    );
}

#[test]
#[ignore = "run only as a child of the separate-process lock regressions"]
fn separate_process_lock_worker() {
    let path = std::env::var_os("ANTIBURN_OPENCODE_LOCK_TEST_DATABASE").unwrap();
    let connection =
        Connection::open_with_flags(PathBuf::from(path), OpenFlags::SQLITE_OPEN_READ_WRITE)
            .unwrap();
    connection.busy_timeout(Duration::ZERO).unwrap();
    match std::env::var("ANTIBURN_OPENCODE_LOCK_TEST_OPERATION")
        .unwrap()
        .as_str()
    {
        "wal-write" => {
            connection
                .execute_batch(
                    "PRAGMA wal_autocheckpoint=0;
                     UPDATE session_message SET data = '{\"text\":\"separate writer value\"}';",
                )
                .unwrap();
            let (busy, frames, checkpointed): (i64, i64, i64) = connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap();
            assert_eq!(busy, 1, "the capture must retain its WAL reader lock");
            assert!(frames > checkpointed);
        }
        "wal-checkpoint" => {
            let checkpoint: (i64, i64, i64) = connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .unwrap();
            assert_eq!(checkpoint, (0, 0, 0));
        }
        "rollback-write" => {
            let error = connection
                .execute(
                    "UPDATE session_message SET data = '{\"text\":\"foreign writer\"}'",
                    [],
                )
                .unwrap_err();
            assert_eq!(
                error.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DatabaseBusy)
            );
        }
        _ => panic!("unknown lock worker operation"),
    }
    println!("lock check complete");
}

#[test]
fn separate_process_writer_cannot_checkpoint_an_open_capture_snapshot() {
    let (_directory, path, connection) = database(EARLIER);
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA wal_autocheckpoint=0;
             INSERT INTO session_message VALUES
               ('msg_wal', 'ses_main', 'user', 2, '{\"text\":\"snapshot WAL value\"}');",
        )
        .unwrap();
    let selected = source(&path, "ses_main");
    let changed = Cell::new(false);
    let mut records = Vec::<Value>::new();
    export(&selected, &Limits::default(), |bytes| {
        if !changed.replace(true) {
            lock_process(&path, "wal-write");
        }
        records.push(serde_json::from_slice(bytes).unwrap());
        Ok(())
    })
    .unwrap();
    assert!(has_native_text(
        &records,
        "{\"text\":\"snapshot WAL value\"}"
    ));
    assert!(!has_native_text(
        &records,
        "{\"text\":\"separate writer value\"}"
    ));
    lock_process(&path, "wal-checkpoint");
}

#[test]
fn dropping_admitted_source_does_not_clear_another_connection_read_lock() {
    let (_directory, path, connection) = database(EARLIER);
    let selected = source(&path, "ses_main");
    connection.execute_batch("BEGIN").unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM session_message", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    drop(selected);
    lock_process(&path, "rollback-write");
    connection.execute_batch("ROLLBACK").unwrap();
}

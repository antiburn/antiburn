use antiburn_checks_store::{
    opencode_journal_receipt_compatible_sql, source_ready_at_sql, source_ready_sql,
};
use antiburn_local::analysis::{ANALYZER_REVISION, EVIDENCE_SCHEMA_REVISION, PARSER_REVISION};
use rusqlite::{Connection, params};
use serde_json::json;

fn database() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(include_str!("fixtures/session.sql"))
        .unwrap();
    connection
        .execute(
            "UPDATE native_file_delivery SET parser_revision = ?1",
            [PARSER_REVISION],
        )
        .unwrap();
    connection
}

fn ready(connection: &Connection, agent: &str) -> bool {
    let predicate = source_ready_at_sql(1, 2, 3, "200");
    connection
        .query_row(
            &format!(
                "SELECT COALESCE(({predicate}), 0) FROM session s
                  LEFT JOIN session_evidence e USING(environment_key, agent, session_id)
                  WHERE s.agent = ?4"
            ),
            params![
                PARSER_REVISION,
                ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION,
                agent
            ],
            |row| row.get(0),
        )
        .unwrap()
}

#[test]
fn readiness_wrappers_share_one_predicate() {
    assert_eq!(
        source_ready_sql(4, 5, 6),
        source_ready_at_sql(4, 5, 6, "unixepoch()")
    );
}

#[test]
fn file_readiness_requires_current_cursor_layout_and_identity() {
    for (agent, format) in [("codex", "codex_rollout_jsonl"), ("pi", "pi_v3_jsonl")] {
        for parser in [63, 64, 65, 66, PARSER_REVISION] {
            for revision in [1, 4, 5] {
                let connection = database();
                connection
                    .execute(
                        "UPDATE session SET agent = ?1, source_format = ?2",
                        params![agent, json!(format).to_string()],
                    )
                    .unwrap();
                let cursor = json!({"revision": revision, "agent": agent, "session_id": "s1", "source_format": format});
                connection.execute("UPDATE native_file_delivery SET agent = ?1, parser_revision = ?2, cursor_json = ?3", params![agent, parser, cursor.to_string()]).unwrap();
                assert_eq!(
                    ready(&connection, agent),
                    revision == 5,
                    "{agent}, parser {parser}, cursor {revision}"
                );
                connection.execute("UPDATE native_file_delivery SET cursor_json = json_set(cursor_json, '$.session_id', 'other')", []).unwrap();
                assert!(!ready(&connection, agent));
            }
        }
    }
}

#[test]
fn claude_predecessor_keeps_its_exact_origin_boundary() {
    for parser in [65, 66, PARSER_REVISION] {
        let connection = database();
        connection
            .execute(
                "UPDATE native_file_delivery SET parser_revision = ?1",
                [parser],
            )
            .unwrap();
        assert!(ready(&connection, "claude-code"));
        for change in [
            "UPDATE setting SET value = json_set(value, '$.source_label', 'other')",
            "UPDATE native_tool_result_publication SET session_incarnation = 8",
            "UPDATE native_tool_result_publication SET gap_kind = 'partial'",
            "UPDATE native_file_delivery SET cursor_json = '{}'",
            "UPDATE native_file_delivery SET blocked = 1",
            "UPDATE native_file_delivery SET last_error = 'synthetic-error'",
        ] {
            let connection = database();
            connection
                .execute(
                    "UPDATE native_file_delivery SET parser_revision = ?1",
                    [parser],
                )
                .unwrap();
            connection.execute_batch(change).unwrap();
            assert!(!ready(&connection, "claude-code"), "{change}");
        }
    }
    let connection = database();
    connection
        .execute("UPDATE native_file_delivery SET parser_revision = 64", [])
        .unwrap();
    assert!(!ready(&connection, "claude-code"));
}

#[test]
fn journal_readiness_requires_live_state_five_and_a_bound_public_baseline() {
    for parser in [63, 64, 65, 66, PARSER_REVISION] {
        for revision in [2, 4, 5] {
            let connection = database();
            connection.execute("UPDATE session SET agent = 'opencode', source_kind = 'providerDb', source_format = '\"open_code_sqlite_v2\"'", []).unwrap();
            let cursor = json!({"session_id": "s1", "owner_id": "owner", "parser_revision": parser,
                "through_sequence": 7, "live": {"revision": revision, "public_baseline": 3}});
            connection.execute("INSERT INTO opencode_journal_delivery VALUES ('native','opencode','s1',7,'source','/repo',?1,?2,0,1,NULL)", params![parser, cursor.to_string()]).unwrap();
            assert_eq!(
                ready(&connection, "opencode"),
                revision == 5,
                "parser {parser}, live state {revision}"
            );
            if revision == 5 {
                connection.execute("UPDATE opencode_journal_delivery SET cursor_json = json_set(cursor_json, '$.live.public_baseline', 8)", []).unwrap();
                assert!(ready(&connection, "opencode"));
            }
            connection.execute("UPDATE opencode_journal_delivery SET cursor_json = json_set(cursor_json, '$.live.public_baseline', -1)", []).unwrap();
            assert!(!ready(&connection, "opencode"));
        }
    }
}

#[test]
fn journal_cold_replay_blocks_readiness_but_allows_retry_and_warm_pending_work() {
    let connection = database();
    connection
        .execute_batch(
            "UPDATE session SET agent = 'opencode', source_kind = 'providerDb',
                source_format = '\"open_code_sqlite_v2\"';
             ALTER TABLE opencode_journal_delivery ADD COLUMN pending INTEGER NOT NULL DEFAULT 1;",
        )
        .unwrap();
    let cursor = json!({
        "session_id": "s1", "owner_id": "owner", "parser_revision": PARSER_REVISION,
        "through_sequence": 7, "live": {"revision": 5, "public_baseline": 7},
        "cold_replay": {"output_sha256": vec![0; 32]}
    });
    connection
        .execute(
            "INSERT INTO opencode_journal_delivery VALUES
                ('native','opencode','s1',7,'source','/repo',?1,?2,0,1,NULL,1)",
            params![PARSER_REVISION, cursor.to_string()],
        )
        .unwrap();
    seed_cached_evidence(&connection, "opencode");
    let compatible = || {
        let predicate = opencode_journal_receipt_compatible_sql(1);
        connection
            .query_row(
                &format!(
                    "SELECT ({predicate}) FROM session s JOIN opencode_journal_delivery d
                     USING(environment_key, agent, session_id) WHERE s.agent = 'opencode'"
                ),
                [PARSER_REVISION],
                |row| row.get::<_, bool>(0),
            )
            .unwrap()
    };
    assert!(compatible());
    assert!(!ready(&connection, "opencode"));
    connection
        .execute(
            "UPDATE opencode_journal_delivery
             SET cursor_json = json_set(cursor_json, '$.cold_replay', NULL)",
            [],
        )
        .unwrap();
    assert!(compatible());
    assert!(!ready(&connection, "opencode"));
    connection
        .execute(
            "UPDATE opencode_journal_delivery
             SET cursor_json = json_remove(cursor_json, '$.cold_replay')",
            [],
        )
        .unwrap();
    assert!(compatible());
    assert!(ready(&connection, "opencode"));
    assert_eq!(
        connection
            .query_row("SELECT pending FROM opencode_journal_delivery", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
}

#[test]
fn journal_format_and_cursor_parser_cannot_fall_back_to_cached_evidence() {
    for change in [
        "UPDATE session SET source_format = NULL",
        "UPDATE session SET source_format = '\"uncharacterized\"'",
        "UPDATE opencode_journal_delivery SET cursor_json = json_set(cursor_json, '$.parser_revision', 62)",
        "UPDATE opencode_journal_delivery SET cursor_json = json_remove(cursor_json, '$.live.public_baseline')",
        "UPDATE opencode_journal_delivery SET main_session = 0",
    ] {
        let connection = database();
        connection.execute("UPDATE session SET agent = 'opencode', source_kind = 'providerDb', source_format = '\"open_code_sqlite_v2\"'", []).unwrap();
        let cursor = json!({"session_id": "s1", "parser_revision": PARSER_REVISION,
            "through_sequence": 7, "live": {"revision": 5, "public_baseline": 3}});
        connection.execute("INSERT INTO opencode_journal_delivery VALUES ('native','opencode','s1',7,'source','/repo',?1,?2,0,1,NULL)", params![PARSER_REVISION, cursor.to_string()]).unwrap();
        seed_cached_evidence(&connection, "opencode");
        connection.execute_batch(change).unwrap();
        assert!(!ready(&connection, "opencode"), "{change}");
    }
}

#[test]
fn v2_uses_a_matching_live_lease_without_a_generation_or_history_gate() {
    let connection = database();
    connection.execute("UPDATE session SET agent = 'opencode', source_kind = 'providerDb', source_format = '\"open_code_v2_sqlite\"'", []).unwrap();
    connection.execute("INSERT INTO opencode_v2_capture VALUES ('native','opencode','s1',7,'source','/repo','epoch',1,201)", []).unwrap();
    assert!(ready(&connection, "opencode"));
    connection
        .execute("UPDATE session SET source_generation = 2", [])
        .unwrap();
    assert!(ready(&connection, "opencode"));
    connection
        .execute(
            "UPDATE opencode_v2_capture SET lease_expires_at_epoch = 200",
            [],
        )
        .unwrap();
    assert!(!ready(&connection, "opencode"));
    connection
        .execute(
            "UPDATE opencode_v2_capture SET lease_expires_at_epoch = 201, incarnation = 8",
            [],
        )
        .unwrap();
    assert!(!ready(&connection, "opencode"));
}

fn seed_cached_evidence(connection: &Connection, agent: &str) {
    connection
        .execute(
            "INSERT INTO session_evidence VALUES ('native',?1,'s1','ready',1,?2,?3,?4)",
            params![
                agent,
                PARSER_REVISION,
                ANALYZER_REVISION,
                EVIDENCE_SCHEMA_REVISION
            ],
        )
        .unwrap();
}

#[test]
fn pinned_main_receipts_remain_unready_with_current_cached_evidence() {
    let file = database();
    file.execute(
        "UPDATE session SET agent = 'codex', source_format = '\"codex_rollout_jsonl\"'",
        [],
    )
    .unwrap();
    let cursor = json!({"revision": 1, "agent": "codex", "session_id": "s1", "source_format": "codex_rollout_jsonl"});
    file.execute(
        "UPDATE native_file_delivery SET agent = 'codex', parser_revision = 64, cursor_json = ?1",
        [cursor.to_string()],
    )
    .unwrap();
    seed_cached_evidence(&file, "codex");
    assert!(!ready(&file, "codex"));
    file.execute("UPDATE native_file_delivery SET parser_revision = ?1, cursor_json = json_set(cursor_json, '$.revision', 5)", [PARSER_REVISION]).unwrap();
    assert!(ready(&file, "codex"));
    file.execute("UPDATE native_file_delivery SET blocked = 1", [])
        .unwrap();
    assert!(!ready(&file, "codex"));

    let journal = database();
    journal.execute("UPDATE session SET agent = 'opencode', source_kind = 'providerDb', source_format = '\"open_code_sqlite_v2\"'", []).unwrap();
    let cursor = json!({"session_id": "s1", "parser_revision": 64, "through_sequence": 7,
        "live": {"revision": 2, "baseline": 0}});
    journal.execute("INSERT INTO opencode_journal_delivery VALUES ('native','opencode','s1',7,'source','/repo',64,?1,0,1,NULL)", [cursor.to_string()]).unwrap();
    seed_cached_evidence(&journal, "opencode");
    assert!(!ready(&journal, "opencode"));
    journal.execute("UPDATE opencode_journal_delivery SET parser_revision = ?1,
        cursor_json = json_set(cursor_json, '$.parser_revision', ?1, '$.live.revision', 5, '$.live.public_baseline', 7)", [PARSER_REVISION]).unwrap();
    assert!(ready(&journal, "opencode"));
    journal
        .execute("UPDATE opencode_journal_delivery SET blocked = 1", [])
        .unwrap();
    assert!(!ready(&journal, "opencode"));
}

#[test]
fn cursor_keeps_only_the_native_inline_snapshot_route() {
    for change in [
        "UPDATE session SET source_kind = 'file'",
        "UPDATE session SET source_label = 'cursor-cli:s1'",
        "UPDATE session SET wsl_distro = 'synthetic'",
        "UPDATE session SET source_generation = 2",
        "INSERT INTO session_relation VALUES ('native','cursor','child','s1','subagent')",
        "INSERT INTO session_relation VALUES ('native','cursor','s1','parent','forkParent')",
    ] {
        let connection = database();
        connection.execute("UPDATE session SET agent = 'cursor', source_kind = 'inline', source_label = 'cursor-desktop:workspace:s1'", []).unwrap();
        seed_cached_evidence(&connection, "cursor");
        assert!(ready(&connection, "cursor"));
        connection.execute_batch(change).unwrap();
        assert!(!ready(&connection, "cursor"), "{change}");
    }
}

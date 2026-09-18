//! Read-only Devin Local migration-17 session analysis.
//!
//! Devin keeps the active conversation as a linked forest in SQLite. This
//! reader walks only the path named by `sessions.main_chain_id`. It retains
//! model, time, and tool-call facts, but does not claim Devin's token or cache
//! semantics.

use std::collections::{HashMap, HashSet};
use std::path::Path;

#[cfg(test)]
use std::path::PathBuf;

use anyhow::Context;
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::analysis::SourceChangedReason;
use crate::analysis::evidence::{SourceCapabilities, SourceFormat};
use crate::analysis::framing::PartialReason;
use crate::analysis::interface::{
    EvidenceObservation, NormalizedRecord, RawSource, RecordSink, RelationProvenance,
    SessionCollector, SessionInput, SessionReader, SessionSummary, VisitOutcome,
};
use crate::analysis::model::{
    NormalizedEvent, NormalizedSession, Role, ToolCall, ToolCategory, Usage,
};
use crate::analysis::records::parse_ts;
use crate::discovery::source_version::{
    DEVIN_SQLITE_MAX_ROWS, DEVIN_SQLITE_MAX_TEXT_BYTES, DevinAcpCompanion,
    devin_acp_companion_records, devin_content_fingerprint, devin_provider_db_fingerprint,
};

pub struct DevinLocalSessionReader;

impl SessionReader for DevinLocalSessionReader {
    fn agent(&self) -> &'static str {
        "windsurf"
    }

    fn capabilities(&self, input: &SessionInput) -> SourceCapabilities {
        let format = input.source_format_or(SourceFormat::DevinLocalSqlite);
        if format != SourceFormat::DevinLocalSqlite {
            return SourceCapabilities::uncharacterized(format);
        }
        SourceCapabilities {
            source_format: format,
            timestamps_and_order: true,
            tool_invocations: true,
            model_identity: true,
            subagent_relationships: true,
            subagent_models: true,
            ..SourceCapabilities::generic()
        }
    }

    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession> {
        let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
        self.visit(input, &mut collector)?;
        collector.into_session()
    }

    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let RawSource::Sqlite(path) = &input.source else {
            anyhow::bail!("Devin Local requires a SQLite source")
        };
        validate_input(input)?;
        let connection = open_database(path)?;
        connection.execute_batch("BEGIN")?;
        validate_schema(&connection)?;
        let summary = visit_connection(&connection, path, &input.session_id, sink)?;
        connection.execute_batch("COMMIT")?;
        sink.finish(summary);
        Ok(VisitOutcome::Unvalidated)
    }

    fn visit_db_claimed(
        &self,
        input: &SessionInput,
        claimed_fingerprint: &str,
        _cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let RawSource::Sqlite(path) = &input.source else {
            anyhow::bail!("a claimed Devin Local source must be SQLite")
        };
        validate_input(input)?;
        let connection = open_database(path)?;
        connection.execute_batch("BEGIN")?;
        validate_schema(&connection)?;
        let actual = session_fingerprint(&connection, path, &input.session_id)
            .map(|(latest, rows)| devin_provider_db_fingerprint(latest, rows));
        if actual.as_deref() != Some(claimed_fingerprint) {
            return Ok(VisitOutcome::SourceChanged(
                SourceChangedReason::FingerprintMismatch,
            ));
        }
        let summary = visit_connection(&connection, path, &input.session_id, sink)?;
        connection.execute_batch("COMMIT")?;

        let verification = open_database(path)?;
        let observed = session_fingerprint(&verification, path, &input.session_id)
            .map(|(latest, rows)| devin_provider_db_fingerprint(latest, rows));
        if observed.as_deref() != Some(claimed_fingerprint) {
            return Ok(VisitOutcome::SourceChanged(
                SourceChangedReason::FingerprintMismatch,
            ));
        }
        sink.finish(summary);
        Ok(VisitOutcome::AcceptedFull)
    }
}

fn validate_input(input: &SessionInput) -> anyhow::Result<()> {
    match (input.source_format, &input.source) {
        (SourceFormat::DevinLocalSqlite, RawSource::Sqlite(_))
        | (SourceFormat::Uncharacterized, RawSource::Sqlite(_)) => Ok(()),
        _ => anyhow::bail!("Devin Local source format does not match its source"),
    }
}

fn open_database(path: &Path) -> anyhow::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening Devin Local database {}", path.display()))
}

fn validate_schema(connection: &Connection) -> anyhow::Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version != 17 {
        anyhow::bail!("Devin Local SQLite migration {version} is not supported")
    }
    let required = [
        (
            "sessions",
            &[
                "id",
                "working_directory",
                "model",
                "main_chain_id",
                "hidden",
                "created_at",
                "last_activity_at",
            ] as &[_],
        ),
        (
            "message_nodes",
            &[
                "node_id",
                "parent_node_id",
                "session_id",
                "raw_message",
                "created_at",
            ] as &[_],
        ),
        (
            "subagent_heads",
            &[
                "session_id",
                "tool_call_id",
                "child_agent_id",
                "child_chain_node_id",
            ] as &[_],
        ),
        (
            "tool_call_state",
            &["session_id", "tool_call_id", "state"] as &[_],
        ),
    ];
    for (table, names) in required {
        let columns = table_columns(connection, table)?;
        if !names.iter().all(|name| columns.contains(*name)) {
            anyhow::bail!("Devin Local SQLite source has an unsupported {table} schema")
        }
    }
    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> anyhow::Result<HashSet<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<_>>()?)
}

type Row = HashMap<String, String>;
struct QueriedRows {
    rows: Vec<Row>,
    partial: bool,
}

fn query_rows(
    connection: &Connection,
    query: &str,
    session_id: &str,
) -> anyhow::Result<QueriedRows> {
    let mut statement = connection.prepare(query)?;
    let columns: Vec<String> = statement
        .column_names()
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let mut rows = statement.query([session_id])?;
    let mut result = Vec::new();
    let mut partial = false;
    while let Some(row) = rows.next()? {
        if result.len() == DEVIN_SQLITE_MAX_ROWS {
            partial = true;
            break;
        }
        let mut values = Row::new();
        for (index, column) in columns.iter().enumerate() {
            let value = row.get_ref(index)?;
            let text = match value {
                rusqlite::types::ValueRef::Null => None,
                rusqlite::types::ValueRef::Text(bytes) => {
                    let end = bytes.len().min(DEVIN_SQLITE_MAX_TEXT_BYTES);
                    let end = (0..=end)
                        .rev()
                        .find(|index| std::str::from_utf8(&bytes[..*index]).is_ok())
                        .unwrap_or(0);
                    if end < bytes.len() {
                        partial = true;
                    }
                    String::from_utf8(bytes[..end].to_vec()).ok()
                }
                rusqlite::types::ValueRef::Integer(value) => Some(value.to_string()),
                rusqlite::types::ValueRef::Real(value) => Some(value.to_string()),
                rusqlite::types::ValueRef::Blob(_) => None,
            };
            if let Some(text) = text {
                values.insert(column.clone(), text);
            }
        }
        result.push(values);
    }
    Ok(QueriedRows {
        rows: result,
        partial,
    })
}

fn session_rows(connection: &Connection, session_id: &str) -> anyhow::Result<QueriedRows> {
    query_rows(
        connection,
        "SELECT id, working_directory, model, main_chain_id, hidden, created_at, last_activity_at
         FROM sessions WHERE id = ?1",
        session_id,
    )
}

fn message_rows(connection: &Connection, session_id: &str) -> anyhow::Result<QueriedRows> {
    query_rows(
        connection,
        "SELECT node_id, parent_node_id, session_id, raw_message, created_at
         FROM message_nodes
         WHERE session_id = ?1
            OR (session_id, node_id) IN (
                SELECT child_agent_id, child_chain_node_id
                FROM subagent_heads WHERE session_id = ?1
            )
         ORDER BY session_id, node_id",
        session_id,
    )
}

fn child_model_rows(connection: &Connection, session_id: &str) -> anyhow::Result<QueriedRows> {
    query_rows(
        connection,
        "SELECT id, model FROM sessions
         WHERE id IN (
             SELECT child_agent_id FROM subagent_heads WHERE session_id = ?1
         )",
        session_id,
    )
}

fn head_rows(connection: &Connection, session_id: &str) -> anyhow::Result<QueriedRows> {
    query_rows(
        connection,
        "SELECT session_id, tool_call_id, child_agent_id, child_chain_node_id
         FROM subagent_heads WHERE session_id = ?1
         ORDER BY tool_call_id, child_agent_id",
        session_id,
    )
}

fn state_rows(connection: &Connection, session_id: &str) -> anyhow::Result<QueriedRows> {
    query_rows(
        connection,
        "SELECT session_id, tool_call_id, state FROM tool_call_state
         WHERE session_id = ?1 ORDER BY tool_call_id",
        session_id,
    )
}

fn session_fingerprint(
    connection: &Connection,
    db_path: &Path,
    session_id: &str,
) -> Option<(u64, u64)> {
    let latest: i64 = connection
        .query_row(
            "SELECT COALESCE((SELECT last_activity_at FROM sessions WHERE id = ?1), (SELECT created_at FROM sessions WHERE id = ?1), 0), (SELECT COUNT(*) FROM message_nodes WHERE session_id = ?1)",
            [session_id],
            |row| row.get(0),
        )
        .ok()?;
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM message_nodes WHERE session_id = ?1",
            [session_id],
            |row| row.get(0),
        )
        .ok()?;
    let content = devin_content_fingerprint(connection, db_path, session_id)?;
    Some((content ^ latest.max(0) as u64, rows.max(0) as u64))
}

fn visit_connection(
    connection: &Connection,
    db_path: &Path,
    session_id: &str,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<SessionSummary> {
    let sessions = session_rows(connection, session_id)?;
    let session = sessions
        .rows
        .iter()
        .find(|row| row.get("id").is_some_and(|id| id == session_id))
        .ok_or_else(|| anyhow::anyhow!("Devin Local session {session_id} was not found"))?;
    if session
        .get("hidden")
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    {
        return Ok(SessionSummary::default());
    }
    let nodes = message_rows(connection, session_id)?;
    let chain = active_chain(session, &nodes.rows)?;
    let mut child_models = HashMap::new();
    let child_models_rows = child_model_rows(connection, session_id)?;
    let child_models_partial = child_models_rows.partial;
    for row in child_models_rows.rows {
        if let (Some(id), Some(model)) = (row.get("id"), row.get("model")) {
            child_models.insert(id.clone(), model.clone());
        }
    }
    let head_query = head_rows(connection, session_id)?;
    let state_query = state_rows(connection, session_id)?;
    let heads = head_query.rows;
    let states = state_query.rows;
    let mut acp_index = None;
    let mut acp_partial = false;
    let mut seen_calls = HashSet::new();
    let mut incomplete = sessions.partial
        || nodes.partial
        || child_models_partial
        || head_query.partial
        || state_query.partial;
    let mut model = session.get("model").cloned();
    for node in chain {
        let Some(raw) = node.get("raw_message") else {
            incomplete = true;
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            incomplete = true;
            sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
            continue;
        };
        let role = match value.get("role").and_then(Value::as_str) {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            Some("tool") => Role::Tool,
            _ => continue,
        };
        let ts_ms = node
            .get("created_at")
            .and_then(|value| value.parse::<i64>().ok())
            .and_then(|value| parse_ts(&Value::from(value)));
        let mut event = NormalizedEvent::new(role);
        event.ts_ms = ts_ms;
        event.model = value
            .pointer("/metadata/generation_model")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| {
                value
                    .pointer("/metadata/model")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .or_else(|| session.get("model").cloned());
        model = event.model.clone().or(model);
        event.message_id = value
            .get("message_id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        event.usage = usage(&value);
        if role == Role::Assistant
            && let Some(calls) = value.get("tool_calls").and_then(Value::as_array)
        {
            for call in calls {
                let Some(name) = call.get("name").and_then(Value::as_str) else {
                    incomplete = true;
                    continue;
                };
                let id = call.get("id").and_then(Value::as_str).unwrap_or("");
                if !seen_calls.insert((session_id.to_owned(), id.to_owned())) {
                    incomplete = true;
                    continue;
                }
                event.tools.push(ToolCall {
                    name: name.to_owned(),
                    category: ToolCategory::from_tool_name(name),
                    detail: None,
                });
                if name == "run_subagent" {
                    match relation(
                        session_id,
                        id,
                        &heads,
                        &states,
                        &child_models,
                        &nodes.rows,
                        acp_index.get_or_insert_with(|| {
                            let (index, partial) = acp_companion_index(db_path, &heads);
                            acp_partial = partial;
                            index
                        }),
                    ) {
                        Some(child_model) => sink.record(NormalizedRecord::Observation(Box::new(
                            EvidenceObservation::SubagentSpawn {
                                ts_ms,
                                parent_model: event.model.clone(),
                                parent_call_id: Some(id.to_owned()),
                                child_model: Some(child_model),
                                provenance: RelationProvenance::TaskToolUse,
                            },
                        ))),
                        None => incomplete = true,
                    }
                }
            }
        }
        sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
    }
    Ok(SessionSummary {
        model,
        coverage_gaps: if incomplete || acp_partial {
            vec![PartialReason::AttributionIncomplete]
        } else {
            Vec::new()
        },
        ..SessionSummary::default()
    })
}

fn active_chain(session: &Row, nodes: &[Row]) -> anyhow::Result<Vec<Row>> {
    let mut by_id = HashMap::new();
    for node in nodes.iter().filter(|node| {
        node.get("session_id")
            .is_some_and(|id| session.get("id") == Some(id))
    }) {
        if let Some(id) = node.get("node_id") {
            by_id.insert(id.clone(), node.clone());
        }
    }
    let mut current = session
        .get("main_chain_id")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing main chain"))?;
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    while let Some(node) = by_id.get(&current) {
        if !seen.insert(current.clone()) {
            anyhow::bail!("cycle in Devin Local main chain")
        }
        current = node.get("parent_node_id").cloned().unwrap_or_default();
        chain.push(node.clone());
        if current.is_empty() {
            break;
        }
    }
    if chain.is_empty() || !current.is_empty() {
        anyhow::bail!("Devin Local main chain is incomplete")
    }
    chain.reverse();
    Ok(chain)
}

fn relation(
    session_id: &str,
    call_id: &str,
    heads: &[Row],
    states: &[Row],
    models: &HashMap<String, String>,
    nodes: &[Row],
    acp_index: &AcpIndex,
) -> Option<String> {
    let matching: Vec<&Row> = heads
        .iter()
        .filter(|row| {
            row.get("session_id") == Some(&session_id.to_owned())
                && row.get("tool_call_id") == Some(&call_id.to_owned())
        })
        .collect();
    if matching.len() != 1 {
        return None;
    }
    let head = matching[0];
    let child = head.get("child_agent_id")?;
    let chain_node = head.get("child_chain_node_id")?;
    if states
        .iter()
        .filter(|row| {
            row.get("session_id") == Some(&session_id.to_owned())
                && row.get("tool_call_id") == Some(&call_id.to_owned())
        })
        .filter_map(|row| row.get("state"))
        .any(|state| match serde_json::from_str::<Value>(state) {
            Ok(state) => state
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| matches!(status, "interrupted" | "failed")),
            Err(_) => true,
        })
    {
        return None;
    }
    let node = nodes.iter().find(|row| {
        row.get("session_id") == Some(child) && row.get("node_id") == Some(chain_node)
    })?;
    let value: Value = serde_json::from_str(node.get("raw_message")?).ok()?;
    let actual = value
        .pointer("/metadata/generation_model")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/metadata/model").and_then(Value::as_str))?;
    let stored = models.get(child)?;
    if actual != stored {
        return None;
    }
    acp_relation_is_consistent(acp_index, session_id, call_id, child, actual)?;
    Some(actual.to_owned())
}

type AcpKey = (String, String, String);
type AcpIndex = HashMap<AcpKey, Vec<AcpCompanion>>;

type AcpCompanion = DevinAcpCompanion;

/// Build one index for the optional ACP companion files used by this session.
fn acp_companion_index(db_path: &Path, heads: &[Row]) -> (AcpIndex, bool) {
    let keys = heads
        .iter()
        .filter_map(|head| {
            Some((
                head.get("session_id")?.clone(),
                head.get("tool_call_id")?.clone(),
                head.get("child_agent_id")?.clone(),
            ))
        })
        .collect();
    let (records, partial) = devin_acp_companion_records(db_path, &keys);
    (
        records
            .into_iter()
            .fold(HashMap::new(), |mut index, companion| {
                index
                    .entry((
                        companion.parent_session_id.clone(),
                        companion.call_id.clone(),
                        companion.child_id.clone(),
                    ))
                    .or_default()
                    .push(companion);
                index
            }),
        partial,
    )
}

/// ACP is a child companion, not a source. Missing companions are valid
/// because ACP is optional; an applicable, malformed, or conflicting
/// companion makes that relation unusable.
fn acp_relation_is_consistent(
    index: &AcpIndex,
    parent_session_id: &str,
    call_id: &str,
    child_id: &str,
    model: &str,
) -> Option<()> {
    let Some(matched) = index.get(&(
        parent_session_id.to_owned(),
        call_id.to_owned(),
        child_id.to_owned(),
    )) else {
        return Some(());
    };
    if matched.len() > 1 {
        return None;
    }
    let companion = matched.first()?;
    if companion
        .status
        .as_deref()
        .is_some_and(|status| matches!(status, "interrupted" | "failed" | "cancelled"))
        || companion
            .model
            .as_deref()
            .is_some_and(|value| value != model)
    {
        return None;
    }
    Some(())
}

fn usage(value: &Value) -> Usage {
    let metric = value.pointer("/metadata/metrics");
    let number = |name| {
        metric
            .and_then(|metric| metric.get(name))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Usage {
        input_tokens: number("input_tokens"),
        output_tokens: number("output_tokens"),
        cache_read_tokens: 0,
        cache_creation_tokens: 0,
        cache_creation_1h_tokens: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::interface::{RecordSink, SessionSummary};
    use std::fs;
    use tempfile::TempDir;

    const FIXTURE: &str = include_str!("../../../tests/fixtures/devin_local_migration_17.sql");

    #[derive(Default)]
    struct Capture {
        observations: Vec<EvidenceObservation>,
        summary: Option<SessionSummary>,
    }

    impl RecordSink for Capture {
        fn record(&mut self, record: NormalizedRecord) {
            if let NormalizedRecord::Observation(observation) = record {
                self.observations.push(*observation);
            }
        }

        fn finish(&mut self, summary: SessionSummary) {
            self.summary = Some(summary);
        }
    }

    fn input(db_path: &Path) -> SessionInput {
        SessionInput {
            agent: "windsurf".to_owned(),
            session_id: "root".to_owned(),
            source: RawSource::Sqlite(db_path.to_owned()),
            source_format: SourceFormat::DevinLocalSqlite,
            fork_parent_session_id: None,
        }
    }

    fn fixture_db() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sessions.db");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(FIXTURE).unwrap();
        drop(connection);
        (dir, path)
    }

    #[test]
    fn migration_17_fixture_emits_one_exact_subagent_relation() {
        let (_dir, path) = fixture_db();
        let mut capture = Capture::default();
        DevinLocalSessionReader
            .visit(&input(&path), &mut capture)
            .unwrap();
        assert!(matches!(
            capture.observations.as_slice(),
            [EvidenceObservation::SubagentSpawn {
                parent_call_id: Some(call),
                parent_model: Some(parent),
                child_model: Some(child),
                provenance: RelationProvenance::TaskToolUse,
                ..
            }] if call == "call-1" && parent == "claude-opus-4-6" && child == "claude-opus-4-7-20260115"
        ));
        assert_eq!(capture.summary.unwrap().coverage_gaps, Vec::new());
    }

    #[test]
    fn fingerprint_changes_when_reader_content_changes_without_row_growth() {
        let (_dir, path) = fixture_db();
        let connection = Connection::open(&path).unwrap();
        let before = session_fingerprint(&connection, &path, "root");
        connection
            .execute(
                "UPDATE sessions SET model = 'child-model-2' WHERE id = 'child'",
                [],
            )
            .unwrap();
        let after_child_model = session_fingerprint(&connection, &path, "root");
        assert_ne!(before, after_child_model);
        connection
            .execute(
                "UPDATE message_nodes SET raw_message = raw_message || ' ' WHERE session_id = 'child' AND node_id = 3",
                [],
            )
            .unwrap();
        let after_child_message = session_fingerprint(&connection, &path, "root");
        assert_ne!(after_child_model, after_child_message);
        let companion_dir = path.parent().unwrap().join("acp-messages");
        fs::create_dir_all(&companion_dir).unwrap();
        fs::write(
            companion_dir.join("child.ndjson"),
            "{\"schema\":6,\"parentSessionId\":\"root\",\"toolCallId\":\"call-1\",\"childAgentId\":\"child\",\"model\":\"child-model-2\",\"status\":\"completed\"}\n",
        )
        .unwrap();
        let after_companion = session_fingerprint(&connection, &path, "root");
        assert_ne!(after_child_message, after_companion);
        assert_eq!(
            before.map(|(_, rows)| rows),
            after_companion.map(|(_, rows)| rows)
        );
    }

    #[test]
    fn missing_duplicate_conflicting_and_interrupted_relations_are_partial() {
        for change in [
            "DELETE FROM subagent_heads",
            "INSERT INTO subagent_heads VALUES ('root', 'call-1', 'child', 3); INSERT INTO subagent_heads VALUES ('root', 'call-1', 'child', 3)",
            "UPDATE subagent_heads SET child_agent_id = 'other'",
            "UPDATE tool_call_state SET state = '{\"status\":\"interrupted\"}'",
        ] {
            let (_dir, path) = fixture_db();
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch(change).unwrap();
            drop(connection);
            let mut capture = Capture::default();
            DevinLocalSessionReader
                .visit(&input(&path), &mut capture)
                .unwrap();
            assert_eq!(
                capture.summary.unwrap().coverage_gaps,
                vec![PartialReason::AttributionIncomplete],
                "relation case: {change}"
            );
        }
    }

    #[test]
    fn acp_schema_6_is_used_only_as_a_joined_child_companion() {
        let (_dir, path) = fixture_db();
        let companion_dir = path.parent().unwrap().join("acp-messages");
        fs::create_dir_all(&companion_dir).unwrap();
        fs::write(
            companion_dir.join("child.ndjson"),
            "{\"schema\":6,\"parentSessionId\":\"root\",\"toolCallId\":\"call-1\",\"childAgentId\":\"child\",\"model\":\"claude-opus-4-7-20260115\",\"status\":\"completed\"}\n",
        )
        .unwrap();
        let mut capture = Capture::default();
        DevinLocalSessionReader
            .visit(&input(&path), &mut capture)
            .unwrap();
        assert_eq!(capture.observations.len(), 1);

        fs::write(
            companion_dir.join("child.ndjson"),
            "{\"schema\":6,\"parentSessionId\":\"root\",\"toolCallId\":\"call-1\",\"childAgentId\":\"child\",\"model\":\"other-model\",\"status\":\"completed\"}\n",
        )
        .unwrap();
        let mut capture = Capture::default();
        DevinLocalSessionReader
            .visit(&input(&path), &mut capture)
            .unwrap();
        assert!(capture.observations.is_empty());
        assert_eq!(
            capture.summary.unwrap().coverage_gaps,
            vec![PartialReason::AttributionIncomplete]
        );
    }
}

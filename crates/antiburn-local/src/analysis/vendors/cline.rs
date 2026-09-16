//! Cline messages-contract-v1 bundle reader.
//!
//! The accepted bundle is a terminal `sessions.db` row, its root manifest, and
//! the canonical root messages artifact. Child rows must point directly at the
//! root and at their canonical artifacts. Message text and tool payloads are
//! deliberately discarded.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;

use crate::analysis::framing::{MAX_RECORD_BYTES, PartialReason};
use crate::analysis::interface::{
    EvidenceObservation, NormalizedRecord, RawSource, RecordSink, RelationProvenance,
    SessionCollector, SessionInput, SessionReader, SessionSummary, VisitOutcome,
};
use crate::analysis::model::{NormalizedEvent, Role, ToolCall, Usage};
use crate::analysis::records::parse_ts;
use crate::analysis::source_validity::{AppendOnlyGuarantee, PinnedSource, SourceClaim};
use crate::analysis::{SourceCapabilities, SourceFormat};

const TERMINAL_STATUSES: &[&str] = &["completed", "failed", "cancelled"];
const SESSION_COLUMNS: &[&str] = &[
    "session_id",
    "status",
    "model",
    "parent_session_id",
    "is_subagent",
    "messages_path",
];

pub struct ClineSessionReader;

impl SessionReader for ClineSessionReader {
    fn agent(&self) -> &'static str {
        "cline"
    }

    fn capabilities(&self, input: &SessionInput) -> SourceCapabilities {
        if input.source_format != SourceFormat::ClineMessagesContractV1 {
            return SourceCapabilities::uncharacterized(input.source_format);
        }
        SourceCapabilities::cline_messages_contract_v1()
    }

    fn normalize(
        &self,
        input: &SessionInput,
    ) -> anyhow::Result<crate::analysis::NormalizedSession> {
        let mut collector = SessionCollector::new(input.agent.clone(), input.session_id.clone());
        self.visit(input, &mut collector)?;
        collector.into_session()
    }

    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let RawSource::ClineBundle {
            db_path,
            manifest_path,
            messages_path,
        } = &input.source
        else {
            anyhow::bail!("Cline messages-contract-v1 requires a Cline bundle");
        };
        if input.source_format != SourceFormat::ClineMessagesContractV1 {
            anyhow::bail!("Cline bundle requires ClineMessagesContractV1 admission");
        }
        let result = visit_bundle(
            db_path,
            manifest_path,
            messages_path,
            &input.session_id,
            sink,
        );
        match result {
            Ok(summary) => {
                sink.finish(summary);
                Ok(VisitOutcome::Unvalidated)
            }
            Err(_) => {
                sink.record(NormalizedRecord::Unusable(PartialReason::MalformedRecord));
                sink.finish(SessionSummary {
                    coverage_gaps: vec![PartialReason::MalformedRecord],
                    ..SessionSummary::default()
                });
                Ok(VisitOutcome::Unvalidated)
            }
        }
    }

    fn visit_claimed(
        &self,
        input: &SessionInput,
        claim: &SourceClaim,
        _guarantee: AppendOnlyGuarantee,
        _cancel: &dyn Fn() -> bool,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let RawSource::ClineBundle { manifest_path, .. } = &input.source else {
            anyhow::bail!("a claimed Cline source must be a Cline bundle");
        };
        let mut pinned = match PinnedSource::open(manifest_path, claim.clone())? {
            Ok(pinned) => pinned,
            Err(reason) => return Ok(VisitOutcome::SourceChanged(reason)),
        };
        let outcome = self.visit(input, sink)?;
        match pinned.recheck_full()? {
            Some(reason) => Ok(VisitOutcome::SourceChanged(reason)),
            None => Ok(outcome),
        }
    }
}

fn visit_bundle(
    db_path: &Path,
    manifest_path: &Path,
    messages_path: &Path,
    session_id: &str,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<SessionSummary> {
    let manifest = read_json(manifest_path)?;
    validate_manifest(&manifest, session_id, messages_path)?;
    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.execute_batch("BEGIN")?;
    validate_schema(&conn)?;
    let root =
        session_row(&conn, session_id)?.ok_or_else(|| anyhow::anyhow!("missing root row"))?;
    validate_root(&root, session_id, messages_path, &manifest)?;
    let mut summary = parse_messages(messages_path, session_id, sink)?;
    summary.model = Some(root.model.clone());

    let mut children = conn.prepare(
        "SELECT session_id, status, model, parent_session_id, is_subagent, messages_path
           FROM sessions WHERE parent_session_id = ?1",
    )?;
    let rows = children.query_map(params![session_id], row_from_sql)?;
    let mut child_ids = HashSet::new();
    for row in rows {
        let child = row?;
        if !child_ids.insert(child.session_id.clone()) {
            anyhow::bail!("duplicate child session row");
        }
        validate_child(
            &child,
            session_id,
            manifest_path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("missing session directory"))?,
        )?;
        sink.record(NormalizedRecord::Observation(Box::new(
            EvidenceObservation::SubagentSpawn {
                ts_ms: None,
                parent_model: Some(root.model.clone()),
                parent_call_id: None,
                child_model: Some(child.model.clone()),
                provenance: RelationProvenance::SessionParentLink,
            },
        )));
        parse_messages(&child.messages_path, &child.session_id, sink)?;
    }
    conn.execute_batch("COMMIT")?;
    Ok(summary)
}

#[derive(Debug)]
struct SessionRow {
    session_id: String,
    status: String,
    model: String,
    parent_session_id: Option<String>,
    is_subagent: i64,
    messages_path: PathBuf,
}

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        session_id: row.get(0)?,
        status: row.get(1)?,
        model: row.get(2)?,
        parent_session_id: row.get(3)?,
        is_subagent: row.get(4)?,
        messages_path: PathBuf::from(row.get::<_, String>(5)?),
    })
}

fn session_row(conn: &Connection, session_id: &str) -> anyhow::Result<Option<SessionRow>> {
    let mut statement = conn.prepare(
        "SELECT session_id, status, model, parent_session_id, is_subagent, messages_path FROM sessions WHERE session_id = ?1",
    )?;
    let mut rows = statement.query(params![session_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let session = row_from_sql(row)?;
    if rows.next()?.is_some() {
        anyhow::bail!("duplicate root session row");
    }
    Ok(Some(session))
}

fn validate_schema(conn: &Connection) -> anyhow::Result<()> {
    let mut statement = conn.prepare("PRAGMA table_info(sessions)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if SESSION_COLUMNS
        .iter()
        .any(|required| !columns.iter().any(|column| column == required))
    {
        anyhow::bail!("unsupported sessions schema");
    }
    Ok(())
}

fn validate_root(
    row: &SessionRow,
    session_id: &str,
    messages_path: &Path,
    manifest: &Value,
) -> anyhow::Result<()> {
    if row.session_id != session_id
        || row.parent_session_id.is_some()
        || row.is_subagent != 0
        || !TERMINAL_STATUSES.contains(&row.status.as_str())
        || row.messages_path != messages_path
        || manifest.get("status").and_then(Value::as_str) != Some(&row.status)
        || manifest.get("model").and_then(Value::as_str) != Some(&row.model)
    {
        anyhow::bail!("root row does not match the manifest and artifact");
    }
    Ok(())
}

fn validate_child(row: &SessionRow, root_id: &str, directory: &Path) -> anyhow::Result<()> {
    let expected = directory.join(format!("{}.messages.json", row.session_id));
    if row.parent_session_id.as_deref() != Some(root_id)
        || row.is_subagent != 1
        || !TERMINAL_STATUSES.contains(&row.status.as_str())
        || row.messages_path != expected
    {
        anyhow::bail!("child row does not match the root artifact contract");
    }
    Ok(())
}

fn validate_manifest(
    manifest: &Value,
    session_id: &str,
    messages_path: &Path,
) -> anyhow::Result<()> {
    if manifest.get("version").and_then(Value::as_u64) != Some(1)
        || manifest.get("session_id").and_then(Value::as_str) != Some(session_id)
        || manifest
            .get("messages_path")
            .and_then(Value::as_str)
            .map(Path::new)
            != Some(messages_path)
    {
        anyhow::bail!("invalid root manifest");
    }
    Ok(())
}

fn parse_messages(
    path: &Path,
    session_id: &str,
    sink: &mut dyn RecordSink,
) -> anyhow::Result<SessionSummary> {
    let value = read_json(path)?;
    if value.get("version").and_then(Value::as_u64) != Some(1)
        || value.get("sessionId").and_then(Value::as_str) != Some(session_id)
    {
        anyhow::bail!("invalid messages artifact");
    }
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("missing messages"))?;
    let mut summary = SessionSummary::default();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing role"))?;
        let content = message
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("missing content"))?;
        if !matches!(role, "user" | "assistant") {
            anyhow::bail!("unsupported message role");
        }
        let mut tools = Vec::new();
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty() && name.len() <= 256)
                    .ok_or_else(|| anyhow::anyhow!("invalid tool identity"))?;
                tools.push(ToolCall::new(name));
            }
        }
        let metrics = message.get("metrics");
        if metrics.is_some() && role != "assistant" {
            anyhow::bail!("metrics on non-assistant message");
        }
        if metrics.is_none() && tools.is_empty() {
            continue;
        }
        let model = message
            .pointer("/modelInfo/id")
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing model identity"))?;
        if message
            .pointer("/modelInfo/provider")
            .and_then(Value::as_str)
            .filter(|provider| !provider.is_empty())
            .is_none()
        {
            anyhow::bail!("missing model provider");
        }
        let ts = message
            .get("ts")
            .and_then(parse_ts)
            .ok_or_else(|| anyhow::anyhow!("missing assistant timestamp"))?;
        let usage = if let Some(metrics) = metrics {
            Usage {
                input_tokens: metric(metrics, "inputTokens")?,
                output_tokens: metric(metrics, "outputTokens")?,
                cache_read_tokens: metric(metrics, "cacheReadTokens")?,
                cache_creation_tokens: metric(metrics, "cacheWriteTokens")?,
                cache_creation_1h_tokens: 0,
            }
        } else {
            Usage::default()
        };
        summary.started_at_ms.get_or_insert(ts);
        sink.record(NormalizedRecord::MetricsEvent(Box::new(NormalizedEvent {
            ts_ms: Some(ts),
            usage_ts_ms: None,
            role: Role::Assistant,
            source: Default::default(),
            usage,
            tools,
            model: Some(model.to_owned()),
            provider: None,
            api: None,
            thinking_mode: None,
            speed: None,
            has_thinking: false,
            message_id: None,
            is_compaction_boundary: false,
            compaction_trigger: None,
            compaction_pre_tokens: None,
            compaction_post_tokens: None,
            wrapper_tool: None,
            may_resolve_late_tool: false,
            late_tool_candidate_is_builtin: false,
            uuid: None,
            parent_uuid: None,
            logical_parent_uuid: None,
            thread_id: None,
        })));
    }
    Ok(summary)
}

fn metric(metrics: &Value, key: &str) -> anyhow::Result<u64> {
    metrics
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("invalid metric"))
}

fn read_json(path: &Path) -> anyhow::Result<Value> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_RECORD_BYTES {
        anyhow::bail!("artifact is too large");
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::interface::{RecordCoverage, SessionCollector};
    use crate::discovery::source_version::head_hash_of;
    use crate::discovery::{FingerprintInputs, SourceStat};
    use tempfile::TempDir;

    const MANIFEST: &str =
        include_str!("../../../tests/fixtures/cline_messages_contract_v1/root.json");
    const ROOT_MESSAGES: &str =
        include_str!("../../../tests/fixtures/cline_messages_contract_v1/root.messages.json");
    const CHILD_MESSAGES: &str =
        include_str!("../../../tests/fixtures/cline_messages_contract_v1/child.messages.json");

    fn bundle(include_child: bool) -> (TempDir, SessionInput) {
        let temp = TempDir::new().unwrap();
        let sessions = temp.path().join("sessions");
        let directory = sessions.join("root_1");
        std::fs::create_dir_all(&directory).unwrap();
        let manifest_path = directory.join("root_1.json");
        let messages_path = directory.join("root_1.messages.json");
        std::fs::write(&messages_path, ROOT_MESSAGES).unwrap();
        std::fs::write(
            &manifest_path,
            MANIFEST.replace(
                "\"PLACEHOLDER\"",
                &serde_json::to_string(&messages_path.to_string_lossy()).unwrap(),
            ),
        )
        .unwrap();
        if include_child {
            std::fs::write(directory.join("child_1.messages.json"), CHILD_MESSAGES).unwrap();
        }
        let db_path = sessions.join("sessions.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode = WAL;").unwrap();
        conn.execute_batch("CREATE TABLE sessions (session_id TEXT, status TEXT, model TEXT, parent_session_id TEXT, is_subagent INTEGER, messages_path TEXT)").unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES (?1, 'completed', 'model-root', NULL, 0, ?2)",
            params!["root_1", messages_path.to_string_lossy()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions VALUES ('child_1', 'completed', 'model-child', 'root_1', 1, ?1)",
            params![directory.join("child_1.messages.json").to_string_lossy()],
        )
        .unwrap();
        (
            temp,
            SessionInput {
                agent: "cline".to_owned(),
                session_id: "root_1".to_owned(),
                source: RawSource::ClineBundle {
                    db_path,
                    manifest_path,
                    messages_path,
                },
                source_format: SourceFormat::ClineMessagesContractV1,
                fork_parent_session_id: None,
            },
        )
    }

    #[test]
    fn terminal_bundle_retains_only_metrics_tools_and_child_model() {
        let (_temp, input) = bundle(true);
        let mut sink = SessionCollector::new("cline", "root_1");
        ClineSessionReader.visit(&input, &mut sink).unwrap();
        let session = sink.into_session().unwrap();
        assert_eq!(session.events.len(), 3);
        assert_eq!(session.events[1].usage.input_tokens, 10);
        assert_eq!(session.events[0].tools[0].name, "Agent");
        assert!(session.events.iter().all(|event| event.provider.is_none()));
    }

    #[test]
    fn missing_child_artifact_fails_closed() {
        let (_temp, input) = bundle(false);
        let mut sink = SessionCollector::new("cline", "root_1");
        ClineSessionReader.visit(&input, &mut sink).unwrap();
        assert_eq!(sink.coverage(), RecordCoverage::Partial);
    }

    #[test]
    fn unsupported_messages_version_fails_closed() {
        let (_temp, input) = bundle(true);
        let RawSource::ClineBundle { messages_path, .. } = &input.source else {
            unreachable!()
        };
        std::fs::write(
            messages_path,
            ROOT_MESSAGES.replacen("\"version\":1", "\"version\":2", 1),
        )
        .unwrap();
        let mut sink = SessionCollector::new("cline", "root_1");
        ClineSessionReader.visit(&input, &mut sink).unwrap();
        assert_eq!(sink.coverage(), RecordCoverage::Partial);
    }

    #[test]
    fn malformed_manifest_fails_closed() {
        let (_temp, input) = bundle(true);
        let RawSource::ClineBundle { manifest_path, .. } = &input.source else {
            unreachable!()
        };
        std::fs::write(manifest_path, "{").unwrap();
        let mut sink = SessionCollector::new("cline", "root_1");
        ClineSessionReader.visit(&input, &mut sink).unwrap();
        assert_eq!(sink.coverage(), RecordCoverage::Partial);
    }

    #[test]
    fn nonterminal_root_fails_closed() {
        let (_temp, input) = bundle(true);
        let RawSource::ClineBundle { db_path, .. } = &input.source else {
            unreachable!()
        };
        Connection::open(db_path)
            .unwrap()
            .execute(
                "UPDATE sessions SET status = 'running' WHERE session_id = 'root_1'",
                [],
            )
            .unwrap();
        let mut sink = SessionCollector::new("cline", "root_1");
        ClineSessionReader.visit(&input, &mut sink).unwrap();
        assert_eq!(sink.coverage(), RecordCoverage::Partial);
    }

    #[test]
    fn duplicate_root_row_fails_closed() {
        let (_temp, input) = bundle(true);
        let RawSource::ClineBundle {
            db_path,
            messages_path,
            ..
        } = &input.source
        else {
            unreachable!()
        };
        Connection::open(db_path)
            .unwrap()
            .execute(
                "INSERT INTO sessions VALUES ('root_1', 'completed', 'model-root', NULL, 0, ?1)",
                params![messages_path.to_string_lossy()],
            )
            .unwrap();
        let mut sink = SessionCollector::new("cline", "root_1");
        ClineSessionReader.visit(&input, &mut sink).unwrap();
        assert_eq!(sink.coverage(), RecordCoverage::Partial);
    }

    #[test]
    fn changed_manifest_rejects_claimed_bundle() {
        let (_temp, input) = bundle(true);
        let RawSource::ClineBundle { manifest_path, .. } = &input.source else {
            unreachable!()
        };
        let file = File::open(manifest_path).unwrap();
        let stat = SourceStat::from_open_std_file(&file).unwrap();
        let bytes = std::fs::read(manifest_path).unwrap();
        let claim = SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
            stat,
            head_hash: Some(head_hash_of(&bytes)),
        });
        std::fs::write(manifest_path, "{}").unwrap();
        let mut sink = SessionCollector::new("cline", "root_1");
        assert!(matches!(
            ClineSessionReader
                .visit_claimed(
                    &input,
                    &claim,
                    AppendOnlyGuarantee::Absent,
                    &|| false,
                    &mut sink
                )
                .unwrap(),
            VisitOutcome::SourceChanged(_)
        ));
    }
}

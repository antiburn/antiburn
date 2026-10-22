//! Bound one OpenCode V2 live segment to a read-only SQLite baseline.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
};

use anyhow::{Context, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{
    NativeToolOutcome, OpenCodeV2SnapshotError, SourceFormat, ToolResultObservation,
    ToolResultPosition, ToolResultStatus,
};

pub const MAX_CAPTURE_SESSIONS: usize = 256;
pub const MAX_CAPTURE_EVENTS: usize = 4096;
pub const MAX_CAPTURE_EVENT_BYTES: usize = 256 * 1024;
pub const MAX_CAPTURE_BUFFER_BYTES: usize = 16 * 1024 * 1024;
const MAX_CAPTURE_ID_BYTES: usize = 1024;
pub const MAX_RECONCILIATION_BINDINGS: usize = 4096;
pub const MAX_RECONCILIATION_BYTES: usize = 16 * 1024 * 1024;

/// Exact saved bindings checked in the same source snapshot as the watermark.
#[derive(Debug, Clone, Default)]
pub struct CaptureReconciliation {
    pub baseline: HashMap<String, Option<u64>>,
    pub bindings: Vec<CaptureBindingReconciliation>,
    pub invalid_scopes: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct CaptureBindingReconciliation {
    pub observation: ToolResultObservation,
    /// False also covers malformed or unavailable current evidence.
    pub retained: bool,
}

/// Check exact saved identities. A bounded transcript page cannot prove absence.
pub fn read_capture_reconciliation_for_scope(
    path: &Path,
    source_format: SourceFormat,
    scopes: &[(String, String)],
    targets: &[ToolResultObservation],
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<CaptureReconciliation> {
    if source_format != SourceFormat::OpenCodeSqliteV2 {
        return Err(OpenCodeV2SnapshotError::UnsupportedSource.into());
    }
    if scopes.len() > MAX_CAPTURE_SESSIONS || targets.len() > MAX_RECONCILIATION_BINDINGS {
        return Err(OpenCodeV2SnapshotError::SourceLimit.into());
    }
    let mut ids = HashSet::new();
    ensure!(
        scopes.iter().all(|(id, cwd)| {
            valid_id(id)
                && ids.insert(id.as_str())
                && !cwd.is_empty()
                && cwd.len() <= 4096
                && !cwd.contains('\0')
        }),
        OpenCodeV2SnapshotError::ScopeChanged
    );
    let mut bytes = 0usize;
    for target in targets {
        ensure!(
            target.source_format == source_format
                && ids.contains(target.session_id.as_str())
                && target.record_id.as_deref().is_some_and(valid_id)
                && target.call_id.as_deref().is_some_and(valid_id),
            OpenCodeV2SnapshotError::ScopeChanged
        );
        bytes += encoded_digest_and_size(target, MAX_RECONCILIATION_BYTES - bytes)
            .ok_or(OpenCodeV2SnapshotError::SourceLimit)?
            .0;
    }
    ensure!(!cancel(), OpenCodeV2SnapshotError::Cancelled);
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("opening OpenCode V2 reconciliation")?;
    connection.execute_batch("BEGIN")?;
    ensure!(
        super::opencode_v2_schema::is_supported(&connection)?
            && super::opencode_v2_schema::has_event_sequence_contract(&connection)?,
        OpenCodeV2SnapshotError::UnsupportedSchema
    );
    ensure!(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('session_v2') WHERE name = 'fork_session_id')",
        [], |row| row.get::<_, bool>(0),
    )?, OpenCodeV2SnapshotError::UnsupportedSchema);
    let mut reconciliation = CaptureReconciliation::default();
    for (id, cwd) in scopes {
        ensure!(!cancel(), OpenCodeV2SnapshotError::Cancelled);
        let row = connection
            .query_row(
                "SELECT e.seq FROM session_v2 s LEFT JOIN event_sequence e ON e.aggregate_id = s.id
              WHERE s.id = ?1 AND s.directory = ?2 AND s.parent_id IS NULL
                AND s.fork_session_id IS NULL",
                params![id, cwd],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?;
        let sequence = row
            .flatten()
            .and_then(|sequence| u64::try_from(sequence).ok());
        if row.is_none() || row.is_some_and(|row| row.is_some() && sequence.is_none()) {
            reconciliation.invalid_scopes.insert(id.clone());
        }
        reconciliation.baseline.insert(id.clone(), sequence);
    }
    let mut messages = MessageCache::default();
    for target in targets {
        ensure!(!cancel(), OpenCodeV2SnapshotError::Cancelled);
        reconciliation.bindings.push(CaptureBindingReconciliation {
            retained: !reconciliation.invalid_scopes.contains(&target.session_id)
                && matches!(target.position, ToolResultPosition::OpenCodeV2Event { sequence, .. }
                if reconciliation.baseline[&target.session_id].is_some_and(|boundary| sequence <= boundary))
                && retained_binding_matches(&connection, target, &mut messages, cancel)?,
            observation: target.clone(),
        });
    }
    ensure!(!cancel(), OpenCodeV2SnapshotError::Cancelled);
    connection.execute_batch("COMMIT")?;
    Ok(reconciliation)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_CAPTURE_ID_BYTES && !id.contains('\0')
}

#[derive(Default)]
struct MessageCache {
    rows: HashMap<(String, String), Option<(String, Value)>>,
    bytes: usize,
}

fn exact_message<'a>(
    connection: &Connection,
    session_id: &str,
    id: &str,
    cache: &'a mut MessageCache,
) -> anyhow::Result<Option<&'a (String, Value)>> {
    let key = (session_id.to_owned(), id.to_owned());
    if !cache.rows.contains_key(&key) {
        let row = connection
            .query_row(
                "SELECT CASE WHEN length(CAST(type AS BLOB)) <= 64 THEN type END,
                     CASE WHEN length(CAST(data AS BLOB)) <= ?3 THEN data END
            FROM session_message WHERE session_id = ?1 AND id = ?2",
                params![
                    session_id,
                    id,
                    MAX_CAPTURE_EVENT_BYTES.min(MAX_RECONCILIATION_BYTES - cache.bytes)
                ],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()?;
        let value = match row {
            Some((Some(kind), Some(data))) => {
                cache.bytes += data.len();
                serde_json::from_str(&data).ok().map(|value| (kind, value))
            }
            _ => None,
        };
        cache.rows.insert(key.clone(), value);
    }
    Ok(cache.rows.get(&key).and_then(Option::as_ref))
}

fn retained_binding_matches(
    connection: &Connection,
    target: &ToolResultObservation,
    messages: &mut MessageCache,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<bool> {
    let Some((kind, message)) = exact_message(
        connection,
        &target.session_id,
        target
            .record_id
            .as_deref()
            .context("missing V2 message identity")?,
        messages,
    )?
    else {
        return Ok(false);
    };
    if kind != "assistant" {
        return Ok(false);
    }
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return Ok(false);
    };
    let mut tools = content.iter().filter(|tool| {
        tool.get("type").and_then(Value::as_str) == Some("tool")
            && tool.get("id").and_then(Value::as_str) == target.call_id.as_deref()
    });
    let Some(tool) = tools.next() else {
        return Ok(false);
    };
    if tools.next().is_some() {
        return Ok(false);
    }
    let Some(name) = target.tool_name.as_deref() else {
        return Ok(false);
    };
    let state = tool.get("state");
    let status = match state
        .and_then(|state| state.get("status"))
        .and_then(Value::as_str)
    {
        Some("completed") => ToolResultStatus::Success,
        Some("error") => ToolResultStatus::Error,
        _ => return Ok(false),
    };
    let executed = tool.get("executed").and_then(Value::as_bool);
    if tool
        .get("executed")
        .is_some_and(|value| !value.is_boolean())
        || tool.get("name").and_then(Value::as_str) != target.tool_name.as_deref()
        || state.and_then(|state| state.get("input")) != target.arguments.as_ref()
        || status != target.status
        || executed != target.provider_executed
        || super::opencode_v2_events::decode_content(state.and_then(|state| state.get("content")))
            != target.content
        || NativeToolOutcome::opencode_v2_tool(
            name,
            state.and_then(|state| state.get("metadata")),
            executed,
            status == ToolResultStatus::Error,
        ) != target.outcome
    {
        return Ok(false);
    }
    if let Some(context) = &target.context {
        for user in &context.user_messages {
            ensure!(!cancel(), OpenCodeV2SnapshotError::Cancelled);
            let Some((kind, current)) =
                exact_message(connection, &target.session_id, &user.message_id, messages)?
            else {
                return Ok(false);
            };
            if kind != "user"
                || user.parts.len() != 1
                || current.get("text").and_then(Value::as_str) != Some(user.parts[0].text.as_str())
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitingSubscription,
    Hydrating,
    Live,
    Gap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundaryKind {
    Seed,
    Live,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CapturedEvent {
    pub session_id: String,
    pub sequence: u64,
    pub kind: BoundaryKind,
    pub event: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureGap {
    SubscriptionNotConfirmed,
    InvalidBoundary,
    InvalidEvent,
    CapacityExceeded,
    ContinuityLost,
}

/// A server subscription and its read-only snapshot boundary.
pub struct OpenCodeV2CaptureEpoch {
    id: Uuid,
    phase: Phase,
    admitted_sessions: HashSet<String>,
    boundaries: HashMap<String, Option<u64>>,
    buffered: Vec<Value>,
    buffered_bytes: usize,
    seen_ids: HashMap<String, (String, [u8; 32])>,
    seen_id_order: VecDeque<String>,
    last_sequences: HashMap<String, u64>,
    gapped_sessions: HashSet<String>,
}

impl OpenCodeV2CaptureEpoch {
    pub fn new(admitted_sessions: Vec<String>) -> Result<Self, CaptureGap> {
        let unique_sessions: HashSet<String> = admitted_sessions.iter().cloned().collect();
        if admitted_sessions.is_empty()
            || admitted_sessions.len() > MAX_CAPTURE_SESSIONS
            || unique_sessions.len() != admitted_sessions.len()
            || admitted_sessions
                .iter()
                .any(|id| id.is_empty() || id.len() > MAX_CAPTURE_ID_BYTES || id.contains('\0'))
        {
            return Err(CaptureGap::InvalidBoundary);
        }
        Ok(Self {
            id: Uuid::new_v4(),
            phase: Phase::AwaitingSubscription,
            admitted_sessions: unique_sessions,
            boundaries: HashMap::new(),
            buffered: Vec::new(),
            buffered_bytes: 0,
            seen_ids: HashMap::new(),
            seen_id_order: VecDeque::new(),
            last_sequences: HashMap::new(),
            gapped_sessions: HashSet::new(),
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }

    pub fn gapped_sessions(&self) -> impl Iterator<Item = &str> {
        self.gapped_sessions.iter().map(String::as_str)
    }

    pub fn mark_session_gap(&mut self, session_id: &str) {
        if self.admitted_sessions.contains(session_id) {
            self.gap_session(session_id);
        }
    }

    /// Add or replace hydrated sessions without changing healthy session state.
    pub fn resume_sessions(&mut self, recovered: Self) -> Result<(), CaptureGap> {
        if self.phase != Phase::Live
            || recovered.phase != Phase::Live
            || !recovered.gapped_sessions.is_empty()
        {
            return Err(CaptureGap::InvalidBoundary);
        }
        if self
            .admitted_sessions
            .union(&recovered.admitted_sessions)
            .count()
            > MAX_CAPTURE_SESSIONS
        {
            return Err(CaptureGap::CapacityExceeded);
        }
        if recovered.seen_ids.keys().any(|id| {
            self.seen_ids
                .get(id)
                .is_some_and(|(session, _)| !recovered.admitted_sessions.contains(session))
        }) {
            return Err(CaptureGap::InvalidEvent);
        }
        for session in &recovered.admitted_sessions {
            self.gapped_sessions.remove(session);
            self.last_sequences.remove(session);
            self.seen_ids.retain(|_, (previous, _)| previous != session);
        }
        self.seen_id_order
            .retain(|id| self.seen_ids.contains_key(id));
        self.admitted_sessions.extend(recovered.admitted_sessions);
        self.boundaries.extend(recovered.boundaries);
        self.last_sequences.extend(recovered.last_sequences);
        for id in recovered.seen_id_order {
            if let Some((session, digest)) = recovered.seen_ids.get(&id) {
                self.remember_seen_id(id, session.clone(), *digest);
            }
        }
        Ok(())
    }

    /// Confirm the feed's `server.connected` marker before snapshot work starts.
    pub fn confirm_subscription(&mut self) -> Result<(), CaptureGap> {
        if self.phase != Phase::AwaitingSubscription {
            return Err(if self.phase == Phase::Gap {
                CaptureGap::ContinuityLost
            } else {
                CaptureGap::SubscriptionNotConfirmed
            });
        }
        self.phase = Phase::Hydrating;
        Ok(())
    }

    /// Save the per-session sequence watermark from one SQLite read snapshot.
    /// Events buffered before this call are classified against these watermarks.
    pub fn finish_hydration(
        &mut self,
        boundaries: HashMap<String, Option<u64>>,
    ) -> Result<Vec<CapturedEvent>, CaptureGap> {
        if self.phase != Phase::Hydrating {
            return Err(if self.phase == Phase::Gap {
                CaptureGap::ContinuityLost
            } else {
                CaptureGap::SubscriptionNotConfirmed
            });
        }
        if boundaries.len() != self.admitted_sessions.len()
            || boundaries.keys().any(|id| {
                id.is_empty()
                    || id.len() > MAX_CAPTURE_ID_BYTES
                    || id.contains('\0')
                    || !self.admitted_sessions.contains(id)
            })
        {
            self.mark_gap();
            return Err(CaptureGap::InvalidBoundary);
        }
        self.boundaries = boundaries;
        let buffered = std::mem::take(&mut self.buffered);
        self.buffered_bytes = 0;
        self.seen_ids.clear();
        self.seen_id_order.clear();
        self.last_sequences.clear();
        let mut classified = Vec::with_capacity(buffered.len());
        for event in buffered {
            if let Some(event) = self.classify(event)? {
                classified.push(event);
            }
        }
        self.phase = Phase::Live;
        Ok(classified)
    }

    /// Buffer events until the snapshot boundary is ready, then classify each
    /// event by its aggregate sequence. Public sequence jumps are allowed.
    pub fn observe(&mut self, event: Value) -> Result<Option<CapturedEvent>, CaptureGap> {
        match self.phase {
            Phase::AwaitingSubscription => Err(CaptureGap::SubscriptionNotConfirmed),
            Phase::Hydrating => {
                let Some((event_id, session_id, sequence)) = self.event_identity(&event)? else {
                    return Ok(None);
                };
                if self.gapped_sessions.contains(session_id) {
                    return Ok(None);
                }
                let Some((bytes, digest)) = event_digest_and_size(&event) else {
                    self.gap_session(session_id);
                    return Ok(None);
                };
                if let Some((previous_session, previous_digest)) =
                    self.seen_ids.get(event_id).cloned()
                {
                    if previous_session != session_id {
                        self.gap_session(&previous_session);
                        self.gap_session(session_id);
                    } else if previous_digest != digest {
                        self.gap_session(session_id);
                    }
                    return Ok(None);
                }
                if self
                    .last_sequences
                    .get(session_id)
                    .is_some_and(|last| sequence <= *last)
                {
                    self.gap_session(session_id);
                    return Ok(None);
                }
                if self.buffered.len() >= MAX_CAPTURE_EVENTS
                    || self.buffered_bytes.saturating_add(bytes) > MAX_CAPTURE_BUFFER_BYTES
                    || self.seen_ids.len() >= MAX_CAPTURE_EVENTS
                {
                    self.mark_gap();
                    return Err(CaptureGap::CapacityExceeded);
                }
                self.buffered_bytes += bytes;
                self.remember_seen_id(event_id.to_owned(), session_id.to_owned(), digest);
                self.last_sequences.insert(session_id.to_owned(), sequence);
                self.buffered.push(event);
                Ok(None)
            }
            Phase::Live => self.classify(event),
            Phase::Gap => Err(CaptureGap::ContinuityLost),
        }
    }

    pub fn mark_gap(&mut self) {
        self.phase = Phase::Gap;
        self.buffered.clear();
        self.buffered_bytes = 0;
        self.seen_ids.clear();
        self.seen_id_order.clear();
        self.last_sequences.clear();
    }

    fn classify(&mut self, event: Value) -> Result<Option<CapturedEvent>, CaptureGap> {
        let Some((event_id, session_id, sequence)) = self.event_identity(&event)? else {
            return Ok(None);
        };
        if self.gapped_sessions.contains(session_id) {
            return Ok(None);
        }
        let Some((_, digest)) = event_digest_and_size(&event) else {
            self.gap_session(session_id);
            return Ok(None);
        };
        let boundary = self
            .boundaries
            .get(session_id)
            .copied()
            .expect("checked key");
        if let Some((previous_session, previous_digest)) = self.seen_ids.get(event_id).cloned() {
            if previous_session != session_id {
                self.gap_session(&previous_session);
                self.gap_session(session_id);
            } else if previous_digest != digest {
                self.gap_session(session_id);
            }
            return Ok(None);
        }
        if self
            .last_sequences
            .get(session_id)
            .is_some_and(|last| sequence <= *last)
        {
            self.gap_session(session_id);
            return Ok(None);
        }
        self.remember_seen_id(event_id.to_owned(), session_id.to_owned(), digest);
        self.last_sequences.insert(session_id.to_owned(), sequence);
        Ok(Some(CapturedEvent {
            session_id: session_id.to_owned(),
            sequence,
            kind: if boundary.is_some_and(|watermark| sequence <= watermark) {
                BoundaryKind::Seed
            } else {
                BoundaryKind::Live
            },
            event,
        }))
    }

    // Keep exact duplicate checks bounded. Sequence checks still reject older replays.
    fn remember_seen_id(&mut self, event_id: String, session_id: String, digest: [u8; 32]) {
        while self.seen_ids.len() >= MAX_CAPTURE_EVENTS {
            let Some(expired) = self.seen_id_order.pop_front() else {
                break;
            };
            self.seen_ids.remove(&expired);
        }
        self.seen_id_order.push_back(event_id.clone());
        self.seen_ids.insert(event_id, (session_id, digest));
    }

    fn gap_session(&mut self, session_id: &str) {
        self.gapped_sessions.insert(session_id.to_owned());
        self.last_sequences.remove(session_id);
        self.seen_ids
            .retain(|_, (seen_session, _)| seen_session != session_id);
        self.seen_id_order
            .retain(|id| self.seen_ids.contains_key(id));
        self.buffered.retain(|event| {
            event
                .pointer("/durable/aggregateID")
                .and_then(Value::as_str)
                != Some(session_id)
        });
        self.buffered_bytes = self
            .buffered
            .iter()
            .filter_map(event_digest_and_size)
            .map(|(bytes, _)| bytes)
            .sum();
    }

    fn event_identity<'a>(
        &mut self,
        event: &'a Value,
    ) -> Result<Option<(&'a str, &'a str, u64)>, CaptureGap> {
        let Some(durable) = event.get("durable").and_then(Value::as_object) else {
            return Ok(None);
        };
        let Some(session_id) = durable.get("aggregateID").and_then(Value::as_str) else {
            self.mark_gap();
            return Err(CaptureGap::InvalidEvent);
        };
        if !self.admitted_sessions.contains(session_id) {
            return Ok(None);
        }
        let (Some(event_id), Some(sequence)) = (
            event.get("id").and_then(Value::as_str),
            durable.get("seq").and_then(Value::as_u64),
        ) else {
            self.gap_session(session_id);
            return Ok(None);
        };
        if event_id.is_empty()
            || event_id.len() > MAX_CAPTURE_ID_BYTES
            || session_id.is_empty()
            || session_id.len() > MAX_CAPTURE_ID_BYTES
            || session_id.contains('\0')
        {
            self.gap_session(session_id);
            return Ok(None);
        }
        Ok(Some((event_id, session_id, sequence)))
    }
}

fn event_digest_and_size(event: &Value) -> Option<(usize, [u8; 32])> {
    encoded_digest_and_size(event, MAX_CAPTURE_EVENT_BYTES)
}

fn encoded_digest_and_size(
    value: &impl serde::Serialize,
    limit: usize,
) -> Option<(usize, [u8; 32])> {
    use std::io::{self, Write};

    struct Counter {
        len: usize,
        limit: usize,
        digest: Sha256,
    }

    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.len = self
                .len
                .checked_add(bytes.len())
                .filter(|len| *len <= self.limit)
                .ok_or_else(|| io::Error::other("capture event exceeds byte limit"))?;
            self.digest.update(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    let mut counter = Counter {
        len: 0,
        limit,
        digest: Sha256::new(),
    };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some((counter.len, counter.digest.finalize().into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use serde_json::json;

    fn event(id: &str, session_id: &str, sequence: u64) -> Value {
        json!({
            "id": id,
            "type": "session.tool.success",
            "durable": {"aggregateID": session_id, "seq": sequence, "version": 1},
            "data": {"sessionID": session_id}
        })
    }

    #[test]
    fn reconciliation_reports_moved_scope_without_rejecting_healthy_baselines() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("opencode.db");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/discovery/agents/tests/fixtures/opencode_pinned_mixed.sql"
            )))
            .unwrap();
        connection
            .execute_batch(
                "CREATE TABLE event_sequence (
                     aggregate_id TEXT PRIMARY KEY NOT NULL,
                     seq INTEGER NOT NULL
                 );
                 INSERT INTO session_v2
                     (id, project_id, slug, directory, title, version, time_created, time_updated)
                 VALUES
                     ('ses_healthy', 'project', 'healthy', '/repo', 'Healthy', '2', 1, 1),
                     ('ses_moved', 'project', 'moved', '/other', 'Moved', '2', 1, 1),
                     ('ses_no_events', 'project', 'empty', '/repo', 'Empty', '2', 1, 1);
                 INSERT INTO event_sequence (aggregate_id, seq) VALUES ('ses_healthy', 15);",
            )
            .unwrap();
        drop(connection);

        let snapshot = read_capture_reconciliation_for_scope(
            &path,
            SourceFormat::OpenCodeSqliteV2,
            &[
                ("ses_healthy".into(), "/repo".into()),
                ("ses_moved".into(), "/repo".into()),
                ("ses_no_events".into(), "/repo".into()),
            ],
            &[],
            &|| false,
        )
        .unwrap();

        assert_eq!(snapshot.baseline.get("ses_healthy"), Some(&Some(15)));
        assert_eq!(snapshot.baseline.get("ses_no_events"), Some(&None));
        assert_eq!(snapshot.baseline.get("ses_moved"), Some(&None));
        assert_eq!(snapshot.invalid_scopes, HashSet::from(["ses_moved".into()]));
    }

    #[test]
    fn subscription_precedes_hydration_and_classifies_buffered_boundary_events() {
        let mut epoch = OpenCodeV2CaptureEpoch::new(vec!["ses_a".into(), "ses_b".into()]).unwrap();
        assert_ne!(epoch.id(), Uuid::nil());
        assert_ne!(
            epoch.id(),
            OpenCodeV2CaptureEpoch::new(vec!["ses_a".into(), "ses_b".into()])
                .unwrap()
                .id()
        );
        assert!(matches!(
            epoch.observe(event("before", "ses_a", 1)),
            Err(CaptureGap::SubscriptionNotConfirmed)
        ));
        epoch.confirm_subscription().unwrap();
        assert_eq!(
            epoch.observe(json!({"type":"server.connected"})).unwrap(),
            None
        );
        assert_eq!(epoch.observe(event("old", "ses_a", 2)).unwrap(), None);
        assert_eq!(epoch.observe(event("new", "ses_a", 9)).unwrap(), None);
        assert_eq!(epoch.observe(event("other", "ses_b", 4)).unwrap(), None);
        assert_eq!(
            epoch
                .observe(json!({
                    "id":"unrelated",
                    "durable":{"aggregateID":"ses_untracked", "seq":1},
                    "payload":"x".repeat(MAX_CAPTURE_EVENT_BYTES + 1)
                }))
                .unwrap(),
            None
        );

        let events = epoch
            .finish_hydration(HashMap::from([
                ("ses_a".to_owned(), Some(2)),
                ("ses_b".to_owned(), Some(3)),
            ]))
            .unwrap();

        assert_eq!(events.len(), 3);
        assert_eq!(events[0].kind, BoundaryKind::Seed);
        assert_eq!(events[1].kind, BoundaryKind::Live);
        assert_eq!(events[2].kind, BoundaryKind::Live);
        let after = epoch.observe(event("later", "ses_a", 13)).unwrap().unwrap();
        assert_eq!(after.kind, BoundaryKind::Live);
    }

    #[test]
    fn public_sequence_jumps_are_safe_and_regression_gaps_only_one_session() {
        let mut epoch = OpenCodeV2CaptureEpoch::new(vec!["ses_a".into(), "ses_b".into()]).unwrap();
        epoch.confirm_subscription().unwrap();
        epoch
            .finish_hydration(HashMap::from([
                ("ses_a".to_owned(), None),
                ("ses_b".to_owned(), None),
            ]))
            .unwrap();
        assert!(epoch.observe(event("first", "ses_a", 1)).unwrap().is_some());
        assert!(
            epoch
                .observe(event("later", "ses_a", 99))
                .unwrap()
                .is_some()
        );
        assert_eq!(
            epoch.observe(event("regressed", "ses_a", 98)).unwrap(),
            None
        );
        assert_eq!(
            epoch.observe(event("after-gap", "ses_a", 100)).unwrap(),
            None
        );
        let other = epoch.observe(event("other", "ses_b", 1)).unwrap().unwrap();
        assert_eq!(other.kind, BoundaryKind::Live);
        assert_eq!(epoch.gapped_sessions().collect::<Vec<_>>(), ["ses_a"]);
    }

    #[test]
    fn hydration_buffer_is_bounded_and_drops_partial_events_on_overflow() {
        let mut epoch = OpenCodeV2CaptureEpoch::new(vec!["ses_a".into()]).unwrap();
        epoch.confirm_subscription().unwrap();
        for sequence in 0..MAX_CAPTURE_EVENTS {
            epoch
                .observe(event(&format!("evt_{sequence}"), "ses_a", sequence as u64))
                .unwrap();
        }
        assert!(matches!(
            epoch.observe(event("overflow", "ses_a", MAX_CAPTURE_EVENTS as u64)),
            Err(CaptureGap::CapacityExceeded)
        ));
        assert_eq!(
            epoch.finish_hydration(HashMap::from([("ses_a".to_owned(), None)])),
            Err(CaptureGap::ContinuityLost)
        );
    }

    #[test]
    fn one_oversized_session_event_does_not_gap_other_sessions() {
        let mut epoch = OpenCodeV2CaptureEpoch::new(vec!["ses_a".into(), "ses_b".into()]).unwrap();
        epoch.confirm_subscription().unwrap();
        epoch
            .finish_hydration(HashMap::from([
                ("ses_a".to_owned(), None),
                ("ses_b".to_owned(), None),
            ]))
            .unwrap();

        let mut oversized = event("large", "ses_a", 1);
        oversized["data"]["payload"] = Value::String("x".repeat(MAX_CAPTURE_EVENT_BYTES + 1));
        assert_eq!(epoch.observe(oversized).unwrap(), None);
        assert_eq!(epoch.gapped_sessions().collect::<Vec<_>>(), ["ses_a"]);
        assert!(epoch.observe(event("other", "ses_b", 1)).unwrap().is_some());
    }

    #[test]
    fn duplicate_events_at_capacity_do_not_create_a_gap_and_old_ids_are_evicted() {
        let mut hydration = OpenCodeV2CaptureEpoch::new(vec!["ses_a".into()]).unwrap();
        hydration.confirm_subscription().unwrap();
        let last = event(
            &format!("event_{}", MAX_CAPTURE_EVENTS - 1),
            "ses_a",
            (MAX_CAPTURE_EVENTS - 1) as u64,
        );
        for sequence in 0..MAX_CAPTURE_EVENTS - 1 {
            hydration
                .observe(event(
                    &format!("event_{sequence}"),
                    "ses_a",
                    sequence as u64,
                ))
                .unwrap();
        }
        hydration.observe(last.clone()).unwrap();
        assert_eq!(hydration.observe(last.clone()).unwrap(), None);
        assert!(hydration.gapped_sessions().next().is_none());

        let mut live = OpenCodeV2CaptureEpoch::new(vec!["ses_a".into()]).unwrap();
        live.confirm_subscription().unwrap();
        live.finish_hydration(HashMap::from([("ses_a".to_owned(), None)]))
            .unwrap();
        for sequence in 0..MAX_CAPTURE_EVENTS {
            live.observe(event(
                &format!("event_{sequence}"),
                "ses_a",
                sequence as u64,
            ))
            .unwrap();
        }
        assert_eq!(live.observe(last).unwrap(), None);
        assert_eq!(
            live.observe(event(
                "event-after-capacity",
                "ses_a",
                MAX_CAPTURE_EVENTS as u64
            ))
            .unwrap()
            .unwrap()
            .sequence,
            MAX_CAPTURE_EVENTS as u64
        );
        assert_eq!(live.phase, Phase::Live);
        assert_eq!(live.seen_ids.len(), MAX_CAPTURE_EVENTS);
        assert_eq!(live.seen_id_order.len(), MAX_CAPTURE_EVENTS);
    }

    #[test]
    fn shared_event_cache_evicts_old_ids_without_restarting_the_epoch() {
        let mut live =
            OpenCodeV2CaptureEpoch::new(vec!["ses_a".into(), "ses_b".into(), "ses_c".into()])
                .unwrap();
        live.confirm_subscription().unwrap();
        live.finish_hydration(HashMap::from([
            ("ses_a".into(), None),
            ("ses_b".into(), None),
            ("ses_c".into(), None),
        ]))
        .unwrap();
        for sequence in 0..MAX_CAPTURE_EVENTS / 2 {
            live.observe(event(&format!("a-{sequence}"), "ses_a", sequence as u64))
                .unwrap();
            live.observe(event(&format!("b-{sequence}"), "ses_b", sequence as u64))
                .unwrap();
        }

        assert!(
            live.observe(event("new-session-event", "ses_c", 1))
                .unwrap()
                .is_some()
        );
        assert_eq!(live.phase, Phase::Live);
        assert_eq!(live.seen_ids.len(), MAX_CAPTURE_EVENTS);
        assert_eq!(live.seen_id_order.len(), MAX_CAPTURE_EVENTS);
        assert_eq!(live.observe(event("a-0", "ses_a", 0)).unwrap(), None);
        assert_eq!(live.gapped_sessions().collect::<Vec<_>>(), ["ses_a"]);
    }

    #[test]
    fn session_recovery_replaces_only_its_boundary_and_keeps_healthy_order() {
        let mut live = OpenCodeV2CaptureEpoch::new(vec!["ses_a".into(), "ses_b".into()]).unwrap();
        live.confirm_subscription().unwrap();
        live.finish_hydration(HashMap::from([
            ("ses_a".into(), Some(4)),
            ("ses_b".into(), Some(4)),
        ]))
        .unwrap();
        live.observe(event("a-old", "ses_a", 5)).unwrap();
        live.observe(event("b-old", "ses_b", 6)).unwrap();
        live.mark_session_gap("ses_a");

        let mut recovered = OpenCodeV2CaptureEpoch::new(vec!["ses_a".into()]).unwrap();
        recovered.confirm_subscription().unwrap();
        recovered.observe(event("a-seed", "ses_a", 20)).unwrap();
        recovered.observe(event("a-live", "ses_a", 21)).unwrap();
        let buffered = recovered
            .finish_hydration(HashMap::from([("ses_a".into(), Some(20))]))
            .unwrap();
        assert_eq!(
            buffered.iter().map(|event| event.kind).collect::<Vec<_>>(),
            [BoundaryKind::Seed, BoundaryKind::Live]
        );
        live.resume_sessions(recovered).unwrap();
        assert!(live.gapped_sessions().next().is_none());
        assert!(
            live.observe(event("a-live", "ses_a", 21))
                .unwrap()
                .is_none()
        );
        assert!(
            live.observe(event("a-future", "ses_a", 22))
                .unwrap()
                .is_some()
        );
        assert!(live.observe(event("b-old", "ses_b", 6)).unwrap().is_none());
        assert!(
            live.observe(event("b-future", "ses_b", 7))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn session_recovery_rejects_unhydrated_scope_and_accepts_a_hydrated_new_scope() {
        let mut live = OpenCodeV2CaptureEpoch::new(vec!["ses_a".into()]).unwrap();
        live.confirm_subscription().unwrap();
        live.finish_hydration(HashMap::from([("ses_a".into(), Some(4))]))
            .unwrap();
        assert_eq!(
            live.resume_sessions(OpenCodeV2CaptureEpoch::new(vec!["ses_a".into()]).unwrap()),
            Err(CaptureGap::InvalidBoundary)
        );
        let mut unrelated = OpenCodeV2CaptureEpoch::new(vec!["ses_b".into()]).unwrap();
        unrelated.confirm_subscription().unwrap();
        unrelated
            .finish_hydration(HashMap::from([("ses_b".into(), None)]))
            .unwrap();
        live.observe(event("a-first", "ses_a", 5)).unwrap();
        live.resume_sessions(unrelated).unwrap();
        assert!(
            live.observe(event("a-first", "ses_a", 5))
                .unwrap()
                .is_none()
        );
        assert!(live.observe(event("a-next", "ses_a", 6)).unwrap().is_some());
        assert!(
            live.observe(event("b-first", "ses_b", 1))
                .unwrap()
                .is_some()
        );
        assert!(
            live.observe(event("b-first", "ses_b", 1))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn session_addition_enforces_session_limit_and_bounds_event_identity_cache() {
        let ids = (0..MAX_CAPTURE_SESSIONS)
            .map(|index| format!("session-{index}"))
            .collect::<Vec<_>>();
        let mut live = OpenCodeV2CaptureEpoch::new(ids.clone()).unwrap();
        live.confirm_subscription().unwrap();
        live.finish_hydration(ids.into_iter().map(|id| (id, None)).collect())
            .unwrap();
        let mut new = OpenCodeV2CaptureEpoch::new(vec!["new".into()]).unwrap();
        new.confirm_subscription().unwrap();
        new.finish_hydration(HashMap::from([("new".into(), None)]))
            .unwrap();
        assert_eq!(live.resume_sessions(new), Err(CaptureGap::CapacityExceeded));
        assert!(
            live.observe(event("still-live", "session-0", 1))
                .unwrap()
                .is_some()
        );
        assert!(
            live.observe(event("not-admitted", "new", 1))
                .unwrap()
                .is_none()
        );

        let mut live = OpenCodeV2CaptureEpoch::new(vec!["healthy".into()]).unwrap();
        live.confirm_subscription().unwrap();
        live.finish_hydration(HashMap::from([("healthy".into(), None)]))
            .unwrap();
        for index in 0..MAX_CAPTURE_EVENTS {
            live.observe(event(&format!("event-{index}"), "healthy", index as u64))
                .unwrap();
        }
        let mut new = OpenCodeV2CaptureEpoch::new(vec!["new".into()]).unwrap();
        new.confirm_subscription().unwrap();
        new.observe(event("new-event", "new", 1)).unwrap();
        new.finish_hydration(HashMap::from([("new".into(), None)]))
            .unwrap();
        live.resume_sessions(new).unwrap();
        assert_eq!(live.phase, Phase::Live);
        assert!(live.gapped_sessions().next().is_none());
        assert_eq!(live.seen_ids.len(), MAX_CAPTURE_EVENTS);
        assert!(
            live.observe(event("new-event", "new", 1))
                .unwrap()
                .is_none()
        );
    }
}

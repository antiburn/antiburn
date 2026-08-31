//! Persisted turn rows.
//!
//! Each parsed turn becomes one row in the `turn` table. This lets the app
//! query the durable rows instead of streaming an accumulator, and lets a
//! catalog or policy change requery rows instead of reparsing a transcript.
//!
//! This module owns only the `turn` and `turn_content` DDL and the read/write
//! functions over a borrowed [`rusqlite::Connection`]. The crate does not open
//! the app database and does not know any other app table.

use std::sync::{Arc, Mutex};

use rusqlite::{Connection, params};

use crate::analysis::evidence_query::{TurnFacts, query_turn_facts};
use crate::analysis::interface::{ContentPart, NormalizedRecord, RecordSink, SessionSummary};
use crate::analysis::model::{CompactionTrigger, EventSource, NormalizedEvent, Role};

/// DDL for the `turn` and `turn_content` tables.
///
/// `turn` holds one row per parsed turn: identity, thread and scope facts,
/// and token accounting. `turn_content` holds the turn's text, keyed by the
/// `turn` rowid and part index (one turn has several parts: text,
/// thinking, each tool input, each tool result), in a separate table so
/// the hot `turn` table stays narrow.
/// `turn_content` is created now; a later change writes to it.
///
/// The caller applies this DDL after its own `session` table exists — the
/// foreign key references `session (environment_key, agent, session_id)`.
///
/// `turn_content` is the only place transcript text is persisted. Evidence
/// JSON, metrics JSON, and every other projection never join to it — see
/// `docs/plans/session-evidence-harness-parity.md`, "Privacy with content
/// stored".
pub const TURN_SCHEMA_SQL: &str = r#"
CREATE TABLE turn (
    rowid INTEGER PRIMARY KEY,
    environment_key TEXT NOT NULL,
    agent TEXT NOT NULL,
    session_id TEXT NOT NULL,
    claim_fence INTEGER NOT NULL,
    source_key TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    turn_index INTEGER NOT NULL,
    scope TEXT NOT NULL CHECK (scope IN ('main','delegated')),
    child_id TEXT,
    role TEXT NOT NULL,
    ts_ms INTEGER,
    model TEXT,
    effort TEXT,
    speed TEXT,
    input_tokens INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL,
    cache_write_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    is_compaction_boundary INTEGER NOT NULL,
    message_id TEXT,
    uuid TEXT,
    parent_uuid TEXT,
    FOREIGN KEY (environment_key, agent, session_id)
      REFERENCES session (environment_key, agent, session_id) ON DELETE CASCADE
) STRICT;

CREATE INDEX turn_session_thread
    ON turn (environment_key, agent, session_id, thread_id, turn_index);

CREATE TABLE turn_content (
    turn_rowid INTEGER NOT NULL REFERENCES turn (rowid) ON DELETE CASCADE,
    part_index INTEGER NOT NULL,
    kind TEXT NOT NULL,
    content BLOB NOT NULL,
    truncated INTEGER NOT NULL,
    PRIMARY KEY (turn_rowid, part_index)
) WITHOUT ROWID, STRICT;
"#;

/// DDL that adds the compaction columns [`TurnRow::compaction_trigger`],
/// [`TurnRow::compaction_pre_tokens`], and [`TurnRow::compaction_post_tokens`]
/// to an existing `turn` table.
///
/// [`TURN_SCHEMA_SQL`] is already applied as schema version 15 on user
/// machines, so this change adds its three new columns through a migration
/// instead of editing that constant. See [`TURN_MIGRATIONS`].
pub const TURN_SCHEMA_V2_SQL: &str = r#"
ALTER TABLE turn ADD COLUMN compaction_trigger TEXT;
ALTER TABLE turn ADD COLUMN compaction_pre_tokens INTEGER;
ALTER TABLE turn ADD COLUMN compaction_post_tokens INTEGER;
"#;

/// Every migration that builds the `turn` and `turn_content` schema, in
/// order. A caller that creates this schema from scratch (a test, an
/// in-memory store) applies every entry in order; the app applies
/// [`TURN_SCHEMA_SQL`] and [`TURN_SCHEMA_V2_SQL`] as its own separately
/// numbered migrations instead, since [`TURN_SCHEMA_SQL`] is already applied
/// on user machines.
pub const TURN_MIGRATIONS: &[&str] = &[TURN_SCHEMA_SQL, TURN_SCHEMA_V2_SQL];

/// Number of rows a [`TurnRowSink`] buffers before it writes them, unless the
/// caller picks a different size with [`TurnRowSink::with_batch_size`].
pub const TURN_ROW_BATCH_SIZE: usize = 512;

/// Whether a turn ran in the session's main loop or in a delegated child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnScope {
    Main,
    Delegated,
}

impl TurnScope {
    pub fn as_str(self) -> &'static str {
        match self {
            TurnScope::Main => "main",
            TurnScope::Delegated => "delegated",
        }
    }

    /// Reads back a value [`Self::as_str`] wrote. Returns `None` for any
    /// other text. The `turn` table's `CHECK` constraint keeps this column
    /// to `'main'` or `'delegated'`, so `None` signals a row this crate did
    /// not write.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "main" => Some(TurnScope::Main),
            "delegated" => Some(TurnScope::Delegated),
            _ => None,
        }
    }
}

/// One parsed turn, ready to become a `turn` row.
///
/// Carries every column except the session key and the claim fence — the
/// caller supplies those once, for the whole batch, in
/// [`insert_turn_rows`].
#[derive(Debug, Clone, PartialEq)]
pub struct TurnRow {
    pub source_key: String,
    pub thread_id: String,
    pub turn_index: u64,
    pub scope: TurnScope,
    pub child_id: Option<String>,
    pub role: &'static str,
    pub ts_ms: Option<i64>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub speed: Option<String>,
    pub input_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub output_tokens: u64,
    pub is_compaction_boundary: bool,
    pub message_id: Option<String>,
    pub uuid: Option<String>,
    pub parent_uuid: Option<String>,
    pub compaction_trigger: Option<CompactionTrigger>,
    pub compaction_pre_tokens: Option<u64>,
    pub compaction_post_tokens: Option<u64>,
    /// This turn's captured message content, when the source adapter emitted
    /// a `TurnContent` record for it. Attached by [`TurnRowSink`] after
    /// [`turn_row_from_event`] builds the row — an event alone carries none.
    pub content: Vec<ContentPart>,
}

fn role_key(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
    }
}

/// Reads back a value [`role_key`] wrote. Returns `None` for any other
/// text, so a caller can treat it as a corrupted row instead of guessing a
/// role.
pub(crate) fn parse_role(value: &str) -> Option<&'static str> {
    match value {
        "user" => Some("user"),
        "assistant" => Some("assistant"),
        "system" => Some("system"),
        "tool" => Some("tool"),
        _ => None,
    }
}

/// Build one row from a normalized event.
///
/// `source_key` names the parent transcript or child file this event comes
/// from. `thread_id` is the event's own derived thread (an adapter that
/// links records, like Claude's `uuid` chain, sets [`NormalizedEvent::thread_id`]);
/// it falls back to `source_key` when the adapter derives no thread.
///
/// Scope is `Delegated` when the event's [`EventSource`] is `Subagent`; its
/// `child_id` is then the event's own thread, so an inline sidechain gets
/// its own child identity distinct from its parent file's `source_key`.
/// Otherwise scope is `Main` with no `child_id`.
pub fn turn_row_from_event(event: &NormalizedEvent, source_key: &str, turn_index: u64) -> TurnRow {
    let thread_id = event
        .thread_id
        .clone()
        .unwrap_or_else(|| source_key.to_owned());
    let (scope, child_id) = match event.source {
        EventSource::Subagent => (TurnScope::Delegated, Some(thread_id.clone())),
        EventSource::Parent => (TurnScope::Main, None),
    };
    TurnRow {
        source_key: source_key.to_owned(),
        thread_id,
        turn_index,
        scope,
        child_id,
        role: role_key(event.role),
        ts_ms: event.ts_ms,
        model: event.model.clone(),
        effort: event.thinking_mode.clone(),
        speed: event.speed.clone(),
        input_tokens: event.usage.input_tokens,
        cache_read_tokens: event.usage.cache_read_tokens,
        cache_write_tokens: event.usage.cache_creation_tokens,
        output_tokens: event.usage.output_tokens,
        is_compaction_boundary: event.is_compaction_boundary,
        message_id: event.message_id.clone(),
        uuid: event.uuid.clone(),
        parent_uuid: event.parent_uuid.clone(),
        compaction_trigger: event.compaction_trigger,
        compaction_pre_tokens: event.compaction_pre_tokens,
        compaction_post_tokens: event.compaction_post_tokens,
        content: Vec::new(),
    }
}

/// A session's identity for row queries. Distinct from the app's own session
/// key type — this crate does not depend on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnSessionKey<'a> {
    pub environment_key: &'a str,
    pub agent: &'a str,
    pub session_id: &'a str,
}

/// One `turn`-row store operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRowError(pub String);

impl std::fmt::Display for TurnRowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "turn row store failed: {}", self.0)
    }
}

impl std::error::Error for TurnRowError {}

impl From<rusqlite::Error> for TurnRowError {
    fn from(error: rusqlite::Error) -> Self {
        TurnRowError(error.to_string())
    }
}

/// Writes and reads a session's batch of turn rows.
///
/// `&self` so a shared handle (an `Arc<Store>`, or an app type that wraps
/// one) can implement this and move into the blocking thread that runs the
/// analysis pass.
pub trait TurnRowStore: Send + Sync {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError>;

    /// Reads the facts for every row this store wrote under its own session
    /// key and fence.
    fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError>;
}

/// A [`RecordSink`] that turns `MetricsEvent` records into [`TurnRow`]s,
/// attaches each row's `TurnContent` record (when one arrives), and writes
/// the rows through a [`TurnRowStore`] in bounded batches.
///
/// `turn_index` is monotonic per `source_key`, starting at zero. After each
/// call to [`Self::observe`] the buffer holds at most `batch_size` rows: once
/// a pushed `MetricsEvent` row takes it over that size, every row except the
/// one just pushed is flushed immediately, so memory stays bounded no matter
/// how long the source runs. The most recently pushed row stays buffered
/// (unwritten) so its `TurnContent` record — which the adapter contract
/// promises arrives right after the `MetricsEvent` it belongs to — can still
/// attach before that row is written. The first write error is kept and
/// stops further writes; [`Self::into_error`] lets the caller surface it.
pub struct TurnRowSink {
    store: Arc<dyn TurnRowStore>,
    source_key: String,
    forced_scope: Option<TurnScope>,
    next_index: u64,
    buffer: Vec<TurnRow>,
    batch_size: usize,
    error: Option<TurnRowError>,
}

impl TurnRowSink {
    /// `scope` overrides the scope [`turn_row_from_event`] derives from each
    /// event's [`EventSource`]. `Some(TurnScope::Delegated)` still sets
    /// `child_id = Some(source_key)`, as if the event had come from a
    /// sub-agent transcript. `None` keeps today's derived scope.
    pub fn new(
        store: Arc<dyn TurnRowStore>,
        source_key: impl Into<String>,
        scope: Option<TurnScope>,
    ) -> Self {
        Self::with_batch_size(store, source_key, scope, TURN_ROW_BATCH_SIZE)
    }

    pub fn with_batch_size(
        store: Arc<dyn TurnRowStore>,
        source_key: impl Into<String>,
        scope: Option<TurnScope>,
        batch_size: usize,
    ) -> Self {
        let batch_size = batch_size.max(1);
        Self {
            store,
            source_key: source_key.into(),
            forced_scope: scope,
            next_index: 0,
            buffer: Vec::with_capacity(batch_size.min(TURN_ROW_BATCH_SIZE)),
            batch_size,
            error: None,
        }
    }

    /// True once a write has failed. No further row reaches the store after
    /// this, but rows already accepted before the failure stay written.
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }

    /// Consumes the sink and returns the write error, if one occurred.
    pub fn into_error(self) -> Option<TurnRowError> {
        self.error
    }

    /// Folds one record without taking it. A `MetricsEvent` becomes a
    /// buffered row; a `TurnContent` record attaches its parts to the most
    /// recently buffered row. `Observation` and `Unusable` records are not
    /// turns.
    pub fn observe(&mut self, record: &NormalizedRecord) {
        if self.error.is_some() {
            return;
        }
        match record {
            NormalizedRecord::MetricsEvent(event) => {
                let mut row = turn_row_from_event(event, &self.source_key, self.next_index);
                if let Some(scope) = self.forced_scope {
                    row.scope = scope;
                    row.child_id = match scope {
                        TurnScope::Delegated => Some(self.source_key.clone()),
                        TurnScope::Main => None,
                    };
                }
                self.next_index += 1;
                self.buffer.push(row);
                if self.buffer.len() > self.batch_size {
                    self.flush_except_last();
                }
            }
            NormalizedRecord::TurnContent(content) => {
                if let Some(row) = self.buffer.last_mut() {
                    row.content.clone_from(&content.parts);
                }
            }
            NormalizedRecord::Observation(_) | NormalizedRecord::Unusable(_) => {}
        }
    }

    /// Reads the row-derived facts for every row this sink's store holds
    /// under its own session key and fence. Correct only after
    /// [`Self::flush`] (or [`RecordSink::finish`], which flushes) — a row
    /// still buffered here is invisible to the store's own query.
    pub fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
        self.store.query_turn_facts()
    }

    /// Writes any buffered rows and clears the buffer.
    pub fn flush(&mut self) {
        if self.error.is_some() || self.buffer.is_empty() {
            return;
        }
        if let Err(error) = self.store.write_turn_rows(&self.buffer) {
            self.error = Some(error);
        }
        self.buffer.clear();
    }

    /// Writes every buffered row except the last, keeping that last row
    /// buffered so a `TurnContent` record for it can still attach before it
    /// is written. Keeps the buffer at `batch_size` rows or fewer once this
    /// returns.
    fn flush_except_last(&mut self) {
        if self.error.is_some() {
            return;
        }
        let Some(last) = self.buffer.pop() else {
            return;
        };
        if let Err(error) = self.store.write_turn_rows(&self.buffer) {
            self.error = Some(error);
        }
        self.buffer.clear();
        self.buffer.push(last);
    }
}

impl RecordSink for TurnRowSink {
    fn record(&mut self, record: NormalizedRecord) {
        self.observe(&record);
    }

    fn finish(&mut self, _summary: SessionSummary) {
        self.flush();
    }
}

const INSERT_TURN_SQL: &str = "INSERT INTO turn (
    environment_key, agent, session_id, claim_fence, source_key, thread_id,
    turn_index, scope, child_id, role, ts_ms, model, effort, speed,
    input_tokens, cache_read_tokens, cache_write_tokens, output_tokens,
    is_compaction_boundary, message_id, uuid, parent_uuid,
    compaction_trigger, compaction_pre_tokens, compaction_post_tokens
) VALUES (
    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
    ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25
)";

const INSERT_TURN_CONTENT_SQL: &str = "INSERT INTO turn_content (
    turn_rowid, part_index, kind, content, truncated
) VALUES (?1, ?2, ?3, ?4, ?5)";

/// Inserts one batch of rows for `key`, all stamped with `claim_fence`, and
/// each row's captured content (if any) into `turn_content`.
///
/// One prepared statement executed once per row, plus one per content part.
/// The caller supplies the transaction (pass a [`rusqlite::Transaction`],
/// which derefs to `Connection`).
pub fn insert_turn_rows(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
    rows: &[TurnRow],
) -> rusqlite::Result<()> {
    let mut statement = conn.prepare(INSERT_TURN_SQL)?;
    let mut content_statement = conn.prepare(INSERT_TURN_CONTENT_SQL)?;
    for row in rows {
        statement.execute(params![
            key.environment_key,
            key.agent,
            key.session_id,
            claim_fence,
            row.source_key,
            row.thread_id,
            row.turn_index as i64,
            row.scope.as_str(),
            row.child_id,
            row.role,
            row.ts_ms,
            row.model,
            row.effort,
            row.speed,
            row.input_tokens as i64,
            row.cache_read_tokens as i64,
            row.cache_write_tokens as i64,
            row.output_tokens as i64,
            i64::from(row.is_compaction_boundary),
            row.message_id,
            row.uuid,
            row.parent_uuid,
            row.compaction_trigger.map(CompactionTrigger::as_str),
            row.compaction_pre_tokens.map(|tokens| tokens as i64),
            row.compaction_post_tokens.map(|tokens| tokens as i64),
        ])?;
        if !row.content.is_empty() {
            let turn_rowid = conn.last_insert_rowid();
            for (part_index, part) in row.content.iter().enumerate() {
                content_statement.execute(params![
                    turn_rowid,
                    part_index as i64,
                    part.kind.as_str(),
                    part.text.as_bytes(),
                    i64::from(part.truncated),
                ])?;
            }
        }
    }
    Ok(())
}

/// Deletes every row for `key` whose `claim_fence` is not `keep_fence`.
///
/// Used after a pass publishes: rows from every earlier, superseded pass are
/// dropped, keeping only the rows the just-published evidence was built
/// from. Returns the number of `turn` rows removed.
pub fn delete_turn_rows_except_fence(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    keep_fence: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM turn_content WHERE turn_rowid IN (
             SELECT rowid FROM turn
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND claim_fence != ?4
         )",
        params![key.environment_key, key.agent, key.session_id, keep_fence],
    )?;
    conn.execute(
        "DELETE FROM turn
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
            AND claim_fence != ?4",
        params![key.environment_key, key.agent, key.session_id, keep_fence],
    )
}

/// Deletes every row for `key` stamped with exactly `claim_fence`.
///
/// Used when a pass loses the publish race (its claim fence no longer
/// matches): the rows it wrote never became part of any published evidence,
/// so they are cleaned up under their own fence rather than left to
/// accumulate. Returns the number of `turn` rows removed.
pub fn delete_turn_rows_for_fence(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM turn_content WHERE turn_rowid IN (
             SELECT rowid FROM turn
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND claim_fence = ?4
         )",
        params![key.environment_key, key.agent, key.session_id, claim_fence],
    )?;
    conn.execute(
        "DELETE FROM turn
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
            AND claim_fence = ?4",
        params![key.environment_key, key.agent, key.session_id, claim_fence],
    )
}

/// Deletes every row for `key`. Used by session delete and
/// clear-local-session-data paths.
pub fn delete_turn_rows(conn: &Connection, key: &TurnSessionKey<'_>) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM turn_content WHERE turn_rowid IN (
             SELECT rowid FROM turn
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
         )",
        params![key.environment_key, key.agent, key.session_id],
    )?;
    conn.execute(
        "DELETE FROM turn
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![key.environment_key, key.agent, key.session_id],
    )
}

/// Counts the rows for `key` stamped with `claim_fence`.
pub fn count_turn_rows(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> rusqlite::Result<u64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM turn
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
            AND claim_fence = ?4",
        params![key.environment_key, key.agent, key.session_id, claim_fence],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as u64)
}

/// Counts the `turn_content` rows for `key`'s turns stamped with
/// `claim_fence`.
pub fn count_turn_content_rows(
    conn: &Connection,
    key: &TurnSessionKey<'_>,
    claim_fence: i64,
) -> rusqlite::Result<u64> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM turn_content
          WHERE turn_rowid IN (
              SELECT rowid FROM turn
               WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                 AND claim_fence = ?4
          )",
        params![key.environment_key, key.agent, key.session_id, claim_fence],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as u64)
}

/// A minimal `session` table, just enough for the `turn` schema's foreign
/// key to be valid. Shared by [`MemoryTurnRowStore`] and this module's own
/// tests — neither owns the app's real `session` table.
const MEMORY_SESSION_SQL: &str = "CREATE TABLE session (
    environment_key TEXT NOT NULL,
    agent TEXT NOT NULL,
    session_id TEXT NOT NULL,
    PRIMARY KEY (environment_key, agent, session_id)
) STRICT;";

/// An in-memory [`TurnRowStore`], for tests and tools.
///
/// Holds a private `rusqlite::Connection` with every [`TURN_MIGRATIONS`]
/// entry applied, under one fixed [`TurnSessionKey`] and claim fence. The
/// app writes through the fenced store in `apps/desktop/src-tauri`'s
/// `store` module instead; this type lets a test stream a fixture through a
/// [`TurnRowSink`] and then read the rows back — through
/// [`Self::query_turn_facts`] or [`Self::with_connection`] — without a real
/// database.
pub struct MemoryTurnRowStore {
    connection: Mutex<Connection>,
    environment_key: String,
    agent: String,
    session_id: String,
    claim_fence: i64,
}

impl MemoryTurnRowStore {
    /// Builds a store scoped to one session, under claim fence `1`.
    pub fn new(agent: impl Into<String>, session_id: impl Into<String>) -> Arc<Self> {
        let agent = agent.into();
        let session_id = session_id.into();
        let environment_key = "native".to_owned();
        let connection = Connection::open_in_memory().expect("open in-memory connection");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("enable foreign keys");
        connection
            .execute_batch(MEMORY_SESSION_SQL)
            .expect("create session table");
        for migration in TURN_MIGRATIONS {
            connection
                .execute_batch(migration)
                .expect("apply turn schema migration");
        }
        connection
            .execute(
                "INSERT INTO session (environment_key, agent, session_id) VALUES (?1, ?2, ?3)",
                params![environment_key, agent, session_id],
            )
            .expect("insert session");
        Arc::new(Self {
            connection: Mutex::new(connection),
            environment_key,
            agent,
            session_id,
            claim_fence: 1,
        })
    }

    fn key(&self) -> TurnSessionKey<'_> {
        TurnSessionKey {
            environment_key: &self.environment_key,
            agent: &self.agent,
            session_id: &self.session_id,
        }
    }

    /// Runs `f` against the underlying connection, for a test that reads
    /// rows directly instead of through [`Self::query_turn_facts`].
    pub fn with_connection<R>(&self, f: impl FnOnce(&Connection) -> R) -> R {
        let connection = self.connection.lock().expect("lock");
        f(&connection)
    }
}

impl TurnRowStore for MemoryTurnRowStore {
    fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError> {
        let connection = self.connection.lock().expect("lock");
        insert_turn_rows(&connection, &self.key(), self.claim_fence, rows)
            .map_err(TurnRowError::from)
    }

    fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
        let connection = self.connection.lock().expect("lock");
        query_turn_facts(&connection, &self.key(), self.claim_fence).map_err(TurnRowError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::model::Usage;

    fn test_connection() -> Connection {
        let conn = Connection::open_in_memory().expect("open in-memory connection");
        conn.pragma_update(None, "foreign_keys", true)
            .expect("enable foreign keys");
        conn.execute_batch(MEMORY_SESSION_SQL)
            .expect("create session table");
        for migration in TURN_MIGRATIONS {
            conn.execute_batch(migration)
                .expect("apply turn schema migration");
        }
        conn
    }

    fn insert_session(conn: &Connection, key: &TurnSessionKey<'_>) {
        conn.execute(
            "INSERT INTO session (environment_key, agent, session_id) VALUES (?1, ?2, ?3)",
            params![key.environment_key, key.agent, key.session_id],
        )
        .expect("insert session");
    }

    fn sample_row(turn_index: u64) -> TurnRow {
        TurnRow {
            source_key: "s1".to_owned(),
            thread_id: "s1".to_owned(),
            turn_index,
            scope: TurnScope::Main,
            child_id: None,
            role: "assistant",
            ts_ms: Some(1_000 + turn_index as i64),
            model: Some("claude-opus-4-6".to_owned()),
            effort: None,
            speed: None,
            input_tokens: 10,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 5,
            is_compaction_boundary: false,
            message_id: None,
            uuid: None,
            parent_uuid: None,
            compaction_trigger: None,
            compaction_pre_tokens: None,
            compaction_post_tokens: None,
            content: Vec::new(),
        }
    }

    #[test]
    fn insert_and_count_round_trip() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        let rows = vec![sample_row(0), sample_row(1)];
        insert_turn_rows(&conn, &key, 7, &rows).expect("insert rows");

        assert_eq!(count_turn_rows(&conn, &key, 7).expect("count"), 2);
        assert_eq!(count_turn_rows(&conn, &key, 8).expect("count"), 0);
    }

    #[test]
    fn delete_except_fence_keeps_only_the_current_pass() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        insert_turn_rows(&conn, &key, 1, &[sample_row(0)]).expect("insert pass 1");
        insert_turn_rows(&conn, &key, 2, &[sample_row(0), sample_row(1)]).expect("insert pass 2");

        let removed = delete_turn_rows_except_fence(&conn, &key, 2).expect("delete stale");
        assert_eq!(removed, 1);
        assert_eq!(count_turn_rows(&conn, &key, 1).expect("count"), 0);
        assert_eq!(count_turn_rows(&conn, &key, 2).expect("count"), 2);
    }

    #[test]
    fn delete_for_fence_removes_only_the_matching_pass() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        insert_turn_rows(&conn, &key, 1, &[sample_row(0)]).expect("insert pass 1");
        insert_turn_rows(&conn, &key, 2, &[sample_row(0), sample_row(1)]).expect("insert pass 2");

        let removed = delete_turn_rows_for_fence(&conn, &key, 1).expect("delete lost race");
        assert_eq!(removed, 1);
        assert_eq!(count_turn_rows(&conn, &key, 1).expect("count"), 0);
        assert_eq!(count_turn_rows(&conn, &key, 2).expect("count"), 2);
    }

    #[test]
    fn delete_turn_rows_removes_every_row_for_the_session() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        insert_turn_rows(&conn, &key, 1, &[sample_row(0), sample_row(1)]).expect("insert");

        let removed = delete_turn_rows(&conn, &key).expect("delete all");
        assert_eq!(removed, 2);
        assert_eq!(count_turn_rows(&conn, &key, 1).expect("count"), 0);
    }

    #[test]
    fn deleting_the_session_cascades_to_its_turn_rows() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        insert_turn_rows(&conn, &key, 1, &[sample_row(0)]).expect("insert");

        conn.execute(
            "DELETE FROM session WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![key.environment_key, key.agent, key.session_id],
        )
        .expect("delete session");

        assert_eq!(count_turn_rows(&conn, &key, 1).expect("count"), 0);
    }

    #[test]
    fn turn_row_from_event_maps_delegated_scope() {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.source = EventSource::Subagent;
        event.usage = Usage {
            input_tokens: 3,
            ..Usage::default()
        };

        let row = turn_row_from_event(&event, "child-1", 2);
        assert_eq!(row.scope, TurnScope::Delegated);
        assert_eq!(row.child_id.as_deref(), Some("child-1"));
        assert_eq!(row.thread_id, "child-1");
        assert_eq!(row.turn_index, 2);
        assert_eq!(row.input_tokens, 3);
    }

    #[test]
    fn turn_row_from_event_maps_main_scope() {
        let event = NormalizedEvent::new(Role::User);
        let row = turn_row_from_event(&event, "parent-1", 0);
        assert_eq!(row.scope, TurnScope::Main);
        assert_eq!(row.child_id, None);
    }

    #[test]
    fn turn_row_from_event_uses_the_events_own_thread_for_a_delegated_child_id() {
        let mut event = NormalizedEvent::new(Role::Assistant);
        event.source = EventSource::Subagent;
        event.thread_id = Some("sidechain-1".to_owned());

        let row = turn_row_from_event(&event, "parent-1", 3);
        // The row's own source is still the parent file, but its thread and
        // child identity are the inline sidechain's, not the parent file's.
        assert_eq!(row.source_key, "parent-1");
        assert_eq!(row.thread_id, "sidechain-1");
        assert_eq!(row.child_id.as_deref(), Some("sidechain-1"));
    }

    #[test]
    fn turn_row_from_event_falls_back_to_the_source_key_without_a_derived_thread() {
        let event = NormalizedEvent::new(Role::Assistant);
        let row = turn_row_from_event(&event, "parent-1", 0);
        assert_eq!(row.thread_id, "parent-1");
    }

    /// A store that always fails to write, so the sink's error path can be
    /// tested without a real connection. Never read, so
    /// [`TurnRowStore::query_turn_facts`] need not really work.
    struct FailingWriter;

    impl TurnRowStore for FailingWriter {
        fn write_turn_rows(&self, _rows: &[TurnRow]) -> Result<(), TurnRowError> {
            Err(TurnRowError("boom".to_owned()))
        }

        fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
            Err(TurnRowError("not readable".to_owned()))
        }
    }

    /// A store over a real connection, so batching tests can assert on rows
    /// actually reaching the table. Never read through
    /// [`TurnRowStore::query_turn_facts`] — the tests here inspect the
    /// table directly.
    struct RecordingWriter {
        conn: Mutex<Connection>,
        key: String,
    }

    impl RecordingWriter {
        fn new(conn: Connection) -> Self {
            Self {
                conn: Mutex::new(conn),
                key: "s1".to_owned(),
            }
        }

        fn count(&self, claim_fence: i64) -> u64 {
            let conn = self.conn.lock().expect("lock");
            count_turn_rows(
                &conn,
                &TurnSessionKey {
                    environment_key: "native",
                    agent: "claude",
                    session_id: &self.key,
                },
                claim_fence,
            )
            .expect("count")
        }

        fn content_count(&self, claim_fence: i64) -> u64 {
            let conn = self.conn.lock().expect("lock");
            count_turn_content_rows(
                &conn,
                &TurnSessionKey {
                    environment_key: "native",
                    agent: "claude",
                    session_id: &self.key,
                },
                claim_fence,
            )
            .expect("count")
        }

        fn scopes(&self, claim_fence: i64) -> Vec<(TurnScope, Option<String>)> {
            let conn = self.conn.lock().expect("lock");
            let mut statement = conn
                .prepare(
                    "SELECT scope, child_id FROM turn
                      WHERE environment_key = 'native' AND agent = 'claude'
                        AND session_id = ?1 AND claim_fence = ?2
                      ORDER BY turn_index",
                )
                .expect("prepare");
            statement
                .query_map(params![self.key, claim_fence], |row| {
                    let scope: String = row.get(0)?;
                    let scope = match scope.as_str() {
                        "main" => TurnScope::Main,
                        _ => TurnScope::Delegated,
                    };
                    Ok((scope, row.get(1)?))
                })
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        }
    }

    impl TurnRowStore for RecordingWriter {
        fn write_turn_rows(&self, rows: &[TurnRow]) -> Result<(), TurnRowError> {
            let conn = self.conn.lock().expect("lock");
            insert_turn_rows(
                &conn,
                &TurnSessionKey {
                    environment_key: "native",
                    agent: "claude",
                    session_id: &self.key,
                },
                1,
                rows,
            )
            .map_err(TurnRowError::from)
        }

        fn query_turn_facts(&self) -> Result<TurnFacts, TurnRowError> {
            let conn = self.conn.lock().expect("lock");
            query_turn_facts(
                &conn,
                &TurnSessionKey {
                    environment_key: "native",
                    agent: "claude",
                    session_id: &self.key,
                },
                1,
            )
            .map_err(TurnRowError::from)
        }
    }

    fn metric_record(role: Role) -> NormalizedRecord {
        NormalizedRecord::MetricsEvent(Box::new(NormalizedEvent::new(role)))
    }

    #[test]
    fn the_buffer_never_exceeds_batch_size_and_index_stays_monotonic() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        let writer = Arc::new(RecordingWriter::new(conn));
        let mut sink = TurnRowSink::with_batch_size(
            Arc::clone(&writer) as Arc<dyn TurnRowStore>,
            "s1",
            None,
            4,
        );

        for _ in 0..10 {
            sink.observe(&metric_record(Role::Assistant));
            assert!(sink.buffer.len() <= 4);
        }
        sink.flush();

        assert_eq!(writer.count(1), 10);
        assert_eq!(sink.next_index, 10);
    }

    #[test]
    fn a_write_error_stops_further_writes_and_is_surfaced() {
        let writer = Arc::new(FailingWriter);
        let mut sink = TurnRowSink::with_batch_size(writer, "s1", None, 1);

        // A row stays buffered, unflushed, until a later row pushes the
        // buffer over `batch_size` — so the first write attempt (and the
        // failure) happens on the second `observe` call, not the first.
        sink.observe(&metric_record(Role::Assistant));
        sink.observe(&metric_record(Role::Assistant));
        assert!(sink.has_error());
        sink.observe(&metric_record(Role::Assistant));

        let error = sink.into_error().expect("error must surface");
        assert_eq!(error.0, "boom");
    }

    #[test]
    fn only_metrics_events_become_rows() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        let writer = Arc::new(RecordingWriter::new(conn));
        let mut sink = TurnRowSink::new(Arc::clone(&writer) as Arc<dyn TurnRowStore>, "s1", None);

        sink.observe(&metric_record(Role::Assistant));
        sink.observe(&NormalizedRecord::Unusable(
            crate::analysis::PartialReason::MalformedRecord,
        ));
        sink.finish(SessionSummary::default());

        assert_eq!(writer.count(1), 1);
    }

    #[test]
    fn a_forced_scope_overrides_the_event_derived_scope_and_sets_child_id() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        let writer = Arc::new(RecordingWriter::new(conn));
        // Every event here is `EventSource::Parent`, which
        // `turn_row_from_event` would otherwise map to `TurnScope::Main`
        // with no `child_id`.
        let mut sink = TurnRowSink::new(
            Arc::clone(&writer) as Arc<dyn TurnRowStore>,
            "s1",
            Some(TurnScope::Delegated),
        );

        sink.observe(&metric_record(Role::Assistant));
        sink.finish(SessionSummary::default());

        assert_eq!(
            writer.scopes(1),
            vec![(TurnScope::Delegated, Some("s1".to_owned()))]
        );
    }

    #[test]
    fn an_inline_sidechain_shares_the_source_key_but_keeps_its_own_thread_and_writes_back() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        let writer = Arc::new(RecordingWriter::new(conn));
        // Both events come from the same source file, so they share
        // `source_key`; their `thread_id`s differ.
        let mut sink = TurnRowSink::new(Arc::clone(&writer) as Arc<dyn TurnRowStore>, "s1", None);

        let mut main_turn = NormalizedEvent::new(Role::Assistant);
        main_turn.thread_id = Some("main-thread".to_owned());
        let mut sidechain_turn = NormalizedEvent::new(Role::Assistant);
        sidechain_turn.source = EventSource::Subagent;
        sidechain_turn.thread_id = Some("sidechain-thread".to_owned());

        sink.observe(&NormalizedRecord::MetricsEvent(Box::new(main_turn)));
        sink.observe(&NormalizedRecord::MetricsEvent(Box::new(sidechain_turn)));
        sink.finish(SessionSummary::default());

        assert_eq!(writer.count(1), 2);
        let rows: Vec<(String, String, Option<String>, i64)> = {
            let conn = writer.conn.lock().expect("lock");
            let mut statement = conn
                .prepare(
                    "SELECT thread_id, scope, child_id, turn_index FROM turn
                      WHERE environment_key = 'native' AND agent = 'claude'
                        AND session_id = 's1' AND claim_fence = 1
                      ORDER BY rowid",
                )
                .expect("prepare");
            statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .expect("query")
                .map(|row| row.expect("row"))
                .collect()
        };
        // Two threads share the source file, and their `turn_index` values
        // interleave (0 and 1, assigned in file order), but each row still
        // reads back under its own thread and child identity.
        assert_eq!(
            rows,
            vec![
                ("main-thread".to_owned(), "main".to_owned(), None, 0),
                (
                    "sidechain-thread".to_owned(),
                    "delegated".to_owned(),
                    Some("sidechain-thread".to_owned()),
                    1
                ),
            ]
        );
    }

    fn content_record(text: &str) -> NormalizedRecord {
        NormalizedRecord::TurnContent(Box::new(crate::analysis::interface::TurnContent {
            parts: vec![crate::analysis::interface::ContentPart::new(
                crate::analysis::interface::ContentKind::AssistantText,
                text,
            )],
        }))
    }

    #[test]
    fn a_turn_content_record_attaches_to_the_row_it_follows() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        let writer = Arc::new(RecordingWriter::new(conn));
        let mut sink = TurnRowSink::new(Arc::clone(&writer) as Arc<dyn TurnRowStore>, "s1", None);

        sink.observe(&metric_record(Role::User));
        sink.observe(&content_record("hello"));
        sink.observe(&metric_record(Role::Assistant));
        sink.finish(SessionSummary::default());

        assert_eq!(writer.count(1), 2);
        assert_eq!(writer.content_count(1), 1);
    }

    #[test]
    fn content_still_attaches_after_the_batch_boundary_flushes_earlier_rows() {
        let conn = test_connection();
        let key = TurnSessionKey {
            environment_key: "native",
            agent: "claude",
            session_id: "s1",
        };
        insert_session(&conn, &key);
        let writer = Arc::new(RecordingWriter::new(conn));
        let mut sink = TurnRowSink::with_batch_size(
            Arc::clone(&writer) as Arc<dyn TurnRowStore>,
            "s1",
            None,
            2,
        );

        sink.observe(&metric_record(Role::Assistant));
        sink.observe(&metric_record(Role::Assistant));
        // Pushing a third row over `batch_size` flushes the first two,
        // keeping only this one buffered for its content to attach to.
        sink.observe(&metric_record(Role::Assistant));
        assert_eq!(sink.buffer.len(), 1);
        sink.observe(&content_record("hello"));
        sink.finish(SessionSummary::default());

        assert_eq!(writer.count(1), 3);
        assert_eq!(writer.content_count(1), 1);
    }
}

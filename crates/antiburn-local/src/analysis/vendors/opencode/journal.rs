//! Bounded delivery from an immutable SQLite journal snapshot.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;

use anyhow::ensure;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{db_session_has_parent_id, open_database, request_context, table_columns};
use crate::analysis::interface::{MAX_CONTENT_PART_BYTES, ToolResultContext};
use crate::analysis::records::tool_call_from_input;
use crate::analysis::{
    EVIDENCE_STRING_CAP, MAX_RECORD_BYTES, MAX_TOOL_RESULT_BYTES, MAX_TOOL_RESULT_OBSERVATIONS,
    PARSER_REVISION, SourceFormat, ToolResultChange, ToolResultCompleteness, ToolResultObservation,
    ToolResultPosition, ToolResultStatus,
};

mod main_64;
mod public;
use public::PublicState;

const PAGE_EVENTS: u64 = 4096;
const CONTEXT_BYTES: usize = MAX_TOOL_RESULT_BYTES / 2;

/// Persist this state in the same transaction that publishes its page.
/// Replay validates a prefix hash. Live delivery retains bounded parser state
/// and validates sampled origin and boundary events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeJournalCursor {
    session_id: String,
    owner_id: Option<String>,
    through_sequence: u64,
    prefix_hash: [u8; 32],
    parser_revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    live: Option<LiveState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cold_replay: Option<ColdReplay>,
    #[serde(skip)]
    main_64: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColdReplay {
    output_sha256: [u8; 32],
    assistant_projections: BTreeMap<String, Option<AssistantProjection>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssistantProjection {
    parent_id: Value,
}

struct AssistantProjectionInputs {
    values: BTreeMap<String, Option<AssistantProjection>>,
    frozen: bool,
}

impl OpenCodeJournalCursor {
    /// Decode only main parser 64's result-only live journal state 2.
    pub fn decode_main_64_json(bytes: &[u8]) -> anyhow::Result<Self> {
        main_64::decode(bytes)
    }

    /// Keep the read sequence and the validated conversion frontier on retry.
    pub fn main_64_retry_json(&self, page: &Self) -> anyhow::Result<Option<String>> {
        if !self.main_64 {
            return Ok(None);
        }
        ensure!(
            self.session_id == page.session_id
                && self.owner_id == page.owner_id
                && self.through_sequence <= page.through_sequence,
            OpenCodeJournalReadError::SourceChanged
        );
        let frontier = page
            .public_baseline()
            .ok_or(OpenCodeJournalReadError::SourceChanged)?;
        let mut cursor = self.clone();
        cursor.live = Some(convert_live_v4(
            cursor
                .live
                .take()
                .ok_or(OpenCodeJournalReadError::SourceChanged)?,
            frontier,
        ));
        cursor.parser_revision = PARSER_REVISION;
        cursor.main_64 = false;
        Ok(Some(serde_json::to_string(&cursor)?))
    }
    /// The live window boundary replaces a legacy replay baseline on attachment.
    pub fn live_baseline(&self) -> Option<u64> {
        self.live.as_ref().map(|state| state.baseline)
    }

    /// New message kinds never use the older result seed boundary.
    pub fn public_baseline(&self) -> Option<u64> {
        self.live.as_ref().and_then(|state| state.public_baseline)
    }
}

/// One bounded delivery page. An empty page can still advance the watermark.
#[derive(Debug, Clone)]
pub struct OpenCodeToolResultPage {
    pub results: Vec<ToolResultObservation>,
    pub evidence: crate::analysis::NativeEvidenceBatch,
    /// Ordered parent-message deletions. The publisher resolves durable tool bindings.
    pub message_removals: Vec<OpenCodeMessageRemoval>,
    pub cursor: OpenCodeJournalCursor,
    pub has_more: bool,
    /// The transaction's initial native frontier, including events beyond this page.
    pub snapshot_end: u64,
    /// Page limits do not imply a source identity, order, or ownership gap.
    pub history_complete: bool,
    /// True only when this page contains the source's entire result membership.
    /// Partial pages must never infer removals from absent identities.
    pub complete: bool,
    /// Live input counters exclude fixed-size boundary and ownership checks.
    /// Strict replay does not collect these counters.
    pub live_input_events: usize,
    pub live_input_bytes: u64,
    assistant_projections: BTreeMap<String, Option<AssistantProjection>>,
}

impl OpenCodeToolResultPage {
    /// Retain the cold frontier and verify the same output on retry.
    pub fn cold_retry_json(&self) -> anyhow::Result<Option<String>> {
        let Some(live) = &self.cursor.live else {
            return Ok(None);
        };
        ensure!(
            self.cursor.parser_revision == PARSER_REVISION
                && live.revision == LIVE_STATE_REVISION
                && live.public_baseline == Some(self.cursor.through_sequence)
                && self.cursor.cold_replay.is_none()
                && !self.cursor.main_64,
            OpenCodeJournalReadError::SourceChanged
        );
        let mut cursor = self.cursor.clone();
        cursor.cold_replay = Some(ColdReplay {
            output_sha256: self.cold_output_digest(PARSER_REVISION)?,
            assistant_projections: self.assistant_projections.clone(),
        });
        let encoded = serde_json::to_string(&cursor)?;
        ensure!(
            encoded.len() <= 1024 * 1024,
            "OpenCode live cursor exceeds its output budget"
        );
        Ok(Some(encoded))
    }

    fn cold_output_digest(&self, parser_revision: i64) -> anyhow::Result<[u8; 32]> {
        let mut cursor = self.cursor.clone();
        cursor.parser_revision = parser_revision;
        Ok(Sha256::digest(serde_json::to_vec(&(
            &cursor,
            &self.evidence,
            self.history_complete,
            &self.assistant_projections,
        ))?)
        .into())
    }
}

/// A validated whole-message deletion from the live journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCodeMessageRemoval {
    pub message_id: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCodeJournalReadError {
    SourceChanged,
    IncompleteHistory,
    Unavailable,
}

impl std::fmt::Display for OpenCodeJournalReadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::SourceChanged => "OpenCode journal prefix changed",
            Self::IncompleteHistory => "OpenCode journal has an unresolved sequence gap",
            Self::Unavailable => "OpenCode has no supported event journal",
        })
    }
}

impl std::error::Error for OpenCodeJournalReadError {}

/// Read the next page without changing the source database.
/// Reject a rewritten, removed, or reset prefix. A caller must not advance its
/// persisted cursor until its result publication commits.
pub fn read_opencode_tool_result_page(
    path: &Path,
    session_id: &str,
    after: Option<&OpenCodeJournalCursor>,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<OpenCodeToolResultPage> {
    read_page(path, session_id, after, ReadMode::Page, cancel)
}

/// Attach to one recent window, then consume only indexed journal suffixes.
/// Persist the returned cursor with its results. This reader does not audit old
/// payloads. The strict replay reader retains full-prefix validation.
pub fn read_opencode_live_tool_result_page(
    path: &Path,
    session_id: &str,
    after: Option<&OpenCodeJournalCursor>,
    _baseline: Option<u64>,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<OpenCodeToolResultPage> {
    let connection = open_database(path)?;
    connection.execute_batch("BEGIN")?;
    let page = read_live(&connection, session_id, after, cancel)?;
    connection.execute_batch("COMMIT")?;
    Ok(page)
}

/// Check the producer frontier without claiming that the consumed prefix is unchanged.
pub fn opencode_journal_has_new_events(
    path: &Path,
    session_id: &str,
    cursor: &OpenCodeJournalCursor,
) -> anyhow::Result<bool> {
    let connection = open_database(path)?;
    require_live_index(&connection)?;
    let state = metadata(&connection, session_id)?.ok_or(OpenCodeJournalReadError::Unavailable)?;
    if let Some(live) = &cursor.live {
        validate_live(&connection, session_id, cursor, live, &state)?;
    }
    Ok(cursor.session_id != session_id
        || cursor.owner_id != state.owner
        || cursor.parser_revision != PARSER_REVISION
        || cursor.live.is_none()
        || cursor.cold_replay.is_some()
        || cursor.through_sequence != state.latest)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadMode {
    Page,
    Tail,
}

fn read_page(
    path: &Path,
    session_id: &str,
    after: Option<&OpenCodeJournalCursor>,
    mode: ReadMode,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<OpenCodeToolResultPage> {
    let connection = open_database(path)?;
    connection.execute_batch("BEGIN")?;
    let page = read(&connection, session_id, after, mode, cancel)?
        .ok_or(OpenCodeJournalReadError::Unavailable)?;
    connection.execute_batch("COMMIT")?;
    Ok(page)
}

pub(super) fn has_schema(connection: &Connection) -> bool {
    [
        ("event_sequence", &["aggregate_id", "seq", "owner_id"][..]),
        ("event", &["id", "aggregate_id", "seq", "type", "data"][..]),
    ]
    .iter()
    .all(|(table, required)| {
        table_columns(connection, table)
            .is_ok_and(|columns| required.iter().all(|column| columns.contains(*column)))
    })
}

struct Event {
    sequence: u64,
    id: String,
    kind: String,
    data: Option<String>,
    bytes: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventHeader {
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
    info: Option<MessageHeader>,
    part: Option<PartHeader>,
    #[serde(rename = "messageID")]
    message_id: Option<String>,
    #[serde(rename = "partID")]
    part_id: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct MessageHeader {
    id: Option<String>,
    role: Option<String>,
    #[serde(rename = "parentID")]
    parent_id: Option<String>,
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
    #[serde(rename = "providerID")]
    provider_id: Option<Value>,
    #[serde(rename = "modelID")]
    model_id: Option<Value>,
    path: Option<Value>,
}

#[derive(Deserialize, Serialize)]
struct PartHeader {
    id: Option<String>,
    #[serde(rename = "messageID")]
    message_id: Option<String>,
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

fn header(event: &Event) -> Option<EventHeader> {
    serde_json::from_str(event.data.as_deref()?).ok()
}

fn context_data(
    event: &Event,
    requests: &request_context::RequestHistory,
    selected: Option<&HashSet<String>>,
) -> Option<Value> {
    let header = header(event)?;
    let full_part = header.part.as_ref().is_some_and(|part| {
        part.id
            .as_ref()
            .is_some_and(|id| selected.is_some_and(|ids| ids.contains(id)))
            || part
                .message_id
                .as_deref()
                .is_some_and(|id| requests.is_user_message(id))
    });
    if full_part {
        serde_json::from_str(event.data.as_deref()?).ok()
    } else {
        Some(serde_json::to_value(header).expect("journal header encodes"))
    }
}

fn events(connection: &Connection) -> rusqlite::Result<rusqlite::Statement<'_>> {
    connection.prepare(
        "SELECT seq,
                CASE WHEN length(CAST(id AS BLOB)) <= 512 THEN id END,
                CASE WHEN length(CAST(type AS BLOB)) <= 512 THEN type END,
                CASE WHEN length(CAST(data AS BLOB)) <= ?3 THEN data END,
                length(CAST(data AS BLOB))
           FROM event WHERE aggregate_id = ?1 AND seq <= ?2 ORDER BY seq",
    )
}

fn event(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    Ok(Event {
        sequence: row.get(0)?,
        id: row.get(1)?,
        kind: row.get(2)?,
        data: row.get(3)?,
        bytes: row.get(4)?,
    })
}

fn hash_event(
    connection: &Connection,
    session: &str,
    hash: &mut [u8; 32],
    event: &Event,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<()> {
    let mut event_hash = Sha256::new();
    event_hash.update(event.sequence.to_le_bytes());
    for text in [&event.id, &event.kind] {
        event_hash.update((text.len() as u64).to_le_bytes());
        event_hash.update(text.as_bytes());
    }
    event_hash.update(event.bytes.to_le_bytes());
    if let Some(data) = &event.data {
        event_hash.update(data.as_bytes());
    } else {
        let mut offset = 0;
        while offset < event.bytes {
            ensure!(!cancel(), "OpenCode journal hash was cancelled");
            let chunk: Vec<u8> = connection.query_row(
                "SELECT substr(CAST(data AS BLOB), ?3, 65536) FROM event
                  WHERE aggregate_id = ?1 AND seq = ?2",
                params![session, event.sequence, offset + 1],
                |row| row.get(0),
            )?;
            ensure!(
                !chunk.is_empty(),
                "OpenCode journal data ended before its recorded length"
            );
            offset += chunk.len() as u64;
            event_hash.update(&chunk);
        }
    }
    let mut chain = Sha256::new();
    chain.update(hash.as_slice());
    chain.update(event_hash.finalize());
    *hash = chain.finalize().into();
    Ok(())
}

fn identity(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= EVIDENCE_STRING_CAP)
}

fn part_id(kind: &str, data: &Value) -> Option<String> {
    let id = match kind {
        "message.part.updated.1" => &data["part"]["id"],
        "message.part.removed.1" => &data["partID"],
        _ => return None,
    };
    identity(id).map(str::to_owned)
}

struct JournalMetadata {
    latest: u64,
    owner: Option<String>,
    root: bool,
    all_events_visible: bool,
}

fn metadata(connection: &Connection, session: &str) -> anyhow::Result<Option<JournalMetadata>> {
    if !has_schema(connection) {
        return Ok(None);
    }
    ensure!(
        !session.is_empty() && session.len() <= EVIDENCE_STRING_CAP,
        "OpenCode session identity exceeds its bound"
    );
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM session WHERE id = ?1)",
        [session],
        |row| row.get(0),
    )?;
    ensure!(exists, "OpenCode journal has no owning session");
    let root = !db_session_has_parent_id(connection)
        || connection.query_row(
            "SELECT parent_id IS NULL FROM session WHERE id = ?1",
            [session],
            |row| row.get::<_, bool>(0),
        )?;
    let metadata: Option<(u64, Option<String>)> = connection
        .query_row(
            "SELECT seq, owner_id FROM event_sequence WHERE aggregate_id = ?1
              AND (owner_id IS NULL OR length(CAST(owner_id AS BLOB)) <= ?2)",
            params![session, EVIDENCE_STRING_CAP],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((advertised, owner)) = metadata else {
        return Ok(None);
    };
    ensure!(
        advertised < i64::MAX as u64,
        "OpenCode journal sequence exceeds SQLite range"
    );
    let actual: Option<u64> = connection.query_row(
        "SELECT MAX(seq) FROM event WHERE aggregate_id = ?1",
        [session],
        |row| row.get(0),
    )?;
    let Some(actual) = actual else {
        return Ok(None);
    };
    // A reserved sequence does not prove that its event is available.
    let latest = advertised.min(actual);
    Ok(Some(JournalMetadata {
        latest,
        owner,
        root,
        all_events_visible: actual == latest,
    }))
}

fn select_parts(
    connection: &Connection,
    session: &str,
    after: Option<&OpenCodeJournalCursor>,
    range: std::ops::RangeInclusive<u64>,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<(HashSet<String>, bool, [u8; 32])> {
    let mut statement = events(connection)?;
    let mut rows = statement.query(params![session, range.end(), MAX_RECORD_BYTES])?;
    let mut selected = HashSet::new();
    let mut expected = 0;
    let mut ordered = true;
    let mut hash = [0; 32];
    let mut cursor_verified = after.is_none();
    let mut prefix_hash = [0; 32];
    while let Some(row) = rows.next()? {
        ensure!(!cancel(), "OpenCode journal read was cancelled");
        let event = event(row)?;
        ordered &= event.sequence == expected;
        expected = event.sequence.saturating_add(1);
        hash_event(connection, session, &mut hash, &event, cancel)?;
        if event.sequence.saturating_add(1) == *range.start() {
            prefix_hash = hash;
        }
        if let Some(cursor) = after
            && cursor.through_sequence == event.sequence
        {
            ensure!(
                hash == cursor.prefix_hash,
                OpenCodeJournalReadError::SourceChanged
            );
            cursor_verified = true;
        }
        let data = header(&event)
            .map(|header| serde_json::to_value(header).expect("journal header encodes"));
        let Some(data) = data else {
            ordered = false;
            continue;
        };
        ordered &= valid_event(&event.kind, &data, session);
        if range.contains(&event.sequence)
            && let Some(id) = part_id(&event.kind, &data)
        {
            selected.insert(id);
        }
    }
    ensure!(cursor_verified, OpenCodeJournalReadError::SourceChanged);
    ordered &= expected == range.end().saturating_add(1);
    if *range.start() == 0 {
        prefix_hash = [0; 32];
    }
    Ok((selected, ordered, prefix_hash))
}

pub(super) fn read(
    connection: &Connection,
    session: &str,
    after: Option<&OpenCodeJournalCursor>,
    mode: ReadMode,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<Option<OpenCodeToolResultPage>> {
    let Some(JournalMetadata {
        latest,
        owner,
        root,
        all_events_visible,
    }) = metadata(connection, session)?
    else {
        return Ok(None);
    };
    if let Some(cursor) = after {
        ensure!(
            cursor.live.is_none()
                && compatible_parser_revision(cursor.parser_revision)
                && cursor.session_id == session
                && cursor.owner_id == owner
                && cursor.through_sequence <= latest,
            OpenCodeJournalReadError::SourceChanged
        );
    }
    let tail = mode == ReadMode::Tail;
    let start = match mode {
        ReadMode::Tail => latest.saturating_sub(PAGE_EVENTS - 1),
        ReadMode::Page => after.map_or(0, |cursor| cursor.through_sequence + 1),
    };
    let end = if tail {
        latest
    } else {
        latest.min(start.saturating_add(PAGE_EVENTS - 1))
    };
    let (selected, ordered, prefix_hash) =
        select_parts(connection, session, after, start..=end, cancel)?;
    let mut statement = events(connection)?;
    let mut rows = statement.query(params![session, end, MAX_RECORD_BYTES])?;
    let mut requests = request_context::RequestHistory::default();
    let mut parts = HashMap::<String, PartState>::new();
    let mut results = VecDeque::new();
    let mut bytes = 0;
    let mut omitted = start != 0;
    let mut source_gap = false;
    let mut through = start.saturating_sub(1);
    let mut hash = prefix_hash;
    let mut accepted_hash = prefix_hash;
    let mut advanced = false;
    while let Some(row) = rows.next()? {
        ensure!(!cancel(), "OpenCode journal read was cancelled");
        let event = event(row)?;
        let data = context_data(&event, &requests, Some(&selected));
        let result = if let Some(data) = data {
            requests.observe(&event.kind, &data, session, event.sequence);
            if let Some(id) = part_id(&event.kind, &data).filter(|id| selected.contains(id)) {
                let state = parts.entry(id.clone()).or_default();
                if state.first_terminal.is_none() && data["part"]["type"] == "tool" {
                    state.facts = data["part"]["messageID"]
                        .as_str()
                        .zip(data["part"]["tool"].as_str())
                        .map(|(message, name)| {
                            requests.tool_facts(
                                session,
                                message,
                                name,
                                data["part"]["state"].get("input"),
                            )
                        });
                }
                let mut result = state.observe(&event.kind, &data, session, &id, event.sequence);
                source_gap |= state.invalid
                    || (event.kind == "message.part.removed.1" && state.identity.is_none());
                if result.as_ref().is_some_and(|result| result.finality)
                    && state.first_terminal == Some(event.sequence)
                    && state.context.is_none()
                {
                    let message = result
                        .as_ref()
                        .and_then(|result| result.record_id.as_deref())
                        .expect("a journal result retains its message identity");
                    let mut context = requests.freeze(session, message, event.sequence);
                    context.main_session &= root;
                    state.context = Some(bound_context(context));
                }
                if event.sequence >= start
                    && let Some(result) = &mut result
                {
                    if result.finality
                        || result.change == ToolResultChange::Removal
                        || state.first_terminal.is_some()
                    {
                        result.context = state.context.clone();
                    } else {
                        result.context = Some(ToolResultContext::pending(
                            result.position,
                            root && requests.main_session(),
                        ));
                    }
                    if !ordered && result.completeness != ToolResultCompleteness::UnsafeIdentity {
                        result.completeness = ToolResultCompleteness::UnsafeOrder;
                    }
                    if result
                        .context
                        .as_ref()
                        .is_some_and(|context| !context.main_session)
                        && matches!(
                            result.completeness,
                            ToolResultCompleteness::Complete | ToolResultCompleteness::Incomplete
                        )
                    {
                        result.completeness = ToolResultCompleteness::UnsafeOwnership;
                    }
                    bound_content(result);
                }
                result.filter(|_| event.sequence >= start)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(result) = result {
            let size = serde_json::to_vec(&result)?.len();
            if !tail
                && (results.len() >= MAX_TOOL_RESULT_OBSERVATIONS
                    || bytes + size > MAX_TOOL_RESULT_BYTES)
            {
                break;
            }
            while tail
                && !results.is_empty()
                && (results.len() >= MAX_TOOL_RESULT_OBSERVATIONS
                    || bytes + size > MAX_TOOL_RESULT_BYTES)
            {
                let removed = results.pop_front().expect("the window is not empty");
                bytes -= serde_json::to_vec(&removed)?.len();
                omitted = true;
            }
            ensure!(
                size <= MAX_TOOL_RESULT_BYTES,
                "OpenCode result exceeds its bounded page"
            );
            bytes += size;
            results.push_back(result);
        }
        if event.sequence >= start {
            hash_event(connection, session, &mut hash, &event, cancel)?;
        }
        through = event.sequence;
        if event.sequence >= start {
            advanced = true;
            accepted_hash = hash;
        }
    }
    ensure!(
        tail || through == latest || advanced,
        OpenCodeJournalReadError::IncompleteHistory
    );
    let evidence = crate::analysis::NativeEvidenceBatch {
        source_format: SourceFormat::OpenCodeSqliteV1,
        events: results
            .iter()
            .cloned()
            .map(|result| crate::analysis::NativeEvidenceEvent {
                fact: crate::analysis::NativeEvidenceFact::Message(
                    crate::analysis::NativeMessageObservation::ToolResult(result),
                ),
                live_eligible: true,
            })
            .collect(),
        history_complete: root && requests.main_session() && ordered && !source_gap,
    };
    Ok(Some(OpenCodeToolResultPage {
        evidence,
        results: results.into(),
        message_removals: Vec::new(),
        cursor: OpenCodeJournalCursor {
            session_id: session.to_owned(),
            owner_id: owner,
            through_sequence: through,
            prefix_hash: accepted_hash,
            parser_revision: PARSER_REVISION,
            live: None,
            cold_replay: None,
            main_64: false,
        },
        has_more: through < latest,
        snapshot_end: latest,
        history_complete: root && requests.main_session() && ordered && !source_gap,
        complete: all_events_visible
            && root
            && requests.main_session()
            && !omitted
            && !source_gap
            && ordered
            && through == latest,
        live_input_events: 0,
        live_input_bytes: 0,
        assistant_projections: BTreeMap::new(),
    }))
}

const LIVE_STATE_REVISION: u32 = 5;
const LIVE_INPUT_BYTES: u64 = 8 * 1024 * 1024;
const LIVE_RESULTS: usize = 40;
const LIVE_PARTS: usize = 128;
const LIVE_PART_BYTES: usize = 128 * 1024;
const LIVE_REQUEST_RECORDS: usize = 128;
const LIVE_REQUEST_BYTES: usize = 64 * 1024;
const LIVE_CONTEXT_BYTES: usize = 256 * 1024;
const LIVE_CONTEXTS: usize = 16;
const ASSISTANT_PROJECTION_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LiveState {
    revision: u32,
    baseline: u64,
    origin: [u8; 32],
    boundary: [u8; 32],
    requests: request_context::RequestHistory,
    parts: BTreeMap<String, LivePart>,
    contexts: VecDeque<(u64, ToolResultContext)>,
    next_context: u64,
    history_safe: bool,
    evidence_gap: bool,
    #[serde(default)]
    public: PublicState,
    #[serde(default)]
    public_baseline: Option<u64>,
}

impl LiveState {
    fn has_unsafe_ownership(&self) -> bool {
        // Message removals use the same flag but have their own exact removal events.
        self.parts.values().any(|part| {
            part.unsafe_ownership
                && part
                    .state
                    .identity
                    .as_ref()
                    .is_some_and(|(owner, _, _, _)| !self.requests.is_removed_message(owner))
        })
    }
}

/// Convert only the immediate result-only predecessor after source validation.
fn convert_live_v4(mut live: LiveState, frontier: u64) -> LiveState {
    live.revision = LIVE_STATE_REVISION;
    live.public = PublicState::default();
    live.public.seed_historical_users(&live.requests, frontier);
    live.public_baseline = Some(frontier);
    live
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LivePart {
    state: PartState,
    context: Option<u64>,
    last_sequence: u64,
    unsafe_ownership: bool,
}

impl LivePart {
    fn retained_bytes(&self, id: &str) -> usize {
        id.len()
            + 128
            + self
                .state
                .facts
                .as_ref()
                .map_or(0, crate::analysis::NativeToolFacts::retained_bytes)
            + self
                .state
                .identity
                .as_ref()
                .map_or(0, |(message, call, tool, kind)| {
                    message.len()
                        + call.as_ref().map_or(0, String::len)
                        + tool.as_ref().map_or(0, String::len)
                        + kind.len()
                })
    }
}

fn require_live_index(connection: &Connection) -> anyhow::Result<()> {
    let unique: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_index_list('event')
          WHERE name = 'event_aggregate_seq_idx' AND \"unique\" = 1 AND partial = 0)",
        [],
        |row| row.get(0),
    )?;
    let mut statement = connection
        .prepare("SELECT name FROM pragma_index_info('event_aggregate_seq_idx') ORDER BY seqno")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        unique && columns == ["aggregate_id", "seq"],
        OpenCodeJournalReadError::Unavailable
    );
    Ok(())
}

/// Samples only the boundary payload. This does not audit the consumed prefix.
fn live_boundary(
    connection: &Connection,
    session: &str,
    sequence: u64,
) -> anyhow::Result<[u8; 32]> {
    let boundary = connection
        .query_row(
        "SELECT id, type, octet_length(data),
                CASE WHEN octet_length(data) <= ?3 THEN substr(CAST(data AS BLOB), 1, 4096) ELSE x'' END,
                CASE WHEN octet_length(data) <= ?3 THEN substr(CAST(data AS BLOB), -4096) ELSE x'' END
           FROM event INDEXED BY event_aggregate_seq_idx WHERE aggregate_id = ?1 AND seq = ?2
             AND length(CAST(id AS BLOB)) <= 512 AND length(CAST(type AS BLOB)) <= 512",
        params![session, sequence, LIVE_INPUT_BYTES],
            |row| {
                let id: String = row.get(0)?;
                let kind: String = row.get(1)?;
                let bytes: u64 = row.get(2)?;
                let head: Vec<u8> = row.get(3)?;
                let tail: Vec<u8> = row.get(4)?;
                let mut hash = Sha256::new();
                hash.update(sequence.to_le_bytes());
                hash.update((id.len() as u64).to_le_bytes());
                hash.update(id.as_bytes());
                hash.update((kind.len() as u64).to_le_bytes());
                hash.update(kind.as_bytes());
                hash.update(bytes.to_le_bytes());
                hash.update(head);
                hash.update(tail);
                Ok(hash.finalize().into())
            },
        )
        .optional()?;
    boundary.ok_or_else(|| OpenCodeJournalReadError::SourceChanged.into())
}

fn compatible_parser_revision(revision: i64) -> bool {
    revision == PARSER_REVISION || matches!(revision, 63..=66)
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;

    #[test]
    fn known_parser_numbers_do_not_admit_main_journal_state_two() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE event (id TEXT, type TEXT, aggregate_id TEXT, seq INTEGER, data TEXT);
                 CREATE UNIQUE INDEX event_aggregate_seq_idx ON event(aggregate_id, seq);
                 INSERT INTO event VALUES ('initial', 'session.created.1', 'root', 0, '{}');",
            )
            .unwrap();
        let boundary = live_boundary(&connection, "root", 0).unwrap();
        let current = LiveState {
            revision: LIVE_STATE_REVISION,
            baseline: 0,
            origin: boundary,
            boundary,
            requests: Default::default(),
            parts: Default::default(),
            contexts: Default::default(),
            next_context: 0,
            history_safe: true,
            evidence_gap: false,
            public: Default::default(),
            public_baseline: Some(0),
        };
        let metadata = JournalMetadata {
            latest: 0,
            owner: None,
            root: true,
            all_events_visible: true,
        };
        for parser_revision in [63, 64, 65, 66, PARSER_REVISION] {
            let cursor = OpenCodeJournalCursor {
                session_id: "root".into(),
                owner_id: None,
                through_sequence: 0,
                prefix_hash: [0; 32],
                parser_revision,
                live: Some(current.clone()),
                cold_replay: None,
                main_64: false,
            };
            validate_live(&connection, "root", &cursor, &current, &metadata).unwrap();
            for revision in [2, 4, 6] {
                let mut incompatible = current.clone();
                incompatible.revision = revision;
                let error = validate_live(&connection, "root", &cursor, &incompatible, &metadata)
                    .unwrap_err();
                assert_eq!(
                    error.downcast_ref::<OpenCodeJournalReadError>(),
                    Some(&OpenCodeJournalReadError::SourceChanged)
                );
            }
        }
    }
}

fn validate_live(
    connection: &Connection,
    session: &str,
    cursor: &OpenCodeJournalCursor,
    live: &LiveState,
    metadata: &JournalMetadata,
) -> anyhow::Result<()> {
    ensure!(
        cursor.session_id == session
            && cursor.owner_id == metadata.owner
            && ((compatible_parser_revision(cursor.parser_revision)
                && live.revision == LIVE_STATE_REVISION
                && live
                    .public_baseline
                    .is_some_and(|baseline| baseline <= metadata.latest))
                || (cursor.parser_revision == 62 && live.revision == 4)
                || (cursor.main_64
                    && metadata.root
                    && cursor.parser_revision == 64
                    && live.revision == 2
                    && live.public_baseline.is_none()
                    && live.public == PublicState::default()))
            && cursor.through_sequence <= metadata.latest
            && cursor.cold_replay.as_ref().is_none_or(|replay| {
                matches!(cursor.parser_revision, 66 | PARSER_REVISION)
                    && live.revision == LIVE_STATE_REVISION
                    && live.baseline <= cursor.through_sequence
                    && live.public_baseline == Some(cursor.through_sequence)
                    && !cursor.main_64
                    && replay.assistant_projections.len() <= LIVE_RESULTS
                    && replay
                        .assistant_projections
                        .iter()
                        .all(|(message, projection)| {
                            !message.is_empty()
                                && message.len() <= EVIDENCE_STRING_CAP
                                && projection.as_ref().is_none_or(|projection| {
                                    projection.parent_id.to_string().len()
                                        <= ASSISTANT_PROJECTION_BYTES
                                })
                        })
            })
            && live.origin == live_boundary(connection, session, 0)?
            && live.boundary == live_boundary(connection, session, cursor.through_sequence)?,
        OpenCodeJournalReadError::SourceChanged
    );
    Ok(())
}

fn live_events(
    connection: &Connection,
    session: &str,
    after: Option<u64>,
    latest: u64,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<(Vec<Event>, u64)> {
    let sql = if after.is_some() {
        "SELECT seq, CASE WHEN octet_length(id) <= 512 THEN id END,
         CASE WHEN octet_length(type) <= 512 THEN type END, NULL, octet_length(data) FROM event INDEXED BY event_aggregate_seq_idx
         WHERE aggregate_id = ?1 AND seq >= ?2 AND seq <= ?3 ORDER BY seq LIMIT ?4"
    } else {
        "SELECT seq, CASE WHEN octet_length(id) <= 512 THEN id END,
         CASE WHEN octet_length(type) <= 512 THEN type END, NULL, octet_length(data) FROM event INDEXED BY event_aggregate_seq_idx
         WHERE aggregate_id = ?1 AND seq >= ?2 AND seq <= ?3 ORDER BY seq DESC LIMIT ?4"
    };
    let mut statement = connection.prepare(sql)?;
    let mut rows = statement.query(params![
        session,
        after.map_or(0, |seq| seq + 1),
        latest,
        PAGE_EVENTS
    ])?;
    let mut events = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        ensure!(!cancel(), "OpenCode live read was cancelled");
        let mut event = event(row)?;
        if event.bytes <= LIVE_INPUT_BYTES && bytes + event.bytes > LIVE_INPUT_BYTES {
            break;
        }
        ensure!(
            event.id.len() <= 512 && event.kind.len() <= 512,
            OpenCodeJournalReadError::IncompleteHistory
        );
        if event.bytes <= LIVE_INPUT_BYTES {
            event.data = Some(connection.query_row(
                "SELECT data FROM event INDEXED BY event_aggregate_seq_idx WHERE aggregate_id = ?1 AND seq = ?2",
                params![session, event.sequence], |row| row.get(0))?);
            bytes += event.bytes;
        }
        events.push(event);
    }
    if after.is_none() {
        events.reverse();
    }
    Ok((events, bytes))
}

fn read_live(
    connection: &Connection,
    session: &str,
    after: Option<&OpenCodeJournalCursor>,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<OpenCodeToolResultPage> {
    require_live_index(connection)?;
    let metadata = metadata(connection, session)?.ok_or(OpenCodeJournalReadError::Unavailable)?;
    let cold_replay = after.and_then(|cursor| cursor.cold_replay.as_ref());
    ensure!(
        cold_replay.is_none() || after.is_some_and(|cursor| cursor.live.is_some()),
        OpenCodeJournalReadError::SourceChanged
    );
    let restored = after.and_then(|cursor| cursor.live.as_ref().map(|live| (cursor, live)));
    if let Some((cursor, live)) = restored {
        validate_live(connection, session, cursor, live, &metadata)?;
    } else if let Some(cursor) = after {
        ensure!(
            cursor.session_id == session
                && cursor.owner_id == metadata.owner
                && (compatible_parser_revision(cursor.parser_revision)
                    || cursor.parser_revision == 62)
                && cursor.through_sequence <= metadata.latest,
            OpenCodeJournalReadError::SourceChanged
        );
    }
    let latest = if cold_replay.is_some() {
        after.expect("a cold replay has a cursor").through_sequence
    } else {
        metadata.latest
    };
    // Rebuild the original bounded window before reading its appended suffix.
    let restored = restored.filter(|_| cold_replay.is_none());
    let (events, input_bytes) = live_events(
        connection,
        session,
        restored.map(|(cursor, _)| cursor.through_sequence),
        latest,
        cancel,
    )?;
    let cold = restored.is_none();
    let mut assistant_projections = AssistantProjectionInputs {
        values: cold_replay
            .map(|replay| replay.assistant_projections.clone())
            .unwrap_or_default(),
        frozen: cold_replay.is_some(),
    };
    let selected = cold.then(|| select_live_window(&events));
    let mut live = match restored {
        Some((cursor, state)) if state.revision == 4 || cursor.main_64 => {
            convert_live_v4(state.clone(), metadata.latest)
        }
        Some((_, state)) => state.clone(),
        None => LiveState {
            revision: LIVE_STATE_REVISION,
            baseline: events
                .first()
                .map_or(latest, |event| event.sequence.saturating_sub(1)),
            origin: live_boundary(connection, session, 0)?,
            boundary: [0; 32],
            requests: request_context::RequestHistory::partial(metadata.root),
            parts: BTreeMap::new(),
            contexts: VecDeque::new(),
            next_context: 0,
            history_safe: true,
            evidence_gap: false,
            public: PublicState::default(),
            public_baseline: Some(latest),
        },
    };
    let mut through = restored.map_or(0, |(cursor, _)| cursor.through_sequence);
    let expected_start = restored.map_or_else(
        || events.first().map_or(0, |event| event.sequence),
        |(cursor, _)| cursor.through_sequence + 1,
    );
    let mut results = VecDeque::new();
    let mut message_removals = Vec::new();
    let mut output_bytes = 0;
    let mut evidence = Vec::new();
    let mut unsafe_ownership = live.has_unsafe_ownership();
    for (index, event) in events.iter().enumerate() {
        ensure!(!cancel(), "OpenCode live read was cancelled");
        ensure!(
            event.sequence == expected_start + index as u64,
            OpenCodeJournalReadError::IncompleteHistory
        );
        if !cold && evidence.len() >= LIVE_RESULTS {
            break;
        }
        let previous_live = (!cold).then(|| live.clone());
        let mut next_live = live;
        let result = observe_live(
            connection,
            session,
            &mut next_live,
            event,
            selected.as_ref(),
            metadata.root,
            &mut assistant_projections,
        )?;
        let public_baseline = next_live.public_baseline.unwrap_or(latest);
        let mut next_public = std::mem::take(&mut next_live.public);
        let mut event_evidence = next_public.observe(
            session,
            event,
            &next_live.requests,
            metadata.root && next_live.history_safe,
            public_baseline,
        );
        next_live.public = next_public;
        for fact in &mut event_evidence {
            let first = match &fact.fact {
                crate::analysis::NativeEvidenceFact::Message(
                    crate::analysis::NativeMessageObservation::User(user),
                ) => user.context.first_eligible_position,
                crate::analysis::NativeEvidenceFact::Message(
                    crate::analysis::NativeMessageObservation::Public(public),
                ) => public
                    .context
                    .as_ref()
                    .map_or(public.position, |context| context.first_eligible_position),
                _ => ToolResultPosition::OpenCodeEvent {
                    sequence: event.sequence,
                    item_index: 0,
                },
            };
            fact.live_eligible = matches!(first, ToolResultPosition::OpenCodeEvent { sequence, .. } if sequence > public_baseline);
        }
        if let Some(result) = &result {
            event_evidence.push(crate::analysis::NativeEvidenceEvent {
                fact: crate::analysis::NativeEvidenceFact::Message(
                    crate::analysis::NativeMessageObservation::ToolResult(result.clone()),
                ),
                live_eligible: event.sequence > next_live.baseline,
            });
        }
        if cold {
            event_evidence.clear();
        }
        let mut event_bytes = serde_json::to_vec(&event_evidence)?.len();
        if !cold
            && (!evidence.is_empty())
            && (evidence.len() + event_evidence.len() > LIVE_RESULTS
                || output_bytes + event_bytes > MAX_TOOL_RESULT_BYTES)
        {
            live = previous_live.expect("a live page retains the prior event state");
            break;
        }
        if event_evidence.len() > LIVE_RESULTS || event_bytes > MAX_TOOL_RESULT_BYTES {
            next_live.evidence_gap = true;
            event_evidence.clear();
            event_bytes = 0;
        }
        live = next_live;
        unsafe_ownership |= live.has_unsafe_ownership();
        if !cold {
            output_bytes += event_bytes;
            evidence.extend(event_evidence);
        }
        if let Some(message_id) = removed_message(event, session) {
            message_removals.push(OpenCodeMessageRemoval {
                message_id,
                sequence: event.sequence,
            });
        }
        if let Some(result) = result {
            let size = serde_json::to_vec(&result)?.len();
            ensure!(
                size <= MAX_TOOL_RESULT_BYTES && (cold || output_bytes <= MAX_TOOL_RESULT_BYTES),
                "OpenCode live result exceeds its output budget"
            );
            if cold {
                retain_latest(&mut results, &mut output_bytes, result, size);
            } else {
                results.push_back(result);
            }
        }
        through = event.sequence;
    }
    ensure!(
        through == latest || !events.is_empty(),
        OpenCodeJournalReadError::IncompleteHistory
    );
    live.boundary = live_boundary(connection, session, through)?;
    let history_complete =
        live.history_safe && !live.evidence_gap && metadata.root && live.requests.main_session();
    if cold {
        live.public.seed_historical_users(&live.requests, latest);
        evidence = results
            .iter()
            .cloned()
            .map(|result| crate::analysis::NativeEvidenceEvent {
                fact: crate::analysis::NativeEvidenceFact::Message(
                    crate::analysis::NativeMessageObservation::ToolResult(result),
                ),
                live_eligible: true,
            })
            .collect();
    }
    let admission_failure = if !metadata.root {
        Some(ToolResultCompleteness::UnsafeOwnership)
    } else {
        live.requests
            .admission_failure()
            .or_else(|| (!live.history_safe).then_some(ToolResultCompleteness::UnsafeOrder))
            .or_else(|| unsafe_ownership.then_some(ToolResultCompleteness::UnsafeOwnership))
    };
    let history_complete = history_complete && admission_failure.is_none();
    let evidence_history_complete = history_complete && live.public.history_complete();
    if let Some(reason) = admission_failure {
        // Unsafe admission replaces this page's unusable facts and always fits the output budget.
        evidence = vec![crate::analysis::NativeEvidenceEvent {
            fact: crate::analysis::NativeEvidenceFact::AdmissionFailure(
                crate::analysis::NativeAdmissionFailure {
                    source_format: SourceFormat::OpenCodeSqliteV1,
                    session_id: session.to_owned(),
                    position: ToolResultPosition::OpenCodeEvent {
                        sequence: through,
                        item_index: u32::MAX,
                    },
                    reason,
                },
            ),
            live_eligible: true,
        }];
    }
    let cursor = OpenCodeJournalCursor {
        session_id: session.to_owned(),
        owner_id: metadata.owner,
        through_sequence: through,
        prefix_hash: [0; 32],
        parser_revision: PARSER_REVISION,
        live: Some(live),
        cold_replay: None,
        main_64: false,
    };
    ensure!(
        serde_json::to_vec(&cursor)?.len() <= 1024 * 1024,
        "OpenCode live cursor exceeds its output budget"
    );
    let page = OpenCodeToolResultPage {
        evidence: crate::analysis::NativeEvidenceBatch {
            source_format: SourceFormat::OpenCodeSqliteV1,
            events: evidence,
            history_complete: evidence_history_complete,
        },
        results: results.into(),
        message_removals,
        cursor,
        has_more: through < metadata.latest,
        snapshot_end: metadata.latest,
        history_complete,
        complete: false,
        live_input_events: events.len(),
        live_input_bytes: input_bytes,
        assistant_projections: assistant_projections.values,
    };
    if let Some(replay) = cold_replay {
        let parser_revision = after.expect("a cold replay has a cursor").parser_revision;
        ensure!(
            page.cold_output_digest(parser_revision)? == replay.output_sha256,
            OpenCodeJournalReadError::SourceChanged
        );
    }
    Ok(page)
}

fn retain_latest(
    results: &mut VecDeque<ToolResultObservation>,
    bytes: &mut usize,
    result: ToolResultObservation,
    size: usize,
) {
    if let Some(index) = results
        .iter()
        .position(|old| old.result_id == result.result_id)
    {
        let removed = results.remove(index).expect("the result index exists");
        *bytes -= serde_json::to_vec(&removed)
            .expect("a result encodes")
            .len();
    }
    while !results.is_empty()
        && (results.len() >= LIVE_RESULTS || *bytes + size > MAX_TOOL_RESULT_BYTES)
    {
        let removed = results.pop_front().expect("the window is not empty");
        *bytes -= serde_json::to_vec(&removed)
            .expect("a result encodes")
            .len();
    }
    *bytes += size;
    results.push_back(result);
}

fn select_live_window(events: &[Event]) -> HashSet<String> {
    #[derive(Deserialize)]
    struct Selection {
        part: Option<Part>,
    }
    #[derive(Deserialize)]
    struct Part {
        id: Option<String>,
        #[serde(rename = "messageID")]
        message_id: Option<String>,
        #[serde(rename = "type")]
        kind: Option<String>,
        state: Option<Status>,
    }
    #[derive(Deserialize)]
    struct Status {
        status: Option<String>,
    }
    let mut seen = HashSet::new();
    let mut selected = HashSet::new();
    let mut removed_messages = HashSet::new();
    for event in events.iter().rev() {
        if event.kind == "message.removed.1" {
            if let Some(message) = header(event).and_then(|header| header.message_id) {
                removed_messages.insert(message);
            }
            continue;
        }
        if event.kind == "message.part.removed.1" {
            if let Some(id) = header(event).and_then(|header| header.part_id) {
                seen.insert(id);
            }
            continue;
        }
        if event.kind != "message.part.updated.1" {
            continue;
        }
        let Some(part) = event
            .data
            .as_deref()
            .and_then(|data| serde_json::from_str::<Selection>(data).ok())
            .and_then(|selection| selection.part)
        else {
            continue;
        };
        if part.kind.as_deref() != Some("tool") {
            continue;
        }
        if part
            .message_id
            .as_ref()
            .is_some_and(|message| removed_messages.contains(message))
        {
            continue;
        }
        let Some(id) = part
            .id
            .filter(|id| !id.is_empty() && id.len() <= EVIDENCE_STRING_CAP)
        else {
            continue;
        };
        if seen.insert(id.clone())
            && part
                .state
                .and_then(|state| state.status)
                .is_some_and(|status| matches!(status.as_str(), "completed" | "error"))
        {
            selected.insert(id);
            if selected.len() == LIVE_RESULTS {
                break;
            }
        }
    }
    selected
}

fn observe_live(
    connection: &Connection,
    session: &str,
    live: &mut LiveState,
    event: &Event,
    selected: Option<&HashSet<String>>,
    root: bool,
    assistant_projections: &mut AssistantProjectionInputs,
) -> anyhow::Result<Option<ToolResultObservation>> {
    let cold = selected.is_some();
    let Some(data) = event
        .data
        .as_deref()
        .and_then(|data| serde_json::from_str::<Value>(data).ok())
    else {
        live.history_safe = false;
        return Ok(None);
    };
    let message_removal = removed_message(event, session);
    live.history_safe &= valid_event(&event.kind, &data, session) || message_removal.is_some();
    if event.kind == "message.part.updated.1"
        && identity(&data["part"]["id"])
            .is_some_and(|id| live.public.is_removed_part(id) || live.requests.is_removed_part(id))
    {
        return Ok(None);
    }
    let part_removal = (event.kind == "message.part.removed.1" && data["sessionID"] == session)
        .then(|| identity(&data["partID"]).zip(identity(&data["messageID"])))
        .flatten();
    let public_part_kind = part_removal
        .and_then(|(id, owner)| live.public.non_tool_part_kind(id, owner))
        .map(str::to_owned);
    let bound_non_tool_removal = part_removal.is_some_and(|(id, owner)| {
        public_part_kind.is_some() || live.requests.owns_non_tool_part(id, owner)
    });
    if let Some(message) = &message_removal {
        live.requests
            .remove_message(message, LIVE_REQUEST_RECORDS, LIVE_REQUEST_BYTES);
    } else if let Some((id, owner)) = part_removal {
        live.requests.remove_part(
            id,
            owner,
            public_part_kind.as_deref(),
            LIVE_REQUEST_RECORDS,
            LIVE_REQUEST_BYTES,
        );
    } else {
        live.requests
            .observe(&event.kind, &data, session, event.sequence);
    }
    live.requests
        .bound(LIVE_REQUEST_RECORDS, LIVE_REQUEST_BYTES);
    if let Some(message) = message_removal {
        for part in live.parts.values_mut() {
            if part
                .state
                .identity
                .as_ref()
                .is_some_and(|(owner, _, _, _)| owner == &message)
            {
                part.unsafe_ownership = true;
            }
        }
        return Ok(None);
    }
    if event.kind == "message.updated.1" && data["info"]["role"] == "user" {
        for part in live.parts.values_mut() {
            part.unsafe_ownership |= part
                .state
                .identity
                .as_ref()
                .is_some_and(|(message, _, _, _)| data["info"]["id"] == *message);
        }
    }
    let Some(id) = part_id(&event.kind, &data) else {
        return Ok(None);
    };
    let publish = selected.is_none_or(|selected| selected.contains(&id));
    let known = live.parts.contains_key(&id);
    if !known && event.kind == "message.part.removed.1" {
        live.evidence_gap |= !bound_non_tool_removal;
        return Ok(None);
    }
    if !known && event.kind == "message.part.updated.1" && data["part"]["type"] != "tool" {
        return Ok(None);
    }
    let entry = live.parts.entry(id.clone()).or_insert_with(|| LivePart {
        state: PartState::default(),
        context: None,
        last_sequence: event.sequence,
        unsafe_ownership: false,
    });
    if entry.state.first_terminal.is_none() && data["part"]["type"] == "tool" {
        entry.state.facts = data["part"]["messageID"]
            .as_str()
            .zip(data["part"]["tool"].as_str())
            .map(|(message, name)| {
                live.requests
                    .tool_facts(session, message, name, data["part"]["state"].get("input"))
            });
    }
    let result = entry
        .state
        .observe(&event.kind, &data, session, &id, event.sequence);
    entry.last_sequence = event.sequence;
    entry.unsafe_ownership |= entry
        .state
        .identity
        .as_ref()
        .is_some_and(|(message, _, _, _)| {
            live.requests.is_user_message(message) || live.requests.is_removed_message(message)
        });
    let unsafe_ownership = entry.unsafe_ownership;
    let Some(mut result) = result else {
        live.history_safe = false;
        bound_live_parts(live, selected);
        return Ok(None);
    };
    // A terminal-only identity outside the retained window can be an old correction.
    if !cold && !known && result.finality {
        entry.state.invalid = true;
    }
    result.revision = event.sequence + 1;
    if entry.state.invalid {
        result.completeness = ToolResultCompleteness::UnsafeIdentity;
    }
    let first_terminal = entry.state.first_terminal;
    let previous_context = entry.context;
    if publish
        && !entry.state.invalid
        && first_terminal == Some(event.sequence)
        && previous_context.is_none()
    {
        let message = result
            .record_id
            .as_deref()
            .expect("a tool part has its message identity");
        if cold {
            seed_live_assistant(
                connection,
                session,
                message,
                event.sequence,
                &mut live.requests,
                assistant_projections,
            )?;
        }
        let mut context = live.requests.freeze(session, message, event.sequence);
        context.main_session &= root;
        let context_key = retain_live_context(live, context);
        live.parts.get_mut(&id).expect("the part exists").context = Some(context_key);
    }
    let context_key = live
        .parts
        .get(&id)
        .and_then(|part| part.context)
        .or(previous_context);
    result.context = context_key
        .and_then(|key| live.contexts.iter().find(|(id, _)| *id == key))
        .map(|(_, context)| context.clone());
    if let Some(context) = &mut result.context {
        context.result_position = ToolResultPosition::OpenCodeEvent {
            sequence: first_terminal.unwrap_or(event.sequence),
            item_index: 0,
        };
    } else {
        result.context = Some(ToolResultContext::pending(
            ToolResultPosition::OpenCodeEvent {
                sequence: first_terminal.unwrap_or(event.sequence),
                item_index: 0,
            },
            root && live.requests.main_session()
                && previous_context.is_none()
                && first_terminal.is_none(),
        ));
    }
    if !live.history_safe && result.completeness != ToolResultCompleteness::UnsafeIdentity {
        result.completeness = ToolResultCompleteness::UnsafeOrder;
    } else if (!root || !live.requests.main_session() || unsafe_ownership)
        && matches!(
            result.completeness,
            ToolResultCompleteness::Complete | ToolResultCompleteness::Incomplete
        )
    {
        result.completeness = ToolResultCompleteness::UnsafeOwnership;
    }
    bound_content(&mut result);
    bound_live_parts(live, selected);
    Ok(
        (publish && (!cold || result.finality || result.change == ToolResultChange::Removal))
            .then_some(result),
    )
}

fn removed_message(event: &Event, session: &str) -> Option<String> {
    if event.kind != "message.removed.1" {
        return None;
    }
    let data = header(event)?;
    if data.session_id.as_deref() != Some(session) {
        return None;
    }
    data.message_id
        .filter(|message| !message.is_empty() && message.len() <= EVIDENCE_STRING_CAP)
}

fn seed_live_assistant(
    connection: &Connection,
    session: &str,
    message: &str,
    sequence: u64,
    requests: &mut request_context::RequestHistory,
    projections: &mut AssistantProjectionInputs,
) -> anyhow::Result<()> {
    if requests.has_message(message) || requests.is_removed_message(message) {
        return Ok(());
    }
    let projection = if let Some(projection) = projections.values.get(message) {
        projection.clone()
    } else {
        ensure!(!projections.frozen, OpenCodeJournalReadError::SourceChanged);
        ensure!(
            projections.values.len() < LIVE_RESULTS,
            "OpenCode cold ownership lookups exceed their count limit"
        );
        let info: Option<String> = connection.query_row(
            "SELECT data FROM message WHERE id = ?1 AND session_id = ?2 AND length(CAST(data AS BLOB)) <= ?3",
            params![message, session, ASSISTANT_PROJECTION_BYTES], |row| row.get(0)).optional()?;
        let projection = info
            .and_then(|info| serde_json::from_str::<Value>(&info).ok())
            .filter(|info| info["role"] == "assistant")
            .map(|info| AssistantProjection {
                parent_id: info["parentID"].clone(),
            });
        ensure!(
            projection.as_ref().is_none_or(|projection| {
                projection.parent_id.to_string().len() <= ASSISTANT_PROJECTION_BYTES
            }),
            "OpenCode cold ownership lookup exceeds its byte limit"
        );
        // Retain absent lookups so later projection rows cannot change replay.
        projections
            .values
            .insert(message.to_owned(), projection.clone());
        projection
    };
    if let Some(projection) = projection {
        // Projection metadata supplies ownership only. It supplies no historical request text.
        let data = serde_json::json!({"sessionID": session, "info": {
            "id": message, "sessionID": session, "role": "assistant", "parentID": projection.parent_id
        }});
        requests.observe("message.updated.1", &data, session, sequence);
        requests.bound(LIVE_REQUEST_RECORDS, LIVE_REQUEST_BYTES);
    }
    Ok(())
}

fn retain_live_context(live: &mut LiveState, mut context: ToolResultContext) -> u64 {
    while !context.user_messages.is_empty()
        && serde_json::to_vec(&context).expect("context encodes").len() > LIVE_REQUEST_BYTES
    {
        context.user_messages.remove(0);
        context.complete_through_event = false;
    }
    if let Some((id, previous)) = live.contexts.back()
        && previous.user_messages == context.user_messages
        && previous.main_session == context.main_session
    {
        return *id;
    }
    let id = live.next_context;
    live.next_context += 1;
    live.contexts.push_back((id, context));
    while live.contexts.len() > LIVE_CONTEXTS
        || live
            .contexts
            .iter()
            .map(|(_, context)| serde_json::to_vec(context).expect("context encodes").len())
            .sum::<usize>()
            > LIVE_CONTEXT_BYTES
    {
        live.contexts.pop_front();
    }
    id
}

fn bound_live_parts(live: &mut LiveState, selected: Option<&HashSet<String>>) {
    while live.parts.len() > LIVE_PARTS
        || live
            .parts
            .iter()
            .map(|(id, part)| part.retained_bytes(id))
            .sum::<usize>()
            > LIVE_PART_BYTES
    {
        let oldest = live
            .parts
            .iter()
            .filter(|(id, _)| selected.is_none_or(|selected| !selected.contains(*id)))
            .min_by_key(|(_, part)| part.last_sequence)
            .map(|(id, _)| id.clone());
        let Some(oldest) = oldest else {
            break;
        };
        live.parts.remove(&oldest);
    }
}

fn valid_event(kind: &str, data: &Value, session: &str) -> bool {
    if data["sessionID"].as_str() != Some(session) {
        return false;
    }
    match kind {
        "session.created.1" | "session.updated.1" => data["info"].is_object(),
        "message.updated.1" => {
            identity(&data["info"]["id"]).is_some()
                && matches!(data["info"]["role"].as_str(), Some("user" | "assistant"))
        }
        "message.part.updated.1" => {
            identity(&data["part"]["id"]).is_some()
                && identity(&data["part"]["messageID"]).is_some()
                && identity(&data["part"]["type"]).is_some()
                && data["part"]["sessionID"]
                    .as_str()
                    .is_none_or(|id| id == session)
        }
        "message.part.removed.1" => {
            identity(&data["partID"]).is_some() && identity(&data["messageID"]).is_some()
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "journal/admission_tests.rs"]
mod admission_tests;

#[derive(Default, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PartState {
    identity: Option<(String, Option<String>, Option<String>, String)>,
    revision: u64,
    first_terminal: Option<u64>,
    context: Option<ToolResultContext>,
    invalid: bool,
    #[serde(default)]
    facts: Option<crate::analysis::NativeToolFacts>,
}

impl PartState {
    fn observe(
        &mut self,
        kind: &str,
        data: &Value,
        session: &str,
        id: &str,
        sequence: u64,
    ) -> Option<ToolResultObservation> {
        if data["sessionID"].as_str() != Some(session) {
            self.invalid = true;
            return None;
        }
        let removal = kind == "message.part.removed.1";
        let part = &data["part"];
        if !removal {
            let current = (
                identity(&part["messageID"])?.to_owned(),
                identity(&part["callID"]).map(str::to_owned),
                identity(&part["tool"]).map(str::to_owned),
                identity(&part["type"])?.to_owned(),
            );
            if let Some(original) = &self.identity {
                self.invalid |= original != &current;
            } else {
                self.identity = Some(current);
            }
        }
        let (message, call, tool, original_kind) = self.identity.as_ref()?;
        if original_kind != "tool" {
            if part["type"] == "tool" {
                self.invalid = true;
            }
            return None;
        }
        self.invalid |= call.is_none()
            || tool.is_none()
            || part["sessionID"]
                .as_str()
                .is_some_and(|owner| owner != session)
            || (removal && data["messageID"].as_str() != Some(message));
        self.revision += 1;
        let state = &part["state"];
        let (status, content, terminal) = match state["status"].as_str() {
            Some("completed") => (ToolResultStatus::Success, state["output"].as_str(), true),
            Some("error")
                if state
                    .pointer("/metadata/interrupted")
                    .and_then(Value::as_bool)
                    == Some(true) =>
            {
                (
                    ToolResultStatus::Interrupted,
                    state.pointer("/metadata/output").and_then(Value::as_str),
                    true,
                )
            }
            Some("error") => (ToolResultStatus::Error, state["error"].as_str(), true),
            _ => (ToolResultStatus::Unknown, None, false),
        };
        if terminal {
            self.first_terminal.get_or_insert(sequence);
        }
        let mut facts = (!self.invalid).then(|| self.facts.clone()).flatten();
        if let Some(facts) = &mut facts {
            facts.operation_paths = None;
            if part
                .pointer("/metadata/providerExecuted")
                .is_some_and(|value| value != false)
            {
                facts.category = Some(crate::analysis::CheckToolCategory::Other);
            }
        }
        Some(ToolResultObservation {
            facts,
            outcome: None,
            provider_executed: None,
            source_format: SourceFormat::OpenCodeSqliteV1,
            session_id: session.to_owned(),
            record_id: Some(message.clone()),
            call_id: call.clone(),
            result_id: Some(id.to_owned()),
            position: ToolResultPosition::OpenCodeEvent {
                sequence,
                item_index: 0,
            },
            revision: self.revision,
            change: if removal && !self.invalid {
                ToolResultChange::Removal
            } else {
                ToolResultChange::Update
            },
            tool_name: tool.clone(),
            tool_category: tool
                .as_deref()
                .map(|tool| tool_call_from_input(tool, state.get("input")).category),
            arguments: (!self.invalid)
                .then(|| state.get("input").cloned())
                .flatten(),
            content: (!self.invalid)
                .then(|| content.map(str::to_owned))
                .flatten(),
            status,
            finality: terminal && !self.invalid,
            completeness: if self.invalid {
                ToolResultCompleteness::UnsafeIdentity
            } else if terminal && content.is_some() {
                ToolResultCompleteness::Complete
            } else {
                ToolResultCompleteness::Incomplete
            },
            context: None,
        })
    }
}

fn bound_context(mut context: ToolResultContext) -> ToolResultContext {
    while !context.user_messages.is_empty()
        && serde_json::to_vec(&context)
            .expect("source context encodes")
            .len()
            > CONTEXT_BYTES
    {
        context.user_messages.remove(0);
        context.complete_through_event = false;
    }
    context
}

fn bound_content(result: &mut ToolResultObservation) {
    let content_large = result
        .content
        .as_ref()
        .is_some_and(|text| text.len() > MAX_CONTENT_PART_BYTES);
    let arguments_large = result
        .arguments
        .as_ref()
        .is_some_and(|value| value.to_string().len() > MAX_CONTENT_PART_BYTES);
    if content_large {
        result.content = None;
    }
    if arguments_large {
        result.arguments = None;
    }
    if (content_large || arguments_large) && result.completeness == ToolResultCompleteness::Complete
    {
        result.completeness = ToolResultCompleteness::Incomplete;
    }
}

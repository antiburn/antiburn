//! Bounded native file delivery, independent of full-session metrics.
//!
//! The cursor retains framing, unresolved joins, and request context. Commit it
//! atomically with each page. A page is a delta, never a complete inventory.
//! Head and tail anchors detect local rewrites; they do not prove that an
//! omitted historical prefix is unchanged.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::framing::live::LiveJsonlFramer;
use super::resume::identity_filter::IdentityFilter;
pub use super::source_validity::NativeFileReadError;
use super::source_validity::{NativeFileAnchor, NativeFileSource, RESUME_TAIL_BYTES};
use super::vendors::{claude, codex, pi};
use super::{
    NormalizedRecord, RawSource, RecordSink, SessionInput, SessionSummary, SourceFormat,
    ToolResultContext, ToolResultObservation, UserMessage,
};

pub const NATIVE_FILE_CURSOR_REVISION: u32 = 1;
pub const NATIVE_FILE_STARTUP_BYTES: u64 = 1024 * 1024;
pub const NATIVE_FILE_PAGE_BYTES: u64 = 256 * 1024;
const MAX_PAGE_RESULTS: usize = 256;
const MAX_PAGE_RESULT_BYTES: usize = 12 * 1024 * 1024;
const MAX_RECORD_RESULT_BYTES: usize = 4 * 1024 * 1024;
const MAX_REQUEST_BYTES: usize = 1024 * 1024;
const MAX_REQUESTS: usize = 256;

/// Exact source-byte binding for a complete JSONL record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeFilePosition {
    pub record_start: u64,
    pub record_end: u64,
    pub item_index: u32,
}

#[derive(Debug, Clone)]
pub struct NativeFileResult {
    pub position: NativeFilePosition,
    pub request_positions: Vec<NativeFilePosition>,
    pub observation: ToolResultObservation,
    pub live_eligible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeFileRequest {
    pub position: NativeFilePosition,
    pub message: UserMessage,
}

#[derive(Debug, Clone)]
pub struct NativeFilePage {
    pub cursor: NativeFileCursor,
    pub results: Vec<NativeFileResult>,
    /// Retained context, with exact byte bindings for every message.
    pub requests: Vec<NativeFileRequest>,
    pub baseline_offset: u64,
    pub baseline_reached: bool,
    pub has_more: bool,
    pub pending_tail: bool,
    pub history_complete: bool,
    pub skipped_results: u64,
    pub bytes_read: u64,
}

/// Opaque durable state. Restore only with the same source and session binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NativeFileCursor {
    revision: u32,
    agent: String,
    session_id: String,
    source_format: SourceFormat,
    anchor: NativeFileAnchor,
    baseline_offset: u64,
    start_offset: u64,
    next_record_index: u64,
    #[serde(with = "framer_codec")]
    framer: LiveJsonlFramer,
    adapter: Adapter,
    requests: VecDeque<NativeFileRequest>,
    request_ids: IdentityFilter,
    request_bytes: usize,
    history_complete: bool,
    pending_results: Option<PendingResults>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingResults {
    position: NativeFilePosition,
    observations_json: VecDeque<String>,
    requests: Vec<NativeFileRequest>,
    main_session: bool,
    history_complete: bool,
}

mod framer_codec {
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::LiveJsonlFramer;

    pub fn serialize<S: Serializer>(
        framer: &LiveJsonlFramer,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            let bytes = postcard::to_allocvec(framer).map_err(serde::ser::Error::custom)?;
            STANDARD.encode(bytes).serialize(serializer)
        } else {
            framer.serialize(serializer)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<LiveJsonlFramer, D::Error> {
        if deserializer.is_human_readable() {
            let encoded = String::deserialize(deserializer)?;
            let bytes = STANDARD.decode(encoded).map_err(serde::de::Error::custom)?;
            postcard::from_bytes(&bytes).map_err(serde::de::Error::custom)
        } else {
            LiveJsonlFramer::deserialize(deserializer)
        }
    }
}

impl NativeFileCursor {
    pub fn encode(&self) -> Vec<u8> {
        postcard::to_allocvec(self).expect("native file cursor encodes")
    }

    pub fn decode(bytes: &[u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            NativeFileReadError::InvalidCursor
        );
        let (cursor, remaining): (Self, _) =
            postcard::take_from_bytes(bytes).map_err(|_| NativeFileReadError::InvalidCursor)?;
        anyhow::ensure!(
            remaining.is_empty() && cursor.valid(),
            NativeFileReadError::InvalidCursor
        );
        Ok(cursor)
    }

    fn valid(&self) -> bool {
        self.revision == NATIVE_FILE_CURSOR_REVISION
            && self.start_offset <= self.offset()
            && self.start_offset <= self.baseline_offset
            && self.baseline_offset <= self.anchor.boundary
            && self.anchor.head.offset <= self.anchor.boundary
            && self.offset() <= self.anchor.boundary
            && self.framer.valid_at(self.offset())
            && self.requests.len() <= MAX_REQUESTS
            && self.request_bytes <= MAX_REQUEST_BYTES
            && matches!(
                (&self.adapter, self.source_format),
                (Adapter::Claude(_), SourceFormat::ClaudeJsonl)
                    | (Adapter::Codex(_), SourceFormat::CodexRolloutJsonl)
                    | (Adapter::Pi(_), SourceFormat::PiV3Jsonl)
            )
    }

    pub fn offset(&self) -> u64 {
        self.anchor.tail.offset
    }
    pub fn baseline_offset(&self) -> u64 {
        self.baseline_offset
    }
    pub fn start_offset(&self) -> u64 {
        self.start_offset
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum Adapter {
    Claude(Box<claude::live::State>),
    Codex(Box<codex::live::State>),
    Pi(Box<pi::live::State>),
}

impl Adapter {
    fn new(input: &SessionInput, header: &Value, suffix_only: bool) -> anyhow::Result<Self> {
        Ok(match (input.agent.as_str(), input.source_format) {
            ("claude", SourceFormat::ClaudeJsonl) => {
                Self::Claude(Box::new(claude::live::State::new(input, header)))
            }
            ("codex", SourceFormat::CodexRolloutJsonl) => Self::Codex(Box::new(
                codex::live::State::new(input, header, suffix_only),
            )),
            ("pi", SourceFormat::PiV3Jsonl) => {
                Self::Pi(Box::new(pi::live::State::new(input, header, suffix_only)))
            }
            _ => anyhow::bail!(NativeFileReadError::Unsupported),
        })
    }

    fn observe(&mut self, value: Value, index: u64, sink: &mut dyn RecordSink) {
        match self {
            Self::Claude(state) => state.observe(value, index, sink),
            Self::Codex(state) => state.observe(value, index, sink),
            Self::Pi(state) => state.observe(value, index, sink),
        }
    }

    fn main_session(&self) -> bool {
        match self {
            Self::Claude(state) => state.main_session(),
            Self::Codex(state) => state.main_session(),
            Self::Pi(state) => state.main_session(),
        }
    }

    fn invalidate(&mut self) {
        match self {
            Self::Claude(state) => state.invalidate(),
            Self::Codex(state) => state.invalidate(),
            Self::Pi(state) => state.invalidate(),
        }
    }
}

/// Reads at most one finite byte page. An unterminated record stays in the cursor.
/// The first read starts within the latest 1 MiB and freezes the initial EOF as
/// the live baseline. A pending record becomes live when its newline arrives.
pub fn read_native_file_page(
    input: &SessionInput,
    cursor: Option<&NativeFileCursor>,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<NativeFilePage> {
    read_page(input, cursor, cancel).map_err(|error| {
        if error.downcast_ref::<NativeFileReadError>().is_some() {
            return error;
        }
        let failure = match error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind)
        {
            Some(std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied) => {
                NativeFileReadError::Unavailable
            }
            _ => NativeFileReadError::ReadFailed,
        };
        error.context(failure)
    })
}

fn read_page(
    input: &SessionInput,
    cursor: Option<&NativeFileCursor>,
    cancel: &dyn Fn() -> bool,
) -> anyhow::Result<NativeFilePage> {
    anyhow::ensure!(!cancel(), NativeFileReadError::Cancelled);
    let RawSource::File(path) = &input.source else {
        anyhow::bail!(NativeFileReadError::Unsupported);
    };
    anyhow::ensure!(
        input.fork_parent_session_id.is_none(),
        NativeFileReadError::Unsupported
    );
    if let Some(cursor) = cursor {
        anyhow::ensure!(
            cursor.valid()
                && cursor.agent == input.agent
                && cursor.session_id == input.session_id
                && cursor.source_format == input.source_format,
            NativeFileReadError::InvalidCursor
        );
    }
    let mut source = NativeFileSource::open(path, cursor.map(|cursor| &cursor.anchor))?;
    let mut cursor = match cursor {
        Some(cursor) => cursor.clone(),
        None => {
            let head = source.read(0, source.size.min(RESUME_TAIL_BYTES))?;
            let header = head
                .split(|byte| *byte == b'\n')
                .next()
                .filter(|_| head.contains(&b'\n'))
                .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
                .ok_or_else(|| anyhow::anyhow!(NativeFileReadError::Unavailable))?;
            let start = source.size.saturating_sub(NATIVE_FILE_STARTUP_BYTES);
            let discard = start > 0 && source.read(start - 1, 1)? != b"\n";
            NativeFileCursor {
                revision: NATIVE_FILE_CURSOR_REVISION,
                agent: input.agent.clone(),
                session_id: input.session_id.clone(),
                source_format: input.source_format,
                anchor: source.anchor(head.len() as u64, start)?,
                baseline_offset: source.size,
                start_offset: start,
                next_record_index: 0,
                framer: LiveJsonlFramer::at(start, discard),
                adapter: Adapter::new(input, &header, start > 0)?,
                requests: VecDeque::new(),
                request_ids: IdentityFilter::default(),
                request_bytes: 0,
                history_complete: start == 0,
                pending_results: None,
            }
        }
    };
    let offset = cursor.offset();
    let mut results = Vec::new();
    let mut result_bytes = 0;
    let mut skipped_results = 0;
    let drained = drain_pending(&mut cursor, &mut results, &mut result_bytes)?;
    let read_limit =
        if drained && results.len() < MAX_PAGE_RESULTS && result_bytes < MAX_PAGE_RESULT_BYTES {
            NATIVE_FILE_PAGE_BYTES.min(source.size - offset)
        } else {
            0
        };
    let bytes = source.read(offset, read_limit)?;
    let mut cancelled = false;
    let mut failure = None;
    let mut framer = std::mem::take(&mut cursor.framer);
    let consumed = framer.push(&bytes, offset, |start, end, bytes| {
        if cancel() {
            cancelled = true;
            return false;
        }
        if bytes.is_some_and(|bytes| bytes.iter().all(u8::is_ascii_whitespace)) {
            return true;
        }
        let index = cursor.next_record_index;
        cursor.next_record_index = cursor.next_record_index.saturating_add(1);
        let Some(value) = bytes.and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        else {
            cursor.history_complete = false;
            cursor.adapter.invalidate();
            return true;
        };
        let mut sink = NativeSink::default();
        cursor.adapter.observe(value, index, &mut sink);
        cursor.history_complete &= !sink.gap && cursor.adapter.main_session();
        if sink.context_invalid {
            cursor.requests.clear();
            cursor.request_bytes = 0;
        }
        skipped_results += sink.skipped_results;
        for mut request in sink.requests {
            if cursor.request_ids.contains(&request.message_id) {
                let exact_replay = cursor.requests.iter().any(|prior| {
                    prior.message.message_id == request.message_id
                        && prior.message.parts == request.parts
                        && prior.message.completeness == request.completeness
                        && prior.message.origin == request.origin
                        && prior.message.proof_revision == request.proof_revision
                });
                if !exact_replay {
                    cursor.history_complete = false;
                    cursor
                        .requests
                        .retain(|prior| prior.message.message_id != request.message_id);
                    cursor.request_bytes = cursor
                        .requests
                        .iter()
                        .flat_map(|request| &request.message.parts)
                        .map(|part| part.text.len())
                        .sum();
                }
                continue;
            }
            cursor.request_ids.insert(&request.message_id);
            request.position = super::UserMessagePosition::FileBytes {
                record_start: start,
                record_end: end,
            };
            let size = request
                .parts
                .iter()
                .map(|part| part.text.len())
                .sum::<usize>();
            cursor.request_bytes += size;
            cursor.requests.push_back(NativeFileRequest {
                position: NativeFilePosition {
                    record_start: start,
                    record_end: end,
                    item_index: 0,
                },
                message: request,
            });
            while cursor.request_bytes > MAX_REQUEST_BYTES || cursor.requests.len() > MAX_REQUESTS {
                let removed = cursor
                    .requests
                    .pop_front()
                    .expect("request limit removes a request");
                cursor.request_bytes -= removed
                    .message
                    .parts
                    .iter()
                    .map(|part| part.text.len())
                    .sum::<usize>();
                cursor.history_complete = false;
            }
        }
        cursor.pending_results = Some(PendingResults {
            position: NativeFilePosition {
                record_start: start,
                record_end: end,
                item_index: 0,
            },
            observations_json: sink
                .results
                .into_iter()
                .map(|observation| {
                    serde_json::to_string(&observation).expect("native observation serializes")
                })
                .collect(),
            requests: cursor.requests.iter().cloned().collect(),
            main_session: cursor.adapter.main_session(),
            history_complete: cursor.history_complete,
        });
        match drain_pending(&mut cursor, &mut results, &mut result_bytes) {
            Ok(drained) => {
                drained && results.len() < MAX_PAGE_RESULTS && result_bytes < MAX_PAGE_RESULT_BYTES
            }
            Err(error) => {
                failure = Some(error);
                false
            }
        }
    });
    cursor.framer = framer;
    if let Some(error) = failure {
        return Err(error);
    }
    anyhow::ensure!(!cancelled && !cancel(), NativeFileReadError::Cancelled);
    source.recheck(path, &cursor.anchor, offset, &bytes)?;
    cursor.anchor = source.anchor(cursor.anchor.head.offset, offset + consumed as u64)?;
    Ok(NativeFilePage {
        has_more: cursor.pending_results.is_some() || cursor.offset() < source.size,
        baseline_reached: cursor.offset() >= cursor.baseline_offset
            && cursor
                .pending_results
                .as_ref()
                .is_none_or(|pending| pending.position.record_end > cursor.baseline_offset),
        pending_tail: cursor.framer.pending(),
        history_complete: cursor.history_complete,
        baseline_offset: cursor.baseline_offset,
        requests: cursor.requests.iter().cloned().collect(),
        cursor,
        results,
        skipped_results,
        bytes_read: bytes.len() as u64,
    })
}

fn drain_pending(
    cursor: &mut NativeFileCursor,
    results: &mut Vec<NativeFileResult>,
    result_bytes: &mut usize,
) -> anyhow::Result<bool> {
    let Some(pending) = cursor.pending_results.as_mut() else {
        return Ok(true);
    };
    while let Some(json) = pending.observations_json.front() {
        let mut observation: ToolResultObservation =
            serde_json::from_str(json).map_err(|_| NativeFileReadError::InvalidCursor)?;
        let super::ToolResultPosition::Jsonl { item_index, .. } = observation.position else {
            anyhow::bail!(NativeFileReadError::InvalidCursor)
        };
        observation.position = super::ToolResultPosition::FileBytes {
            record_start: pending.position.record_start,
            record_end: pending.position.record_end,
            item_index,
        };
        observation.context = Some(ToolResultContext {
            result_position: observation.position,
            main_session: pending.main_session,
            complete_through_event: pending.history_complete,
            user_messages: pending
                .requests
                .iter()
                .map(|request| request.message.clone())
                .collect(),
        });
        let size = serde_json::to_vec(&observation)?.len();
        anyhow::ensure!(
            size <= MAX_PAGE_RESULT_BYTES,
            NativeFileReadError::InvalidCursor
        );
        if results.len() >= MAX_PAGE_RESULTS || size > MAX_PAGE_RESULT_BYTES - *result_bytes {
            return Ok(false);
        }
        *result_bytes += size;
        results.push(NativeFileResult {
            position: NativeFilePosition {
                item_index,
                ..pending.position
            },
            request_positions: pending
                .requests
                .iter()
                .map(|request| request.position)
                .collect(),
            observation,
            live_eligible: pending.position.record_end > cursor.baseline_offset,
        });
        pending.observations_json.pop_front();
    }
    cursor.pending_results = None;
    Ok(true)
}

#[derive(Default)]
struct NativeSink {
    results: Vec<ToolResultObservation>,
    requests: Vec<UserMessage>,
    gap: bool,
    result_bytes: usize,
    skipped_results: u64,
    context_invalid: bool,
}

impl RecordSink for NativeSink {
    fn record(&mut self, record: NormalizedRecord) {
        match record {
            NormalizedRecord::ToolResult(mut result) => {
                if result.tool_name.is_none() {
                    result.call_id = None;
                }
                let bytes = serde_json::to_vec(&result)
                    .expect("native result serializes")
                    .len();
                if self.results.len() < MAX_PAGE_RESULTS
                    && bytes <= MAX_RECORD_RESULT_BYTES - self.result_bytes
                {
                    self.result_bytes += bytes;
                    self.results.push(*result);
                } else {
                    self.skipped_results += 1;
                    self.gap = true;
                }
            }
            NormalizedRecord::Unusable(_) => self.gap = true,
            _ => {}
        }
    }
    fn user_message(&mut self, message: UserMessage) {
        self.gap |= message.completeness == super::UserMessageCompleteness::Incomplete;
        if message.is_bounded() {
            self.requests.push(message);
        } else {
            self.gap = true;
        }
    }
    fn tool_result_gap(&mut self) {
        self.gap = true;
    }
    fn tool_result_admission_gap(&mut self, _: super::ToolResultAdmission) {
        self.gap = true;
        self.context_invalid = true;
    }
    fn finish(&mut self, _: SessionSummary) {}
}

#[cfg(test)]
mod tests;

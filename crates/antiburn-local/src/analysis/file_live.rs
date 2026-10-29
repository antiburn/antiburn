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
use super::public_events::{JsonState, freeze_message, user_observation};
use super::resume::identity_filter::IdentityFilter;
pub use super::source_validity::NativeFileReadError;
use super::source_validity::{NativeFileAnchor, NativeFileSource, RESUME_TAIL_BYTES};
use super::vendors::{claude, codex, pi};
use super::{
    NativeAdmissionFailure, NativeEvidenceBatch, NativeEvidenceEvent, NativeEvidenceFact,
    NativeMessageObservation,
};
use super::{
    NormalizedRecord, RawSource, RecordSink, SessionInput, SessionSummary, SourceFormat,
    ToolResultContext, ToolResultObservation, UserMessage,
};

mod legacy;

pub const NATIVE_FILE_CURSOR_REVISION: u32 = 5;
pub const NATIVE_FILE_STARTUP_BYTES: u64 = 1024 * 1024;
pub const NATIVE_FILE_PAGE_BYTES: u64 = 256 * 1024;
const MAX_PAGE_RESULTS: usize = 256;
const MAX_PAGE_RESULT_BYTES: usize = 12 * 1024 * 1024;
const MAX_RECORD_RESULT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PENDING_EVIDENCE_BYTES: usize = 20 * 1024 * 1024;
const EVENT_ENVELOPE_BYTES_BOUND: usize = 64;
const ACTIVITY_BYTES_RESERVE: usize = 8192;
const PAGE_ENVELOPE_BYTES_RESERVE: usize = 1024;
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
    /// Initial read state for retry when the host publishes only part of the first page.
    pub initial_cursor: Option<NativeFileCursor>,
    pub results: Vec<NativeFileResult>,
    pub evidence: NativeEvidenceBatch,
    /// Known unsafe admission applies before the host accepts this page receipt.
    pub admission_failure: Option<NativeAdmissionFailure>,
    /// Retained context, with exact byte bindings for every message.
    pub requests: Vec<NativeFileRequest>,
    pub baseline_offset: u64,
    pub baseline_reached: bool,
    pub has_more: bool,
    pub pending_tail: bool,
    pub history_complete: bool,
    /// Legacy gap counter. It also counts new evidence facts that exceed output bounds.
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
    evidence_baseline_offset: u64,
    pending_evidence: JsonState<VecDeque<NativeEvidenceEvent>>,
    admission_failure: JsonState<Option<NativeAdmissionFailure>>,
    #[serde(skip)]
    main_64_public_baseline_pending: bool,
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
        // Check the revision before decoding changed postcard adapter layouts.
        let (revision, _): (u32, _) =
            postcard::take_from_bytes(bytes).map_err(|_| NativeFileReadError::InvalidCursor)?;
        if revision == 4 {
            return Self::convert_v4(bytes);
        }
        anyhow::ensure!(
            revision == NATIVE_FILE_CURSOR_REVISION,
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
            && self.evidence_baseline_offset <= self.anchor.boundary
            && self.anchor.head.offset <= self.anchor.boundary
            && self.offset() <= self.anchor.boundary
            && self.framer.valid_at(self.offset())
            && self.requests.len() <= MAX_REQUESTS
            && self.request_bytes <= MAX_REQUEST_BYTES
            && match &self.adapter {
                Adapter::Pi(state) => state.valid(),
                _ => true,
            }
            && self.admission_failure.0.as_ref().is_none_or(|failure| {
                failure.valid()
                    && failure.source_format == self.source_format
                    && failure.session_id == self.session_id
                    && matches!(failure.position, super::ToolResultPosition::FileBytes { record_end, .. } if record_end <= self.anchor.boundary)
            })
            && self.pending_evidence.0.len() <= 4096
            && self.pending_evidence.0.iter().all(|event| match &event.fact {
                NativeEvidenceFact::AdmissionFailure(failure) => self.admission_failure.0.as_ref() == Some(failure),
                _ => true,
            })
            && self
                .pending_evidence
                .0
                .iter()
                .try_fold(0usize, |bytes, event| {
                    let size = serde_json::to_vec(event).ok()?.len();
                    bytes
                        .checked_add(size)
                        .filter(|total| *total <= MAX_PENDING_EVIDENCE_BYTES)
                })
                .is_some()
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

    pub fn evidence_baseline_offset(&self) -> u64 {
        self.evidence_baseline_offset
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
            ("claude", SourceFormat::ClaudeJsonl) => Self::Claude(Box::new(
                claude::live::State::new(input, header, suffix_only),
            )),
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
    anyhow::ensure!(
        !input.session_id.is_empty() && input.session_id.len() <= 4096,
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
    let cold_read = cursor.is_none();
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
                evidence_baseline_offset: source.size,
                pending_evidence: JsonState::default(),
                admission_failure: JsonState::default(),
                main_64_public_baseline_pending: false,
            }
        }
    };
    let initial_cursor = cold_read.then(|| cursor.clone());
    if cursor.main_64_public_baseline_pending {
        cursor.evidence_baseline_offset = source.size;
        cursor.anchor.boundary = source.size;
        cursor.main_64_public_baseline_pending = false;
    }
    let offset = cursor.offset();
    let mut results = Vec::new();
    let mut result_bytes = serde_json::to_vec(&NativeEvidenceBatch {
        source_format: input.source_format,
        events: Vec::new(),
        history_complete: false,
    })?
    .len();
    let mut events = Vec::new();
    let mut skipped_results = 0;
    let drained = drain_pending(&mut cursor, &mut results, &mut result_bytes)?;
    for result in &results {
        events.push(NativeEvidenceEvent {
            fact: NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(
                result.observation.clone(),
            )),
            live_eligible: result.live_eligible,
        });
    }
    let drained =
        drained && drain_evidence(&mut cursor, &mut results, &mut events, &mut result_bytes)?;
    let read_limit =
        if drained && events.len() < MAX_PAGE_RESULTS && result_bytes < MAX_PAGE_RESULT_BYTES {
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
        let mut sink = NativeSink {
            position: Some(NativeFilePosition {
                record_start: start,
                record_end: end,
                item_index: 0,
            }),
            prefix: cursor
                .requests
                .iter()
                .map(|request| request.message.clone())
                .collect(),
            history_complete: cursor.history_complete,
            main_session: cursor.adapter.main_session(),
            known_admission: cursor
                .admission_failure
                .0
                .as_ref()
                .map(|failure| failure.reason),
            ..Default::default()
        };
        cursor.adapter.observe(value, index, &mut sink);
        if let Some(reason) = sink.failure_reason
            && cursor.admission_failure.0.is_none()
        {
            let failure = NativeAdmissionFailure {
                source_format: input.source_format,
                session_id: input.session_id.clone(),
                position: super::ToolResultPosition::FileBytes {
                    record_start: start,
                    record_end: end,
                    item_index: u32::MAX,
                },
                reason,
            };
            cursor.admission_failure.0 = Some(failure.clone());
            sink.push_failure(failure);
        }
        cursor.history_complete &= !sink.gap && cursor.adapter.main_session();
        if sink.context_invalid || cursor.admission_failure.0.is_some() {
            cursor.requests.clear();
            cursor.request_bytes = 0;
        }
        skipped_results += sink.skipped_results;
        for mut request in sink
            .requests
            .into_iter()
            .filter(|_| cursor.admission_failure.0.is_none())
        {
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
        for fact in sink.events {
            let baseline = if matches!(
                &fact,
                NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(_))
            ) {
                cursor.baseline_offset
            } else {
                cursor.evidence_baseline_offset
            };
            cursor.pending_evidence.0.push_back(NativeEvidenceEvent {
                fact,
                live_eligible: end > baseline,
            });
        }
        match drain_evidence(&mut cursor, &mut results, &mut events, &mut result_bytes) {
            Ok(drained) => {
                drained && events.len() < MAX_PAGE_RESULTS && result_bytes < MAX_PAGE_RESULT_BYTES
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
    if let Some(failure) = &cursor.admission_failure.0 {
        for event in events
            .iter_mut()
            .chain(cursor.pending_evidence.0.iter_mut())
        {
            event.fact.invalidate_admission(failure.reason);
            if !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)) {
                event.live_eligible = false;
            }
        }
        for result in &mut results {
            result.observation.completeness = failure.reason;
            if let Some(context) = &mut result.observation.context {
                context.main_session = false;
                context.complete_through_event = false;
            }
            result.live_eligible = false;
        }
        let mut pending_bytes = cursor
            .pending_evidence
            .0
            .iter()
            .map(|event| {
                serde_json::to_vec(event)
                    .expect("native event encodes")
                    .len()
            })
            .sum::<usize>();
        while pending_bytes > MAX_PENDING_EVIDENCE_BYTES {
            let index = cursor
                .pending_evidence
                .0
                .iter()
                .position(|event| !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
                .expect("admission state fits its bound");
            let removed = cursor
                .pending_evidence
                .0
                .remove(index)
                .expect("pending event exists");
            pending_bytes -= serde_json::to_vec(&removed)
                .expect("native event encodes")
                .len();
            skipped_results += 1;
        }
        let mut page_bytes = serde_json::to_vec(&NativeEvidenceBatch {
            source_format: input.source_format,
            events: Vec::new(),
            history_complete: false,
        })?
        .len()
            + events
                .iter()
                .map(|event| {
                    serde_json::to_vec(event)
                        .expect("native event encodes")
                        .len()
                        + 1
                })
                .sum::<usize>();
        while page_bytes > MAX_PAGE_RESULT_BYTES {
            let index = events
                .iter()
                .rposition(|event| !matches!(event.fact, NativeEvidenceFact::AdmissionFailure(_)))
                .expect("admission state fits its bound");
            let removed = events.remove(index);
            page_bytes -= serde_json::to_vec(&removed)
                .expect("native event encodes")
                .len()
                + 1;
            if let NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result)) =
                removed.fact
            {
                results.retain(|item| item.observation.position != result.position);
            }
            skipped_results += 1;
        }
    }
    Ok(NativeFilePage {
        has_more: cursor.pending_results.is_some()
            || !cursor.pending_evidence.0.is_empty()
            || cursor.offset() < source.size,
        baseline_reached: cursor.offset() >= cursor.baseline_offset
            && cursor
                .pending_evidence
                .0
                .iter()
                .all(|event| matches!(event.fact.position(), super::ToolResultPosition::FileBytes { record_end, .. } if record_end > cursor.baseline_offset))
            && cursor
                .pending_results
                .as_ref()
                .is_none_or(|pending| pending.position.record_end > cursor.baseline_offset),
        pending_tail: cursor.framer.pending(),
        history_complete: cursor.history_complete,
        baseline_offset: cursor.baseline_offset,
        requests: cursor.requests.iter().cloned().collect(),
        evidence: NativeEvidenceBatch {
            source_format: input.source_format,
            events,
            history_complete: cursor.history_complete,
        },
        admission_failure: cursor.admission_failure.0.clone(),
        cursor,
        initial_cursor,
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
        observation.revision = pending.position.record_end;
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
        let size = serde_json::to_vec(&NativeEvidenceEvent {
            fact: NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(
                observation.clone(),
            )),
            live_eligible: pending.position.record_end > cursor.baseline_offset,
        })?
        .len()
            + 1;
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
    events: Vec<NativeEvidenceFact>,
    requests: Vec<UserMessage>,
    gap: bool,
    result_bytes: usize,
    skipped_results: u64,
    context_invalid: bool,
    position: Option<NativeFilePosition>,
    prefix: Vec<UserMessage>,
    history_complete: bool,
    main_session: bool,
    known_admission: Option<super::ToolResultCompleteness>,
    failure_reason: Option<super::ToolResultCompleteness>,
}

impl RecordSink for NativeSink {
    fn record(&mut self, record: NormalizedRecord) {
        match record {
            NormalizedRecord::ToolResult(mut result) => {
                if result.tool_name.is_none() {
                    result.call_id = None;
                }
                let result =
                    self.finalize_native_message(NativeMessageObservation::ToolResult(*result));
                self.push_fact(NativeEvidenceFact::Message(result));
            }
            NormalizedRecord::PublicMessage(message) => {
                let message =
                    self.finalize_native_message(NativeMessageObservation::Public(*message));
                self.push_fact(NativeEvidenceFact::Message(message));
            }
            NormalizedRecord::ResponseCycle(mut cycle) => {
                if let Some(position) = self.position {
                    cycle.position = super::ToolResultPosition::FileBytes {
                        record_start: position.record_start,
                        record_end: position.record_end,
                        item_index: u32::MAX,
                    };
                    cycle.revision = position.record_end;
                }
                self.push_fact(NativeEvidenceFact::ResponseCycle(*cycle));
            }
            NormalizedRecord::Unusable(_) => self.gap = true,
            _ => {}
        }
    }
    fn user_message(&mut self, message: UserMessage) {
        self.gap |= message.completeness == super::UserMessageCompleteness::Incomplete;
        self.history_complete &= message.completeness != super::UserMessageCompleteness::Incomplete;
        if message.is_bounded() {
            if let Some(observation) = user_observation(message.clone()) {
                let finalized =
                    self.finalize_native_message(NativeMessageObservation::User(observation));
                if let NativeMessageObservation::User(value) = &finalized {
                    self.prefix.push(value.message.clone());
                    while self.prefix.len() > MAX_REQUESTS
                        || self
                            .prefix
                            .iter()
                            .flat_map(|message| &message.parts)
                            .map(|part| part.text.len())
                            .sum::<usize>()
                            > MAX_REQUEST_BYTES
                    {
                        self.prefix.remove(0);
                        self.history_complete = false;
                    }
                }
                self.push_fact(NativeEvidenceFact::Message(finalized));
            }
            self.requests.push(message);
        } else {
            self.gap = true;
        }
    }
    fn tool_result_gap(&mut self) {
        self.gap = true;
        self.history_complete = false;
    }
    fn tool_result_admission_gap(&mut self, admission: super::ToolResultAdmission) {
        let Some(reason) = super::public_events::admission_reason(admission) else {
            return;
        };
        self.gap = true;
        self.context_invalid = true;
        self.failure_reason
            .get_or_insert(self.known_admission.unwrap_or(reason));
    }
    fn native_admission_failure(&mut self, failure: NativeAdmissionFailure) {
        self.tool_result_admission_gap(failure.admission());
    }
    fn finish(&mut self, _: SessionSummary) {}

    fn finalize_native_message(
        &mut self,
        message: NativeMessageObservation,
    ) -> NativeMessageObservation {
        let original = message.position();
        let position = match (self.position, original) {
            (Some(binding), super::ToolResultPosition::Jsonl { item_index, .. }) => {
                super::ToolResultPosition::FileBytes {
                    record_start: binding.record_start,
                    record_end: binding.record_end,
                    item_index,
                }
            }
            _ => original,
        };
        let admitted = match &message {
            NativeMessageObservation::User(value) => value.context.main_session,
            NativeMessageObservation::Public(value) => value
                .context
                .as_ref()
                .is_some_and(|context| context.main_session),
            NativeMessageObservation::ToolResult(value) => value
                .context
                .as_ref()
                .is_some_and(|context| context.main_session),
        };
        let prefix = match (&message, position) {
            (NativeMessageObservation::ToolResult(_), super::ToolResultPosition::FileBytes { record_start, .. }) => self.prefix.iter().filter(|request| {
                matches!(request.position, super::UserMessagePosition::FileBytes { record_end, .. } if record_end <= record_start)
            }).cloned().collect(),
            _ => self.prefix.clone(),
        };
        let mut message = freeze_message(
            message,
            position,
            prefix,
            self.history_complete,
            self.main_session && admitted,
        );
        if let Some(reason) = self.known_admission.or(self.failure_reason) {
            message.invalidate_admission(reason);
        }
        message
    }

    fn native_activity(&mut self, mut activity: super::NativeActivityObservation) {
        if let Some(position) = self.position {
            activity.position = super::ToolResultPosition::FileBytes {
                record_start: position.record_start,
                record_end: position.record_end,
                item_index: u32::MAX - 1,
            };
        }
        self.push_fact(NativeEvidenceFact::Activity(activity));
    }

    fn native_context(
        &mut self,
        _source_format: SourceFormat,
        _session_id: &str,
        position: super::ToolResultPosition,
        main_session: bool,
    ) -> super::NativeEventContext {
        let position = match (self.position, position) {
            (Some(binding), super::ToolResultPosition::Jsonl { item_index, .. }) => {
                super::ToolResultPosition::FileBytes {
                    record_start: binding.record_start,
                    record_end: binding.record_end,
                    item_index,
                }
            }
            (_, position) => position,
        };
        super::NativeEventContext {
            first_eligible_position: position,
            main_session: main_session
                && self.main_session
                && self.known_admission.or(self.failure_reason).is_none(),
            complete_through_event: main_session
                && self.main_session
                && self.history_complete
                && self.known_admission.or(self.failure_reason).is_none(),
            user_messages: self.prefix.clone(),
        }
    }

    fn session_selection(&mut self, mut selection: super::NativeSessionSelectionObservation) {
        if let Some(position) = self.position {
            selection.position = super::ToolResultPosition::FileBytes {
                record_start: position.record_start,
                record_end: position.record_end,
                item_index: 0,
            };
        }
        self.push_fact(NativeEvidenceFact::SessionSelection(selection));
    }
}

impl NativeSink {
    fn push_failure(&mut self, failure: NativeAdmissionFailure) {
        let fact = NativeEvidenceFact::AdmissionFailure(failure);
        let bytes = serde_json::to_vec(&fact)
            .expect("admission failure encodes")
            .len()
            + EVENT_ENVELOPE_BYTES_BOUND;
        while !self.events.is_empty()
            && (self.events.len() >= 4096
                || bytes > MAX_PENDING_EVIDENCE_BYTES.saturating_sub(self.result_bytes))
        {
            let removed = self.events.pop().expect("bounded output contains a fact");
            self.result_bytes -= serde_json::to_vec(&removed)
                .expect("native fact encodes")
                .len()
                + EVENT_ENVELOPE_BYTES_BOUND;
            self.skipped_results += 1;
        }
        self.result_bytes += bytes;
        self.events.push(fact);
    }

    fn push_fact(&mut self, fact: NativeEvidenceFact) {
        let bytes = serde_json::to_vec(&fact)
            .expect("native fact encodes")
            .len()
            + EVENT_ENVELOPE_BYTES_BOUND;
        let activity = matches!(&fact, NativeEvidenceFact::Activity(_));
        let fact_limit = if matches!(
            &fact,
            NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(_))
        ) {
            MAX_PAGE_RESULT_BYTES - PAGE_ENVELOPE_BYTES_RESERVE
        } else {
            MAX_RECORD_RESULT_BYTES
        };
        let count_limit = if activity { 4096 } else { 4095 };
        let byte_limit =
            MAX_PENDING_EVIDENCE_BYTES - if activity { 0 } else { ACTIVITY_BYTES_RESERVE };
        if self.events.len() >= count_limit
            || bytes > fact_limit
            || bytes > byte_limit.saturating_sub(self.result_bytes)
        {
            self.skipped_results += 1;
            self.gap = true;
            self.history_complete = false;
            return;
        }
        self.result_bytes += bytes;
        self.events.push(fact);
    }
}

fn drain_evidence(
    cursor: &mut NativeFileCursor,
    results: &mut Vec<NativeFileResult>,
    events: &mut Vec<NativeEvidenceEvent>,
    bytes: &mut usize,
) -> anyhow::Result<bool> {
    while let Some(event) = cursor.pending_evidence.0.front() {
        let size = serde_json::to_vec(event)?.len() + 1;
        anyhow::ensure!(
            size <= MAX_PAGE_RESULT_BYTES,
            NativeFileReadError::InvalidCursor
        );
        if events.len() >= MAX_PAGE_RESULTS || size > MAX_PAGE_RESULT_BYTES.saturating_sub(*bytes) {
            return Ok(false);
        }
        if let NativeEvidenceFact::Message(NativeMessageObservation::ToolResult(result)) =
            &event.fact
        {
            let super::ToolResultPosition::FileBytes {
                record_start,
                record_end,
                item_index,
            } = result.position
            else {
                anyhow::bail!(NativeFileReadError::InvalidCursor)
            };
            let request_positions = result
                .context
                .as_ref()
                .into_iter()
                .flat_map(|context| &context.user_messages)
                .filter_map(|request| match request.position {
                    super::UserMessagePosition::FileBytes {
                        record_start,
                        record_end,
                    } => Some(NativeFilePosition {
                        record_start,
                        record_end,
                        item_index: 0,
                    }),
                    _ => None,
                })
                .collect();
            results.push(NativeFileResult {
                position: NativeFilePosition {
                    record_start,
                    record_end,
                    item_index,
                },
                request_positions,
                observation: result.clone(),
                live_eligible: event.live_eligible,
            });
        }
        *bytes += size;
        events.push(
            cursor
                .pending_evidence
                .0
                .pop_front()
                .expect("pending event exists"),
        );
    }
    Ok(true)
}

#[cfg(test)]
mod tests;

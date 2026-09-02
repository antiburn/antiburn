//! The vendor interface layer.
//!
//! This is the seam that lets *every* agent vendor be analyzed through one
//! pipeline. Each vendor implements [`VendorAdapter`] to turn its raw
//! transcript (JSONL text, a SQLite database, …) into a
//! [`NormalizedSession`]. The engine never knows which vendor it is looking at.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::analysis::framing::PartialReason;
use crate::analysis::initial_context::InitialContextBreakdown;
use crate::analysis::model::{NormalizedEvent, NormalizedSession, ToolCall};
use crate::analysis::resume::{AdapterResume, AdapterSnapshot, StreamSnapshot};
use crate::analysis::source_validity::{AppendOnlyGuarantee, SourceClaim};

/// Where a session's raw bytes come from. Adapters choose how to read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawSource {
    /// Transcript content already in memory (line-delimited JSON for most agents).
    Jsonl(String),
    /// A transcript file on disk the adapter should read itself.
    File(PathBuf),
    /// A SQLite database file (Codex, OpenCode, …).
    Sqlite(PathBuf),
}

/// One unit of work handed to the analysis pipeline: a single live session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInput {
    /// Vendor label, e.g. `"claude"`, `"codex"`, `"opencode"`. Used to pick an
    /// adapter; unrecognized labels fall back to the generic JSONL adapter.
    pub agent: String,
    pub session_id: String,
    pub source: RawSource,
}

/// One framed record's outcome, in transcript order.
pub enum NormalizedRecord {
    MetricsEvent(Box<NormalizedEvent>),
    Observation(Box<EvidenceObservation>),
    /// One turn's captured message content. An adapter emits this
    /// immediately after the `MetricsEvent` it belongs to, and only when it
    /// has at least one part. The metrics and evidence sinks ignore it; only
    /// [`crate::analysis::rows::TurnRowSink`] reads it.
    TurnContent(Box<TurnContent>),
    Unusable(PartialReason),
}

/// Cap on one [`ContentPart`]'s byte length. [`ContentPart::new`] truncates
/// longer text on a char boundary. This bounds the row sink's per-batch
/// memory no matter how long a single message part is.
pub const MAX_CONTENT_PART_BYTES: usize = 256 * 1024;

/// One turn's captured message content, ready to become `turn_content` rows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TurnContent {
    pub parts: Vec<ContentPart>,
}

/// One piece of a turn's captured text: one text block, one thinking block,
/// one tool call's input, or one tool call's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentPart {
    pub kind: ContentKind,
    pub text: String,
    /// True when `text` was cut short at [`MAX_CONTENT_PART_BYTES`].
    pub truncated: bool,
}

impl ContentPart {
    /// Builds a part, capping `text` at [`MAX_CONTENT_PART_BYTES`] bytes on a
    /// char boundary.
    pub fn new(kind: ContentKind, text: impl Into<String>) -> Self {
        let mut text = text.into();
        let mut truncated = false;
        if text.len() > MAX_CONTENT_PART_BYTES {
            let mut boundary = MAX_CONTENT_PART_BYTES;
            while boundary > 0 && !text.is_char_boundary(boundary) {
                boundary -= 1;
            }
            text.truncate(boundary);
            truncated = true;
        }
        Self {
            kind,
            text,
            truncated,
        }
    }
}

/// The kind of text one [`ContentPart`] carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    UserText,
    AssistantText,
    Thinking,
    ToolInput,
    ToolResult,
}

impl ContentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ContentKind::UserText => "user",
            ContentKind::AssistantText => "assistant",
            ContentKind::Thinking => "thinking",
            ContentKind::ToolInput => "tool_input",
            ContentKind::ToolResult => "tool_result",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceObservation {
    ContextSource {
        kind: ContextSourceKind,
        name: String,
        description: Option<String>,
    },
    SubagentSpawn {
        ts_ms: Option<i64>,
        parent_model: Option<String>,
        provenance: RelationProvenance,
    },
    DelegatedTurn {
        is_sidechain: bool,
        is_assistant: bool,
        model: Option<String>,
    },
    /// One record's thread identity (Claude `uuid` / `parentUuid`), emitted
    /// for every record that carries either field — including eventless
    /// records — so the evidence sink can resolve parent links against every
    /// identity the source declares, not only the counted turns.
    ThreadLink {
        uuid: Option<String>,
        parent_uuid: Option<String>,
    },
    /// One non-turn record's provider timestamp.
    RecordTimestamp {
        ts_ms: i64,
    },
    /// One inherited record does not contribute to the child session.
    InheritedRecord,
    UnrecognizedType {
        discriminator: String,
        inert: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSourceKind {
    Skill,
    McpServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationProvenance {
    TaskToolUse,
    /// A Codex `spawn_agent` function call.
    SpawnAgentCall,
    /// An OpenCode descendant session's `parent_id` row.
    SessionParentLink,
}

/// One provider name observed with the model it billed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHint {
    pub provider: String,
    pub model: Option<String>,
}

pub const MAX_PROVIDER_HINTS: usize = crate::analysis::evidence::MAX_MODELS;

pub(crate) fn push_provider_hint(
    hints: &mut Vec<ProviderHint>,
    provider: &str,
    model: Option<&str>,
) {
    let Some(provider) = bounded_provider_hint_value(provider) else {
        return;
    };
    let model = model.and_then(bounded_provider_hint_value);
    let hint = ProviderHint { provider, model };
    if hints.contains(&hint) || hints.len() >= MAX_PROVIDER_HINTS {
        return;
    }
    hints.push(hint);
}

pub(crate) fn bounded_provider_hint_value(value: &str) -> Option<String> {
    use crate::analysis::evidence::EVIDENCE_STRING_CAP;

    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let mut end = value.len().min(EVIDENCE_STRING_CAP);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    Some(value[..end].to_owned())
}

/// Session facts that an adapter can state only after the last record.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionSummary {
    /// True when this adapter can observe cache-write tokens.
    pub cache_write_tokens_available: bool,
    pub context_window: Option<u64>,
    pub model: Option<String>,
    /// Unique, bounded raw provider and model observations.
    pub provider_hints: Vec<ProviderHint>,
    /// The provider-declared session start, in Unix milliseconds.
    pub started_at_ms: Option<i64>,
    /// Source gaps that become known only when the stream ends.
    pub coverage_gaps: Vec<PartialReason>,
    /// Tool calls resolved at the end of the stream, keyed by event ordinal.
    pub late_tools: Vec<(usize, ToolCall)>,
    pub initial_context: Option<InitialContextBreakdown>,
    pub skill_descriptions: HashMap<String, String>,
}

pub trait RecordSink {
    /// The adapter calls this once per framed record, in transcript order.
    fn record(&mut self, record: NormalizedRecord);

    /// The adapter calls this once after the last record.
    fn finish(&mut self, summary: SessionSummary);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordCoverage {
    Complete,
    Partial,
}

/// A sink that rebuilds a `NormalizedSession` from a record stream.
pub struct SessionCollector {
    agent: String,
    session_id: String,
    events: Vec<NormalizedEvent>,
    partial_reasons: BTreeSet<PartialReason>,
    summary: Option<SessionSummary>,
}

impl SessionCollector {
    pub fn new(agent: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            agent: agent.into(),
            session_id: session_id.into(),
            events: Vec::new(),
            partial_reasons: BTreeSet::new(),
            summary: None,
        }
    }

    /// Returns `Partial` when one or more records were unusable.
    pub fn coverage(&self) -> RecordCoverage {
        if self.partial_reasons.is_empty() {
            RecordCoverage::Complete
        } else {
            RecordCoverage::Partial
        }
    }

    pub fn partial_reasons(&self) -> &BTreeSet<PartialReason> {
        &self.partial_reasons
    }

    /// Returns an error when the adapter did not finish the record stream.
    pub fn into_session(mut self) -> anyhow::Result<NormalizedSession> {
        let summary = self
            .summary
            .take()
            .ok_or_else(|| anyhow::anyhow!("record stream ended without a session summary"))?;
        for (ordinal, tool) in summary.late_tools {
            if let Some(event) = self.events.get_mut(ordinal) {
                event.tools.push(tool);
            }
        }
        Ok(NormalizedSession {
            agent: self.agent,
            session_id: self.session_id,
            events: self.events,
            cache_write_tokens_available: summary.cache_write_tokens_available,
            context_window: summary.context_window,
            model: summary.model,
        })
    }
}

impl RecordSink for SessionCollector {
    fn record(&mut self, record: NormalizedRecord) {
        match record {
            NormalizedRecord::MetricsEvent(event) => self.events.push(*event),
            NormalizedRecord::Observation(_) => {}
            NormalizedRecord::TurnContent(_) => {}
            NormalizedRecord::Unusable(reason) => {
                self.partial_reasons.insert(reason);
            }
        }
    }

    fn finish(&mut self, summary: SessionSummary) {
        self.partial_reasons
            .extend(summary.coverage_gaps.iter().copied());
        self.summary = Some(summary);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisitOutcome {
    /// The adapter reads the source to its end without a source-validity check.
    /// This outcome states only that the stream completed.
    Unvalidated,
    /// The post-read recheck confirms the same source version for the whole source.
    AcceptedFull,
    AcceptedPrefix {
        boundary: u64,
    },
    SourceChanged(SourceChangedReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceChangedReason {
    IdentityMismatch,
    ShortAtOpen {
        size: u64,
        boundary: u64,
    },
    HeadRegionMismatch,
    ShortRead {
        consumed: u64,
        boundary: u64,
    },
    TruncatedAfterRead {
        size: u64,
        boundary: u64,
    },
    FingerprintMismatch,
    /// A resumed open's file is shorter than the claimed resume offset, or
    /// the trailing window before that offset no longer hashes to the
    /// claimed tail hash. See [`crate::analysis::source_validity::ResumePoint`].
    ResumeTailMismatch,
}

/// The result of a resumed streaming pass
/// ([`VendorAdapter::visit_claimed_resumed`]).
pub struct ResumedVisit {
    pub outcome: VisitOutcome,
    /// `None` when the adapter's end-of-stream state is not a safe resume
    /// point (see the "unsettled" rule on
    /// [`VendorAdapter::visit_claimed_resumed`]), or when `outcome` is
    /// [`VisitOutcome::SourceChanged`].
    ///
    /// `Some` carries only the adapter's own half of a resume: the new
    /// [`crate::analysis::source_validity::ResumePoint`] and the adapter's
    /// own opaque state. The adapter streams new records into `sink`
    /// through the same `RecordSink` interface every adapter uses, so it
    /// never sees the sink's own accumulator types behind
    /// `&mut dyn RecordSink` and cannot build a full [`StreamSnapshot`] on
    /// its own. The caller combines this with its own sink's state via
    /// [`crate::analysis::evidence_sink::CompositeSink::snapshot`].
    pub resume: Option<AdapterResume>,
}

/// Implemented once per vendor format. Stateless and `Sync` so adapters can be
/// stored as `&'static dyn VendorAdapter` in the registry.
pub trait VendorAdapter: Sync {
    /// Stable label this adapter handles (for diagnostics/tests).
    fn agent(&self) -> &'static str;

    /// Parse one raw session into the normalized model. Implementations should
    /// be resilient: skip malformed records rather than failing the whole
    /// session, and only return `Err` for unreadable sources (missing file,
    /// unopenable DB).
    fn normalize(&self, input: &SessionInput) -> anyhow::Result<NormalizedSession>;

    /// Streams one raw session into `sink`, one record at a time.
    fn visit(
        &self,
        input: &SessionInput,
        sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        let session = self.normalize(input)?;
        let NormalizedSession {
            events,
            cache_write_tokens_available,
            context_window,
            model,
            ..
        } = session;
        for event in events {
            sink.record(NormalizedRecord::MetricsEvent(Box::new(event)));
        }
        sink.finish(SessionSummary {
            cache_write_tokens_available,
            context_window,
            model,
            provider_hints: Vec::new(),
            started_at_ms: None,
            coverage_gaps: Vec::new(),
            late_tools: Vec::new(),
            initial_context: None,
            skill_descriptions: HashMap::new(),
        });
        Ok(VisitOutcome::Unvalidated)
    }

    /// Streams a file after the caller captures its source claim.
    fn visit_claimed(
        &self,
        _input: &SessionInput,
        _claim: &SourceClaim,
        _guarantee: AppendOnlyGuarantee,
        _cancel: &dyn Fn() -> bool,
        _sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        anyhow::bail!("claimed streaming is unsupported for this adapter")
    }

    /// Streams a provider database from one stable read snapshot.
    fn visit_db_claimed(
        &self,
        _input: &SessionInput,
        _claimed_fingerprint: &str,
        _cancel: &dyn Fn() -> bool,
        _sink: &mut dyn RecordSink,
    ) -> anyhow::Result<VisitOutcome> {
        anyhow::bail!("claimed database streaming is unsupported for this adapter")
    }

    /// Streams a file from a verified [`StreamSnapshot`] instead of from the
    /// start, then reports a new snapshot for the next resume.
    ///
    /// `resume.resume` (a [`crate::analysis::source_validity::ResumePoint`])
    /// names the byte offset to read from; the adapter restores its own
    /// whole-stream state from `resume.adapter` before it starts. Passing a
    /// snapshot with `resume.resume.offset == 0` and a fresh adapter's
    /// default state runs a full first pass that still produces a snapshot
    /// for the next resume — the same [`PinnedSource::open_resumed`] path
    /// handles an empty tail (see its offset-zero test) — so this one method
    /// covers both the first snapshot-producing pass and every later
    /// resumed pass, and `visit_claimed` needs no change to support resume.
    ///
    /// "Unsettled" rule: an adapter returns `resume: None` inside
    /// [`ResumedVisit`] when its end-of-stream state is not a safe resume
    /// point (a case a specific adapter's own doc comment must name — for
    /// example, a fork whose ownership is still undetermined at EOF). The
    /// default implementation here returns `Err`, so every adapter that
    /// does not override this method is simply unsupported for resume.
    ///
    /// [`PinnedSource::open_resumed`]: crate::analysis::source_validity::PinnedSource::open_resumed
    fn visit_claimed_resumed(
        &self,
        _input: &SessionInput,
        _claim: &SourceClaim,
        _resume: &StreamSnapshot,
        _cancel: &dyn Fn() -> bool,
        _sink: &mut dyn RecordSink,
    ) -> anyhow::Result<ResumedVisit> {
        anyhow::bail!("resume is unsupported for this adapter")
    }

    /// This adapter's own encoded, empty resume state: the form
    /// [`Self::visit_claimed_resumed`]'s `resume.adapter` field takes to
    /// start the very first resumable pass over a source with no stored
    /// snapshot yet. A caller pairs this with a [`StreamSnapshot`] whose
    /// [`crate::analysis::source_validity::ResumePoint::offset`] is zero
    /// and calls [`Self::visit_claimed_resumed`], which then reads the
    /// whole source from the start — the same result [`Self::visit_claimed`]
    /// would give — while still producing a real [`AdapterResume`] for the
    /// next pass to resume from.
    ///
    /// `None` when this adapter does not support resume, matching
    /// [`Self::visit_claimed_resumed`]'s own "unsupported" default.
    fn empty_resume_state(&self) -> Option<AdapterSnapshot> {
        None
    }
}

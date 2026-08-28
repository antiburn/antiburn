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
    Unusable(PartialReason),
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
}

/// Session facts that an adapter can state only after the last record.
#[derive(Default)]
pub struct SessionSummary {
    /// True when this adapter can observe cache-write tokens.
    pub cache_write_tokens_available: bool,
    pub context_window: Option<u64>,
    pub model: Option<String>,
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
    ShortAtOpen { size: u64, boundary: u64 },
    HeadRegionMismatch,
    ShortRead { consumed: u64, boundary: u64 },
    TruncatedAfterRead { size: u64, boundary: u64 },
    FingerprintMismatch,
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
}

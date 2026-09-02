//! Turning a located transcript into the analysis the views render.
//!
//! Every number here is produced by the engine. This module's job is the
//! plumbing around it: find the transcript, render a vendor database into the
//! shape the analysis layer expects, run the orchestrator and its sub-agents in
//! one pass, and map the result onto the wire types.
//!
//! Analysis is CPU-bound and synchronous, so it runs on the blocking pool —
//! never on a runtime worker, where a multi-megabyte transcript would stall
//! every other command.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use antiburn_local::analysis::{
    ANALYZER_REVISION, ActiveSessionsSummary, AdapterResume, COVERAGE_SCHEMA_REVISION,
    CompositeSink, EVIDENCE_SCHEMA_REVISION, EfficiencyTotals, EvidenceResumeState,
    EvidenceSnapshot, EvidenceSource, InitialContextBreakdown, MAX_PROVIDER_HINTS,
    METRICS_SCHEMA_REVISION, ModelRun, PARSER_REVISION, ProviderHint, RESUME_SNAPSHOT_REVISION,
    RawSource, ResumePoint, ResumeRevisions, ResumedVisit, SessionCost, SessionEvidence,
    SessionEvidenceAccumulator, SessionInput, SessionMetrics, SessionMetricsAccumulator,
    SessionSummary, SkillUse, SourceCapabilities, SourceClaim, SourceKind, StoredResume,
    StreamSnapshot, TurnRow, TurnRowSink, TurnRowStore, TurnScope, VendorAdapter, VisitOutcome,
    adapter_for, aggregate_metrics, append_only_guarantee, evidence_from_facts, merge_metrics,
    metrics_by_source, metrics_from_rows, price_breakdown, pricing_generation,
};
use antiburn_local::discovery::{
    ACTIVE_SESSION_WINDOW_SECS, Explorers, FORK_OBSERVATION_KEY, FingerprintInputs,
    ForkObservation, SessionSource, SourceStat, session_source_content, session_source_preview,
};
use antiburn_local::model::AgentKind;
use antiburn_local::pricing::ModelTokens;

#[cfg(test)]
use antiburn_local::analysis::{
    ClaudeAdapter, CoverageReason, EvidenceValue, FAST_SPEED_KEY, MemoryTurnRowStore,
    analyze_sources_with,
};
#[cfg(test)]
use antiburn_local::insights::{DetectorId, clean_facts_complete, eligible};

use crate::agents::{supports_analysis, vendor_label};
use crate::dto::{BillableTokens, OrchestrationStatus, SubagentMember};
use crate::store::{
    AnalysisRecord, ProjectionRevisions, RelationKind, SessionKey, SourcePublishMode,
    SourcePublishOutcome, Store,
};

/// Minimum sub-agents before a session reads as an orchestrator. One delegated
/// task is ordinary; two or more is genuine fan-out. Mirrors the webview's
/// `MIN_ORCHESTRATED_SUBAGENTS`.
pub const MIN_ORCHESTRATED_SUBAGENTS: u32 = 2;

pub struct ClaimedSource {
    pub fingerprint: Option<String>,
    pub generation: i64,
}

#[derive(Clone)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn never() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub(crate) fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

/// This signal keeps cancellation and progress within one evidence claim.
#[derive(Clone)]
pub struct PassSignal {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
}

impl PassSignal {
    pub fn new() -> Self {
        Self {
            cancel: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn from_cancel(cancel: CancelFlag) -> Self {
        let signal = Self {
            cancel: cancel.flag(),
            progress: Arc::new(AtomicU64::new(0)),
        };
        if cancel.cancelled() {
            signal.cancel();
        }
        signal
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn progress(&self) -> u64 {
        self.progress.load(Ordering::SeqCst)
    }

    /// This observation advances progress and reports the current cancellation state.
    pub fn observe(&self) -> bool {
        self.progress.fetch_add(1, Ordering::SeqCst);
        self.cancel.load(Ordering::SeqCst)
    }
}

impl Default for PassSignal {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassOutcome {
    Published,
    SourceChanged,
    SourceMissing,
    Unsupported,
    Unreadable,
}

pub struct EvidencePass {
    pub analysis: SessionAnalysis,
    pub evidence: Option<SessionEvidence>,
    pub outcome: PassOutcome,
    /// Each source this pass read, and how — see [`StreamedSession::
    /// source_outcomes`]. Empty for every outcome but `Published`.
    pub source_outcomes: Vec<SourcePublishOutcome>,
}

pub struct StreamedSession {
    pub parent: SessionMetrics,
    pub merged: SessionMetrics,
    /// Each sub-agent's own metrics, paired with the unix-second timestamp of
    /// its earliest transcript event. `None` when that child's accumulator
    /// retained no timestamped turn.
    pub subagents: Vec<(SessionMetrics, Option<i64>)>,
    pub started_at_epoch: Option<i64>,
    pub evidence: Option<SessionEvidence>,
    /// `inclusive_model_breakdown` and `model_runs`, read back from turn
    /// rows instead of the accumulator. `Some` only when this pass had a
    /// `turn_row_store` — see [`RowProjections`].
    pub row_projections: Option<RowProjections>,
    /// Each source's own `SessionSummary`, keyed by `source_key`. `Some`
    /// only when this pass had a `turn_row_store` — see
    /// [`stream_vendor_with_hooks`].
    pub source_summaries: Option<BTreeMap<String, SessionSummary>>,
    /// Each source this pass read, whether it resumed or read fully, and
    /// the resume snapshot to persist for it (if any) — the worker passes
    /// this straight through to `Store::publish_projections`. Empty for a
    /// pass with no `turn_row_store`: only the durable worker resumes a
    /// source or persists its snapshot.
    pub source_outcomes: Vec<SourcePublishOutcome>,
}

/// `inclusive_model_breakdown` and `model_runs`, derived from published
/// turn rows instead of `SessionMetricsAccumulator`.
///
/// A pass with a `turn_row_store` (the durable evidence worker) computes
/// these from `query_model_breakdown`/`query_model_runs` instead of the
/// accumulator's own tally, proving the two agree over every
/// characterization fixture — see the engine crate's
/// `tests/turn_facts_parity.rs`, the `*_model_projections_match_the_
/// accumulator_for_every_fixture` tests. A pass with no row store (an
/// on-demand view, or a non-evidence-cohort agent) keeps the accumulator's
/// values — see `analyze_for_evidence`.
pub struct RowProjections {
    pub model_breakdown: std::collections::BTreeMap<String, ModelTokens>,
    pub model_runs: Vec<ModelRun>,
}

enum StreamOutcome {
    Published {
        session: Box<StreamedSession>,
        parent_fingerprint: Option<String>,
    },
    SourceChanged,
    ParentMissing,
    ParentUnsupported,
    ParentUnreadable,
}

enum ComputedAnalysis {
    Published {
        parent: Box<SessionMetrics>,
        merged: Box<SessionMetrics>,
        subagents: Vec<(SessionMetrics, Option<i64>)>,
        started_at_epoch: Option<i64>,
        parent_fingerprint: Option<String>,
        evidence: Box<Option<SessionEvidence>>,
        row_projections: Option<RowProjections>,
        source_summaries: Option<BTreeMap<String, SessionSummary>>,
        source_outcomes: Vec<SourcePublishOutcome>,
    },
    SourceChanged,
    Missing,
    Unsupported,
    Unavailable,
}

/// Longest a skill description may be once it leaves this module.
///
/// A skill's description comes from the transcript's skill listing.
/// The engine applies this limit before metrics leave its accumulator.
/// The app applies it again before values reach the store or an export.
pub const SKILL_DESCRIPTION_MAX_CHARS: usize = 300;

/// The character appended to a description this module had to shorten, so a
/// reader can see that they are looking at the front of something longer.
const TRUNCATION_MARK: char = '…';

/// Hold every skill description to [`SKILL_DESCRIPTION_MAX_CHARS`].
///
/// Applied before skill invocations enter an export.
fn cap_skill_descriptions(skills: &mut [SkillUse]) {
    for skill in skills {
        if let Some(description) = skill.description.take() {
            skill.description = Some(cap_excerpt(&description, SKILL_DESCRIPTION_MAX_CHARS));
        }
    }
}

/// `text` shortened to at most `max` characters, counting characters rather
/// than bytes so a multi-byte description cannot be cut mid-character.
fn cap_excerpt(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut capped: String = text.chars().take(max.saturating_sub(1)).collect();
    capped.push(TRUNCATION_MARK);
    capped
}

/// One session's analysis, before it is split between the wire payload, the
/// store, and the export document.
pub struct SessionAnalysis {
    /// The parent session's own metrics.
    pub metrics: Option<SessionMetrics>,
    /// The same metrics shaped as the one-session summary the views render.
    pub summary: Option<ActiveSessionsSummary>,
    /// Cost of the parent transcript plus every sub-agent it launched.
    ///
    /// This is the session's total cost. The activity list and the export
    /// document show this figure.
    ///
    /// The value is `None` when a model in the combined breakdown has no
    /// price. A partial total hides real cost.
    pub cost: Option<SessionCost>,
    /// Cost of the parent transcript, without any sub-agent.
    pub top_level_cost: Option<SessionCost>,
    /// Cost of every sub-agent this session launched, combined.
    ///
    /// The value is `None` when the session has no sub-agent, or when no
    /// sub-agent could be priced.
    pub subagents_cost: Option<SessionCost>,
    /// Billable token counts that back [`Self::cost`]. The count sums the
    /// parent transcript and every sub-agent.
    pub inclusive_tokens: Option<BillableTokens>,
    /// Billable token counts that back [`Self::subagents_cost`]. The count
    /// sums every sub-agent. The value is `None` when the session has no
    /// sub-agent.
    pub subagents_tokens: Option<BillableTokens>,
    /// Where the spend went, summed over the parent thread and every
    /// sub-agent thread. `None` when the transcript could not be read.
    pub efficiency: Option<EfficiencyTotals>,
    /// Every model that contributed billable tokens.
    pub models: Vec<String>,
    /// Parent model runs followed by runs used only by sub-agents.
    pub model_runs: Vec<ModelRun>,
    /// Billable tokens per model. The map merges the parent transcript and
    /// every sub-agent. The cache stores this map, so a later pass can
    /// re-price the session without reading any transcript again.
    pub inclusive_model_breakdown: HashMap<String, ModelTokens>,
    pub skills: Vec<SkillUse>,
    pub orchestration: Option<OrchestrationStatus>,
    /// The transcript this analysis was read from, when it is a file.
    pub source_path: Option<String>,
    /// `mtime:size` of that file, so a rescan can tell whether to redo the work.
    pub fingerprint: String,
    pub analyzed_generation: i64,
    pub started_at_epoch: Option<i64>,
    pub source_changed: bool,
    /// Each source's own `SessionSummary`, keyed by `source_key`. `Some`
    /// only for a worker pass (one with a `turn_row_store`) over an
    /// evidence-cohort agent — see [`stream_vendor_with_hooks`]. The
    /// drilldown's rows-replay path reads this back (persisted as
    /// `source_summaries_json`) to rebuild per-source metrics without a
    /// transcript.
    pub source_summaries: Option<BTreeMap<String, SessionSummary>>,
}

impl SessionAnalysis {
    /// The empty result for a session whose transcript could not be located or
    /// read. Deliberately not an error: a transcript the user deleted is an
    /// ordinary state, and the view has an empty state for it.
    pub fn unavailable() -> SessionAnalysis {
        SessionAnalysis {
            metrics: None,
            summary: None,
            cost: None,
            top_level_cost: None,
            subagents_cost: None,
            inclusive_tokens: None,
            subagents_tokens: None,
            efficiency: None,
            models: Vec::new(),
            model_runs: Vec::new(),
            inclusive_model_breakdown: HashMap::new(),
            skills: Vec::new(),
            orchestration: None,
            source_path: None,
            fingerprint: MISSING_FINGERPRINT.to_string(),
            analyzed_generation: 0,
            started_at_epoch: None,
            source_changed: false,
            source_summaries: None,
        }
    }

    /// The cache record for this analysis, when there is anything to cache.
    pub fn record(&self, key: &SessionKey) -> Option<AnalysisRecord> {
        self.metrics.as_ref()?;
        if self.source_changed {
            return None;
        }
        let revisions = projection_revisions();
        // `initial_context` sits on `metrics` (see `analyze_for_evidence`,
        // which grafts the parent's own breakdown onto the merged pass), so
        // this reads the same value on both the worker and legacy paths.
        let initial_context_json = self
            .metrics
            .as_ref()
            .and_then(|metrics| metrics.initial_context.as_ref())
            .and_then(|breakdown| serde_json::to_string(breakdown).ok());
        // `None` on every path except a worker pass over an evidence-cohort
        // agent — see `Self::source_summaries`'s doc comment.
        let source_summaries_json = self
            .source_summaries
            .as_ref()
            .and_then(|summaries| serde_json::to_string(summaries).ok());
        let provider_hints_json = self.source_summaries.as_ref().and_then(|summaries| {
            let mut hints: Vec<ProviderHint> = Vec::new();
            for hint in summaries
                .values()
                .flat_map(|summary| &summary.provider_hints)
            {
                if hints.len() == MAX_PROVIDER_HINTS {
                    break;
                }
                if !hints.contains(hint) {
                    hints.push(hint.clone());
                }
            }
            serde_json::to_string(&hints).ok()
        });
        Some(AnalysisRecord {
            key: key.clone(),
            model_breakdown_json: serde_json::to_string(&self.inclusive_model_breakdown)
                .unwrap_or_else(|_| "{}".to_string()),
            inclusive_models_json: serde_json::to_string(&self.model_runs)
                .unwrap_or_else(|_| "[]".to_string()),
            initial_context_json,
            source_summaries_json,
            provider_hints_json,
            source_fingerprint: self.fingerprint.clone(),
            pricing_generation: pricing_generation() as i64,
            analyzed_generation: self.analyzed_generation,
            parser_revision: revisions.parser_revision,
            analyzer_revision: revisions.analyzer_revision,
            metrics_schema_revision: revisions.metrics_schema_revision,
        })
    }
}

/// Merge per-model token breakdowns. Sum each model's counts across every
/// breakdown. A model that both the parent and a sub-agent use adds its
/// counts. No sub-agent spend is lost.
fn merge_model_breakdowns<'a>(
    breakdowns: impl IntoIterator<Item = &'a HashMap<String, ModelTokens>>,
) -> HashMap<String, ModelTokens> {
    let mut merged: HashMap<String, ModelTokens> = HashMap::new();
    for breakdown in breakdowns {
        for (model, tokens) in breakdown {
            let entry = merged.entry(model.clone()).or_default();
            entry.input_tokens += tokens.input_tokens;
            entry.output_tokens += tokens.output_tokens;
            entry.cache_read_tokens += tokens.cache_read_tokens;
            entry.cache_creation_tokens += tokens.cache_creation_tokens;
            entry.cache_creation_1h_tokens += tokens.cache_creation_1h_tokens;
        }
    }
    merged
}

/// Sum billable tokens across every model in one breakdown.
///
/// The sum matches the `billable_*` fields the engine computes on
/// `SessionMetrics` for a single transcript. Use this function for a
/// breakdown that spans more than one transcript, such as a merged
/// sub-agent breakdown.
fn sum_billable_tokens(breakdown: &HashMap<String, ModelTokens>) -> BillableTokens {
    let mut sum = BillableTokens::default();
    for tokens in breakdown.values() {
        sum.input_tokens += tokens.input_tokens;
        sum.output_tokens += tokens.output_tokens;
        sum.cache_read_tokens += tokens.cache_read_tokens;
        sum.cache_creation_tokens += tokens.cache_creation_tokens;
    }
    sum
}

/// Fingerprint stood in for a source with no file behind it (a vendor database,
/// an inline label). Such a source is never cache-skipped: it re-analyzes every
/// pass, because there is no cheap way to tell whether it changed.
pub const MISSING_FINGERPRINT: &str = "-";

/// This version invalidates cached values when the analysis cache contract changes.
const ANALYSIS_FINGERPRINT_VERSION: u8 = 2;

/// `mtime:size` of a transcript file, or [`MISSING_FINGERPRINT`].
pub fn fingerprint_of(source: &SessionSource) -> String {
    let SessionSource::File(path) = source else {
        return MISSING_FINGERPRINT.to_string();
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return MISSING_FINGERPRINT.to_string();
    };
    let mtime = metadata
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0);
    format!("{mtime}:{}", metadata.len())
}

/// Fingerprint the parent transcript and all current sub-agent transcripts.
pub async fn fingerprint_with_subagents(
    agent: AgentKind,
    session_id: &str,
    wsl_distro: Option<&str>,
    source: &SessionSource,
) -> String {
    let mut subagent_paths = Explorers::DISK
        .list_subagents_in_environment(&agent, session_id, wsl_distro)
        .await;
    subagent_paths.sort();
    let parent_fingerprint = match source {
        SessionSource::ProviderDb {
            agent,
            db_path,
            session_id,
        } => Explorers::DISK
            .provider_db_fingerprint(agent, db_path, session_id)
            .await
            .map(|(latest, rows)| format!("sv1:db:{latest}:{rows}"))
            .unwrap_or_else(|| MISSING_FINGERPRINT.to_string()),
        _ => fingerprint_of(source),
    };
    match source {
        SessionSource::ProviderDb { .. } if subagent_paths.is_empty() => parent_fingerprint,
        _ => combined_fingerprint_from_parent(parent_fingerprint, &subagent_paths),
    }
}

/// Cheap fingerprint for the open-detail poll.
///
/// Antigravity databases use file, WAL, and transcript metadata here. Claimed
/// ingestion still uses the full row-content fingerprint.
pub async fn poll_fingerprint_with_subagents(
    agent: AgentKind,
    session_id: &str,
    wsl_distro: Option<&str>,
    source: &SessionSource,
) -> String {
    if let SessionSource::ProviderDb {
        agent: AgentKind::Antigravity,
        db_path,
        session_id,
    } = source
    {
        let mut paths = vec![db_path.clone()];
        let mut wal = db_path.as_os_str().to_os_string();
        wal.push("-wal");
        paths.push(std::path::PathBuf::from(wal));
        if let Some(transcript) = antigravity_sibling_transcript(db_path, session_id) {
            paths.push(transcript);
        }
        let mut subagent_paths = Explorers::DISK
            .list_subagents_in_environment(&agent, session_id, wsl_distro)
            .await;
        subagent_paths.sort();
        paths.extend(subagent_paths);
        return poll_fingerprint_from_paths(paths);
    }
    fingerprint_with_subagents(agent, session_id, wsl_distro, source).await
}

fn poll_fingerprint_from_paths(paths: Vec<std::path::PathBuf>) -> String {
    let parts = paths
        .into_iter()
        .map(|path| {
            (
                path.to_string_lossy().into_owned(),
                fingerprint_of_path(&path),
            )
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&parts)
        .map(|fingerprint| format!("poll-v1:{fingerprint}"))
        .unwrap_or_else(|_| MISSING_FINGERPRINT.to_string())
}

fn fingerprint_of_path(path: &std::path::Path) -> String {
    let Ok(metadata) = std::fs::metadata(path) else {
        return MISSING_FINGERPRINT.to_string();
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{modified}:{}", metadata.len())
}

fn antigravity_sibling_transcript(
    db_path: &std::path::Path,
    session_id: &str,
) -> Option<std::path::PathBuf> {
    let conversations = db_path.parent()?;
    if conversations.file_name()?.to_str()? != "conversations" {
        return None;
    }
    Some(
        conversations
            .parent()?
            .join("brain")
            .join(session_id)
            .join(".system_generated")
            .join("logs")
            .join("transcript.jsonl"),
    )
}

/// Build one stable fingerprint from a parent and its sorted child paths.
fn combined_fingerprint(source: &SessionSource, subagent_paths: &[std::path::PathBuf]) -> String {
    combined_fingerprint_from_parent(fingerprint_of(source), subagent_paths)
}

fn combined_fingerprint_from_parent(
    parent_fingerprint: String,
    subagent_paths: &[std::path::PathBuf],
) -> String {
    if parent_fingerprint == MISSING_FINGERPRINT {
        return parent_fingerprint;
    }

    let parts = std::iter::once(("parent".to_string(), parent_fingerprint)).chain(
        subagent_paths.iter().map(|path| {
            (
                path.to_string_lossy().into_owned(),
                fingerprint_of(&SessionSource::File(path.clone())),
            )
        }),
    );
    serde_json::to_string(&parts.collect::<Vec<_>>())
        .map(|fingerprint| format!("v{ANALYSIS_FINGERPRINT_VERSION}:{fingerprint}"))
        .unwrap_or_else(|_| MISSING_FINGERPRINT.to_string())
}

/// The path of a file-backed source.
pub fn source_path(source: &SessionSource) -> Option<String> {
    match source {
        SessionSource::File(path) => Some(path.to_string_lossy().to_string()),
        _ => None,
    }
}

/// Locate one session's transcript, honoring the environment it ran in.
pub async fn locate(
    agent: AgentKind,
    session_id: &str,
    wsl_distro: Option<&str>,
) -> Option<SessionSource> {
    Explorers::DISK
        .locate_session_source_in_environment(&agent, session_id, wsl_distro)
        .await
}

/// Shape a located source into the raw payload the analysis layer reads.
async fn raw_source(source: &SessionSource) -> Option<RawSource> {
    match source {
        SessionSource::File(path) => Some(RawSource::File(path.clone())),
        SessionSource::Inline { content, .. } => Some(RawSource::Jsonl(content.clone())),
        SessionSource::ProviderDb {
            agent: AgentKind::OpenCode | AgentKind::Antigravity,
            db_path,
            ..
        } => Some(RawSource::Sqlite(db_path.clone())),
        SessionSource::ProviderDb { .. } => {
            session_source_content(source).await.map(RawSource::Jsonl)
        }
    }
}

fn claim_file(path: &std::path::Path) -> anyhow::Result<SourceClaim> {
    let mut file = std::fs::File::open(path)?;
    let stat = SourceStat::from_open_std_file(&file)
        .ok_or_else(|| anyhow::anyhow!("cannot read source metadata"))?;
    let mut head = Vec::new();
    file.by_ref()
        .take(antiburn_local::discovery::source_version::FINGERPRINT_HEAD_BYTES as u64)
        .read_to_end(&mut head)?;
    Ok(SourceClaim::from_fingerprint_inputs(&FingerprintInputs {
        stat,
        head_hash: Some(antiburn_local::discovery::source_version::head_hash_of(
            &head,
        )),
    }))
}

fn inline_fingerprint(content: &str) -> String {
    FingerprintInputs {
        stat: SourceStat {
            identity: None,
            size: content.len() as u64,
            modified_nanos: None,
            changed_nanos: None,
        },
        head_hash: Some(antiburn_local::discovery::source_version::head_hash_of(
            content.as_bytes(),
        )),
    }
    .fingerprint()
}

/// No-op `after_claim` hook for [`stream_vendor_with_hooks`]. Production
/// callers (`analyze_for_evidence`) have no reason to observe a source's
/// claim as it streams; a test that does inject its own hook directly
/// instead, through [`stream_vendor_with_claim_hook`].
fn no_after_claim(_: usize, _: &std::path::Path) {}

// `stream_vendor` and `stream_vendor_with_claim_hook` have no production
// caller left: the drilldown's live-parse fallback (`analyze_subagent`) is
// gone, and every remaining caller is test scaffolding for
// `stream_vendor_with_hooks`'s streaming contract — see the `save_analysis`
// precedent for the same treatment. `stream_vendor_with_hooks` itself stays
// outside `#[cfg(test)]`: `analyze_for_evidence` still calls it directly in
// production.
#[cfg(test)]
fn stream_vendor(inputs: &[SessionInput], cancel: &CancelFlag) -> StreamOutcome {
    stream_vendor_with_claim_hook(inputs, cancel, &no_after_claim)
}

#[cfg(test)]
fn stream_vendor_with_claim_hook(
    inputs: &[SessionInput],
    cancel: &CancelFlag,
    after_claim: &dyn Fn(usize, &std::path::Path),
) -> StreamOutcome {
    stream_vendor_with_hooks(inputs, &|| cancel.cancelled(), after_claim, None, None)
}

/// Every source's evidence-streaming contract. Total: an
/// uncharacterized vendor (Cursor, Antigravity, or any generic-JSONL agent)
/// still gets a real `SourceCapabilities` profile — an honest, mostly- or
/// fully-unset one — so it takes the same streaming path as every other
/// vendor instead of a separate, uncharacterized fallback. `published_status`
/// (insights_worker.rs) is what turns an unset profile into the terminal
/// `Unsupported` state; this function's job is only to describe the source,
/// never to decide whether it is good enough.
fn capabilities_for_source(agent: &str, source: &RawSource) -> SourceCapabilities {
    let mut capabilities = match agent {
        "claude" => SourceCapabilities::claude(),
        "codex" => SourceCapabilities::codex(),
        "opencode" => SourceCapabilities::opencode(),
        "pi" => SourceCapabilities::pi(),
        "cursor" => SourceCapabilities::cursor(),
        "antigravity" => SourceCapabilities::antigravity(),
        _ => SourceCapabilities::generic(),
    };
    if agent == "antigravity" && matches!(source, RawSource::Sqlite(_)) {
        capabilities.cache_write_tokens = true;
        capabilities.token_classes = true;
    }
    capabilities
}

fn adapter_supports_provider_db(agent: &str) -> bool {
    matches!(agent, "opencode" | "antigravity")
}

/// One child input's contribution to the parent's folded coverage, kept
/// apart from the parent's own residual until the fold at the end of
/// [`stream_vendor_with_hooks`]. `Unreadable` marks a discovered child this
/// pass could not read; `Coverage` carries that child's own residual.
enum ChildFold {
    Coverage(Box<SessionEvidenceAccumulator>),
    Unreadable,
}

/// The bootstrap [`StreamSnapshot`] for a file source with no valid stored
/// resume yet: offset zero (so the adapter reads the whole source from the
/// start, like `visit_claimed`), fresh adapter state, and fresh metrics and
/// evidence. `None` when the adapter has no bootstrap state at all — it
/// does not support resume — so the caller reads fully through
/// `visit_claimed` instead.
///
/// Still worth attempting on a full read forced by something other than a
/// missing snapshot (a tail rewrite, or a resumed visit that failed
/// partway): passing a fresh, offset-zero snapshot to
/// `visit_claimed_resumed` gives the same read as `visit_claimed` while
/// still producing a real `AdapterResume` for the next pass — see
/// `VendorAdapter::visit_claimed_resumed`'s doc comment.
fn bootstrap_snapshot(
    adapter: &dyn VendorAdapter,
    agent: &str,
    session_id: &str,
    kind: SourceKind,
    capabilities: SourceCapabilities,
) -> Option<StreamSnapshot> {
    let adapter_state = adapter.empty_resume_state()?;
    let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
        agent: agent.to_owned(),
        session_id: session_id.to_owned(),
        kind,
        capabilities,
    });
    Some(StreamSnapshot {
        revision: RESUME_SNAPSHOT_REVISION,
        resume: ResumePoint {
            offset: 0,
            tail_hash: antiburn_local::discovery::source_version::head_hash_of(&[]),
            tail_len: 0,
        },
        adapter: adapter_state,
        metrics: SessionMetricsAccumulator::new(agent, session_id),
        evidence: EvidenceSnapshot {
            record: evidence.coverage_record(),
            resume: EvidenceResumeState::default(),
        },
        next_turn_index: 0,
    })
}

/// The [`StreamSnapshot`] to attempt `visit_claimed_resumed` with for one
/// file source, and whether it came from a genuine stored resume (`true`)
/// or [`bootstrap_snapshot`] (`false`). `None` when `bootstrap_snapshot`
/// itself would return `None` — the adapter does not support resume at
/// all — so the caller reads fully through `visit_claimed` instead,
/// exactly as before this change.
fn stream_snapshot_to_attempt(
    store: &dyn TurnRowStore,
    adapter: &dyn VendorAdapter,
    agent: &str,
    session_id: &str,
    kind: SourceKind,
    capabilities: SourceCapabilities,
    current_revisions: &ResumeRevisions,
) -> Option<(StreamSnapshot, bool)> {
    let stored_snapshot = store
        .read_resume(session_id)
        .ok()
        .flatten()
        .filter(|stored| current_revisions.matches(stored))
        .and_then(|stored| StreamSnapshot::decode(&stored.snapshot).ok())
        .filter(StreamSnapshot::is_current);
    if let Some(snapshot) = stored_snapshot {
        return Some((snapshot, true));
    }
    Some((
        bootstrap_snapshot(adapter, agent, session_id, kind, capabilities)?,
        false,
    ))
}

/// A [`StoredResume`] for `adapter_resume`, built from `sink`'s own
/// metrics, evidence, and row-index state — see [`CompositeSink::snapshot`].
/// `None` when `sink` cannot publish yet (the same rule
/// [`CompositeSink::snapshot`] itself uses).
fn stored_resume(
    sink: &CompositeSink,
    adapter_resume: AdapterResume,
    current_revisions: &ResumeRevisions,
    source_fingerprint: String,
) -> Option<StoredResume> {
    let snapshot = sink.snapshot(adapter_resume)?;
    Some(StoredResume {
        snapshot: snapshot.encode(),
        snapshot_revision: current_revisions.snapshot_revision,
        parser_revision: current_revisions.parser_revision,
        analyzer_revision: current_revisions.analyzer_revision,
        metrics_schema_revision: current_revisions.metrics_schema_revision,
        evidence_schema_revision: current_revisions.evidence_schema_revision,
        coverage_schema_revision: current_revisions.coverage_schema_revision,
        source_fingerprint,
    })
}

/// The result of [`delete_then_full_read`]'s retry: the read outcome, the
/// resume it produced (if any), and — only when the bootstrap path won —
/// the sink that now holds this pass's rows and metrics for the source,
/// which the caller must swap in for its own accumulator.
struct FullReadRetry {
    outcome: anyhow::Result<VisitOutcome>,
    resume: Option<AdapterResume>,
    sink: Option<CompositeSink>,
}

/// The per-source values [`delete_then_full_read`] needs to build both a
/// bootstrap [`StreamSnapshot`] and a fresh [`TurnRowSink`] for it,
/// grouped so the retry helper stays under the argument-count lint.
#[derive(Clone, Copy)]
struct SourceContext {
    kind: SourceKind,
    capabilities: SourceCapabilities,
    scope: Option<TurnScope>,
}

/// Drops a source's rows already flushed this pass under the claim fence,
/// then reads it again in full from the start. Used whenever a resumed
/// visit's own rows can no longer be trusted for the rest of this pass:
/// its own tail check caught a mid-stream rewrite, or the adapter failed
/// partway through and left an unknown amount flushed. Either way the
/// retried full read must start a fresh row sequence, not join whatever
/// the resumed attempt already wrote. A delete failure fails the whole
/// pass, the same as any other unreadable parent.
///
/// The retry itself first tries [`bootstrap_snapshot`] through
/// `visit_claimed_resumed`, the same fresh-offset-zero approach the very
/// first pass over a source uses, so a source forced back to a full read
/// still comes out of this pass resumable next time instead of costing
/// another full read on top. Only when that bootstrap attempt itself
/// fails (or the adapter has no bootstrap state at all) does this fall
/// back to the plain `visit_claimed` that never produces a resume.
fn delete_then_full_read(
    store: &Arc<dyn TurnRowStore>,
    adapter: &dyn VendorAdapter,
    input: &SessionInput,
    claim: &SourceClaim,
    context: SourceContext,
    cancelled: &dyn Fn() -> bool,
    accumulator: &mut CompositeSink,
) -> Result<FullReadRetry, StreamOutcome> {
    if store.delete_rows_for_source(&input.session_id).is_err() {
        return Err(StreamOutcome::ParentUnreadable);
    }
    if let Some(snapshot) = bootstrap_snapshot(
        adapter,
        &input.agent,
        &input.session_id,
        context.kind,
        context.capabilities,
    ) {
        let mut bootstrap_sink = CompositeSink::with_turn_rows(
            SessionMetricsAccumulator::restore(snapshot.metrics.clone()),
            SessionEvidenceAccumulator::from_coverage_record_with_resume(
                snapshot.evidence.record.clone(),
                snapshot.evidence.resume.clone(),
            ),
            TurnRowSink::new(Arc::clone(store), input.session_id.clone(), context.scope)
                .with_start_index(snapshot.next_turn_index),
        );
        match adapter.visit_claimed_resumed(input, claim, &snapshot, cancelled, &mut bootstrap_sink)
        {
            Ok(visit) => {
                return Ok(FullReadRetry {
                    outcome: Ok(visit.outcome),
                    resume: visit.resume,
                    sink: Some(bootstrap_sink),
                });
            }
            Err(_) => {
                // The bootstrap attempt may itself have flushed some rows
                // before failing partway through. The plain fallback below
                // must not join them.
                if store.delete_rows_for_source(&input.session_id).is_err() {
                    return Err(StreamOutcome::ParentUnreadable);
                }
            }
        }
    }
    let outcome = adapter.visit_claimed(
        input,
        claim,
        append_only_guarantee(adapter.agent()),
        cancelled,
        accumulator,
    );
    Ok(FullReadRetry {
        outcome,
        resume: None,
        sink: None,
    })
}

fn stream_vendor_with_hooks(
    inputs: &[SessionInput],
    cancelled: &dyn Fn() -> bool,
    after_claim: &dyn Fn(usize, &std::path::Path),
    database_claim: Option<&str>,
    turn_row_store: Option<Arc<dyn TurnRowStore>>,
) -> StreamOutcome {
    let mut metrics_accumulators = Vec::with_capacity(inputs.len());
    // The parent's own residual evidence accumulator, set once index 0
    // streams successfully. Unlike `child_folds`, this never has a later
    // child folded into it in place: the fold that produces the coverage
    // record runs once, at the end, over a clone of this value — see
    // `ChildFold` and the fold below the loop.
    let mut parent_residual: Option<SessionEvidenceAccumulator> = None;
    // Each child's own contribution, kept apart from the parent's residual
    // until the fold at the end, in input order.
    let mut child_folds: Vec<ChildFold> = Vec::new();
    let mut parent_fingerprint = None;
    // Captured only for a worker pass (a `turn_row_store` is given): the
    // point of persisting these is to replay rows later, so a pass with no
    // rows to replay skips the clone. See `SessionAnalysis::source_summaries`.
    let mut source_summaries: BTreeMap<String, SessionSummary> = BTreeMap::new();
    // Every source this pass read through the resume-aware path, and how —
    // see `stream_snapshot_to_attempt`. Threaded to `Store::publish_projections`
    // through `StreamedSession::source_outcomes`.
    let mut source_outcomes: Vec<SourcePublishOutcome> = Vec::new();
    let current_revisions = resume_revisions();
    for (index, input) in inputs.iter().enumerate() {
        if cancelled() {
            return StreamOutcome::ParentUnreadable;
        }
        // Every vendor label now resolves to a real (possibly all-unset)
        // capability profile, so this can never fall through to
        // `StreamOutcome::ParentUnsupported` the way an unrecognized vendor
        // used to; a SQLite source from an adapter without database support is the one
        // remaining path that outcome still covers, further down.
        let capabilities = capabilities_for_source(&input.agent, &input.source);
        let kind = SourceKind::from(&input.source);
        let adapter = adapter_for(&input.agent);
        // Every input after the parent is a discovered child transcript, so
        // its rows get `Delegated` scope from position. The adapter's own
        // `EventSource` flag is not the only source of scope.
        let scope = (index > 0).then_some(TurnScope::Delegated);
        let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            kind,
            capabilities,
        });
        // The turn-row source key is the input's own session id: the parent
        // transcript's id for index 0, a child's own id for every other
        // input. `thread_id` equals `source_key` in this change.
        let mut accumulator = match turn_row_store.as_ref() {
            Some(store) => CompositeSink::with_turn_rows(
                metrics,
                evidence,
                TurnRowSink::new(Arc::clone(store), input.session_id.clone(), scope),
            ),
            None => CompositeSink::new(metrics, evidence),
        };
        // Each source's own resume mode and adapter-side resume state,
        // carried from the `RawSource::File` arm below to the point past
        // `accumulator.observe_source_outcome` where `CompositeSink::snapshot`
        // becomes callable. `None` for a source this pass never tried to
        // resume or bootstrap — every non-`File` source, and a `File`
        // source whose adapter does not support resume at all.
        let mut pending_resume: Option<(SourcePublishMode, Option<AdapterResume>, String)> = None;
        let result = match &input.source {
            RawSource::File(path) => {
                let claim = match claim_file(path) {
                    Ok(claim) => claim,
                    Err(_) if cancelled() => return StreamOutcome::ParentUnreadable,
                    Err(error)
                        if index == 0
                            && error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                                error.kind() == std::io::ErrorKind::NotFound
                            }) =>
                    {
                        return StreamOutcome::ParentMissing;
                    }
                    Err(_) if index == 0 => return StreamOutcome::ParentUnreadable,
                    Err(_) => {
                        child_folds.push(ChildFold::Unreadable);
                        continue;
                    }
                };
                let fingerprint = claim.fingerprint.clone();
                after_claim(index, path);

                let attempt = turn_row_store.as_ref().and_then(|store| {
                    stream_snapshot_to_attempt(
                        store.as_ref(),
                        adapter,
                        &input.agent,
                        &input.session_id,
                        kind,
                        capabilities,
                        &current_revisions,
                    )
                });
                let outcome = if let (Some(store), Some((snapshot, is_real_resume))) =
                    (turn_row_store.as_ref(), attempt)
                {
                    let mut resumed = CompositeSink::with_turn_rows(
                        SessionMetricsAccumulator::restore(snapshot.metrics.clone()),
                        SessionEvidenceAccumulator::from_coverage_record_with_resume(
                            snapshot.evidence.record.clone(),
                            snapshot.evidence.resume.clone(),
                        ),
                        TurnRowSink::new(Arc::clone(store), input.session_id.clone(), scope)
                            .with_start_index(snapshot.next_turn_index),
                    );
                    match adapter.visit_claimed_resumed(
                        input,
                        &claim,
                        &snapshot,
                        cancelled,
                        &mut resumed,
                    ) {
                        Ok(ResumedVisit {
                            outcome: VisitOutcome::SourceChanged(_),
                            ..
                        }) => {
                            // The resumed visit's own tail check caught a
                            // rewrite. Whatever it already flushed under
                            // this pass's claim fence must not survive
                            // alongside the full read's own rows.
                            match delete_then_full_read(
                                store,
                                adapter,
                                input,
                                &claim,
                                SourceContext {
                                    kind,
                                    capabilities,
                                    scope,
                                },
                                cancelled,
                                &mut accumulator,
                            ) {
                                Ok(retry) => {
                                    pending_resume = Some((
                                        SourcePublishMode::Full,
                                        retry.resume,
                                        fingerprint.clone(),
                                    ));
                                    if let Some(sink) = retry.sink {
                                        accumulator = sink;
                                    }
                                    retry.outcome
                                }
                                Err(early_return) => return early_return,
                            }
                        }
                        Ok(visit) => {
                            if is_real_resume {
                                // Only this source's new rows have moved to
                                // this pass's claim fence; its old rows stay
                                // at the published fence until publish
                                // re-stamps them. Tell the store so a
                                // mid-pass fact read still sees both.
                                store.note_resumed_source(&input.session_id);
                            }
                            pending_resume = Some((
                                if is_real_resume {
                                    SourcePublishMode::Resumed
                                } else {
                                    SourcePublishMode::Full
                                },
                                visit.resume,
                                fingerprint.clone(),
                            ));
                            accumulator = resumed;
                            Ok(visit.outcome)
                        }
                        Err(_) => {
                            // The resumed visit failed partway through, so
                            // it may have already flushed some rows under
                            // this pass's claim fence before it did. The
                            // fallback full read must not join those.
                            match delete_then_full_read(
                                store,
                                adapter,
                                input,
                                &claim,
                                SourceContext {
                                    kind,
                                    capabilities,
                                    scope,
                                },
                                cancelled,
                                &mut accumulator,
                            ) {
                                Ok(retry) => {
                                    pending_resume = Some((
                                        SourcePublishMode::Full,
                                        retry.resume,
                                        fingerprint.clone(),
                                    ));
                                    if let Some(sink) = retry.sink {
                                        accumulator = sink;
                                    }
                                    retry.outcome
                                }
                                Err(early_return) => return early_return,
                            }
                        }
                    }
                } else {
                    adapter.visit_claimed(
                        input,
                        &claim,
                        append_only_guarantee(adapter.agent()),
                        cancelled,
                        &mut accumulator,
                    )
                };
                if index == 0 {
                    parent_fingerprint = Some(fingerprint);
                }
                outcome
            }
            RawSource::Jsonl(content) => {
                if index == 0 {
                    parent_fingerprint = Some(inline_fingerprint(content));
                }
                adapter.visit(input, &mut accumulator)
            }
            RawSource::Sqlite(_)
                if !adapter_supports_provider_db(adapter.agent()) && index == 0 =>
            {
                return StreamOutcome::ParentUnsupported;
            }
            RawSource::Sqlite(_) if !adapter_supports_provider_db(adapter.agent()) => continue,
            RawSource::Sqlite(_) if index == 0 => {
                let outcome = match database_claim {
                    Some(fingerprint) => {
                        adapter.visit_db_claimed(input, fingerprint, cancelled, &mut accumulator)
                    }
                    None => adapter.visit(input, &mut accumulator),
                };
                if database_claim.is_some() {
                    parent_fingerprint = database_claim.map(str::to_owned);
                }
                outcome
            }
            RawSource::Sqlite(_) => continue,
        };
        match result {
            Ok(outcome @ VisitOutcome::SourceChanged(_)) => {
                accumulator.observe_source_outcome(outcome);
                return StreamOutcome::SourceChanged;
            }
            Ok(outcome) => {
                accumulator.observe_source_outcome(outcome);
                if cancelled() {
                    return StreamOutcome::ParentUnreadable;
                }
                // A turn-row write failure must fail the whole pass rather
                // than publish metrics and evidence the rows disagree with.
                // Retried like any other unreadable source.
                if accumulator.turn_row_write_failed() {
                    return StreamOutcome::ParentUnreadable;
                }
                if turn_row_store.is_some()
                    && let Some(summary) = accumulator.summary()
                {
                    source_summaries.insert(input.session_id.clone(), summary.clone());
                }
                // `CompositeSink::snapshot` needs `&accumulator`, so this
                // must run before `into_parts` consumes it below. Both
                // share the same "can this residual publish yet" gate, so
                // a source `into_parts` is about to reject here (unusable)
                // resolves to `None` here too and is discarded with it.
                let resolved_resume =
                    pending_resume
                        .take()
                        .map(|(mode, adapter_resume, source_fingerprint)| {
                            let resume = adapter_resume.and_then(|adapter_resume| {
                                stored_resume(
                                    &accumulator,
                                    adapter_resume,
                                    &current_revisions,
                                    source_fingerprint,
                                )
                            });
                            (mode, resume)
                        });
                let Some((metrics, residual)) = accumulator.into_parts() else {
                    if index == 0 {
                        return StreamOutcome::ParentUnreadable;
                    }
                    child_folds.push(ChildFold::Unreadable);
                    continue;
                };
                if index == 0 {
                    parent_residual = Some(residual);
                } else {
                    child_folds.push(ChildFold::Coverage(Box::new(residual)));
                }
                metrics_accumulators.push(metrics);
                if let Some((mode, resume)) = resolved_resume {
                    source_outcomes.push(SourcePublishOutcome {
                        source_key: input.session_id.clone(),
                        mode,
                        resume,
                    });
                }
            }
            Err(_) if cancelled() || index == 0 => {
                return StreamOutcome::ParentUnreadable;
            }
            Err(_) => {
                child_folds.push(ChildFold::Unreadable);
                continue;
            }
        }
    }
    if cancelled() {
        return StreamOutcome::ParentUnreadable;
    }
    let Some((parent, children)) = metrics_accumulators.split_first() else {
        return StreamOutcome::ParentUnreadable;
    };
    let started_at_epoch = metrics_accumulators
        .iter()
        .filter_map(SessionMetricsAccumulator::started_at_ms)
        .min()
        .map(|timestamp| timestamp / 1000);
    let parent_metrics = parent.metrics();
    let subagents = children
        .iter()
        .map(|child| (child.metrics(), child.started_at_ms().map(|ts| ts / 1000)))
        .collect();
    let merged = merge_metrics(parent, children);
    // The coverage record folds a clone of the parent's own residual with
    // every child's residual (or unreadable marker), in input order. This
    // never mutates `parent_residual` itself, so a future caller can still
    // read each source's own, unfolded residual — see `ChildFold`.
    let folded_residual = parent_residual.clone().map(|mut folded| {
        for child in &child_folds {
            match child {
                ChildFold::Coverage(residual) => folded.observe_child_coverage(residual),
                ChildFold::Unreadable => folded.observe_child_unreadable(),
            }
        }
        folded
    });
    // A pass without a row store publishes no evidence. Rows and a coverage
    // record are both required. Neither exists without a store. A query or
    // write failure fails the whole pass. This matches the turn-row write
    // failure above: published metrics must never disagree with rows this
    // pass could not read back. The same fail-the-pass rule covers
    // `row_projections`: `inclusive_model_breakdown` and `model_runs` must
    // never publish out of step with the rows this pass itself wrote.
    let (evidence, row_projections) = match turn_row_store {
        Some(store) => {
            // The worker never builds `SessionEvidence` from its own
            // in-memory fold. It writes the fold's `SessionCoverageRecord`
            // under this pass's claim fence. Then it reads the facts back
            // through the store's own SQL query and reads the record back
            // too. It replays both with `evidence_from_facts`. A resumed
            // pass with no live fold needs the same rebuild, so the worker
            // exercises it on every publish.
            let evidence = match folded_residual {
                Some(residual) => {
                    let record = residual.coverage_record();
                    if store.write_coverage_record(&record).is_err() {
                        return StreamOutcome::ParentUnreadable;
                    }
                    let facts = match store.query_turn_facts() {
                        Ok(facts) => facts,
                        Err(_) => return StreamOutcome::ParentUnreadable,
                    };
                    let record = match store.query_coverage_record() {
                        Ok(Some(record)) => record,
                        Ok(None) | Err(_) => return StreamOutcome::ParentUnreadable,
                    };
                    Some(evidence_from_facts(&facts, &record))
                }
                None => None,
            };
            let model_breakdown = match store.query_model_breakdown() {
                Ok(model_breakdown) => model_breakdown,
                Err(_) => return StreamOutcome::ParentUnreadable,
            };
            let model_runs = match store.query_model_runs() {
                Ok(model_runs) => model_runs,
                Err(_) => return StreamOutcome::ParentUnreadable,
            };
            (
                evidence,
                Some(RowProjections {
                    model_breakdown,
                    model_runs,
                }),
            )
        }
        None => (None, None),
    };
    // `row_projections` is `Some` exactly when this pass had a
    // `turn_row_store` — the same condition that gates the capture loop
    // above — so it doubles as the flag for whether to keep the captured
    // summaries rather than discard them.
    let source_summaries = row_projections.is_some().then_some(source_summaries);
    StreamOutcome::Published {
        session: Box::new(StreamedSession {
            parent: parent_metrics,
            merged,
            subagents,
            started_at_epoch,
            evidence,
            row_projections,
            source_summaries,
            source_outcomes,
        }),
        parent_fingerprint,
    }
}

fn attributed_generation(claimed: &ClaimedSource, actual_fingerprint: Option<&str>) -> i64 {
    match (claimed.fingerprint.as_deref(), actual_fingerprint) {
        (Some(expected), Some(actual)) if expected == actual => claimed.generation,
        _ => 0,
    }
}

/// Order the sub-agent roster earliest-first.
///
/// A member with no known start time sorts after every timed member. Ties —
/// including every `None`, which tie with each other — keep `members`' own
/// incoming order, because `sort_by_key` is stable and the roster already
/// arrives sorted by transcript path.
fn sort_members(members: &mut [SubagentMember]) {
    members.sort_by_key(|member| (member.started_at_epoch.is_none(), member.started_at_epoch));
}

/// Analyze one session and, in the same pass, every sub-agent it launched.
///
/// One pass avoids an extra analysis call for each sub-agent. The engine
/// returns the parent and sub-agent metrics in one batch, analyzed
/// independently, which this function uses for the top-level/sub-agents cost
/// split. Separately, this merges every stream's events into one
/// time-aligned session (sub-agents are an implementation detail of the
/// parent, per the product rule) and analyzes that once more for the
/// session's own headline metrics — buckets, token totals, tool mix — so the
/// detail view's chart and header sum a sub-agent's activity into the same
/// session instead of hiding it.
///
/// `export_session` (`commands.rs`) is this function's only caller now. The
/// drilldown itself reads rows instead — see `analysis_from_rows` — because
/// export wants a live parse: a fresh fingerprint and a `source_path` an
/// exported document can point to, neither of which a row replay carries.
pub async fn analyze(
    agent: AgentKind,
    session_id: &str,
    wsl_distro: Option<&str>,
    claimed: ClaimedSource,
    cancel: CancelFlag,
) -> SessionAnalysis {
    let pass = analyze_for_evidence(
        agent,
        session_id,
        wsl_distro,
        claimed,
        PassSignal::from_cancel(cancel),
        None,
    )
    .await;
    debug_assert!(pass.evidence.is_none() || pass.outcome == PassOutcome::Published);
    pass.analysis
}

/// `turn_row_store` is `Some` only for a pass the durable worker runs under
/// a claimed evidence fence — see `insights_worker::run_record_pass`. Every
/// other caller (an on-demand session view, a scan-triggered pass with no
/// claim) passes `None`: without a claim fence, rows would have nothing to
/// be stamped with and nothing to be cleaned up under.
pub async fn analyze_for_evidence(
    agent: AgentKind,
    session_id: &str,
    wsl_distro: Option<&str>,
    claimed: ClaimedSource,
    signal: PassSignal,
    turn_row_store: Option<Arc<dyn TurnRowStore>>,
) -> EvidencePass {
    let Some(source) = locate(agent, session_id, wsl_distro).await else {
        return unavailable_evidence_pass(PassOutcome::SourceMissing, None, None);
    };
    let Some(raw) = raw_source(&source).await else {
        return unavailable_evidence_pass(
            PassOutcome::Unreadable,
            source_path(&source),
            Some(fingerprint_of(&source)),
        );
    };

    let label = vendor_label(agent);
    let parent_input = SessionInput {
        agent: label.to_string(),
        session_id: session_id.to_string(),
        source: raw,
    };

    // Sub-agent transcripts, resolved before the analysis so all of them ride
    // the same batch. The engine short-circuits for vendors that record no
    // orchestration, so this needs no per-agent gate of its own.
    let mut subagent_paths = Explorers::DISK
        .list_subagents_in_environment(&agent, session_id, wsl_distro)
        .await;
    subagent_paths.sort();
    let database_claim = matches!(&source, SessionSource::ProviderDb { .. })
        .then(|| claimed.fingerprint.clone())
        .flatten();
    let fingerprint = if matches!(&source, SessionSource::ProviderDb { .. }) {
        let parent_fingerprint = claimed
            .fingerprint
            .clone()
            .unwrap_or_else(|| MISSING_FINGERPRINT.to_string());
        if subagent_paths.is_empty() {
            parent_fingerprint
        } else {
            combined_fingerprint_from_parent(parent_fingerprint, &subagent_paths)
        }
    } else {
        combined_fingerprint(&source, &subagent_paths)
    };
    let mut subagents: Vec<(String, String, SessionInput)> = Vec::new();
    for path in subagent_paths {
        let Some(subagent_id) = Explorers::DISK.subagent_id(&agent, &path) else {
            continue;
        };
        let label_text = Explorers::DISK.subagent_label(&agent, &path).await;
        subagents.push((
            subagent_id.clone(),
            label_text,
            SessionInput {
                agent: label.to_string(),
                session_id: subagent_id,
                source: RawSource::File(path),
            },
        ));
    }

    let source_path = source_path(&source);
    let parent_session_id = session_id.to_string();
    let agent_slug = agent.slug().to_string();

    let mut inputs = Vec::with_capacity(subagents.len() + 1);
    inputs.push(parent_input.clone());
    inputs.extend(subagents.iter().map(|(_, _, input)| input.clone()));

    let roster: Vec<(String, String)> = subagents
        .into_iter()
        .map(|(id, label, _)| (id, label))
        .collect();

    // The engine's analysis is synchronous and CPU-bound; keep it off the
    // runtime's worker threads. Every vendor now has a real (possibly
    // all-unset) `SourceCapabilities` profile, so every session streams
    // through `stream_vendor_with_hooks` — there is no separate,
    // uncharacterized-vendor fallback pass any more.
    let signal_for_pass = signal.clone();
    let computed = tauri::async_runtime::spawn_blocking(move || {
        let cancelled = || signal_for_pass.observe();
        match stream_vendor_with_hooks(
            &inputs,
            &cancelled,
            &no_after_claim,
            database_claim.as_deref(),
            turn_row_store,
        ) {
            StreamOutcome::Published {
                session,
                parent_fingerprint,
            } => ComputedAnalysis::Published {
                parent: Box::new(session.parent),
                merged: Box::new(session.merged),
                subagents: session.subagents,
                started_at_epoch: session.started_at_epoch,
                parent_fingerprint,
                evidence: Box::new(session.evidence),
                row_projections: session.row_projections,
                source_summaries: session.source_summaries,
                source_outcomes: session.source_outcomes,
            },
            StreamOutcome::SourceChanged => ComputedAnalysis::SourceChanged,
            StreamOutcome::ParentMissing => ComputedAnalysis::Missing,
            StreamOutcome::ParentUnsupported => ComputedAnalysis::Unsupported,
            StreamOutcome::ParentUnreadable => ComputedAnalysis::Unavailable,
        }
    })
    .await;

    debug_assert!(signal.progress() > 0);
    let Ok(computed) = computed else {
        return unavailable_evidence_pass(PassOutcome::Unreadable, source_path, Some(fingerprint));
    };
    let (
        parent_metrics,
        merged,
        subagents,
        started_at_epoch,
        parent_fingerprint,
        evidence,
        row_projections,
        source_summaries,
        source_outcomes,
    ) = match computed {
        ComputedAnalysis::Published {
            parent,
            merged,
            subagents,
            started_at_epoch,
            parent_fingerprint,
            evidence,
            row_projections,
            source_summaries,
            source_outcomes,
        } => (
            *parent,
            *merged,
            subagents,
            started_at_epoch,
            parent_fingerprint,
            *evidence,
            row_projections,
            source_summaries,
            source_outcomes,
        ),
        ComputedAnalysis::SourceChanged => {
            return EvidencePass {
                analysis: SessionAnalysis {
                    source_path,
                    fingerprint,
                    source_changed: true,
                    ..SessionAnalysis::unavailable()
                },
                evidence: None,
                outcome: PassOutcome::SourceChanged,
                source_outcomes: Vec::new(),
            };
        }
        ComputedAnalysis::Missing => {
            return unavailable_evidence_pass(
                PassOutcome::SourceMissing,
                source_path,
                Some(fingerprint),
            );
        }
        ComputedAnalysis::Unsupported => {
            return unavailable_evidence_pass(
                PassOutcome::Unsupported,
                source_path,
                Some(fingerprint),
            );
        }
        ComputedAnalysis::Unavailable => {
            return unavailable_evidence_pass(
                PassOutcome::Unreadable,
                source_path,
                Some(fingerprint),
            );
        }
    };
    let analyzed_generation = attributed_generation(&claimed, parent_fingerprint.as_deref());
    let by_id: HashMap<String, (SessionMetrics, Option<i64>)> = subagents
        .into_iter()
        .map(|(metrics, started_at_epoch)| {
            (metrics.session_id.clone(), (metrics, started_at_epoch))
        })
        .collect();
    let analysis = assemble_session_analysis(AssembledMetrics {
        parent_metrics,
        merged,
        by_id,
        roster,
        row_projections,
        agent_slug,
        parent_session_id,
        source_path,
        fingerprint,
        analyzed_generation,
        started_at_epoch,
        source_summaries,
    });
    EvidencePass {
        analysis,
        evidence,
        outcome: PassOutcome::Published,
        source_outcomes,
    }
}

/// Everything [`assemble_session_analysis`] needs to build one
/// [`SessionAnalysis`] from a parent's and every sub-agent's own metrics —
/// whether they came from a live streaming pass ([`analyze_for_evidence`])
/// or the drilldown's rows-replay path ([`analysis_from_rows`]).
struct AssembledMetrics {
    parent_metrics: SessionMetrics,
    /// The parent's and every sub-agent's events, merged and time-aligned —
    /// see `merge_metrics`/`metrics_from_rows`.
    merged: SessionMetrics,
    /// Each sub-agent's own metrics, paired with the unix-second timestamp
    /// of its earliest transcript event, keyed by its own session id.
    by_id: HashMap<String, (SessionMetrics, Option<i64>)>,
    /// `(subagent_id, label)` for every sub-agent this session's roster
    /// names, in no particular order — `sort_members` orders the result.
    roster: Vec<(String, String)>,
    row_projections: Option<RowProjections>,
    agent_slug: String,
    parent_session_id: String,
    source_path: Option<String>,
    fingerprint: String,
    analyzed_generation: i64,
    started_at_epoch: Option<i64>,
    source_summaries: Option<BTreeMap<String, SessionSummary>>,
}

/// Builds the session-detail [`SessionAnalysis`] from a parent's and every
/// sub-agent's own metrics. Shared by the live streaming pass
/// ([`analyze_for_evidence`]) and the drilldown's rows-replay path
/// ([`analysis_from_rows`]), so the two build the identical DTO shape from
/// whichever source supplied the metrics.
fn assemble_session_analysis(input: AssembledMetrics) -> SessionAnalysis {
    let AssembledMetrics {
        parent_metrics,
        merged,
        by_id,
        roster,
        row_projections,
        agent_slug,
        parent_session_id,
        source_path,
        fingerprint,
        analyzed_generation,
        started_at_epoch,
        source_summaries,
    } = input;

    // `metrics` is the session's headline view: buckets, token totals, and
    // tool mix summed across the parent and every sub-agent, time-aligned.
    // `initial_context` and `skill_uses` stay off the merged pass — they are
    // grafted onto `parent_metrics` from the parent's own raw transcript, and
    // a sub-agent's initial context describes a different, disposable
    // context window, not this session's. `merged` can only be `None` if the
    // parent transcript stopped normalizing between the two analysis passes
    // above, which does not happen in practice; falling back to
    // `parent_metrics` keeps that theoretical case merely parent-only
    // instead of unavailable.
    let mut metrics = merged;
    metrics.initial_context = parent_metrics.initial_context.clone();
    metrics.skill_uses = parent_metrics.skill_uses.clone();
    // Each sub-agent runs its own context window, so its spend split comes
    // from its own thread and is added to the parent's, not read off the
    // merged stream.
    let mut efficiency = parent_metrics.efficiency;
    for (child, _) in by_id.values() {
        efficiency.add(child.efficiency);
    }
    metrics.efficiency = efficiency;

    // The views key icons and copy off the discovery slug, so the vendor label
    // the adapter registry dispatches on never leaves this module.
    metrics.agent = agent_slug.clone();
    cap_skill_descriptions(&mut metrics.skill_uses);

    let mut members: Vec<SubagentMember> = roster
        .into_iter()
        .map(|(subagent_id, label)| {
            // The roster and `by_id` both key on the sub-agent's own session
            // id. A roster entry with no matching metrics means the child
            // transcript could not be analyzed this pass, so its cost and
            // tokens are `None` rather than a zeroed figure.
            let child_metrics = by_id.get(&subagent_id);
            let cost = child_metrics.and_then(|(child, _)| price_breakdown(&child.model_breakdown));
            let tokens =
                child_metrics.map(|(child, _)| sum_billable_tokens(&child.model_breakdown));
            let model_runs = child_metrics
                .map(|(child, _)| model_runs_for_metrics(child))
                .unwrap_or_default();
            let started_at_epoch =
                child_metrics.and_then(|(_, started_at_epoch)| *started_at_epoch);
            SubagentMember {
                agent: agent_slug.clone(),
                subagent_id,
                label,
                cost,
                tokens,
                model_runs,
                started_at_epoch,
            }
        })
        .collect();
    sort_members(&mut members);

    let orchestration = (!members.is_empty()).then_some(OrchestrationStatus {
        orchestrating: members.len() as u32 >= MIN_ORCHESTRATED_SUBAGENTS,
        orchestrator_agent: agent_slug,
        orchestrator_session_id: parent_session_id,
        subagent_count: members.len() as u32,
        members,
    });

    // `top_level_cost` prices the parent transcript alone; `subagents_cost`
    // merges every sub-agent's own breakdown. `metrics.model_breakdown` is
    // already inclusive — it comes from the merged event stream above, so it
    // sums to the same totals `top_level_cost` and `subagents_cost` would
    // combine to. `cost`/`inclusive_model_breakdown` read it directly instead
    // of re-merging the per-source breakdowns a second way.
    let subagent_breakdowns: Vec<&HashMap<String, ModelTokens>> = by_id
        .values()
        .map(|(child, _)| &child.model_breakdown)
        .collect();
    let has_subagents = !subagent_breakdowns.is_empty();
    let subagents_model_breakdown = merge_model_breakdowns(subagent_breakdowns.iter().copied());
    let accumulator_model_breakdown = metrics.model_breakdown.clone();
    let accumulator_model_runs =
        model_runs_parent_first(&parent_metrics, by_id.values().map(|(child, _)| child));
    // The worker path (`row_projections` is `Some`) reads
    // `inclusive_model_breakdown` and `model_runs` back from published turn
    // rows instead of the accumulator — see `RowProjections`. Every other
    // caller keeps the accumulator's own values.
    let (inclusive_model_breakdown, model_runs) = match row_projections {
        Some(RowProjections {
            model_breakdown,
            model_runs,
        }) => (model_breakdown.into_iter().collect(), model_runs),
        None => (accumulator_model_breakdown, accumulator_model_runs),
    };

    let top_level_cost = price_breakdown(&parent_metrics.model_breakdown);
    let subagents_cost = price_breakdown(&subagents_model_breakdown);
    let cost = metrics.cost;
    let models = sorted_models(&inclusive_model_breakdown);
    let inclusive_tokens = Some(sum_billable_tokens(&inclusive_model_breakdown));
    let subagents_tokens = has_subagents.then(|| sum_billable_tokens(&subagents_model_breakdown));
    let skills = metrics.skill_uses.clone();
    let summary = aggregate_metrics(vec![metrics.clone()]);

    SessionAnalysis {
        efficiency: Some(metrics.efficiency),
        metrics: Some(metrics),
        summary: Some(summary),
        cost,
        top_level_cost,
        subagents_cost,
        inclusive_tokens,
        subagents_tokens,
        models,
        model_runs,
        inclusive_model_breakdown,
        skills,
        orchestration,
        source_path,
        fingerprint,
        analyzed_generation,
        started_at_epoch,
        source_changed: false,
        source_summaries,
    }
}

/// Rebuilds one session's `SessionAnalysis` from its last-published turn
/// rows and the per-source summaries a worker pass persisted alongside
/// them — seam R3c. Touches no transcript: every input comes from `store`.
///
/// Returns `None` when replay cannot proceed, which the caller (the
/// `get_session_analysis` command switch) reads as "fall back to the live
/// parse": no published rows yet (evidence not `ready`), no cached
/// analysis record, no `source_summaries_json` on that record (a legacy or
/// scan-triggered pass, never a worker one, wrote it), that JSON failing to
/// parse, or [`metrics_from_rows`] finding no row group for the session's
/// own id ([`antiburn_local::analysis::MissingParentRows`] — the same
/// "rows exist but look wrong" signal a live production command must not
/// panic on).
pub fn analysis_from_rows(
    store: &Store,
    key: &SessionKey,
    session_id: &str,
    agent_slug: &str,
) -> Option<SessionAnalysis> {
    let rows = store.published_turn_rows(key).ok().flatten()?;
    let record = store.analysis(key).ok().flatten()?;
    let source_summaries_json = record.source_summaries_json.as_deref()?;
    let source_summaries: BTreeMap<String, SessionSummary> =
        serde_json::from_str(source_summaries_json).ok()?;
    let initial_context: Option<InitialContextBreakdown> = record
        .initial_context_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok());

    let summary_for = |source_key: &str| {
        source_summaries
            .get(source_key)
            .cloned()
            .unwrap_or_default()
    };
    let mut by_source = metrics_by_source(agent_slug, session_id, &rows, summary_for);
    let mut parent_metrics = by_source.remove(session_id)?;
    // `metrics_by_source` replays each source's own tool-call history from
    // its rows' last-tool-only summary (`replay.rs`'s module doc comment),
    // so the `initial_context` it projects is not the live pass's own — the
    // R3b parity harness excludes it from comparison for the same reason.
    // The persisted `initial_context_json` (seam R3) is the live pass's own
    // value; grafting it here is exactly what the live path's own
    // `assemble_session_analysis` graft step already does downstream, from
    // whichever `parent_metrics.initial_context` this function hands it.
    if let Some(initial_context) = initial_context {
        parent_metrics.initial_context = Some(initial_context);
    }
    let merged = metrics_from_rows(agent_slug, session_id, &rows, summary_for).ok()?;

    let by_id: HashMap<String, (SessionMetrics, Option<i64>)> = by_source
        .into_iter()
        .map(|(source_key, metrics)| {
            let started_at_epoch = source_started_at_epoch(&source_summaries, &rows, &source_key);
            (source_key, (metrics, started_at_epoch))
        })
        .collect();

    // The roster's labels come from the last worker pass's own discovery,
    // not from rows (a row carries no label) — `publish_projections`
    // persists them as `session_relation` rows in the same transaction
    // that publishes these turn rows, so the two are never out of step.
    let roster = store
        .relations(key)
        .unwrap_or_default()
        .into_iter()
        .filter(|relation| relation.kind == RelationKind::Subagent)
        .map(|relation| {
            let label = relation.label.unwrap_or_else(|| "Sub-agent".to_string());
            (relation.related_id, label)
        })
        .collect();

    let started_at_epoch = source_started_at_epoch(&source_summaries, &rows, session_id);

    Some(assemble_session_analysis(AssembledMetrics {
        parent_metrics,
        merged,
        by_id,
        roster,
        // The accumulator's own model breakdown and runs are what
        // `metrics_by_source`/`metrics_from_rows` just derived — there is
        // no separate row-store query to reconcile against here, unlike
        // the worker pass's own `RowProjections`.
        row_projections: None,
        agent_slug: agent_slug.to_string(),
        parent_session_id: session_id.to_string(),
        // Rows carry no file path; the drilldown's reveal action stays
        // unavailable for a replayed view. `get_session_analysis_fingerprint`
        // still resolves the live path independently for the freshness poll.
        source_path: None,
        fingerprint: record.source_fingerprint,
        analyzed_generation: record.analyzed_generation,
        started_at_epoch,
        // Nothing new to persist: this call replays the worker's own
        // published rows, it does not write anything.
        source_summaries: None,
    }))
}

/// One source's started-at time, unix seconds: the persisted summary's own
/// `started_at_ms` when it has one, else the earliest `ts_ms` among that
/// source's own rows — the same two-step fallback
/// `SessionMetricsAccumulator::started_at_ms` uses live.
fn source_started_at_epoch(
    source_summaries: &BTreeMap<String, SessionSummary>,
    rows: &[TurnRow],
    source_key: &str,
) -> Option<i64> {
    source_summaries
        .get(source_key)
        .and_then(|summary| summary.started_at_ms)
        .or_else(|| {
            rows.iter()
                .filter(|row| row.source_key == source_key)
                .filter_map(|row| row.ts_ms)
                .min()
        })
        .map(|ms| ms / 1000)
}

pub(crate) fn unsupported_evidence_pass() -> EvidencePass {
    unavailable_evidence_pass(PassOutcome::Unsupported, None, None)
}

fn unavailable_evidence_pass(
    outcome: PassOutcome,
    source_path: Option<String>,
    fingerprint: Option<String>,
) -> EvidencePass {
    EvidencePass {
        analysis: SessionAnalysis {
            source_path,
            fingerprint: fingerprint.unwrap_or_else(|| MISSING_FINGERPRINT.to_string()),
            ..SessionAnalysis::unavailable()
        },
        evidence: None,
        outcome,
        source_outcomes: Vec::new(),
    }
}

#[cfg(test)]
pub(crate) fn evidence_pass(inputs: &[SessionInput], cancelled: &dyn Fn() -> bool) -> EvidencePass {
    evidence_pass_with_hook(inputs, cancelled, &no_after_claim, None)
}

/// Like [`evidence_pass`], with a [`TurnRowStore`] fanned out through the
/// same real `stream_vendor_with_hooks` path the worker uses. Lets a test
/// outside this module (`insights_worker`'s) assert on turn rows a published
/// pass wrote, without a fake analyzer that skips the row sink entirely.
#[cfg(test)]
pub(crate) fn evidence_pass_with_turn_rows(
    inputs: &[SessionInput],
    cancelled: &dyn Fn() -> bool,
    turn_row_store: Option<Arc<dyn TurnRowStore>>,
) -> EvidencePass {
    evidence_pass_with_hook(inputs, cancelled, &no_after_claim, turn_row_store)
}

#[cfg(test)]
fn evidence_pass_with_hook(
    inputs: &[SessionInput],
    cancelled: &dyn Fn() -> bool,
    after_claim: &dyn Fn(usize, &std::path::Path),
    turn_row_store: Option<Arc<dyn TurnRowStore>>,
) -> EvidencePass {
    match stream_vendor_with_hooks(inputs, cancelled, after_claim, None, turn_row_store) {
        StreamOutcome::Published { session, .. } => {
            let StreamedSession {
                merged,
                evidence,
                started_at_epoch,
                source_summaries,
                source_outcomes,
                ..
            } = *session;
            EvidencePass {
                analysis: SessionAnalysis {
                    metrics: Some(merged),
                    started_at_epoch,
                    source_summaries,
                    ..SessionAnalysis::unavailable()
                },
                evidence,
                outcome: PassOutcome::Published,
                source_outcomes,
            }
        }
        StreamOutcome::SourceChanged => {
            unavailable_evidence_pass(PassOutcome::SourceChanged, None, None)
        }
        StreamOutcome::ParentMissing => {
            unavailable_evidence_pass(PassOutcome::SourceMissing, None, None)
        }
        StreamOutcome::ParentUnsupported => {
            unavailable_evidence_pass(PassOutcome::Unsupported, None, None)
        }
        StreamOutcome::ParentUnreadable => {
            unavailable_evidence_pass(PassOutcome::Unreadable, None, None)
        }
    }
}

pub fn projection_revisions() -> ProjectionRevisions {
    ProjectionRevisions {
        parser_revision: PARSER_REVISION,
        analyzer_revision: ANALYZER_REVISION,
        metrics_schema_revision: METRICS_SCHEMA_REVISION,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
    }
}

/// The revisions a stored `source_resume` snapshot must match to be
/// trusted. Passed to `Store::purge_stale_source_resume` on startup,
/// alongside `projection_revisions` feeding `reconcile_evidence_revisions`.
pub fn resume_revisions() -> ResumeRevisions {
    ResumeRevisions {
        snapshot_revision: RESUME_SNAPSHOT_REVISION,
        parser_revision: PARSER_REVISION,
        analyzer_revision: ANALYZER_REVISION,
        metrics_schema_revision: METRICS_SCHEMA_REVISION,
        evidence_schema_revision: EVIDENCE_SCHEMA_REVISION,
        coverage_schema_revision: COVERAGE_SCHEMA_REVISION,
    }
}

/// Builds the session-detail [`SessionAnalysis`] for a transcript with no
/// sub-agent split of its own — a sub-agent viewed on its own, from the
/// drilldown's rows-replay path ([`subagent_analysis_from_rows`]). `cost` and
/// `top_level_cost` name the same figure: a sub-agent launches no sub-agent
/// of its own, so its own transcript is the whole story.
fn standalone_session_analysis(
    mut metrics: SessionMetrics,
    agent_slug: String,
    source_path: Option<String>,
    fingerprint: String,
    started_at_epoch: Option<i64>,
) -> SessionAnalysis {
    metrics.agent = agent_slug;
    cap_skill_descriptions(&mut metrics.skill_uses);

    let cost = price_breakdown(&metrics.model_breakdown);
    let models = sorted_models(&metrics.model_breakdown);
    let model_runs = model_runs_for_metrics(&metrics);
    let inclusive_model_breakdown = metrics.model_breakdown.clone();
    let inclusive_tokens = Some(sum_billable_tokens(&inclusive_model_breakdown));
    let skills = metrics.skill_uses.clone();
    let summary = aggregate_metrics(vec![metrics.clone()]);

    SessionAnalysis {
        efficiency: Some(metrics.efficiency),
        metrics: Some(metrics),
        summary: Some(summary),
        cost,
        top_level_cost: cost,
        subagents_cost: None,
        inclusive_tokens,
        subagents_tokens: None,
        models,
        model_runs,
        inclusive_model_breakdown,
        skills,
        orchestration: None,
        source_path,
        fingerprint,
        analyzed_generation: 0,
        started_at_epoch,
        source_changed: false,
        source_summaries: None,
    }
}

/// Rebuilds one sub-agent's `SessionAnalysis` from the parent session's
/// last-published turn rows and persisted per-source summaries — the same
/// data [`analysis_from_rows`] reads, assembled as a standalone view of
/// just that sub-agent's own rows (`source_key == subagent_id`).
///
/// Returns `None` under the same conditions [`analysis_from_rows`] does,
/// plus one more: no row group in `rows` carries `subagent_id` as its own
/// `source_key` (the child transcript's rows are not among what this pass
/// published — a session with sub-agents this worker pass could not read).
pub fn subagent_analysis_from_rows(
    store: &Store,
    parent_key: &SessionKey,
    parent_session_id: &str,
    subagent_id: &str,
    agent_slug: &str,
) -> Option<SessionAnalysis> {
    let rows = store.published_turn_rows(parent_key).ok().flatten()?;
    let record = store.analysis(parent_key).ok().flatten()?;
    let source_summaries_json = record.source_summaries_json.as_deref()?;
    let source_summaries: BTreeMap<String, SessionSummary> =
        serde_json::from_str(source_summaries_json).ok()?;

    let summary_for = |source_key: &str| {
        source_summaries
            .get(source_key)
            .cloned()
            .unwrap_or_default()
    };
    let mut by_source = metrics_by_source(agent_slug, parent_session_id, &rows, summary_for);
    let metrics = by_source.remove(subagent_id)?;
    let started_at_epoch = source_started_at_epoch(&source_summaries, &rows, subagent_id);

    Some(standalone_session_analysis(
        metrics,
        agent_slug.to_string(),
        None,
        record.source_fingerprint,
        started_at_epoch,
    ))
}

/// Whether analysis of this agent's transcripts is more than a generic parse.
pub fn analysis_supported(agent: AgentKind) -> bool {
    supports_analysis(agent)
}

/// How many leading transcript lines are searched for fork evidence.
/// The evidence is in a metadata header near the start of the transcript.
const FORK_OBSERVATION_LINES: usize = 5;

/// How deep the search descends into a header record. The observation sits at
/// the top level or one nesting down (`metadata`, `raw`); four is slack.
const FORK_OBSERVATION_DEPTH: usize = 4;

/// The session this one was branched from, when the vendor's store records it.
///
/// Lineage is *evidence the transcript carries*, so it is resolved when a
/// session is opened rather than on every scan: reading every transcript to
/// look for a header would cost far more than the relationship is worth.
pub async fn fork_parent(source: &SessionSource) -> Option<String> {
    let content = match source {
        SessionSource::Inline { content, .. } => content.clone(),
        SessionSource::File(path) => tokio::fs::read_to_string(path).await.ok()?,
        SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path,
            session_id,
        } => {
            return antiburn_local::discovery::agents::opencode::db_fork_parent(
                db_path.clone(),
                session_id.clone(),
            )
            .await;
        }
        SessionSource::ProviderDb { .. } => session_source_preview(source).await?,
    };
    fork_parent_from_content(&content)
}

/// Read a declared fork parent from a bounded transcript preview.
pub fn fork_parent_from_content(content: &str) -> Option<String> {
    content
        .lines()
        .take(FORK_OBSERVATION_LINES)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|value| find_fork_parent(&value, FORK_OBSERVATION_DEPTH))
}

/// Read a fork parent from vendor metadata or a normalized observation.
fn find_fork_parent(value: &serde_json::Value, depth: usize) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let object = value.as_object()?;
    if object.get("type").and_then(serde_json::Value::as_str) == Some("session_meta")
        && let Some(parent_id) = object
            .get("payload")
            .and_then(|payload| payload.get("forked_from_id"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|parent_id| !parent_id.is_empty())
    {
        return Some(parent_id.to_string());
    }
    if let Some(observation) = object.get(FORK_OBSERVATION_KEY)
        && let Ok(observation) = serde_json::from_value::<ForkObservation>(observation.clone())
        && !observation.parent_agent_session_id.is_empty()
    {
        return Some(observation.parent_agent_session_id);
    }
    object
        .values()
        .find_map(|nested| find_fork_parent(nested, depth - 1))
}

/// Every model that contributed billable tokens, in a stable order.
pub fn sorted_models(breakdown: &HashMap<String, ModelTokens>) -> Vec<String> {
    let mut models: Vec<String> = breakdown
        .keys()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect();
    models.sort();
    models
}

/// Return sorted parent model runs, followed by sorted child-only model runs.
fn model_runs_parent_first<'a>(
    parent: &SessionMetrics,
    children: impl Iterator<Item = &'a SessionMetrics>,
) -> Vec<ModelRun> {
    model_runs_parent_first_lists(
        model_runs_for_metrics(parent),
        children.map(model_runs_for_metrics),
    )
}

fn model_runs_parent_first_lists(
    mut runs: Vec<ModelRun>,
    child_runs: impl Iterator<Item = Vec<ModelRun>>,
) -> Vec<ModelRun> {
    let mut seen: HashSet<ModelRun> = runs.iter().cloned().collect();
    let child_runs = child_runs.flatten().collect::<BTreeSet<_>>().into_iter();
    runs.extend(child_runs.filter(|run| seen.insert(run.clone())));
    runs
}

/// Return the distinct model runs for one session.
fn model_runs_for_metrics(metrics: &SessionMetrics) -> Vec<ModelRun> {
    if metrics.model_runs.is_empty() {
        return sorted_models(&metrics.model_breakdown)
            .into_iter()
            .map(|model| ModelRun {
                model,
                thinking_mode: None,
            })
            .collect();
    }
    metrics
        .model_runs
        .iter()
        .filter_map(normalize_model_run)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_model_run(run: &ModelRun) -> Option<ModelRun> {
    let model = run.model.trim();
    if model.is_empty() {
        return None;
    }
    let mode = run
        .thinking_mode
        .as_deref()
        .map(str::trim)
        .filter(|mode| !mode.is_empty());
    Some(ModelRun {
        model: model.to_string(),
        thinking_mode: mode.map(str::to_string),
    })
}

/// Read the inclusive model runs from a cached analysis.
pub fn cached_inclusive_model_runs(inclusive_models_json: &str) -> Vec<ModelRun> {
    let mut seen = HashSet::new();
    serde_json::from_str::<Vec<ModelRun>>(inclusive_models_json)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|run| normalize_model_run(&run))
        .filter(|run| seen.insert(run.clone()))
        .collect()
}

/// Whether meaningful session activity fell inside the active window. The
/// timestamp is semantic when the transcript provides one, rather than a
/// filesystem-touch heartbeat.
pub fn is_active(updated_at_epoch: Option<i64>, now: i64) -> bool {
    updated_at_epoch.is_some_and(|updated| now.saturating_sub(updated) < ACTIVE_SESSION_WINDOW_SECS)
}

/// Re-price a cached model breakdown against the current pricing table.
pub fn price_cached_breakdown(model_breakdown_json: &str) -> (Option<SessionCost>, Vec<String>) {
    let Ok(breakdown) = serde_json::from_str::<HashMap<String, ModelTokens>>(model_breakdown_json)
    else {
        return (None, Vec::new());
    };
    (price_breakdown(&breakdown), sorted_models(&breakdown))
}

#[cfg(test)]
mod tests;

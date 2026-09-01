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
    ANALYZER_REVISION, ActiveSessionsSummary, CompositeSink, EVIDENCE_SCHEMA_REVISION,
    EfficiencyTotals, EvidenceSource, InitialContextBreakdown, MAX_PROVIDER_HINTS,
    METRICS_SCHEMA_REVISION, ModelRun, PARSER_REVISION, ProviderHint, RawSource, SessionCost,
    SessionEvidence, SessionEvidenceAccumulator, SessionInput, SessionMetrics,
    SessionMetricsAccumulator, SessionSummary, SkillUse, SourceCapabilities, SourceClaim,
    SourceKind, TurnRow, TurnRowSink, TurnRowStore, TurnScope, VisitOutcome, adapter_for,
    aggregate_metrics, append_only_guarantee, merge_metrics, metrics_by_source, metrics_from_rows,
    price_breakdown, pricing_generation,
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
    VendorAdapter, analyze_sources_with,
};
#[cfg(test)]
use antiburn_local::insights::{DetectorId, clean_facts_complete, eligible};

use crate::agents::{supports_analysis, vendor_label};
use crate::dto::{BillableTokens, OrchestrationStatus, SubagentMember};
use crate::store::{AnalysisRecord, ProjectionRevisions, RelationKind, SessionKey, Store};

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

/// Records one discovered child that this pass could not read. The parent
/// streams at index 0, so the residual exists for every child path; the
/// guard keeps a broken invariant from panicking the blocking thread.
fn note_child_unreadable(parent_residual: &mut Option<SessionEvidenceAccumulator>) {
    if let Some(parent) = parent_residual.as_mut() {
        parent.observe_child_unreadable();
    }
}

fn stream_vendor_with_hooks(
    inputs: &[SessionInput],
    cancelled: &dyn Fn() -> bool,
    after_claim: &dyn Fn(usize, &std::path::Path),
    database_claim: Option<&str>,
    turn_row_store: Option<Arc<dyn TurnRowStore>>,
) -> StreamOutcome {
    let mut metrics_accumulators = Vec::with_capacity(inputs.len());
    // The parent's residual evidence accumulator. Set once index 0 streams
    // successfully, then folded into by every child that streams after it —
    // see `SessionEvidenceAccumulator::observe_child_coverage` and
    // `observe_child_unreadable`. One document covers the parent and every
    // child, so this must outlive the loop rather than live inside the
    // per-index accumulator the loop below builds.
    let mut parent_residual: Option<SessionEvidenceAccumulator> = None;
    let mut parent_fingerprint = None;
    // Captured only for a worker pass (a `turn_row_store` is given): the
    // point of persisting these is to replay rows later, so a pass with no
    // rows to replay skips the clone. See `SessionAnalysis::source_summaries`.
    let mut source_summaries: BTreeMap<String, SessionSummary> = BTreeMap::new();
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
        let adapter = adapter_for(&input.agent);
        let metrics = SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
        let evidence = SessionEvidenceAccumulator::new(EvidenceSource {
            agent: input.agent.clone(),
            session_id: input.session_id.clone(),
            kind: SourceKind::from(&input.source),
            capabilities,
        });
        // The turn-row source key is the input's own session id: the parent
        // transcript's id for index 0, a child's own id for every other
        // input. `thread_id` equals `source_key` in this change. Every
        // input after the parent is a discovered child transcript, so its
        // rows get `Delegated` scope from position. The adapter's own
        // `EventSource` flag is not the only source of scope.
        let mut accumulator = match turn_row_store.as_ref() {
            Some(store) => {
                let scope = (index > 0).then_some(TurnScope::Delegated);
                CompositeSink::with_turn_rows(
                    metrics,
                    evidence,
                    TurnRowSink::new(Arc::clone(store), input.session_id.clone(), scope),
                )
            }
            None => CompositeSink::new(metrics, evidence),
        };
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
                        note_child_unreadable(&mut parent_residual);
                        continue;
                    }
                };
                let fingerprint = claim.fingerprint.clone();
                after_claim(index, path);
                let outcome = adapter.visit_claimed(
                    input,
                    &claim,
                    append_only_guarantee(adapter.agent()),
                    cancelled,
                    &mut accumulator,
                );
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
                let Some((metrics, residual)) = accumulator.into_parts() else {
                    if index == 0 {
                        return StreamOutcome::ParentUnreadable;
                    }
                    note_child_unreadable(&mut parent_residual);
                    continue;
                };
                if index == 0 {
                    parent_residual = Some(residual);
                } else if let Some(parent) = parent_residual.as_mut() {
                    parent.observe_child_coverage(&residual);
                }
                metrics_accumulators.push(metrics);
            }
            Err(_) if cancelled() || index == 0 => {
                return StreamOutcome::ParentUnreadable;
            }
            Err(_) => {
                note_child_unreadable(&mut parent_residual);
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
    // A pass without a row store publishes no evidence: the residual alone
    // cannot build a `SessionEvidence`, since every row-derived group comes
    // from `TurnFacts`. A row query failure fails the whole pass, the same
    // way a turn-row write failure does above — published metrics must
    // never disagree with rows this pass could not read back. The same
    // fail-the-pass rule covers `row_projections`: `inclusive_model_breakdown`
    // and `model_runs` must never publish out of step with the rows this
    // pass itself wrote.
    let (evidence, row_projections) = match turn_row_store {
        Some(store) => {
            let facts = match store.query_turn_facts() {
                Ok(facts) => facts,
                Err(_) => return StreamOutcome::ParentUnreadable,
            };
            let evidence = parent_residual.map(|residual| residual.evidence(&facts));
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
        } => (
            *parent,
            *merged,
            subagents,
            started_at_epoch,
            parent_fingerprint,
            *evidence,
            row_projections,
            source_summaries,
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
mod tests {
    use super::*;

    /// A row store for a test pass that wants published evidence. A pass
    /// without a row store publishes no evidence, so any test that reads
    /// `session.evidence` or `pass.evidence` needs one of these.
    fn turn_row_store(agent: &str, session_id: &str) -> Arc<dyn TurnRowStore> {
        MemoryTurnRowStore::new(agent, session_id)
    }

    /// Reads the value out of an `EvidenceValue`, for `Complete` and
    /// `Partial` alike. Panics on `Unsupported` — every group these tests
    /// read is supported by the Claude capability set.
    fn observed<T: Clone>(value: &EvidenceValue<T>) -> T {
        match value {
            EvidenceValue::Complete(observed) | EvidenceValue::Partial { observed, .. } => {
                observed.clone()
            }
            EvidenceValue::Unsupported => panic!("evidence group must be supported"),
        }
    }

    #[tokio::test]
    async fn opencode_lineage_does_not_render_an_ordinary_database_session() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let db_path = directory.path().join("opencode.db");
        let connection = rusqlite::Connection::open(&db_path).expect("database");
        connection
            .execute_batch(
                "CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, title TEXT);
                 INSERT INTO session VALUES ('ses_root', '/repo', 'Ordinary session');",
            )
            .expect("schema");
        drop(connection);
        antiburn_local::discovery::track_provider_db_renders(&db_path);

        let parent = fork_parent(&SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: db_path.clone(),
            session_id: "ses_root".to_string(),
        })
        .await;

        assert_eq!(parent, None);
        assert_eq!(
            antiburn_local::discovery::take_tracked_provider_db_renders(&db_path),
            0
        );
    }

    fn member_with_start(subagent_id: &str, started_at_epoch: Option<i64>) -> SubagentMember {
        SubagentMember {
            agent: "claude-code".to_string(),
            subagent_id: subagent_id.to_string(),
            label: "Sub-agent".to_string(),
            cost: None,
            tokens: None,
            model_runs: Vec::new(),
            started_at_epoch,
        }
    }

    #[test]
    fn sort_members_orders_earliest_first_and_puts_unknown_starts_last() {
        let mut members = vec![
            member_with_start("late", Some(200)),
            member_with_start("unknown-first", None),
            member_with_start("early", Some(100)),
            member_with_start("unknown-second", None),
        ];

        sort_members(&mut members);

        let ids: Vec<&str> = members
            .iter()
            .map(|member| member.subagent_id.as_str())
            .collect();
        // Timed members sort earliest-first; both `None` members follow,
        // keeping their original relative order (a stable sort).
        assert_eq!(
            ids,
            vec!["early", "late", "unknown-first", "unknown-second"]
        );
    }

    fn claude_record(id: &str, timestamp: i64) -> String {
        format!(
            concat!(
                r#"{{"type":"assistant","timestamp":{timestamp},"message":{{"id":"{id}","role":"assistant","model":"claude-3-5-haiku-20241022","usage":{{"input_tokens":2,"output_tokens":3}},"content":[{{"type":"text","text":"Synthetic output."}}]}}}}"#,
                "\n"
            ),
            timestamp = timestamp,
            id = id,
        )
    }

    /// Like [`claude_record`], with an explicit model and an optional
    /// top-level `speed` signal.
    fn claude_record_with(id: &str, timestamp: i64, model: &str, speed: Option<&str>) -> String {
        let speed_field = speed
            .map(|speed| format!(",\"speed\":\"{speed}\""))
            .unwrap_or_default();
        format!(
            "{{\"type\":\"assistant\",\"timestamp\":{timestamp}{speed_field},\"message\":{{\"id\":\"{id}\",\"role\":\"assistant\",\"model\":\"{model}\",\"usage\":{{\"input_tokens\":2,\"output_tokens\":3}},\"content\":[{{\"type\":\"text\",\"text\":\"Synthetic output.\"}}]}}}}\n"
        )
    }

    fn file_input(path: &std::path::Path, id: &str) -> SessionInput {
        SessionInput {
            agent: "claude".to_string(),
            session_id: id.to_string(),
            source: RawSource::File(path.to_path_buf()),
        }
    }

    fn inline_input(content: String, id: &str) -> SessionInput {
        SessionInput {
            agent: "claude".to_string(),
            session_id: id.to_string(),
            source: RawSource::Jsonl(content),
        }
    }

    fn opencode_database() -> (tempfile::TempDir, std::path::PathBuf, String) {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("opencode.db");
        let connection = rusqlite::Connection::open(&path).expect("database");
        connection
            .execute_batch(
                r#"CREATE TABLE session (
                     id TEXT PRIMARY KEY, parent_id TEXT, title TEXT,
                     time_created INTEGER, time_updated INTEGER
                 );
                 CREATE TABLE message (
                     id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                     time_updated INTEGER, data TEXT
                 );
                 CREATE TABLE part (
                     id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                     time_created INTEGER, time_updated INTEGER, data TEXT
                 );
                 INSERT INTO session (id, parent_id, time_created, time_updated)
                 VALUES ('root', NULL, 100, 120);
                 INSERT INTO message VALUES (
                     'message', 'root', 110, 110,
                     '{"role":"assistant","modelID":"model-a","tokens":{"input":12,"output":3}}'
                 );"#,
            )
            .expect("OpenCode fixture");
        drop(connection);
        (directory, path, "sv1:db:120:2".to_owned())
    }

    fn antigravity_database() -> (tempfile::TempDir, std::path::PathBuf) {
        fn varint(mut value: u64, out: &mut Vec<u8>) {
            while value >= 0x80 {
                out.push((value as u8 & 0x7f) | 0x80);
                value >>= 7;
            }
            out.push(value as u8);
        }
        fn scalar(field: u64, value: u64, out: &mut Vec<u8>) {
            varint(field << 3, out);
            varint(value, out);
        }
        fn bytes(field: u64, value: &[u8], out: &mut Vec<u8>) {
            varint((field << 3) | 2, out);
            varint(value.len() as u64, out);
            out.extend_from_slice(value);
        }

        let mut usage = Vec::new();
        scalar(1, 777, &mut usage);
        scalar(2, 30, &mut usage);
        scalar(3, 50, &mut usage);
        scalar(4, 7, &mut usage);
        scalar(5, 800, &mut usage);
        scalar(9, 40, &mut usage);
        scalar(10, 10, &mut usage);
        bytes(11, b"response-1", &mut usage);
        let mut chat_model = Vec::new();
        bytes(4, &usage, &mut chat_model);
        bytes(19, b"gemini-3.6-flash", &mut chat_model);
        let mut blob = Vec::new();
        bytes(1, &chat_model, &mut blob);

        let directory = tempfile::TempDir::new().expect("tempdir");
        let subroot = directory.path().join("antigravity-cli");
        let conversations = subroot.join("conversations");
        let logs = subroot
            .join("brain")
            .join("root")
            .join(".system_generated")
            .join("logs");
        std::fs::create_dir_all(&conversations).unwrap();
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(
            logs.join("transcript.jsonl"),
            concat!(
                r#"{"type":"USER_INPUT","created_at":"2026-01-01T00:00:00Z","content":"hello"}"#,
                "\n",
                r#"{"type":"PLANNER_RESPONSE","created_at":"2026-01-01T00:00:01Z","content":"done"}"#,
                "\n"
            ),
        )
        .unwrap();
        let path = conversations.join("root.db");
        let connection = rusqlite::Connection::open(&path).expect("database");
        connection
            .execute_batch(
                "CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
                 CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB, size INTEGER NOT NULL DEFAULT 0);
                 INSERT INTO steps(idx) VALUES (0), (1);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO gen_metadata(idx, data, size) VALUES (0, ?1, ?2)",
                rusqlite::params![blob, blob.len() as i64],
            )
            .unwrap();
        drop(connection);
        (directory, path)
    }

    #[tokio::test]
    async fn an_opencode_provider_database_stays_native() {
        let (_directory, path, _) = opencode_database();
        let source = SessionSource::ProviderDb {
            agent: AgentKind::OpenCode,
            db_path: path.clone(),
            session_id: "root".to_owned(),
        };

        assert_eq!(raw_source(&source).await, Some(RawSource::Sqlite(path)));
    }

    #[tokio::test]
    async fn a_claimed_antigravity_database_stays_native_and_publishes() {
        let (_directory, path) = antigravity_database();
        let source = SessionSource::ProviderDb {
            agent: AgentKind::Antigravity,
            db_path: path.clone(),
            session_id: "root".to_owned(),
        };
        assert_eq!(
            raw_source(&source).await,
            Some(RawSource::Sqlite(path.clone()))
        );
        let (latest, rows) = Explorers::DISK
            .provider_db_fingerprint(&AgentKind::Antigravity, &path, "root")
            .await
            .expect("database fingerprint");
        let fingerprint = format!("sv1:db:{latest}:{rows}");
        let polled =
            fingerprint_with_subagents(AgentKind::Antigravity, "root", None, &source).await;
        assert_eq!(polled, fingerprint);
        let input = SessionInput {
            agent: "antigravity".to_owned(),
            session_id: "root".to_owned(),
            source: RawSource::Sqlite(path),
        };

        let outcome = stream_vendor_with_hooks(
            &[input],
            &|| false,
            &|_, _| {},
            Some(&fingerprint),
            Some(turn_row_store("antigravity", "root")),
        );

        let StreamOutcome::Published {
            session,
            parent_fingerprint,
        } = outcome
        else {
            panic!("a stable Antigravity database must publish");
        };
        assert_eq!(parent_fingerprint.as_deref(), Some(fingerprint.as_str()));
        assert_eq!(session.parent.billable_input_tokens, 30);
        assert_eq!(session.parent.billable_output_tokens, 50);
        assert_eq!(session.parent.peak_context_tokens, 837);
        assert_eq!(session.parent.billable_cache_creation_tokens, 7);
        let evidence = session.evidence.expect("database evidence");
        assert_eq!(evidence.provenance.parser_revision, PARSER_REVISION);
        assert!(evidence.capabilities.cache_write_tokens);
        assert!(evidence.capabilities.token_classes);
        assert_eq!(observed(&evidence.context).max_request_context_tokens, 837);
        assert_eq!(observed(&evidence.cache).cache_creation_tokens, 7);
        assert!(!observed(&evidence.models).by_model.is_empty());
    }

    #[tokio::test]
    async fn antigravity_poll_fingerprint_tracks_transcript_metadata() {
        let (_directory, path) = antigravity_database();
        let source = SessionSource::ProviderDb {
            agent: AgentKind::Antigravity,
            db_path: path.clone(),
            session_id: "root".to_owned(),
        };
        let transcript = antigravity_sibling_transcript(&path, "root").unwrap();

        let before =
            poll_fingerprint_with_subagents(AgentKind::Antigravity, "root", None, &source).await;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(transcript)
            .unwrap();
        std::io::Write::write_all(&mut file, b"{}\n").unwrap();
        let after =
            poll_fingerprint_with_subagents(AgentKind::Antigravity, "root", None, &source).await;

        assert!(before.starts_with("poll-v1:"));
        assert_ne!(before, after);
    }

    #[test]
    fn antigravity_poll_fingerprint_tracks_child_only_changes() {
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("root.db");
        let child = directory.path().join("child.jsonl");
        std::fs::write(&parent, "parent").unwrap();
        std::fs::write(&child, "child").unwrap();
        let paths = vec![parent, child.clone()];

        let before = poll_fingerprint_from_paths(paths.clone());
        std::fs::write(&child, "child changed").unwrap();

        assert_ne!(before, poll_fingerprint_from_paths(paths));
    }

    #[test]
    fn a_claimed_opencode_database_publishes_from_the_validated_snapshot() {
        let (_directory, path, fingerprint) = opencode_database();
        let input = SessionInput {
            agent: "opencode".to_owned(),
            session_id: "root".to_owned(),
            source: RawSource::Sqlite(path),
        };

        let outcome = stream_vendor_with_hooks(
            &[input],
            &|| false,
            &|_, _| {},
            Some(&fingerprint),
            Some(turn_row_store("opencode", "root")),
        );

        let StreamOutcome::Published {
            session,
            parent_fingerprint,
        } = outcome
        else {
            panic!("a stable OpenCode database must publish");
        };
        assert_eq!(parent_fingerprint.as_deref(), Some(fingerprint.as_str()));
        assert_eq!(session.parent.billable_input_tokens, 12);
        assert_eq!(session.parent.billable_output_tokens, 3);
        assert!(session.evidence.is_some());
    }

    /// Like [`opencode_database`], with one `parent_id` child session
    /// carrying one assistant message on `model`. A separate helper (rather
    /// than a parameter on `opencode_database`) keeps that helper's own
    /// fingerprint assertions stable.
    fn opencode_database_with_delegated_child(
        model: &str,
    ) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("opencode.db");
        let connection = rusqlite::Connection::open(&path).expect("database");
        connection
            .execute_batch(
                r#"CREATE TABLE session (
                     id TEXT PRIMARY KEY, parent_id TEXT, title TEXT,
                     time_created INTEGER, time_updated INTEGER
                 );
                 CREATE TABLE message (
                     id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                     time_updated INTEGER, data TEXT
                 );
                 CREATE TABLE part (
                     id TEXT PRIMARY KEY, message_id TEXT, session_id TEXT,
                     time_created INTEGER, time_updated INTEGER, data TEXT
                 );
                 INSERT INTO session (id, parent_id, time_created, time_updated)
                 VALUES ('root', NULL, 100, 120);
                 INSERT INTO session (id, parent_id, time_created, time_updated)
                 VALUES ('child', 'root', 110, 115);
                 INSERT INTO message VALUES (
                     'message', 'root', 100, 100,
                     '{"role":"assistant","modelID":"model-a","tokens":{"input":12,"output":3}}'
                 );"#,
            )
            .expect("OpenCode fixture");
        connection
            .execute(
                "INSERT INTO message VALUES ('child-message', 'child', 110, 110, ?1)",
                [format!(
                    r#"{{"role":"assistant","modelID":"{model}","tokens":{{"input":4,"output":1}}}}"#
                )],
            )
            .expect("child message");
        drop(connection);
        (directory, path)
    }

    #[test]
    fn an_opencode_parent_id_child_links_as_a_delegated_thread() {
        let (_directory, path) = opencode_database_with_delegated_child("model-b");
        let input = SessionInput {
            agent: "opencode".to_owned(),
            session_id: "root".to_owned(),
            source: RawSource::Sqlite(path),
        };

        let pass = evidence_pass_with_turn_rows(
            &[input],
            &|| false,
            Some(turn_row_store("opencode", "root")),
        );
        let evidence = pass.evidence.expect("published evidence");

        assert!(matches!(evidence.subagents, EvidenceValue::Complete(_)));
        let subagents = observed(&evidence.subagents);
        assert_eq!(subagents.spawn_count, 1);
        assert!(subagents.delegated_models.contains("model-b"));
        assert_eq!(subagents.delegated_turns, 1);
    }

    fn codex_record() -> String {
        [
            r#"{"timestamp":"2026-08-01T09:59:58Z","type":"session_meta","payload":{"id":"synthetic","timestamp":"2026-08-01T09:59:58Z","source":"cli"}}"#,
            r#"{"timestamp":"2026-08-01T10:00:00Z","type":"turn_context","payload":{"model":"gpt-test","effort":"medium"}}"#,
            r#"{"timestamp":"2026-08-01T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":112},"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":112},"model_context_window":200000}}}"#,
        ]
        .join("\n")
            + "\n"
    }

    fn codex_file_input(path: &std::path::Path, id: &str) -> SessionInput {
        SessionInput {
            agent: "codex".to_string(),
            session_id: id.to_string(),
            source: RawSource::File(path.to_path_buf()),
        }
    }

    fn pi_record() -> String {
        [
            r#"{"type":"session","version":3,"timestamp":"2026-08-01T09:59:58Z"}"#,
            r#"{"type":"thinking_level_change","timestamp":"2026-08-01T10:00:00Z","thinkingLevel":"medium"}"#,
            r#"{"type":"message","timestamp":"2026-08-01T10:00:01Z","message":{"role":"assistant","api":"anthropic-messages","provider":"anthropic","model":"model-a","usage":{"input":2,"output":3,"cacheRead":5,"cacheWrite":7},"content":[]}}"#,
        ]
        .join("\n")
            + "\n"
    }

    #[test]
    fn an_accepted_claude_read_publishes_metrics_and_a_start_time() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("parent.jsonl");
        std::fs::write(&path, claude_record("parent", 1_760_000_000)).expect("write parent");

        let StreamOutcome::Published { session, .. } =
            stream_vendor(&[file_input(&path, "parent")], &CancelFlag::never())
        else {
            panic!("stable source must publish");
        };
        assert_eq!(session.parent.event_count, 1);
        assert_eq!(session.parent.tokens_in, 2);
        assert_eq!(session.parent.tokens_out, 3);
        assert_eq!(session.started_at_epoch, Some(1_760_000_000));
    }

    #[test]
    fn codex_read_publishes_its_capabilities_and_provider_start() {
        let input = SessionInput {
            agent: "codex".to_owned(),
            session_id: "codex-inline".to_owned(),
            source: RawSource::Jsonl(codex_record()),
        };

        let StreamOutcome::Published { session, .. } =
            stream_vendor(std::slice::from_ref(&input), &CancelFlag::never())
        else {
            panic!("Codex source must publish");
        };
        assert_eq!(session.started_at_epoch, Some(1_785_578_398));
        // `stream_vendor` attaches no row store, so this pass alone
        // publishes no evidence; the row-backed pass below does.
        assert!(session.evidence.is_none());

        let pass = evidence_pass_with_turn_rows(
            &[input],
            &|| false,
            Some(turn_row_store("codex", "codex-inline")),
        );
        assert_eq!(pass.outcome, PassOutcome::Published);
        // `cache_write_tokens` is observed per session: `codex_record`
        // carries no cache-write alias key, so it reads false even though
        // `SourceCapabilities::codex()` now defaults it true.
        let mut expected_capabilities = SourceCapabilities::codex();
        expected_capabilities.cache_write_tokens = false;
        assert_eq!(pass.evidence.unwrap().capabilities, expected_capabilities);
    }

    #[test]
    fn pi_read_publishes_through_the_evidence_path() {
        let input = SessionInput {
            agent: "pi".to_owned(),
            session_id: "pi-inline".to_owned(),
            source: RawSource::Jsonl(pi_record()),
        };

        let StreamOutcome::Published { session, .. } =
            stream_vendor(std::slice::from_ref(&input), &CancelFlag::never())
        else {
            panic!("Pi source must publish");
        };
        assert_eq!(session.started_at_epoch, Some(1_785_578_398));
        assert_eq!(session.parent.peak_context_tokens, 14);
        // `stream_vendor` attaches no row store, so this pass alone
        // publishes no evidence; the row-backed pass below does.
        assert!(session.evidence.is_none());

        let pass = evidence_pass_with_turn_rows(
            &[input],
            &|| false,
            Some(turn_row_store("pi", "pi-inline")),
        );
        assert_eq!(pass.outcome, PassOutcome::Published);
        let record = pass
            .analysis
            .record(&SessionKey::new("native", "pi", "pi-inline"))
            .expect("Pi analysis record");
        assert_eq!(
            record.provider_hints_json.as_deref(),
            Some(r#"[{"provider":"anthropic","model":"model-a"}]"#)
        );
        assert_eq!(
            pass.evidence.unwrap().capabilities,
            SourceCapabilities::pi()
        );
    }

    /// Seam 4f: a Pi file whose messages chain through a `model_change`
    /// stays one thread, so `cache` publishes `Complete` and the
    /// over-depth check becomes assessable — the capability the Pi
    /// `thread_identity` flip is for.
    #[test]
    fn pi_thread_chain_through_a_model_change_supports_cache_and_overdepth() {
        let input = SessionInput {
            agent: "pi".to_owned(),
            session_id: "pi-thread-chain".to_owned(),
            source: RawSource::Jsonl(
                [
                    r#"{"type":"session","version":3,"timestamp":"2026-08-01T09:59:58Z"}"#,
                    r#"{"type":"message","id":"pi-thread-1","parentId":null,"timestamp":"2026-08-01T10:00:00Z","message":{"role":"assistant","model":"model-a","usage":{"input":2,"output":3,"cacheRead":5,"cacheWrite":7},"content":[]}}"#,
                    r#"{"type":"model_change","id":"pi-thread-2","parentId":"pi-thread-1","timestamp":"2026-08-01T10:00:01Z","modelId":"model-b"}"#,
                    r#"{"type":"message","id":"pi-thread-3","parentId":"pi-thread-2","timestamp":"2026-08-01T10:00:02Z","message":{"role":"assistant","model":"model-b","usage":{"input":3,"output":4,"cacheRead":1,"cacheWrite":0},"content":[]}}"#,
                ]
                .join("\n")
                    + "\n",
            ),
        };

        let pass = evidence_pass_with_turn_rows(
            &[input],
            &|| false,
            Some(turn_row_store("pi", "pi-thread-chain")),
        );
        assert_eq!(pass.outcome, PassOutcome::Published);
        let evidence = pass.evidence.expect("published evidence");

        assert!(evidence.capabilities.thread_identity);
        assert!(matches!(evidence.cache, EvidenceValue::Complete(_)));
        let cache = observed(&evidence.cache);
        assert_eq!(
            cache.model_transitions.len(),
            1,
            "the model_change between the two messages must count as one transition on their shared thread"
        );

        assert!(
            eligible(DetectorId::SessionsOverDepth, &evidence),
            "SessionsOverDepth must be eligible once thread_identity is set"
        );
    }

    /// Seam 5a: a Codex rollout is one thread, but its records carry no
    /// per-record id. `thread_identity` stays set and unblocks
    /// SessionsOverDepth; `record_identity` stays unset, but Codex's own
    /// `linear_record_order` (one rollout, one append-only stream) attests
    /// `RecordLinkage` from line order instead, so CacheChurn can read
    /// clean once more.
    #[test]
    fn codex_thread_identity_without_record_identity_still_attests_linkage_from_order_for_cache_churn()
     {
        let input = SessionInput {
            agent: "codex".to_owned(),
            session_id: "codex-thread-identity".to_owned(),
            source: RawSource::Jsonl(codex_record()),
        };

        let pass = evidence_pass_with_turn_rows(
            &[input],
            &|| false,
            Some(turn_row_store("codex", "codex-thread-identity")),
        );
        assert_eq!(pass.outcome, PassOutcome::Published);
        let evidence = pass.evidence.expect("published evidence");

        assert!(evidence.capabilities.thread_identity);
        assert!(!evidence.capabilities.record_identity);
        assert!(evidence.capabilities.linear_record_order);

        assert!(
            eligible(DetectorId::SessionsOverDepth, &evidence),
            "SessionsOverDepth must be eligible for Codex"
        );
        // Codex reports `token_classes` and `request_context_tokens`.
        // `evidence_sink` pins Codex to uncached-input accounting for
        // `repeated_context` regardless of `cache_write_tokens`.
        // This makes Cache Churn eligible. This fixture has no record loss.
        // The order route makes `RecordLinkage` complete, so Cache Churn reads clean.
        assert!(
            eligible(DetectorId::CacheChurn, &evidence),
            "CacheChurn must be eligible for Codex under uncached-input accounting"
        );
        assert!(
            clean_facts_complete(DetectorId::CacheChurn, &evidence),
            "CacheChurn must read clean for Codex once linear record order attests linkage"
        );
    }

    #[test]
    fn a_changed_codex_source_publishes_neither_projection() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("codex.jsonl");
        std::fs::write(&path, codex_record()).expect("write Codex source");
        let hook = |_: usize, path: &std::path::Path| {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("open Codex source")
                .write_all(b"{}\n")
                .expect("append Codex source");
        };

        assert!(matches!(
            stream_vendor_with_claim_hook(
                &[codex_file_input(&path, "codex")],
                &CancelFlag::never(),
                &hook,
            ),
            StreamOutcome::SourceChanged
        ));
    }

    #[test]
    fn an_accepted_child_read_publishes_the_merged_metrics() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let child = directory.path().join("child.jsonl");
        std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
        std::fs::write(&child, claude_record("child", 1_760_000_001)).expect("write child");
        let inputs = [file_input(&parent, "parent"), file_input(&child, "child")];

        let StreamOutcome::Published { session, .. } = stream_vendor(&inputs, &CancelFlag::never())
        else {
            panic!("stable sources must publish");
        };
        assert_eq!(session.subagents.len(), 1);
        assert_eq!(session.merged.event_count, 2);
        assert_eq!(session.merged.tokens_in, 4);
        assert_eq!(session.merged.tokens_out, 6);
    }

    #[test]
    fn a_changed_parent_source_publishes_neither_projection() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("parent.jsonl");
        std::fs::write(&path, claude_record("parent", 1_760_000_000)).expect("write parent");
        let hook = |_: usize, path: &std::path::Path| {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("open parent")
                .write_all(claude_record("later", 1_760_000_001).as_bytes())
                .expect("append parent");
        };

        assert!(matches!(
            stream_vendor_with_claim_hook(
                &[file_input(&path, "parent")],
                &CancelFlag::never(),
                &hook,
            ),
            StreamOutcome::SourceChanged
        ));
    }

    #[test]
    fn a_changed_child_source_publishes_neither_projection() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let child = directory.path().join("child.jsonl");
        std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
        std::fs::write(&child, claude_record("child", 1_760_000_001)).expect("write child");
        let hook = |index: usize, path: &std::path::Path| {
            if index == 1 {
                use std::io::Write;
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(path)
                    .expect("open child")
                    .write_all(claude_record("later", 1_760_000_002).as_bytes())
                    .expect("append child");
            }
        };

        assert!(matches!(
            stream_vendor_with_claim_hook(
                &[file_input(&parent, "parent"), file_input(&child, "child")],
                &CancelFlag::never(),
                &hook,
            ),
            StreamOutcome::SourceChanged
        ));
    }

    #[test]
    fn a_missing_child_is_skipped_and_the_session_still_publishes() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
        let missing = directory.path().join("missing.jsonl");

        let StreamOutcome::Published { session, .. } = stream_vendor(
            &[file_input(&parent, "parent"), file_input(&missing, "child")],
            &CancelFlag::never(),
        ) else {
            panic!("missing child must not block the parent");
        };
        assert!(session.subagents.is_empty());
        assert_eq!(session.parent.event_count, 1);
        assert_eq!(session.parent.tokens_in, 2);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_child_is_skipped_and_the_session_still_publishes() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let unreadable = directory.path().join("unreadable.jsonl");
        let remaining = directory.path().join("remaining.jsonl");
        std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
        std::fs::write(&unreadable, claude_record("unreadable", 1_760_000_001))
            .expect("write unreadable child");
        std::fs::write(&remaining, claude_record("remaining", 1_760_000_002))
            .expect("write remaining child");
        let make_child_unreadable = |index: usize, path: &std::path::Path| {
            if index == 1 {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
                    .expect("remove child read permission");
            }
        };

        let outcome = stream_vendor_with_claim_hook(
            &[
                file_input(&parent, "parent"),
                file_input(&unreadable, "unreadable"),
                file_input(&remaining, "remaining"),
            ],
            &CancelFlag::never(),
            &make_child_unreadable,
        );
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600))
            .expect("restore child read permission");

        let StreamOutcome::Published { session, .. } = outcome else {
            panic!("unreadable child must not block readable sources");
        };
        assert_eq!(session.parent.session_id, "parent");
        assert_eq!(session.parent.event_count, 1);
        assert_eq!(session.subagents.len(), 1);
        assert_eq!(session.subagents[0].0.session_id, "remaining");
        assert_eq!(session.subagents[0].0.event_count, 1);
        assert_eq!(session.merged.event_count, 2);
    }

    #[test]
    fn streaming_inline_metrics_equal_the_shipped_batch() {
        let input = inline_input(claude_record("inline-equality", 1_760_000_000), "inline");
        let expected = analyze_sources_with(vec![input.clone()], true)
            .sessions
            .into_iter()
            .next()
            .expect("batch metrics");

        let StreamOutcome::Published { session, .. } =
            stream_vendor(&[input], &CancelFlag::never())
        else {
            panic!("inline source must publish");
        };
        assert_eq!(session.parent, expected);
    }

    #[test]
    fn an_inline_source_reports_unvalidated_and_publishes() {
        let input = inline_input(claude_record("inline-outcome", 1_760_000_000), "inline");
        let mut accumulator = SessionMetricsAccumulator::new("claude", "inline");

        assert_eq!(
            ClaudeAdapter
                .visit(&input, &mut accumulator)
                .expect("inline visit"),
            VisitOutcome::Unvalidated
        );
        assert!(matches!(
            stream_vendor(&[input], &CancelFlag::never()),
            StreamOutcome::Published { .. }
        ));
    }

    #[test]
    fn an_inline_source_still_publishes_metrics() {
        let pass = evidence_pass(
            &[inline_input(
                claude_record("inline-metrics", 1_760_000_000),
                "inline-metrics",
            )],
            &|| false,
        );

        assert_eq!(pass.outcome, PassOutcome::Published);
        assert!(pass.analysis.metrics.is_some());
    }

    #[test]
    fn a_published_claude_pass_carries_evidence() {
        let pass = evidence_pass_with_turn_rows(
            &[inline_input(
                claude_record("inline-evidence", 1_760_000_000),
                "inline-evidence",
            )],
            &|| false,
            Some(turn_row_store("claude", "inline-evidence")),
        );

        let evidence = pass.evidence.expect("published evidence");
        assert_eq!(pass.outcome, PassOutcome::Published);
        assert_eq!(evidence.schema_revision, EVIDENCE_SCHEMA_REVISION);
    }

    #[test]
    fn a_child_only_fast_signal_reaches_the_parents_evidence() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let child = directory.path().join("child.jsonl");
        std::fs::write(&parent, claude_record("fast-signal-parent", 1_760_000_000))
            .expect("write parent");
        std::fs::write(
            &child,
            claude_record_with(
                "fast-signal-child",
                1_760_000_001,
                "claude-opus-4-6",
                Some("fast"),
            ),
        )
        .expect("write child");

        let pass = evidence_pass_with_turn_rows(
            &[
                file_input(&parent, "fast-signal-parent"),
                file_input(&child, "fast-signal-child"),
            ],
            &|| false,
            Some(turn_row_store("claude", "fast-signal-parent")),
        );
        let evidence = pass.evidence.expect("published evidence");

        let models = observed(&evidence.models);
        let fast = models
            .fast_modes
            .get(FAST_SPEED_KEY)
            .expect("the child's fast signal must reach the parent's evidence");
        assert_eq!(fast.delegated, 1);
        assert_eq!(fast.main_loop, 0);

        let subagents = observed(&evidence.subagents);
        assert!(subagents.delegated_models.contains("claude-opus-4-6"));
    }

    /// A Codex parent rollout: one `turn_context` naming `model`, then one
    /// `spawn_agent` function call that starts a subagent.
    fn codex_spawn_parent_record(model: &str) -> String {
        format!(
            concat!(
                r#"{{"timestamp":"2026-08-12T10:00:00Z","type":"turn_context","payload":{{"model":"{model}","effort":"medium"}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-12T10:00:01Z","type":"response_item","payload":{{"type":"function_call","name":"spawn_agent","arguments":"{{\"agent_type\":\"worker\"}}","call_id":"call-spawn"}}}}"#,
                "\n",
            ),
            model = model,
        )
    }

    /// A discovered Codex child rollout: `session_meta` marks it a subagent
    /// replaying its parent's history, then the task addressed to the
    /// child's agent path opens its owned usage window, which carries one
    /// `turn_context` naming `model` and one assistant turn with usage.
    fn codex_spawn_child_record(model: &str) -> String {
        format!(
            concat!(
                r#"{{"timestamp":"2026-08-12T10:00:02Z","type":"session_meta","payload":{{"id":"synthetic-spawn-child","thread_source":"subagent","agent_path":"worker","source":"cli"}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-12T10:00:03Z","type":"event_msg","payload":{{"type":"task_started"}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-12T10:00:04Z","type":"response_item","payload":{{"type":"agent_message","author":"parent","recipient":"worker","content":[{{"type":"input_text","text":"Handle the synthetic task."}}]}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-12T10:00:05Z","type":"turn_context","payload":{{"model":"{model}","effort":"low"}}}}"#,
                "\n",
                r#"{{"timestamp":"2026-08-12T10:00:06Z","type":"event_msg","payload":{{"type":"token_count","info":{{"last_token_usage":{{"input_tokens":300,"cached_input_tokens":100,"output_tokens":40,"total_tokens":340}},"total_token_usage":{{"input_tokens":300,"cached_input_tokens":100,"output_tokens":40,"total_tokens":340}},"model_context_window":100000}}}}}}"#,
                "\n",
            ),
            model = model,
        )
    }

    #[test]
    fn a_codex_spawn_agent_call_links_to_its_discovered_child() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let child = directory.path().join("child.jsonl");
        std::fs::write(&parent, codex_spawn_parent_record("gpt-parent")).expect("write parent");
        std::fs::write(&child, codex_spawn_child_record("gpt-child")).expect("write child");

        let pass = evidence_pass_with_turn_rows(
            &[
                codex_file_input(&parent, "spawn-parent"),
                codex_file_input(&child, "spawn-child"),
            ],
            &|| false,
            Some(turn_row_store("codex", "spawn-parent")),
        );
        let evidence = pass.evidence.expect("published evidence");

        assert!(matches!(evidence.subagents, EvidenceValue::Complete(_)));
        let subagents = observed(&evidence.subagents);
        assert_eq!(subagents.spawn_count, 1);
        assert!(subagents.delegated_models.contains("gpt-child"));
        assert_eq!(subagents.delegated_turns, 1);
    }

    #[test]
    fn a_model_switch_confined_to_one_child_produces_no_transition() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let child = directory.path().join("child.jsonl");
        std::fs::write(&parent, claude_record("switch-parent", 1_760_000_000))
            .expect("write parent");
        std::fs::write(
            &child,
            claude_record_with("switch-child-1", 1_760_000_001, "model-a", None)
                + &claude_record_with("switch-child-2", 1_760_000_002, "model-b", None),
        )
        .expect("write child");

        let pass = evidence_pass_with_turn_rows(
            &[
                file_input(&parent, "switch-parent"),
                file_input(&child, "switch-child"),
            ],
            &|| false,
            Some(turn_row_store("claude", "switch-parent")),
        );
        let evidence = pass.evidence.expect("published evidence");

        let cache = observed(&evidence.cache);
        assert!(
            cache.model_transitions.is_empty(),
            "a model switch inside one child must not become a parent-thread transition"
        );
    }

    #[test]
    fn an_unreadable_discovered_child_degrades_child_dependent_groups() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        std::fs::write(&parent, claude_record("unreadable-parent", 1_760_000_000))
            .expect("write parent");
        let missing_child = directory.path().join("missing-child.jsonl");

        let pass = evidence_pass_with_turn_rows(
            &[
                file_input(&parent, "unreadable-parent"),
                file_input(&missing_child, "unreadable-child"),
            ],
            &|| false,
            Some(turn_row_store("claude", "unreadable-parent")),
        );
        assert_eq!(pass.outcome, PassOutcome::Published);
        let evidence = pass.evidence.expect("published evidence");

        assert_eq!(evidence.diagnostics.children_unreadable, 1);
        assert!(matches!(
            evidence.subagents,
            EvidenceValue::Partial {
                reason: CoverageReason::ReadFailed,
                ..
            }
        ));
        assert!(matches!(
            evidence.models,
            EvidenceValue::Partial {
                reason: CoverageReason::ReadFailed,
                ..
            }
        ));
    }

    #[test]
    fn two_children_with_different_models_share_no_transition_or_idle_gap() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let first_child = directory.path().join("first-child.jsonl");
        let second_child = directory.path().join("second-child.jsonl");
        std::fs::write(&parent, claude_record("siblings-parent", 1_760_000_000))
            .expect("write parent");
        std::fs::write(
            &first_child,
            claude_record_with("siblings-child-1", 1_760_000_001, "model-a", None),
        )
        .expect("write first child");
        std::fs::write(
            &second_child,
            // Far enough past the first child's turn that a single shared
            // clock would read as a long idle gap.
            claude_record_with("siblings-child-2", 1_760_100_000, "model-b", None),
        )
        .expect("write second child");

        let pass = evidence_pass_with_turn_rows(
            &[
                file_input(&parent, "siblings-parent"),
                file_input(&first_child, "siblings-child-1"),
                file_input(&second_child, "siblings-child-2"),
            ],
            &|| false,
            Some(turn_row_store("claude", "siblings-parent")),
        );
        let evidence = pass.evidence.expect("published evidence");

        let cache = observed(&evidence.cache);
        assert!(
            cache.model_transitions.is_empty(),
            "two children never share a thread, so they form no transition"
        );
        assert_eq!(
            cache.longest_idle_gap_ms, 0,
            "two children never share a thread, so they form no idle gap"
        );
    }

    /// A Claude assistant record with an explicit thread-identity chain
    /// (`uuid` / `parentUuid`), optionally an inline sidechain.
    fn claude_thread_record(
        uuid: &str,
        parent_uuid: Option<&str>,
        is_sidechain: bool,
        timestamp: i64,
        model: &str,
    ) -> String {
        let parent_uuid_field = parent_uuid
            .map(|parent| format!("\"{parent}\""))
            .unwrap_or_else(|| "null".to_owned());
        let sidechain_field = if is_sidechain {
            ",\"isSidechain\":true"
        } else {
            ""
        };
        format!(
            "{{\"type\":\"assistant\",\"uuid\":\"{uuid}\",\"parentUuid\":{parent_uuid_field}{sidechain_field},\"timestamp\":{timestamp},\"message\":{{\"id\":\"msg-{uuid}\",\"role\":\"assistant\",\"model\":\"{model}\",\"usage\":{{\"input_tokens\":2,\"output_tokens\":3}},\"content\":[{{\"type\":\"text\",\"text\":\"Synthetic sidechain output.\"}}]}}}}\n"
        )
    }

    #[test]
    fn a_discovered_child_repeating_a_sidechains_uuid_degrades_instead_of_double_counting() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let child = directory.path().join("child.jsonl");
        // The parent's main turn, then an inline sidechain rooted at
        // "shared-uuid".
        std::fs::write(
            &parent,
            claude_record("dup-parent", 1_760_000_000)
                + &claude_thread_record("shared-uuid", None, true, 1_760_000_001, "model-a"),
        )
        .expect("write parent");
        // The discovered child file repeats "shared-uuid" — child files are
        // authoritative, so the parent's own copy must not double count.
        std::fs::write(
            &child,
            claude_thread_record("shared-uuid", None, true, 1_760_000_002, "model-b"),
        )
        .expect("write child");

        let pass = evidence_pass_with_turn_rows(
            &[
                file_input(&parent, "dup-parent"),
                file_input(&child, "dup-child"),
            ],
            &|| false,
            Some(turn_row_store("claude", "dup-parent")),
        );
        let evidence = pass.evidence.expect("published evidence");

        assert_eq!(evidence.diagnostics.duplicate_turn_identities, 1);
        assert!(matches!(
            evidence.models,
            EvidenceValue::Partial {
                reason: CoverageReason::AttributionIncomplete,
                ..
            }
        ));
    }

    #[test]
    fn a_changed_source_publishes_neither_projection() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("changed.jsonl");
        std::fs::write(&path, claude_record("before", 1_760_000_000)).expect("write source");
        let change_source = |_: usize, path: &std::path::Path| {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .expect("open source")
                .write_all(claude_record("after", 1_760_000_001).as_bytes())
                .expect("change source");
        };

        let pass = evidence_pass_with_hook(
            &[file_input(&path, "changed")],
            &|| false,
            &change_source,
            None,
        );

        assert_eq!(pass.outcome, PassOutcome::SourceChanged);
        assert!(pass.analysis.metrics.is_none());
        assert!(pass.evidence.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_deleted_source_is_missing_not_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().expect("tempdir");
        let deleted = directory.path().join("deleted.jsonl");
        let unreadable = directory.path().join("unreadable.jsonl");
        std::fs::write(&deleted, claude_record("deleted", 1_760_000_000))
            .expect("write deleted source");
        std::fs::write(&unreadable, claude_record("unreadable", 1_760_000_000))
            .expect("write unreadable source");
        let deleted_input = file_input(&deleted, "deleted");
        let unreadable_input = file_input(&unreadable, "unreadable");
        std::fs::remove_file(&deleted).expect("remove source");
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000))
            .expect("remove read permission");

        let missing = evidence_pass(&[deleted_input], &|| false);
        let unreadable_pass = evidence_pass(&[unreadable_input], &|| false);
        std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o600))
            .expect("restore read permission");

        assert_eq!(missing.outcome, PassOutcome::SourceMissing);
        assert_eq!(unreadable_pass.outcome, PassOutcome::Unreadable);
    }

    #[test]
    fn an_unsupported_schema_terminates() {
        let pass = evidence_pass(
            &[SessionInput {
                agent: "claude".to_string(),
                session_id: "unsupported".to_string(),
                source: RawSource::Sqlite(std::path::PathBuf::from("unsupported.sqlite")),
            }],
            &|| false,
        );

        assert_eq!(pass.outcome, PassOutcome::Unsupported);
        assert!(pass.analysis.metrics.is_none());
        assert!(pass.evidence.is_none());
    }

    #[test]
    fn a_pass_signal_counts_every_record_and_carries_cancellation() {
        use std::io::Cursor;

        let first = PassSignal::new();
        let second = PassSignal::new();
        let mut reader = antiburn_local::analysis::BoundedJsonlReader::new(Cursor::new(
            b"one\ntwo\n".as_slice(),
        ));
        while reader.next_record(&|| first.observe()).is_some() {}

        assert!(first.progress() > 0);
        assert!(!first.observe());
        first.cancel();
        assert!(first.observe());
        assert!(!second.observe());
        assert_eq!(second.progress(), 1);
    }

    #[test]
    fn an_inline_source_records_matching_and_mismatching_generations() {
        let content = claude_record("inline", 1_760_000_000);
        let fingerprint = inline_fingerprint(&content);
        let matching = ClaimedSource {
            fingerprint: Some(fingerprint),
            generation: 9,
        };
        let mismatching = ClaimedSource {
            fingerprint: Some("sv1:different".to_string()),
            generation: 9,
        };
        let actual = inline_fingerprint(&content);
        let StreamOutcome::Published { session, .. } = stream_vendor(
            &[SessionInput {
                agent: "claude".to_string(),
                session_id: "inline".to_string(),
                source: RawSource::Jsonl(content),
            }],
            &CancelFlag::never(),
        ) else {
            panic!("inline source must publish");
        };
        let key = SessionKey::new("native", "claude-code", "inline");
        let matching_analysis = SessionAnalysis {
            metrics: Some(session.parent.clone()),
            analyzed_generation: attributed_generation(&matching, Some(&actual)),
            ..SessionAnalysis::unavailable()
        };
        let mismatching_analysis = SessionAnalysis {
            metrics: Some(session.parent),
            analyzed_generation: attributed_generation(&mismatching, Some(&actual)),
            ..SessionAnalysis::unavailable()
        };

        assert_eq!(
            matching_analysis
                .record(&key)
                .expect("matching analysis record")
                .analyzed_generation,
            9
        );
        assert_eq!(
            mismatching_analysis
                .record(&key)
                .expect("mismatching analysis record")
                .analyzed_generation,
            0
        );
    }

    #[test]
    fn cancellation_during_a_child_read_publishes_and_persists_nothing() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let child = directory.path().join("child.jsonl");
        std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
        let child_content = format!(
            "{}{}",
            claude_record("child-first", 1_760_000_001),
            claude_record("child-second", 1_760_000_002)
        );
        std::fs::write(&child, child_content).expect("write child");
        let reading_child = std::cell::Cell::new(false);
        let child_cancel_checks = std::cell::Cell::new(0);
        let cancelled = || {
            if !reading_child.get() {
                return false;
            }
            let checks = child_cancel_checks.get();
            if checks >= 3 {
                return true;
            }
            child_cancel_checks.set(checks + 1);
            checks + 1 >= 3
        };
        let hook = |index: usize, _: &std::path::Path| {
            if index == 1 {
                reading_child.set(true);
            }
        };

        let analysis = match stream_vendor_with_hooks(
            &[file_input(&parent, "parent"), file_input(&child, "child")],
            &cancelled,
            &hook,
            None,
            None,
        ) {
            StreamOutcome::ParentUnreadable => SessionAnalysis::unavailable(),
            StreamOutcome::Published { .. } => panic!("cancelled child read must not publish"),
            StreamOutcome::SourceChanged => panic!("cancelled child read is not a source change"),
            StreamOutcome::ParentMissing | StreamOutcome::ParentUnsupported => {
                panic!("cancelled child read must stay unreadable")
            }
        };

        assert_eq!(child_cancel_checks.get(), 3);
        assert!(analysis.metrics.is_none());
        assert!(analysis.summary.is_none());
        assert!(
            analysis
                .record(&SessionKey::new("native", "claude-code", "parent"))
                .is_none()
        );
    }

    #[test]
    fn a_cancelled_pass_publishes_nothing() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("parent.jsonl");
        std::fs::write(&path, claude_record("parent", 1_760_000_000)).expect("write parent");
        let flag = CancelFlag(Arc::new(AtomicBool::new(true)));

        assert!(matches!(
            stream_vendor(&[file_input(&path, "parent")], &flag),
            StreamOutcome::ParentUnreadable
        ));
    }

    #[test]
    fn a_session_written_moments_ago_reads_as_active() {
        let now = 1_800_000_000;
        assert!(is_active(Some(now - 5), now));
        assert!(is_active(Some(now), now));
        assert!(!is_active(Some(now - ACTIVE_SESSION_WINDOW_SECS - 1), now));
        assert!(!is_active(None, now));
    }

    #[test]
    fn inclusive_model_runs_put_parent_modes_before_subagent_modes() {
        let parent = vec![
            ModelRun {
                model: "claude-opus-4-6".to_string(),
                thinking_mode: Some("high".to_string()),
            },
            ModelRun {
                model: "gpt-5.6-sol".to_string(),
                thinking_mode: Some("xhigh".to_string()),
            },
        ];
        let child = vec![
            ModelRun {
                model: "claude-fable-5".to_string(),
                thinking_mode: Some("high".to_string()),
            },
            ModelRun {
                model: "claude-haiku-4-5".to_string(),
                thinking_mode: Some("low".to_string()),
            },
            ModelRun {
                model: "gpt-5.6-sol".to_string(),
                thinking_mode: Some("xhigh".to_string()),
            },
        ];

        assert_eq!(
            model_runs_parent_first_lists(parent.clone(), [child.clone()].into_iter()),
            vec![
                parent[0].clone(),
                parent[1].clone(),
                child[0].clone(),
                child[1].clone(),
            ]
        );
    }

    #[test]
    fn model_runs_are_trimmed_without_losing_the_thinking_mode() {
        assert_eq!(
            normalize_model_run(&ModelRun {
                model: " gpt-5.6-sol ".to_string(),
                thinking_mode: Some(" xhigh ".to_string()),
            }),
            Some(ModelRun {
                model: "gpt-5.6-sol".to_string(),
                thinking_mode: Some("xhigh".to_string()),
            })
        );
    }

    #[test]
    fn cached_inclusive_model_runs_reject_invalid_json_and_normalize_values() {
        assert!(cached_inclusive_model_runs("not json").is_empty());
        assert_eq!(
            cached_inclusive_model_runs(
                r#"[{"model":" model-b ","thinkingMode":" high "},{"model":"model-b","thinkingMode":"high"},{"model":""}]"#,
            ),
            vec![ModelRun {
                model: "model-b".to_string(),
                thinking_mode: Some("high".to_string()),
            }]
        );
    }

    #[test]
    fn a_missing_transcript_fingerprints_as_missing() {
        let source = SessionSource::File("/does/not/exist/session.jsonl".into());
        assert_eq!(fingerprint_of(&source), MISSING_FINGERPRINT);
        assert_eq!(
            source_path(&source).as_deref(),
            Some("/does/not/exist/session.jsonl")
        );

        let inline = SessionSource::Inline {
            label: "opencode:abc".into(),
            content: "{}".into(),
        };
        assert_eq!(fingerprint_of(&inline), MISSING_FINGERPRINT);
        assert_eq!(source_path(&inline), None);
    }

    #[test]
    fn a_child_transcript_change_updates_the_combined_fingerprint() {
        let directory = tempfile::TempDir::new().unwrap();
        let parent = directory.path().join("parent.jsonl");
        let child = directory.path().join("child.jsonl");
        std::fs::write(&parent, "parent").unwrap();
        std::fs::write(&child, "child").unwrap();
        let source = SessionSource::File(parent);

        let before = combined_fingerprint(&source, std::slice::from_ref(&child));
        assert!(before.starts_with(&format!("v{ANALYSIS_FINGERPRINT_VERSION}:")));
        std::fs::write(&child, "child has more model events").unwrap();

        assert_ne!(
            before,
            combined_fingerprint(&source, std::slice::from_ref(&child))
        );
    }

    #[test]
    fn cached_costs_re_price_from_the_stored_breakdown() {
        // `ModelTokens` serializes snake_case (it carries no `rename_all`), and
        // the cache is written and read with that same type, so the stored
        // spelling is snake_case by construction.
        let stored = serde_json::to_string(&HashMap::from([(
            "claude-opus-4-6".to_string(),
            ModelTokens {
                input_tokens: 1_000_000,
                ..ModelTokens::default()
            },
        )]))
        .unwrap();

        let (cost, models) = price_cached_breakdown(&stored);
        assert_eq!(models, vec!["claude-opus-4-6".to_string()]);
        let cost = cost.expect("a known model prices");
        assert!((cost.input_usd - 5.0).abs() < 1e-9);

        // An unpriceable model yields no estimate rather than a wrong zero.
        let unknown = serde_json::to_string(&HashMap::from([(
            "some-unreleased-model".to_string(),
            ModelTokens::default(),
        )]))
        .unwrap();
        let (cost, models) = price_cached_breakdown(&unknown);
        assert!(cost.is_none());
        assert_eq!(models, vec!["some-unreleased-model".to_string()]);

        // Garbage in the cache degrades to "unknown", never to a panic.
        assert_eq!(price_cached_breakdown("not json").0, None);
    }

    fn tokens(input: u64) -> ModelTokens {
        ModelTokens {
            input_tokens: input,
            ..ModelTokens::default()
        }
    }

    #[test]
    fn merging_breakdowns_sums_a_model_used_by_the_parent_and_a_sub_agent() {
        let parent = HashMap::from([("claude-opus-4-6".to_string(), tokens(100))]);
        let child = HashMap::from([("claude-opus-4-6".to_string(), tokens(50))]);

        let merged = merge_model_breakdowns([&parent, &child]);

        assert_eq!(merged.len(), 1);
        assert_eq!(merged["claude-opus-4-6"].input_tokens, 150);
    }

    #[test]
    fn merging_breakdowns_keeps_a_model_only_one_side_used() {
        let parent = HashMap::from([("claude-opus-4-6".to_string(), tokens(100))]);
        let child_a = HashMap::from([
            ("claude-opus-4-6".to_string(), tokens(50)),
            ("claude-sonnet-4-5".to_string(), tokens(20)),
        ]);
        let child_b = HashMap::from([("gpt-5.6".to_string(), tokens(10))]);

        let merged = merge_model_breakdowns([&parent, &child_a, &child_b]);

        assert_eq!(merged.len(), 3);
        assert_eq!(merged["claude-opus-4-6"].input_tokens, 150);
        assert_eq!(merged["claude-sonnet-4-5"].input_tokens, 20);
        assert_eq!(merged["gpt-5.6"].input_tokens, 10);
    }

    #[test]
    fn merging_no_breakdowns_yields_an_empty_map() {
        assert!(merge_model_breakdowns(std::iter::empty()).is_empty());
    }

    #[test]
    fn an_inclusive_breakdown_prices_the_parent_and_every_sub_agent_together() {
        // This test mirrors the bug this rollup fixes. A parent spends
        // little. Its sub-agents together spend much more. The session's
        // cost must not show only the parent's price.
        let parent = HashMap::from([("claude-opus-4-6".to_string(), tokens(1_000_000))]);
        let subagent_a = HashMap::from([("claude-opus-4-6".to_string(), tokens(2_000_000))]);
        let subagent_b = HashMap::from([("claude-opus-4-6".to_string(), tokens(3_000_000))]);

        let top_level_cost = price_breakdown(&parent).expect("the parent alone prices");
        let inclusive = merge_model_breakdowns([&parent, &subagent_a, &subagent_b]);
        let inclusive_cost = price_breakdown(&inclusive).expect("the merged breakdown prices");

        // 1M + 2M + 3M input tokens of the same model total 6x the parent alone.
        assert!((inclusive_cost.total_usd - top_level_cost.total_usd * 6.0).abs() < 1e-6);
        assert!(inclusive_cost.total_usd > top_level_cost.total_usd);
    }

    /// A full `ModelTokens`, so the token-sum test below exercises every
    /// billable component, not only input tokens.
    fn full_tokens(input: u64, output: u64, cache_read: u64, cache_creation: u64) -> ModelTokens {
        ModelTokens {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_creation_tokens: cache_creation,
            cache_creation_1h_tokens: 0,
        }
    }

    #[test]
    fn the_inclusive_token_sum_equals_the_parent_sum_plus_the_sub_agents_sum() {
        let parent = HashMap::from([("claude-opus-4-6".to_string(), full_tokens(100, 20, 5, 3))]);
        let subagent_a =
            HashMap::from([("claude-opus-4-6".to_string(), full_tokens(50, 10, 2, 1))]);
        let subagent_b =
            HashMap::from([("claude-sonnet-4-5".to_string(), full_tokens(30, 6, 1, 0))]);

        let subagents_merged = merge_model_breakdowns([&subagent_a, &subagent_b]);
        let inclusive_merged = merge_model_breakdowns([&parent, &subagent_a, &subagent_b]);

        let parent_sum = sum_billable_tokens(&parent);
        let subagents_sum = sum_billable_tokens(&subagents_merged);
        let inclusive_sum = sum_billable_tokens(&inclusive_merged);

        assert_eq!(
            inclusive_sum,
            BillableTokens {
                input_tokens: parent_sum.input_tokens + subagents_sum.input_tokens,
                output_tokens: parent_sum.output_tokens + subagents_sum.output_tokens,
                cache_read_tokens: parent_sum.cache_read_tokens + subagents_sum.cache_read_tokens,
                cache_creation_tokens: parent_sum.cache_creation_tokens
                    + subagents_sum.cache_creation_tokens,
            }
        );
    }

    #[test]
    fn models_are_sorted_and_blank_keys_dropped() {
        let breakdown = HashMap::from([
            ("gpt-5.6".to_string(), ModelTokens::default()),
            ("claude-opus-4-6".to_string(), ModelTokens::default()),
            ("  ".to_string(), ModelTokens::default()),
        ]);
        assert_eq!(
            sorted_models(&breakdown),
            vec!["claude-opus-4-6".to_string(), "gpt-5.6".to_string()]
        );
    }

    #[test]
    fn a_fork_observation_is_recovered_from_a_synthetic_header() {
        let header = serde_json::json!({
            "type": "session_meta",
            "metadata": {
                FORK_OBSERVATION_KEY: {
                    "parent_agent": "cursor",
                    "parent_agent_session_id": "parent-42",
                    "fork_kind": "fork",
                    "provider_fork_point_id": serde_json::Value::Null,
                    "detection_source": "stable_id_prefix",
                    "confidence": 100,
                    "inherited_item_count": 12,
                    "extractor_version": "1",
                }
            }
        });
        assert_eq!(
            find_fork_parent(&header, FORK_OBSERVATION_DEPTH).as_deref(),
            Some("parent-42")
        );
    }

    #[test]
    fn a_codex_session_header_declares_its_fork_parent() {
        let header = serde_json::json!({
            "timestamp": "2026-08-22T04:05:01.756Z",
            "type": "session_meta",
            "payload": {
                "id": "child-42",
                "forked_from_id": "parent-42",
                "source": "cli",
                "thread_source": "user",
            }
        });
        assert_eq!(
            find_fork_parent(&header, FORK_OBSERVATION_DEPTH).as_deref(),
            Some("parent-42")
        );
    }

    #[test]
    fn a_codex_fork_parent_requires_a_session_header_and_a_nonempty_id() {
        let message = serde_json::json!({
            "type": "response_item",
            "payload": { "forked_from_id": "not-a-parent" }
        });
        let empty = serde_json::json!({
            "type": "session_meta",
            "payload": { "forked_from_id": "  " }
        });
        assert_eq!(find_fork_parent(&message, FORK_OBSERVATION_DEPTH), None);
        assert_eq!(find_fork_parent(&empty, FORK_OBSERVATION_DEPTH), None);
    }

    #[test]
    fn a_header_without_an_observation_yields_no_parent() {
        let header = serde_json::json!({ "type": "session_meta", "metadata": { "cwd": "/x" } });
        assert_eq!(find_fork_parent(&header, FORK_OBSERVATION_DEPTH), None);
        // A malformed observation is ignored rather than half-read.
        let broken = serde_json::json!({ FORK_OBSERVATION_KEY: { "parent_agent": "cursor" } });
        assert_eq!(find_fork_parent(&broken, FORK_OBSERVATION_DEPTH), None);
    }

    #[test]
    fn the_observation_search_stops_at_its_depth_budget() {
        let deep = serde_json::json!({ "a": { "b": { "c": { "d": {
            FORK_OBSERVATION_KEY: { "parent_agent_session_id": "too-deep" }
        }}}}});
        assert_eq!(find_fork_parent(&deep, FORK_OBSERVATION_DEPTH), None);
    }

    fn skill(description: Option<&str>) -> SkillUse {
        SkillUse {
            name: "commit-helper".into(),
            progress: 0.5,
            description: description.map(str::to_string),
            duration_ms: None,
            tokens_out: 0,
            context_tokens: 0,
        }
    }

    #[test]
    fn a_short_skill_description_is_left_exactly_as_it_was() {
        let mut skills = vec![skill(Some("Draft a conventional commit message"))];
        cap_skill_descriptions(&mut skills);
        assert_eq!(
            skills[0].description.as_deref(),
            Some("Draft a conventional commit message")
        );
    }

    #[test]
    fn a_long_skill_description_is_capped_before_it_can_reach_the_store_or_an_export() {
        let paragraph = "x".repeat(5_000);
        let mut skills = vec![skill(Some(&paragraph))];
        cap_skill_descriptions(&mut skills);

        let capped = skills[0]
            .description
            .as_deref()
            .expect("the description survives, shortened");
        assert_eq!(
            capped.chars().count(),
            SKILL_DESCRIPTION_MAX_CHARS,
            "the contract in store::schema and export names this exact ceiling"
        );
        assert!(
            capped.ends_with(TRUNCATION_MARK),
            "a shortened excerpt must look shortened"
        );
    }

    #[test]
    fn a_skill_with_no_description_stays_without_one() {
        let mut skills = vec![skill(None)];
        cap_skill_descriptions(&mut skills);
        assert_eq!(skills[0].description, None);
    }

    #[test]
    fn a_multi_byte_description_is_cut_on_a_character_and_never_mid_glyph() {
        // Counting bytes here would split a three-byte character in half and
        // produce a string that is not valid UTF-8 to begin with.
        let text = "é".repeat(SKILL_DESCRIPTION_MAX_CHARS + 50);
        let capped = cap_excerpt(&text, SKILL_DESCRIPTION_MAX_CHARS);
        assert_eq!(capped.chars().count(), SKILL_DESCRIPTION_MAX_CHARS);
        assert!(capped.starts_with('é'));
        assert!(capped.ends_with(TRUNCATION_MARK));

        // Exactly at the ceiling is not "too long".
        let exact = "a".repeat(SKILL_DESCRIPTION_MAX_CHARS);
        assert_eq!(cap_excerpt(&exact, SKILL_DESCRIPTION_MAX_CHARS), exact);
    }

    #[test]
    fn an_unavailable_analysis_caches_nothing() {
        let analysis = SessionAnalysis::unavailable();
        assert!(
            analysis
                .record(&SessionKey::new("native", "claude-code", "abc"))
                .is_none()
        );
    }
}

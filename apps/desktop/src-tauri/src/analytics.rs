// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use antiburn_local::analysis::{
    ANALYZER_REVISION, ActiveSessionsSummary, ClaudeAdapter, METRICS_SCHEMA_REVISION, ModelRun,
    NormalizedSession, PARSER_REVISION, RawSource, SessionCost, SessionInput, SessionMetrics,
    SessionMetricsAccumulator, SkillUse, SourceClaim, VendorAdapter, VisitOutcome,
    aggregate_metrics, analyze_session, analyze_sources_with, append_only_guarantee, merge_metrics,
    merge_subagent_events, normalize_source, price_breakdown, pricing_generation,
};
use antiburn_local::discovery::{
    ACTIVE_SESSION_WINDOW_SECS, Explorers, FORK_OBSERVATION_KEY, FingerprintInputs,
    ForkObservation, SessionSource, SourceStat, session_source_content,
};
use antiburn_local::model::AgentKind;
use antiburn_local::pricing::ModelTokens;

use crate::agents::{supports_analytics, vendor_label};
use crate::dto::{BillableTokens, OrchestrationStatus, SubagentMember};
use crate::store::{AnalysisRecord, SessionKey};

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
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }

    pub fn never() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct StreamedSession {
    pub parent: SessionMetrics,
    pub merged: SessionMetrics,
    pub subagents: Vec<SessionMetrics>,
    pub started_at_epoch: Option<i64>,
}

enum StreamOutcome {
    Published {
        session: Box<StreamedSession>,
        parent_fingerprint: Option<String>,
    },
    SourceChanged,
    ParentUnreadable,
}

enum ComputedAnalysis {
    Published {
        parent: Box<SessionMetrics>,
        merged: Box<SessionMetrics>,
        subagents: Vec<SessionMetrics>,
        started_at_epoch: Option<i64>,
        parent_fingerprint: Option<String>,
    },
    SourceChanged,
    Unavailable,
}

/// Longest a skill description may be once it leaves this module.
///
/// A skill's description is lifted verbatim from the line the transcript's own
/// skill listing carries, and the engine puts no bound on it — a listing that
/// inlined a paragraph would put that paragraph in the local store and in every
/// export. That is a *derived excerpt*, and an excerpt with no ceiling is just
/// the text. One line of prose is what the tooltip renders and what the
/// contract in [`crate::store::schema`] and [`crate::export`] promises, so the
/// ceiling is applied here, at the app's boundary, before the value can reach
/// either.
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
        }
    }

    /// The cache record for this analysis, when there is anything to cache.
    pub fn record(&self, key: &SessionKey) -> Option<AnalysisRecord> {
        self.metrics.as_ref()?;
        if self.source_changed {
            return None;
        }
        Some(AnalysisRecord {
            key: key.clone(),
            model_breakdown_json: serde_json::to_string(&self.inclusive_model_breakdown)
                .unwrap_or_else(|_| "{}".to_string()),
            inclusive_models_json: serde_json::to_string(&self.model_runs)
                .unwrap_or_else(|_| "[]".to_string()),
            source_fingerprint: self.fingerprint.clone(),
            pricing_generation: pricing_generation() as i64,
            analyzed_generation: self.analyzed_generation,
            parser_revision: PARSER_REVISION,
            analyzer_revision: ANALYZER_REVISION,
            metrics_schema_revision: METRICS_SCHEMA_REVISION,
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
    combined_fingerprint(source, &subagent_paths)
}

/// Build one stable fingerprint from a parent and its sorted child paths.
fn combined_fingerprint(source: &SessionSource, subagent_paths: &[std::path::PathBuf]) -> String {
    let parent_fingerprint = fingerprint_of(source);
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

/// Whether a cached analysis is still good for `source`.
///
/// A cache entry is stale when the transcript changed or when a newer pricing
/// snapshot was installed, since cost is baked into the cached record.
pub fn cache_is_fresh(cached: &AnalysisRecord, fingerprint: &str) -> bool {
    fingerprint != MISSING_FINGERPRINT
        && cached.source_fingerprint == fingerprint
        && cached.pricing_generation == pricing_generation() as i64
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
///
/// A vendor database is rendered to the discovery layer's synthesized JSONL
/// rather than handed over as a file: the dedicated adapters (OpenCode's, in
/// particular) are written against that stream, and passing the database
/// directly would silently drop them onto the schema-agnostic fallback.
async fn raw_source(source: &SessionSource) -> Option<RawSource> {
    match source {
        SessionSource::File(path) => Some(RawSource::File(path.clone())),
        SessionSource::Inline { content, .. } => Some(RawSource::Jsonl(content.clone())),
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

fn stream_claude(inputs: &[SessionInput], cancel: &CancelFlag) -> StreamOutcome {
    stream_claude_with_claim_hook(inputs, cancel, &test_subagent_after_claim)
}

#[cfg(not(test))]
fn test_subagent_after_claim(_: usize, _: &std::path::Path) {}

#[cfg(test)]
fn test_subagent_after_claim(_: usize, path: &std::path::Path) {
    use std::io::Write;

    let append = {
        let mut override_ = subagent_test_override()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        override_
            .as_mut()
            .filter(|override_| override_.source_path == path)
            .and_then(|override_| override_.append_after_claim.take())
    };
    if let Some(append) = append {
        std::fs::OpenOptions::new()
            .append(true)
            .open(path)
            .expect("open changed sub-agent source")
            .write_all(append.as_bytes())
            .expect("append changed sub-agent source");
    }
}

fn stream_claude_with_claim_hook(
    inputs: &[SessionInput],
    cancel: &CancelFlag,
    after_claim: &dyn Fn(usize, &std::path::Path),
) -> StreamOutcome {
    stream_claude_with_hooks(inputs, &|| cancel.cancelled(), after_claim)
}

fn stream_claude_with_hooks(
    inputs: &[SessionInput],
    cancelled: &dyn Fn() -> bool,
    after_claim: &dyn Fn(usize, &std::path::Path),
) -> StreamOutcome {
    let mut accumulators = Vec::with_capacity(inputs.len());
    let mut parent_fingerprint = None;
    for (index, input) in inputs.iter().enumerate() {
        if cancelled() {
            return StreamOutcome::ParentUnreadable;
        }
        let mut accumulator =
            SessionMetricsAccumulator::new(input.agent.clone(), input.session_id.clone());
        let result = match &input.source {
            RawSource::File(path) => {
                let claim = match claim_file(path) {
                    Ok(claim) => claim,
                    Err(_) if cancelled() || index == 0 => {
                        return StreamOutcome::ParentUnreadable;
                    }
                    Err(_) => continue,
                };
                let fingerprint = claim.fingerprint.clone();
                after_claim(index, path);
                let outcome = ClaudeAdapter.visit_claimed(
                    input,
                    &claim,
                    append_only_guarantee("claude"),
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
                ClaudeAdapter.visit(input, &mut accumulator)
            }
            RawSource::Sqlite(_) if index == 0 => return StreamOutcome::ParentUnreadable,
            RawSource::Sqlite(_) => continue,
        };
        match result {
            Ok(VisitOutcome::SourceChanged(_)) => return StreamOutcome::SourceChanged,
            Ok(_) if cancelled() => return StreamOutcome::ParentUnreadable,
            Ok(_) => accumulators.push(accumulator),
            Err(_) if cancelled() || index == 0 => {
                return StreamOutcome::ParentUnreadable;
            }
            Err(_) => continue,
        }
    }
    if cancelled() {
        return StreamOutcome::ParentUnreadable;
    }
    let Some((parent, children)) = accumulators.split_first() else {
        return StreamOutcome::ParentUnreadable;
    };
    let started_at_epoch = accumulators
        .iter()
        .filter_map(SessionMetricsAccumulator::earliest_ts_ms)
        .min()
        .map(|timestamp| timestamp / 1000);
    let parent_metrics = parent.metrics();
    let subagents = children
        .iter()
        .map(SessionMetricsAccumulator::metrics)
        .collect();
    let merged = merge_metrics(parent, children);
    StreamOutcome::Published {
        session: Box::new(StreamedSession {
            parent: parent_metrics,
            merged,
            subagents,
            started_at_epoch,
        }),
        parent_fingerprint,
    }
}

/// Normalize the parent and every sub-agent input, then merge their events
/// into one time-aligned session via [`merge_subagent_events`].
///
/// Matches inputs to the parent by `session_id` rather than by position, so
/// this stays correct even if a vendor adapter fails to read one input (the
/// same tolerance [`analyze_sources_with`] gives the per-source batch).
/// `None` when the parent itself could not be normalized.
fn attributed_generation(claimed: &ClaimedSource, actual_fingerprint: Option<&str>) -> i64 {
    match (claimed.fingerprint.as_deref(), actual_fingerprint) {
        (Some(expected), Some(actual)) if expected == actual => claimed.generation,
        _ => 0,
    }
}

fn merge_parent_and_subagents(
    inputs: &[SessionInput],
    parent_session_id: &str,
) -> Option<NormalizedSession> {
    let mut parent = None;
    let mut subagents = Vec::new();
    for input in inputs {
        let Ok(normalized) = normalize_source(input) else {
            continue;
        };
        if normalized.session_id == parent_session_id {
            parent = Some(normalized);
        } else {
            subagents.push(normalized);
        }
    }
    Some(merge_subagent_events(parent?, subagents))
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
pub async fn analyze(
    agent: AgentKind,
    session_id: &str,
    wsl_distro: Option<&str>,
    claimed: ClaimedSource,
    cancel: CancelFlag,
) -> SessionAnalysis {
    let Some(source) = locate(agent, session_id, wsl_distro).await else {
        return SessionAnalysis::unavailable();
    };
    let Some(raw) = raw_source(&source).await else {
        return SessionAnalysis::unavailable();
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
    let fingerprint = combined_fingerprint(&source, &subagent_paths);
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

    let inputs_for_merge = inputs.clone();
    let parent_session_id_for_merge = parent_session_id.clone();
    let is_claude = label == "claude";

    // The engine's analysis is synchronous and CPU-bound; keep it off the
    // runtime's worker threads.
    let computed = tauri::async_runtime::spawn_blocking(move || {
        if is_claude {
            return match stream_claude(&inputs, &cancel) {
                StreamOutcome::Published {
                    session,
                    parent_fingerprint,
                } => ComputedAnalysis::Published {
                    parent: Box::new(session.parent),
                    merged: Box::new(session.merged),
                    subagents: session.subagents,
                    started_at_epoch: session.started_at_epoch,
                    parent_fingerprint,
                },
                StreamOutcome::SourceChanged => ComputedAnalysis::SourceChanged,
                StreamOutcome::ParentUnreadable => ComputedAnalysis::Unavailable,
            };
        }
        let batch = analyze_sources_with(inputs, true);
        let Some(merged) =
            merge_parent_and_subagents(&inputs_for_merge, &parent_session_id_for_merge)
                .map(|session| analyze_session(&session))
        else {
            return ComputedAnalysis::Unavailable;
        };
        let mut sessions = batch.sessions;
        let Some(parent_index) = sessions
            .iter()
            .position(|metrics| metrics.session_id == parent_session_id_for_merge)
        else {
            return ComputedAnalysis::Unavailable;
        };
        let parent = sessions.remove(parent_index);
        ComputedAnalysis::Published {
            parent: Box::new(parent),
            merged: Box::new(merged),
            subagents: sessions,
            started_at_epoch: None,
            parent_fingerprint: None,
        }
    })
    .await;

    let Ok(computed) = computed else {
        return SessionAnalysis::unavailable();
    };
    let (parent_metrics, merged, subagents, started_at_epoch, parent_fingerprint) = match computed {
        ComputedAnalysis::Published {
            parent,
            merged,
            subagents,
            started_at_epoch,
            parent_fingerprint,
        } => (
            *parent,
            *merged,
            subagents,
            started_at_epoch,
            parent_fingerprint,
        ),
        ComputedAnalysis::SourceChanged => {
            return SessionAnalysis {
                source_path,
                fingerprint,
                source_changed: true,
                ..SessionAnalysis::unavailable()
            };
        }
        ComputedAnalysis::Unavailable => {
            return SessionAnalysis {
                source_path,
                fingerprint,
                ..SessionAnalysis::unavailable()
            };
        }
    };
    let analyzed_generation = attributed_generation(&claimed, parent_fingerprint.as_deref());
    let by_id: HashMap<String, SessionMetrics> = subagents
        .into_iter()
        .map(|metrics| (metrics.session_id.clone(), metrics))
        .collect();

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

    // The views key icons and copy off the discovery slug, so the vendor label
    // the adapter registry dispatches on never leaves this module.
    metrics.agent = agent_slug.clone();
    cap_skill_descriptions(&mut metrics.skill_uses);

    let members: Vec<SubagentMember> = roster
        .into_iter()
        .map(|(subagent_id, label)| SubagentMember {
            agent: agent_slug.clone(),
            subagent_id,
            label,
        })
        .collect();

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
    let subagent_breakdowns: Vec<&HashMap<String, ModelTokens>> =
        by_id.values().map(|child| &child.model_breakdown).collect();
    let has_subagents = !subagent_breakdowns.is_empty();
    let subagents_model_breakdown = merge_model_breakdowns(subagent_breakdowns.iter().copied());
    let inclusive_model_breakdown = metrics.model_breakdown.clone();

    let top_level_cost = price_breakdown(&parent_metrics.model_breakdown);
    let subagents_cost = price_breakdown(&subagents_model_breakdown);
    let cost = metrics.cost;
    let models = sorted_models(&inclusive_model_breakdown);
    let model_runs = model_runs_parent_first(&parent_metrics, by_id.values());
    let inclusive_tokens = Some(sum_billable_tokens(&inclusive_model_breakdown));
    let subagents_tokens = has_subagents.then(|| sum_billable_tokens(&subagents_model_breakdown));
    let skills = metrics.skill_uses.clone();
    let summary = aggregate_metrics(vec![metrics.clone()]);

    SessionAnalysis {
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
    }
}

/// Analyze one sub-agent transcript on its own.
///
/// A sub-agent is a session in its own right. The analytics surface opens its
/// transcript like any other session. Vendors do not nest orchestration, so
/// this path analyzes one input.
pub async fn analyze_subagent(
    agent: AgentKind,
    parent_session_id: &str,
    subagent_id: &str,
    wsl_distro: Option<&str>,
    cancel: CancelFlag,
) -> SessionAnalysis {
    let Some(source) =
        locate_subagent_source(agent, parent_session_id, subagent_id, wsl_distro).await
    else {
        return SessionAnalysis::unavailable();
    };
    let Some(raw) = raw_source(&source).await else {
        return SessionAnalysis::unavailable();
    };

    let input = SessionInput {
        agent: vendor_label(agent).to_string(),
        session_id: subagent_id.to_string(),
        source: raw,
    };
    let fingerprint = fingerprint_of(&source);
    let source_path = source_path(&source);
    let agent_slug = agent.slug().to_string();

    let is_claude = vendor_label(agent) == "claude";
    let computed = tauri::async_runtime::spawn_blocking(move || {
        if is_claude {
            return match stream_claude(&[input], &cancel) {
                StreamOutcome::Published { session, .. } => {
                    Some((session.parent, session.started_at_epoch))
                }
                StreamOutcome::SourceChanged | StreamOutcome::ParentUnreadable => None,
            };
        }
        analyze_sources_with(vec![input], true)
            .sessions
            .into_iter()
            .next()
            .map(|metrics| (metrics, None))
    })
    .await;
    let Ok(Some((mut metrics, started_at_epoch))) = computed else {
        return SessionAnalysis {
            source_path,
            fingerprint,
            ..SessionAnalysis::unavailable()
        };
    };
    metrics.agent = agent_slug;
    cap_skill_descriptions(&mut metrics.skill_uses);

    // A sub-agent launches no sub-agent of its own. Its own transcript is
    // the whole story. `cost` and `top_level_cost` name the same figure
    // here.
    let cost = price_breakdown(&metrics.model_breakdown);
    let models = sorted_models(&metrics.model_breakdown);
    let model_runs = model_runs_for_metrics(&metrics);
    let inclusive_model_breakdown = metrics.model_breakdown.clone();
    let inclusive_tokens = Some(sum_billable_tokens(&inclusive_model_breakdown));
    let skills = metrics.skill_uses.clone();
    let summary = aggregate_metrics(vec![metrics.clone()]);

    SessionAnalysis {
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
    }
}

async fn locate_subagent_source(
    agent: AgentKind,
    parent_session_id: &str,
    subagent_id: &str,
    wsl_distro: Option<&str>,
) -> Option<SessionSource> {
    #[cfg(test)]
    {
        let override_ = subagent_test_override()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(override_) = override_.as_ref()
            && override_.parent_session_id == parent_session_id
            && override_.subagent_id == subagent_id
        {
            return Some(SessionSource::File(override_.source_path.clone()));
        }
    }
    Explorers::DISK
        .locate_subagent_source_in_environment(&agent, parent_session_id, subagent_id, wsl_distro)
        .await
}

#[cfg(test)]
struct SubagentTestOverride {
    parent_session_id: String,
    subagent_id: String,
    source_path: std::path::PathBuf,
    append_after_claim: Option<String>,
}

#[cfg(test)]
fn subagent_test_override() -> &'static std::sync::Mutex<Option<SubagentTestOverride>> {
    static OVERRIDE: std::sync::OnceLock<std::sync::Mutex<Option<SubagentTestOverride>>> =
        std::sync::OnceLock::new();
    OVERRIDE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Whether analysis of this agent's transcripts is more than a generic parse.
pub fn analytics_supported(agent: AgentKind) -> bool {
    supports_analytics(agent)
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
        SessionSource::ProviderDb { .. } => session_source_content(source).await?,
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

    struct SubagentOverrideGuard;

    impl Drop for SubagentOverrideGuard {
        fn drop(&mut self) {
            *subagent_test_override()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        }
    }

    #[test]
    fn an_accepted_claude_read_publishes_metrics_and_a_start_time() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("parent.jsonl");
        std::fs::write(&path, claude_record("parent", 1_760_000_000)).expect("write parent");

        let StreamOutcome::Published { session, .. } =
            stream_claude(&[file_input(&path, "parent")], &CancelFlag::never())
        else {
            panic!("stable source must publish");
        };
        assert_eq!(session.parent.event_count, 1);
        assert_eq!(session.parent.tokens_in, 2);
        assert_eq!(session.parent.tokens_out, 3);
        assert_eq!(session.started_at_epoch, Some(1_760_000_000));
    }

    #[test]
    fn an_accepted_child_read_publishes_the_merged_metrics() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let parent = directory.path().join("parent.jsonl");
        let child = directory.path().join("child.jsonl");
        std::fs::write(&parent, claude_record("parent", 1_760_000_000)).expect("write parent");
        std::fs::write(&child, claude_record("child", 1_760_000_001)).expect("write child");
        let inputs = [file_input(&parent, "parent"), file_input(&child, "child")];

        let StreamOutcome::Published { session, .. } = stream_claude(&inputs, &CancelFlag::never())
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
            stream_claude_with_claim_hook(
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
            stream_claude_with_claim_hook(
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

        let StreamOutcome::Published { session, .. } = stream_claude(
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

        let outcome = stream_claude_with_claim_hook(
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
        assert_eq!(session.subagents[0].session_id, "remaining");
        assert_eq!(session.subagents[0].event_count, 1);
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
            stream_claude(&[input], &CancelFlag::never())
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
            stream_claude(&[input], &CancelFlag::never()),
            StreamOutcome::Published { .. }
        ));
    }

    #[tokio::test]
    async fn a_changed_subagent_source_rejects_the_direct_subagent_view() {
        let directory = tempfile::TempDir::new().expect("tempdir");
        let path = directory.path().join("agent-review-gap.jsonl");
        std::fs::write(&path, claude_record("before-change", 1_760_000_000))
            .expect("write sub-agent");
        let parent_session_id = "review-gap-parent";
        let subagent_id = "review-gap-child";
        {
            let mut override_ = subagent_test_override()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            assert!(override_.is_none());
            *override_ = Some(SubagentTestOverride {
                parent_session_id: parent_session_id.to_string(),
                subagent_id: subagent_id.to_string(),
                source_path: path,
                append_after_claim: Some(claude_record("after-change", 1_760_000_001)),
            });
        }
        let _guard = SubagentOverrideGuard;

        let analysis = analyze_subagent(
            AgentKind::Claude,
            parent_session_id,
            subagent_id,
            None,
            CancelFlag::never(),
        )
        .await;

        assert!(analysis.metrics.is_none());
        assert!(analysis.summary.is_none());
        assert!(analysis.cost.is_none());
        assert!(analysis.inclusive_tokens.is_none());
        assert!(analysis.inclusive_model_breakdown.is_empty());
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
        let StreamOutcome::Published { session, .. } = stream_claude(
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

        let analysis = match stream_claude_with_hooks(
            &[file_input(&parent, "parent"), file_input(&child, "child")],
            &cancelled,
            &hook,
        ) {
            StreamOutcome::ParentUnreadable => SessionAnalysis::unavailable(),
            StreamOutcome::Published { .. } => panic!("cancelled child read must not publish"),
            StreamOutcome::SourceChanged => panic!("cancelled child read is not a source change"),
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
        let flag = Arc::new(AtomicBool::new(true));

        assert!(matches!(
            stream_claude(&[file_input(&path, "parent")], &CancelFlag::from_flag(flag),),
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
    fn a_source_with_no_file_behind_it_never_satisfies_the_cache() {
        let cached = AnalysisRecord {
            key: SessionKey::new("native", "claude-code", "abc"),
            model_breakdown_json: "{}".into(),
            inclusive_models_json: "[]".into(),
            source_fingerprint: MISSING_FINGERPRINT.into(),
            pricing_generation: pricing_generation() as i64,
            analyzed_generation: 0,
            parser_revision: 0,
            analyzer_revision: 0,
            metrics_schema_revision: 0,
        };
        assert!(!cache_is_fresh(&cached, MISSING_FINGERPRINT));

        let cached = AnalysisRecord {
            source_fingerprint: "123:456".into(),
            ..cached
        };
        assert!(cache_is_fresh(&cached, "123:456"));
        assert!(!cache_is_fresh(&cached, "123:999"));
    }

    #[test]
    fn a_stale_pricing_generation_invalidates_a_matching_fingerprint() {
        let cached = AnalysisRecord {
            key: SessionKey::new("native", "claude-code", "abc"),
            model_breakdown_json: "{}".into(),
            inclusive_models_json: "[]".into(),
            source_fingerprint: "123:456".into(),
            pricing_generation: pricing_generation() as i64 - 1,
            analyzed_generation: 0,
            parser_revision: 0,
            analyzer_revision: 0,
            metrics_schema_revision: 0,
        };
        assert!(!cache_is_fresh(&cached, "123:456"));
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

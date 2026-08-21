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

use antiburn_local::analysis::{
    ActiveSessionsSummary, ModelRun, RawSource, SessionCost, SessionInput, SessionMetrics,
    SkillUse, active_time_fraction, aggregate_metrics, analyze_sources_with, normalize_source,
    price_breakdown, pricing_generation,
};
use antiburn_local::discovery::{
    ACTIVE_SESSION_WINDOW_SECS, Explorers, FORK_OBSERVATION_KEY, ForkObservation, SessionSource,
    session_source_content,
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
/// Applied to the metrics' own `skill_uses`, which is the single value that
/// becomes the cached `metrics_json`, the exported `metrics`, and the exported
/// `skills` — so capping it once here caps all three.
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
        }
    }

    /// The cache record for this analysis, when there is anything to cache.
    pub fn record(&self, key: &SessionKey) -> Option<AnalysisRecord> {
        let metrics = self.metrics.as_ref()?;
        Some(AnalysisRecord {
            key: key.clone(),
            metrics_json: serde_json::to_string(metrics).ok()?,
            cost_json: self
                .cost
                .as_ref()
                .and_then(|cost| serde_json::to_string(cost).ok()),
            model_breakdown_json: serde_json::to_string(&self.inclusive_model_breakdown)
                .unwrap_or_else(|_| "{}".to_string()),
            inclusive_models_json: serde_json::to_string(&self.model_runs)
                .unwrap_or_else(|_| "[]".to_string()),
            active_secs: metrics.active_secs as i64,
            duration_secs: metrics.duration_secs as i64,
            pattern_score: i64::from(metrics.pattern_score),
            source_fingerprint: self.fingerprint.clone(),
            pricing_generation: pricing_generation() as i64,
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
const ANALYSIS_FINGERPRINT_VERSION: u8 = 1;

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

/// Analyze one session and, in the same pass, every sub-agent it launched.
///
/// One pass rather than N+1 because the engine's entry point already takes a
/// batch: the sub-agents' metrics come back alongside the parent's, and the
/// roster's health scores cost nothing extra.
pub async fn analyze(
    agent: AgentKind,
    session_id: &str,
    wsl_distro: Option<&str>,
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

    // The engine's analysis is synchronous and CPU-bound; keep it off the
    // runtime's worker threads.
    let computed = tauri::async_runtime::spawn_blocking(move || {
        let batch = analyze_sources_with(inputs, true);
        // The spawn dots need the parent's normalized event stream to map a
        // child's first-activity instant onto the parent's active-time axis.
        let parent_stream = normalize_source(&parent_input).ok();
        (batch, parent_stream)
    })
    .await;

    let Ok((batch, parent_stream)) = computed else {
        return SessionAnalysis::unavailable();
    };

    let mut by_id: HashMap<String, SessionMetrics> = batch
        .sessions
        .into_iter()
        .map(|metrics| (metrics.session_id.clone(), metrics))
        .collect();

    let Some(mut metrics) = by_id.remove(&parent_session_id) else {
        // The transcript was readable but produced nothing analyzable — an
        // empty session, not a failure.
        return SessionAnalysis {
            source_path,
            fingerprint,
            ..SessionAnalysis::unavailable()
        };
    };

    // The views key icons and copy off the discovery slug, so the vendor label
    // the adapter registry dispatches on never leaves this module.
    metrics.agent = agent_slug.clone();
    cap_skill_descriptions(&mut metrics.skill_uses);

    let members: Vec<SubagentMember> = roster
        .into_iter()
        .map(|(subagent_id, label)| {
            let child = by_id.get(&subagent_id);
            let spawn_progress = child
                .and_then(|child| child.first_ts_ms)
                .zip(parent_stream.as_ref())
                .and_then(|(first_ts_ms, stream)| active_time_fraction(stream, first_ts_ms));
            SubagentMember {
                agent: agent_slug.clone(),
                subagent_id,
                label,
                pattern_score: child.map(|child| child.pattern_score).unwrap_or(0),
                spawn_progress,
            }
        })
        .collect();

    let orchestration = (!members.is_empty()).then_some(OrchestrationStatus {
        orchestrating: members.len() as u32 >= MIN_ORCHESTRATED_SUBAGENTS,
        orchestrator_agent: agent_slug,
        orchestrator_session_id: parent_session_id,
        subagent_count: members.len() as u32,
        members,
    });

    // Merge every sub-agent's breakdown into one map. Merge that map with
    // the parent's breakdown too. `cost` then prices the whole session, not
    // only the transcript a reader opened.
    let subagent_breakdowns: Vec<&HashMap<String, ModelTokens>> =
        by_id.values().map(|child| &child.model_breakdown).collect();
    let has_subagents = !subagent_breakdowns.is_empty();
    let subagents_model_breakdown = merge_model_breakdowns(subagent_breakdowns.iter().copied());
    let inclusive_model_breakdown = merge_model_breakdowns(
        std::iter::once(&metrics.model_breakdown).chain(subagent_breakdowns.iter().copied()),
    );

    let top_level_cost = price_breakdown(&metrics.model_breakdown);
    let subagents_cost = price_breakdown(&subagents_model_breakdown);
    let cost = price_breakdown(&inclusive_model_breakdown);
    let models = sorted_models(&inclusive_model_breakdown);
    let model_runs = model_runs_parent_first(&metrics, by_id.values());
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
    }
}

/// Analyze one sub-agent transcript on its own.
///
/// A sub-agent is a session in its own right — its own transcript, its own
/// health — so the analytics surface opens it the same way it opens anything
/// else. It has no roster of its own (vendors do not nest orchestration), so
/// this is the single-input path rather than the batch above.
pub async fn analyze_subagent(
    agent: AgentKind,
    parent_session_id: &str,
    subagent_id: &str,
    wsl_distro: Option<&str>,
) -> SessionAnalysis {
    let Some(source) = Explorers::DISK
        .locate_subagent_source_in_environment(&agent, parent_session_id, subagent_id, wsl_distro)
        .await
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

    let Ok(batch) =
        tauri::async_runtime::spawn_blocking(move || analyze_sources_with(vec![input], true)).await
    else {
        return SessionAnalysis::unavailable();
    };

    let Some(mut metrics) = batch.sessions.into_iter().next() else {
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
    }
}

/// Whether analysis of this agent's transcripts is more than a generic parse.
pub fn analytics_supported(agent: AgentKind) -> bool {
    supports_analytics(agent)
}

/// How many leading transcript lines are searched for a fork observation.
/// Vendors write it into the synthetic metadata header, which is the first
/// record; a handful of lines is generous and keeps the read bounded.
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
    content
        .lines()
        .take(FORK_OBSERVATION_LINES)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find_map(|value| find_fork_observation(&value, FORK_OBSERVATION_DEPTH))
}

/// Recursively look for the engine's fork-observation key and pull the parent
/// session id out of it.
fn find_fork_observation(value: &serde_json::Value, depth: usize) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let object = value.as_object()?;
    if let Some(observation) = object.get(FORK_OBSERVATION_KEY)
        && let Ok(observation) = serde_json::from_value::<ForkObservation>(observation.clone())
        && !observation.parent_agent_session_id.is_empty()
    {
        return Some(observation.parent_agent_session_id);
    }
    object
        .values()
        .find_map(|nested| find_fork_observation(nested, depth - 1))
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
            metrics_json: "{}".into(),
            cost_json: None,
            model_breakdown_json: "{}".into(),
            inclusive_models_json: "[]".into(),
            active_secs: 0,
            duration_secs: 0,
            pattern_score: 0,
            source_fingerprint: MISSING_FINGERPRINT.into(),
            pricing_generation: pricing_generation() as i64,
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
            metrics_json: "{}".into(),
            cost_json: None,
            model_breakdown_json: "{}".into(),
            inclusive_models_json: "[]".into(),
            active_secs: 0,
            duration_secs: 0,
            pattern_score: 0,
            source_fingerprint: "123:456".into(),
            pricing_generation: pricing_generation() as i64 - 1,
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
            find_fork_observation(&header, FORK_OBSERVATION_DEPTH).as_deref(),
            Some("parent-42")
        );
    }

    #[test]
    fn a_header_without_an_observation_yields_no_parent() {
        let header = serde_json::json!({ "type": "session_meta", "metadata": { "cwd": "/x" } });
        assert_eq!(find_fork_observation(&header, FORK_OBSERVATION_DEPTH), None);
        // A malformed observation is ignored rather than half-read.
        let broken = serde_json::json!({ FORK_OBSERVATION_KEY: { "parent_agent": "cursor" } });
        assert_eq!(find_fork_observation(&broken, FORK_OBSERVATION_DEPTH), None);
    }

    #[test]
    fn the_observation_search_stops_at_its_depth_budget() {
        let deep = serde_json::json!({ "a": { "b": { "c": { "d": {
            FORK_OBSERVATION_KEY: { "parent_agent_session_id": "too-deep" }
        }}}}});
        assert_eq!(find_fork_observation(&deep, FORK_OBSERVATION_DEPTH), None);
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

use std::collections::BTreeMap;

use crate::analysis::{EvidenceValue, SessionEvidence, lookup_turn_pricing, strip_window_tag};
use crate::pricing::{ModelPricing, canonical_model_key};

use super::DetectorId;
use super::report::EfficiencyReport;
use crate::checks::{self as detectors, DetectorStatus, ReportCatalogs};

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;

/// Maximum normalized turns retained for one session's effort comparison.
const MAX_TOKEN_BURN_COMPARISON_TURNS: usize = 4_096;

#[derive(Debug, Clone, PartialEq)]
pub struct TokenBurnSourceEvidence {
    // Collectors omit source groups when deferred loading prevents token attribution.
    /// The agent and installation scope used for window-level grouping.
    pub scope: String,
    /// The normalized source name used for window-level grouping.
    pub name: String,
    /// Definition tokens repeated across compatible main turns.
    pub replicated_tokens: u128,
    /// Whether this session invoked the source after loading it.
    pub invoked: bool,
    /// The priced cost of `replicated_tokens`, at the cache-read rate for
    /// each contributing turn's model. `None` when no contributing turn's
    /// model has a resolvable price; tokens and cost fail independently.
    pub replicated_cost_usd: Option<f64>,
}

/// One attributed assistant turn used only for report-time token estimates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenBurnTurnEvidence {
    pub scope: String,
    pub model: String,
    pub effort: Option<String>,
    pub speed: Option<String>,
    pub ts_ms: Option<i64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// The subset of `cache_write_tokens` that the provider keeps for one hour.
    pub cache_write_1h_tokens: u64,
}

impl TokenBurnTurnEvidence {
    fn total_tokens(&self) -> Option<u128> {
        u128::from(self.input_tokens)
            .checked_add(u128::from(self.output_tokens))?
            .checked_add(u128::from(self.cache_read_tokens))?
            .checked_add(u128::from(self.cache_write_tokens))
    }

    fn context_tokens(&self) -> Option<u128> {
        u128::from(self.input_tokens)
            .checked_add(u128::from(self.cache_read_tokens))?
            .checked_add(u128::from(self.cache_write_tokens))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ComparisonCandidate {
    context_tokens: u128,
    output_tokens: u64,
    assumed_tokens: u128,
}

#[derive(Debug, Default)]
struct ComparisonGroup {
    lower_effort: Vec<ComparisonCandidate>,
    above_cap: Vec<ComparisonCandidate>,
}

/// Streams one session's report-time turn estimates.
///
/// The accumulator retains at most 4,096 normalized turns for exact observed
/// effort comparisons. After that limit, it drops the retained comparisons and
/// uses the accumulated effort-tier assumptions for the whole session.
#[derive(Debug)]
pub struct TokenBurnTurnAccumulator<'a> {
    catalogs: &'a ReportCatalogs,
    comparison_groups: BTreeMap<String, BTreeMap<String, ComparisonGroup>>,
    retained_comparison_turns: usize,
    comparison_bound_exceeded: bool,
    overthinking_complete: bool,
    overthinking_assumed: Option<u128>,
    overpowered_subagents: Option<u128>,
    old_model: Option<u128>,
    fast_mode: Option<u128>,
}

impl<'a> TokenBurnTurnAccumulator<'a> {
    pub fn new(catalogs: &'a ReportCatalogs) -> Self {
        Self {
            catalogs,
            comparison_groups: BTreeMap::new(),
            retained_comparison_turns: 0,
            comparison_bound_exceeded: false,
            overthinking_assumed: Some(0),
            overthinking_complete: true,
            overpowered_subagents: Some(0),
            old_model: Some(0),
            fast_mode: Some(0),
        }
    }

    pub fn observe(&mut self, turn: TokenBurnTurnEvidence) {
        let canonical_model = canonical_model_key(&turn.model);
        let family = model_family_from_canonical(&canonical_model);
        let effort = turn
            .effort
            .as_deref()
            .map(|value| value.trim().to_lowercase());
        let context_tokens = turn.context_tokens();

        if let Some(effort) = effort.as_deref() {
            if let Some(policy) = self.catalogs.families.get(&family) {
                let above_cap = policy.effort.above_cap.contains(effort);
                let lower_effort = policy.effort.recognized.contains(effort) && !above_cap;
                let assumed_tokens = if above_cap {
                    match effort {
                        "xhigh" => percentage_of_tokens(u128::from(turn.output_tokens), 20),
                        "max" | "ultra" => percentage_of_tokens(u128::from(turn.output_tokens), 35),
                        _ => percentage_of_tokens(u128::from(turn.output_tokens), 10),
                    }
                } else {
                    Some(0)
                };
                if above_cap {
                    self.overthinking_assumed =
                        checked_accumulate(self.overthinking_assumed, assumed_tokens);
                }
                if (above_cap || lower_effort)
                    && let Some(context_tokens) = context_tokens
                {
                    self.retain_comparison(
                        turn.scope.clone(),
                        canonical_model.clone(),
                        ComparisonCandidate {
                            context_tokens,
                            output_tokens: turn.output_tokens,
                            assumed_tokens: assumed_tokens.unwrap_or(0),
                        },
                        above_cap,
                    );
                }
            } else {
                self.overthinking_complete = false;
            }
        }

        if turn.scope == "delegated"
            && let Some(replacement) = premium_replacement(family, &canonical_model, self.catalogs)
        {
            self.overpowered_subagents = checked_accumulate(
                self.overpowered_subagents,
                priced_saving(&turn, &canonical_model, replacement),
            );
        }
        if let Some(replacement) = self
            .catalogs
            .model_replacements
            .entries
            .get(&canonical_model)
            && turn
                .ts_ms
                .is_some_and(|timestamp| timestamp >= replacement.available_since_ts_ms)
        {
            self.old_model = checked_accumulate(
                self.old_model,
                priced_saving(&turn, &canonical_model, &replacement.replacement),
            );
        }
        if turn.scope == "delegated"
            && turn
                .speed
                .as_deref()
                .is_some_and(|speed| speed.trim().eq_ignore_ascii_case("fast"))
        {
            self.fast_mode =
                checked_accumulate(self.fast_mode, fast_mode_saving(&turn, &canonical_model));
        }
    }

    fn retain_comparison(
        &mut self,
        scope: String,
        canonical_model: String,
        candidate: ComparisonCandidate,
        above_cap: bool,
    ) {
        if self.comparison_bound_exceeded {
            return;
        }
        if self.retained_comparison_turns == MAX_TOKEN_BURN_COMPARISON_TURNS {
            self.comparison_bound_exceeded = true;
            self.comparison_groups.clear();
            self.retained_comparison_turns = 0;
            return;
        }
        self.retained_comparison_turns += 1;
        let group = self
            .comparison_groups
            .entry(scope)
            .or_default()
            .entry(canonical_model)
            .or_default();
        if above_cap {
            group.above_cap.push(candidate);
        } else {
            group.lower_effort.push(candidate);
        }
    }

    pub fn finish_into(self, evidence: &mut SessionTokenBurnEvidence) {
        let model_overthinking = if !self.overthinking_complete {
            None
        } else if self.comparison_bound_exceeded {
            self.overthinking_assumed
        } else {
            self.comparison_groups
                .into_values()
                .flat_map(BTreeMap::into_values)
                .try_fold(0_u128, |total, group| {
                    total.checked_add(overthinking_group_tokens(group).tokens?)
                })
        };
        evidence.model_overthinking = model_overthinking;
        evidence.overpowered_subagents = self.overpowered_subagents;
        evidence.old_model = self.old_model;
        evidence.fast_mode = self.fast_mode;
    }

    #[cfg(test)]
    fn retained_comparison_turns(&self) -> usize {
        self.retained_comparison_turns
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OverthinkingGroupEstimate {
    tokens: Option<u128>,
    operations: usize,
}

fn overthinking_group_tokens(mut group: ComparisonGroup) -> OverthinkingGroupEstimate {
    group
        .lower_effort
        .sort_unstable_by_key(|turn| (turn.output_tokens, turn.context_tokens));
    group.above_cap.sort_unstable_by_key(|turn| {
        (turn.output_tokens, turn.context_tokens, turn.assumed_tokens)
    });
    let mut active = BTreeMap::<u128, ComparisonCandidate>::new();
    let mut lower_index = 0;
    let mut total = Some(0_u128);
    let mut operations = 0;
    for turn in group.above_cap {
        while lower_index < group.lower_effort.len()
            && group.lower_effort[lower_index].output_tokens < turn.output_tokens
        {
            let candidate = group.lower_effort[lower_index];
            active
                .entry(candidate.context_tokens)
                .and_modify(|current| {
                    if candidate.output_tokens > current.output_tokens {
                        *current = candidate;
                    }
                })
                .or_insert(candidate);
            lower_index += 1;
            operations += 1;
        }
        let lower_bound = turn.context_tokens - turn.context_tokens / 5;
        let upper_bound = turn.context_tokens.saturating_add(turn.context_tokens / 4);
        let left = active.range(lower_bound..=turn.context_tokens).next_back();
        let right = active.range(turn.context_tokens..=upper_bound).next();
        operations += 2;
        let observed = [left, right]
            .into_iter()
            .flatten()
            .map(|(_, candidate)| {
                (
                    turn.context_tokens.abs_diff(candidate.context_tokens),
                    u64::MAX - candidate.output_tokens,
                    candidate.context_tokens,
                    u128::from(turn.output_tokens - candidate.output_tokens),
                )
            })
            .min_by_key(|(difference, reverse_output, context, _)| {
                (*difference, *reverse_output, *context)
            })
            .map(|(_, _, _, tokens)| tokens);
        total = checked_accumulate(total, observed.or(Some(turn.assumed_tokens)));
    }
    OverthinkingGroupEstimate {
        tokens: total,
        operations,
    }
}

fn checked_accumulate(total: Option<u128>, value: Option<u128>) -> Option<u128> {
    total?.checked_add(value?)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionTokenBurnEvidence {
    /// All attributed input, output, cache-read, and cache-write tokens.
    pub total_tokens: Option<u128>,
    /// Cache token events that a context cap would remove.
    pub overdepth_avoidable_tokens: Option<u128>,
    /// Paid context token events beyond positive context growth.
    pub repeated_context_avoidable_tokens: Option<u128>,
    model_overthinking: Option<u128>,
    overpowered_subagents: Option<u128>,
    old_model: Option<u128>,
    fast_mode: Option<u128>,
    pub mcp_sources: Option<Vec<TokenBurnSourceEvidence>>,
    pub built_in_tool_sources: Option<Vec<TokenBurnSourceEvidence>>,
    pub skill_sources: Option<Vec<TokenBurnSourceEvidence>>,
    /// The pricing table generation active while this report ran, stamped
    /// once regardless of whether any source priced. `None` before the
    /// report-time pricing pass runs.
    pub pricing_revision: Option<String>,
}

impl SessionTokenBurnEvidence {
    pub fn from_session(evidence: &SessionEvidence) -> Self {
        let models = match &evidence.models {
            EvidenceValue::Partial {
                observed: models, ..
            }
            | EvidenceValue::Complete(models) => Some(models),
            _ => None,
        };
        let total_tokens = models.and_then(|models| {
            models.by_model.values().try_fold(0_u128, |total, tokens| {
                total
                    .checked_add(u128::from(tokens.input))?
                    .checked_add(u128::from(tokens.output))?
                    .checked_add(u128::from(tokens.cache_read))?
                    .checked_add(u128::from(tokens.cache_creation))
            })
        });
        let repeated_context_avoidable_tokens = models
            .filter(|models| models.unattributed_turns == 0)
            .and_then(|_| match &evidence.cache {
                EvidenceValue::Partial {
                    observed: cache, ..
                }
                | EvidenceValue::Complete(cache) => match &cache.repeated_context {
                    EvidenceValue::Partial {
                        observed: repeated, ..
                    }
                    | EvidenceValue::Complete(repeated) => {
                        Some(u128::from(repeated.repeated_tokens))
                    }
                    _ => None,
                },
                _ => None,
            });
        Self {
            total_tokens,
            repeated_context_avoidable_tokens,
            ..Self::default()
        }
    }
}

fn percentage_of_tokens(tokens: u128, percentage: u128) -> Option<u128> {
    tokens
        .checked_mul(percentage)?
        .checked_add(99)
        .map(|scaled| scaled / 100)
}

fn token_cost(tokens: &TokenBurnTurnEvidence, pricing: &ModelPricing) -> f64 {
    tokens.input_tokens as f64 * pricing.input_cost_per_token
        + tokens.output_tokens as f64 * pricing.output_cost_per_token
        + tokens.cache_read_tokens as f64 * pricing.cache_read_cost_per_token
        + tokens
            .cache_write_tokens
            .saturating_sub(tokens.cache_write_1h_tokens) as f64
            * pricing.cache_write_cost_per_token
        + tokens.cache_write_1h_tokens as f64 * pricing.input_cost_per_token * 2.0
}

fn cost_saving_tokens(
    turn: &TokenBurnTurnEvidence,
    actual: &ModelPricing,
    replacement: &ModelPricing,
) -> Option<u128> {
    let total_tokens = turn.total_tokens()?;
    let actual_cost = token_cost(turn, actual);
    let replacement_cost = token_cost(turn, replacement);
    if !actual_cost.is_finite()
        || !replacement_cost.is_finite()
        || actual_cost <= replacement_cost
        || actual_cost <= 0.0
    {
        return None;
    }
    let equivalent = total_tokens as f64 * (actual_cost - replacement_cost) / actual_cost;
    if !equivalent.is_finite() || equivalent <= 0.0 {
        return None;
    }
    Some((equivalent.round() as u128).clamp(1, total_tokens))
}

fn report_turn_pricing(
    model: &str,
    canonical_model: &str,
    speed: Option<&str>,
) -> Option<ModelPricing> {
    lookup_turn_pricing(model, speed).or_else(|| lookup_turn_pricing(canonical_model, speed))
}

fn priced_saving(
    turn: &TokenBurnTurnEvidence,
    canonical_model: &str,
    replacement: &str,
) -> Option<u128> {
    let replacement_canonical = canonical_model_key(replacement);
    report_turn_pricing(&turn.model, canonical_model, turn.speed.as_deref())
        .zip(report_turn_pricing(
            replacement,
            &replacement_canonical,
            turn.speed.as_deref(),
        ))
        .and_then(|(actual, replacement)| cost_saving_tokens(turn, &actual, &replacement))
}

fn model_family_from_canonical(canonical: &str) -> detectors::ModelFamily {
    if canonical.starts_with("claude-") {
        detectors::ModelFamily::Claude
    } else if canonical.starts_with("gpt-")
        || canonical.starts_with("o1")
        || canonical.starts_with("o3")
        || canonical.starts_with("o4")
    {
        detectors::ModelFamily::OpenAi
    } else if canonical.starts_with("gemini-") {
        detectors::ModelFamily::Google
    } else {
        detectors::ModelFamily::Unknown
    }
}

fn premium_replacement(
    family: detectors::ModelFamily,
    canonical_model: &str,
    catalogs: &ReportCatalogs,
) -> Option<&'static str> {
    let policy = &catalogs.families.get(&family)?.premium;
    if !policy.reviewed || !policy.is_premium(canonical_model) {
        return None;
    }
    match family {
        detectors::ModelFamily::OpenAi => Some("gpt-5.6-luna"),
        detectors::ModelFamily::Claude => Some("claude-sonnet-5"),
        detectors::ModelFamily::Google => Some("gemini-3.8-flash"),
        detectors::ModelFamily::Unknown => None,
    }
}

fn fast_mode_saving(turn: &TokenBurnTurnEvidence, canonical_model: &str) -> Option<u128> {
    let observed_model = strip_window_tag(&turn.model).trim();
    let observed_model = crate::pricing::normalize_model_key(observed_model);
    let standard_model = observed_model
        .strip_suffix("-fast")
        .unwrap_or(observed_model);
    let standard_canonical = canonical_model
        .strip_suffix("-fast")
        .unwrap_or(canonical_model);
    report_turn_pricing(standard_model, standard_canonical, Some("fast"))
        .zip(report_turn_pricing(
            standard_model,
            standard_canonical,
            None,
        ))
        .and_then(|(fast, standard)| cost_saving_tokens(turn, &fast, &standard))
}

#[derive(Default)]
struct SourceAggregate {
    invoked: bool,
    by_session: BTreeMap<usize, u128>,
}

#[derive(Default)]
struct SessionTokenContribution {
    overdepth: Option<u128>,
    repeated_context: Option<u128>,
    model_overthinking: Option<u128>,
    overpowered_subagents: Option<u128>,
    old_model: Option<u128>,
    fast_mode: Option<u128>,
}

#[derive(Default)]
pub(super) struct TokenBurnAccumulator {
    total_complete: bool,
    total_tokens: u128,
    // Exact overlap needs one compact contribution per session and source/session pair.
    sessions: Vec<SessionTokenContribution>,
    sources: [BTreeMap<(String, String), SourceAggregate>; 3],
    source_complete: [bool; 3],
}

pub(super) type TokenBurnResult = (
    Option<u16>,
    [Option<u16>; DetectorId::COUNT],
    [Option<Vec<u128>>; DetectorId::COUNT],
);

impl TokenBurnAccumulator {
    pub(super) fn new() -> Self {
        Self {
            total_complete: true,
            source_complete: [true; 3],
            ..Self::default()
        }
    }

    pub(super) fn observe(
        &mut self,
        token_evidence: SessionTokenBurnEvidence,
        findings: [bool; DetectorId::COUNT],
        source_eligible: [bool; 3],
        cache_assessed: bool,
    ) {
        if let Some(session_tokens) = token_evidence.total_tokens {
            if let Some(total_tokens) = self.total_tokens.checked_add(session_tokens) {
                self.total_tokens = total_tokens;
            } else {
                self.total_complete = false;
            }
        }
        let session_index = self.sessions.len();
        self.sessions.push(SessionTokenContribution {
            overdepth: if findings[DetectorId::SessionsOverDepth.index()] {
                token_evidence.overdepth_avoidable_tokens
            } else {
                Some(0)
            },
            repeated_context: if findings[DetectorId::CacheChurn.index()] {
                token_evidence.repeated_context_avoidable_tokens
            } else if cache_assessed {
                Some(0)
            } else {
                None
            },
            model_overthinking: if findings[DetectorId::ModelOverthinking.index()] {
                token_evidence.model_overthinking
            } else {
                Some(0)
            },
            overpowered_subagents: if findings[DetectorId::OverpoweredSubagents.index()] {
                token_evidence.overpowered_subagents
            } else {
                Some(0)
            },
            old_model: if findings[DetectorId::OldModelUsage.index()] {
                token_evidence.old_model
            } else {
                Some(0)
            },
            fast_mode: if findings[DetectorId::OveruseOfFastMode.index()] {
                token_evidence.fast_mode
            } else {
                Some(0)
            },
        });

        let source_groups = [
            token_evidence.mcp_sources,
            token_evidence.built_in_tool_sources,
            token_evidence.skill_sources,
        ];
        for (index, sources) in source_groups.into_iter().enumerate() {
            let detector = [
                DetectorId::UnusedMcpServers,
                DetectorId::UnusedBuiltInTools,
                DetectorId::UnusedSkills,
            ][index];
            let Some(sources) = sources else {
                continue;
            };
            for source in sources {
                let aggregate = self.sources[index]
                    .entry((source.scope, source.name))
                    .or_default();
                aggregate.invoked |= source.invoked;
                if !source_eligible[index] || !findings[detector.index()] {
                    continue;
                }
                let entry = aggregate.by_session.entry(session_index).or_default();
                let Some(total) = entry.checked_add(source.replicated_tokens) else {
                    self.source_complete[index] = false;
                    continue;
                };
                *entry = total;
            }
        }
    }

    pub(super) fn denominator(&self) -> Option<u128> {
        (self.total_complete && self.total_tokens > 0).then_some(self.total_tokens)
    }

    pub(super) fn finish(self, statuses: &[DetectorStatus; DetectorId::COUNT]) -> TokenBurnResult {
        let mut numerators = [None; DetectorId::COUNT];
        let mut contributions = core::array::from_fn(|_| None);
        let mut combined_by_session = vec![0_u128; self.sessions.len()];
        let mut source_combined_by_session = vec![0_u128; self.sessions.len()];
        let mut source_detector_by_session = vec![0_u128; self.sessions.len()];
        let denominator = self.denominator();
        let can_measure = denominator.is_some();

        for (detector, value_for) in [
            (
                DetectorId::SessionsOverDepth,
                (|session: &SessionTokenContribution| session.overdepth)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::CacheChurn,
                (|session: &SessionTokenContribution| session.repeated_context)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::ModelOverthinking,
                (|session: &SessionTokenContribution| session.model_overthinking)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::OverpoweredSubagents,
                (|session: &SessionTokenContribution| session.overpowered_subagents)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::OldModelUsage,
                (|session: &SessionTokenContribution| session.old_model)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
            (
                DetectorId::OveruseOfFastMode,
                (|session: &SessionTokenContribution| session.fast_mode)
                    as fn(&SessionTokenContribution) -> Option<u128>,
            ),
        ] {
            if !matches!(statuses[detector.index()], DetectorStatus::Findings(_)) {
                if matches!(statuses[detector.index()], DetectorStatus::Clean) {
                    numerators[detector.index()] = Some(0);
                }
                continue;
            }
            if !can_measure {
                continue;
            }
            if self
                .sessions
                .iter()
                .any(|session| value_for(session).is_none())
            {
                continue;
            }
            let Some(total) = self.sessions.iter().try_fold(0_u128, |total, session| {
                total.checked_add(value_for(session).unwrap_or(0))
            }) else {
                return empty_token_burn_result();
            };
            numerators[detector.index()] = Some(total);
            contributions[detector.index()] = Some(
                self.sessions
                    .iter()
                    .map(|session| value_for(session).unwrap_or(0))
                    .collect(),
            );
            for (index, session) in self.sessions.iter().enumerate() {
                combined_by_session[index] =
                    combined_by_session[index].max(value_for(session).unwrap_or(0));
            }
        }

        for (source_index, detector) in [
            DetectorId::UnusedMcpServers,
            DetectorId::UnusedBuiltInTools,
            DetectorId::UnusedSkills,
        ]
        .into_iter()
        .enumerate()
        {
            if matches!(statuses[detector.index()], DetectorStatus::Clean) {
                numerators[detector.index()] = Some(0);
                continue;
            }
            if !matches!(statuses[detector.index()], DetectorStatus::Findings(_))
                || !can_measure
                || !self.source_complete[source_index]
            {
                continue;
            }
            source_detector_by_session.fill(0);
            let mut qualifying_source = false;
            for aggregate in self.sources[source_index].values() {
                if aggregate.invoked {
                    continue;
                }
                qualifying_source = true;
                for (session, tokens) in &aggregate.by_session {
                    let Some(total) = source_detector_by_session[*session].checked_add(*tokens)
                    else {
                        return empty_token_burn_result();
                    };
                    source_detector_by_session[*session] = total;
                }
            }
            if !qualifying_source {
                continue;
            }
            let Some(total) = source_detector_by_session
                .iter()
                .try_fold(0_u128, |total, value| total.checked_add(*value))
            else {
                return empty_token_burn_result();
            };
            numerators[detector.index()] = Some(total);
            contributions[detector.index()] = Some(source_detector_by_session.clone());
            for (index, value) in source_detector_by_session.iter().enumerate() {
                let Some(total) = source_combined_by_session[index].checked_add(*value) else {
                    return empty_token_burn_result();
                };
                source_combined_by_session[index] = total;
            }
        }
        for (index, source_tokens) in source_combined_by_session.into_iter().enumerate() {
            combined_by_session[index] = combined_by_session[index].max(source_tokens);
        }

        let percentage =
            |numerator| denominator.and_then(|total| token_burn_basis_points(numerator, total));
        let assessed_sessions = self.sessions.len() as u64;
        let estimates = core::array::from_fn(|index| match &statuses[index] {
            DetectorStatus::Findings(findings) => {
                numerators[index].and_then(percentage).or_else(|| {
                    fallback_token_burn_basis_points(
                        DetectorId::ALL[index],
                        findings.finding_sessions,
                        assessed_sessions,
                    )
                })
            }
            DetectorStatus::Clean => Some(0),
            DetectorStatus::NotAssessed(_) => None,
        });
        let has_measured_finding = statuses.iter().enumerate().any(|(index, status)| {
            matches!(status, DetectorStatus::Findings(_)) && numerators[index].is_some()
        });
        let combined = if has_measured_finding {
            combined_by_session
                .into_iter()
                .try_fold(0_u128, u128::checked_add)
                .and_then(percentage)
        } else {
            estimates.iter().flatten().copied().max()
        };
        (combined, estimates, contributions)
    }
}

fn empty_token_burn_result() -> TokenBurnResult {
    (
        None,
        [None; DetectorId::COUNT],
        core::array::from_fn(|_| None),
    )
}

impl EfficiencyReport {
    pub fn estimated_token_burn_for_attributed_tokens(&self, tokens: u128) -> Option<u16> {
        token_burn_basis_points(tokens, self.token_burn_denominator?)
    }

    pub fn estimated_token_burn_for_active_detectors(
        &self,
        active_detector_mask: u16,
        resource_assessments: &[ResourceTokenBurnAssessment<'_>],
    ) -> Option<u16> {
        let mut non_resource_by_session = vec![0_u128; self.assessed_sessions as usize];
        let mut resource_by_session = vec![0_u128; self.assessed_sessions as usize];
        let mut measured_finding = false;
        let mut fallback = None;

        for detector in DetectorId::ALL {
            if active_detector_mask & (1 << detector.index()) == 0 {
                continue;
            }
            let resource = resource_assessments
                .iter()
                .find(|assessment| assessment.detector == detector);
            let (finding_count, clean, contribution) = if let Some(resource) = resource {
                (
                    resource.finding_count,
                    resource.clean,
                    resource.tokens_by_session.and_then(|tokens| {
                        let mut contribution = vec![0_u128; self.assessed_sessions as usize];
                        for &(session, value) in tokens {
                            let total = contribution.get_mut(session)?;
                            *total = total.checked_add(value)?;
                        }
                        Some(contribution)
                    }),
                )
            } else {
                let (finding_count, clean) = match &self.detector_statuses[detector.index()] {
                    DetectorStatus::Findings(findings) => (findings.finding_sessions, false),
                    DetectorStatus::Clean => (0, true),
                    DetectorStatus::NotAssessed(_) => (0, false),
                };
                (
                    finding_count,
                    clean,
                    self.token_burn_by_detector_by_session[detector.index()].clone(),
                )
            };

            if finding_count > 0 {
                fallback = fallback.max(fallback_token_burn_basis_points(
                    detector,
                    finding_count,
                    self.assessed_sessions,
                ));
                if self.token_burn_denominator.is_some()
                    && let Some(contribution) = contribution
                {
                    measured_finding = true;
                    let combined = if resource.is_some() {
                        &mut resource_by_session
                    } else {
                        &mut non_resource_by_session
                    };
                    for (total, value) in combined.iter_mut().zip(contribution) {
                        if resource.is_some() {
                            *total = total.checked_add(value)?;
                        } else {
                            *total = (*total).max(value);
                        }
                    }
                }
            } else if clean {
                fallback = fallback.max(Some(0));
            }
        }

        if !measured_finding {
            return fallback;
        }
        let numerator = non_resource_by_session
            .into_iter()
            .zip(resource_by_session)
            .try_fold(0_u128, |total, (non_resource, resource)| {
                total.checked_add(non_resource.max(resource))
            })?;
        self.estimated_token_burn_for_attributed_tokens(numerator)
    }
}

pub const MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS: u16 = 10_000;
const BASIS_POINTS_SCALE: u16 = 10_000;

#[derive(Debug, Clone, Copy)]
pub struct ResourceTokenBurnAssessment<'a> {
    pub detector: DetectorId,
    pub finding_count: u64,
    pub clean: bool,
    pub tokens_by_session: Option<&'a [(usize, u128)]>,
}

/// Returns a conservative proxy when a finding has no attributable token or pricing evidence.
/// The proxy scales a detector-specific workload share by the finding rate.
pub fn fallback_token_burn_basis_points(
    detector: DetectorId,
    finding_sessions: u64,
    assessed_sessions: u64,
) -> Option<u16> {
    if finding_sessions == 0
        || assessed_sessions == 0
        || matches!(
            detector,
            DetectorId::IgnoredInstructions
                | DetectorId::SkillOpportunities
                | DetectorId::OverExploring
                | DetectorId::ScopeCreep
        )
    {
        return None;
    }
    let detector_share: u16 = match detector {
        DetectorId::SessionsOverDepth => 1_000,
        DetectorId::ModelOverthinking => 2_000,
        DetectorId::OverpoweredSubagents => 2_500,
        DetectorId::UnusedMcpServers
        | DetectorId::UnusedBuiltInTools
        | DetectorId::UnusedSkills => 500,
        DetectorId::OldModelUsage => 1_500,
        DetectorId::OveruseOfFastMode => 1_000,
        DetectorId::CacheChurn => 1_000,
        DetectorId::IgnoredInstructions
        | DetectorId::SkillOpportunities
        | DetectorId::OverExploring
        | DetectorId::ScopeCreep => return None,
    };
    let scaled = u128::from(detector_share)
        .checked_mul(u128::from(finding_sessions))?
        .checked_add(u128::from(assessed_sessions / 2))?
        / u128::from(assessed_sessions);
    Some(scaled.clamp(1, u128::from(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS)) as u16)
}

fn token_burn_basis_points(numerator: u128, denominator: u128) -> Option<u16> {
    numerator
        .checked_mul(u128::from(BASIS_POINTS_SCALE))
        .and_then(|scaled| scaled.checked_add(denominator / 2))
        .map(|rounded| {
            (rounded / denominator).min(u128::from(MAX_ESTIMATED_TOKEN_BURN_BASIS_POINTS)) as u16
        })
}

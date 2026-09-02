//! Per-session estimates of provider-reported allowance use.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use antiburn_local::analysis::{ProviderHint, lookup_pricing};
use antiburn_local::pricing::{
    ModelPricing, ModelTokens, calc::calculate_cache_write_cost, canonical_model_key,
};
use serde::Deserialize;
use time::{Duration, OffsetDateTime, format_description::well_known::Rfc3339};

use super::attribute;
use crate::dto::{
    LiveProviderUsage, LiveUsageFreshness, LiveUsageSummary, LiveUsageWindow,
    SessionLimitAllocation, SessionLimitMetric,
};
use crate::provider_usage::live::{history::History, model::Freshness};
use crate::store::{SessionKey, SessionUsageRecord};

#[derive(Debug, Clone, PartialEq, Eq)]
enum AccountEvidence {
    None,
    One(String),
    Ambiguous,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountObservation {
    provider: String,
    account_key: String,
}

#[derive(Debug, Clone)]
struct WeightedTurn {
    key: SessionKey,
    wsl_distro: Option<String>,
    provider: &'static str,
    account: AccountEvidence,
    at_ms: i64,
    model: String,
    price_weight: Option<f64>,
    token_weight: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WeightBasis {
    Price,
    Tokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum EstimateTier {
    TokenCurrent,
    PriceCurrent,
    TokenHistory,
    PriceHistory,
}

impl EstimateTier {
    fn new(basis: WeightBasis, uses_history: bool) -> Self {
        match (basis, uses_history) {
            (WeightBasis::Price, true) => Self::PriceHistory,
            (WeightBasis::Tokens, true) => Self::TokenHistory,
            (WeightBasis::Price, false) => Self::PriceCurrent,
            (WeightBasis::Tokens, false) => Self::TokenCurrent,
        }
    }
}

#[derive(Debug)]
struct Candidate {
    allocation: SessionLimitAllocation,
    tier: EstimateTier,
}

impl WeightedTurn {
    fn weight(&self, basis: WeightBasis) -> Option<f64> {
        match basis {
            WeightBasis::Price => self.price_weight,
            WeightBasis::Tokens => Some(self.token_weight),
        }
    }
}

fn account_evidence(json: &str) -> HashMap<String, AccountEvidence> {
    let mut accounts: HashMap<String, BTreeSet<String>> = HashMap::new();
    for entry in serde_json::from_str::<Vec<AccountObservation>>(json).unwrap_or_default() {
        if entry.account_key.len() == 64 {
            accounts
                .entry(entry.provider)
                .or_default()
                .insert(entry.account_key);
        }
    }
    accounts
        .into_iter()
        .map(|(provider, accounts)| {
            let evidence = if accounts.len() == 1 {
                AccountEvidence::One(accounts.into_iter().next().expect("one account"))
            } else {
                AccountEvidence::Ambiguous
            };
            (provider, evidence)
        })
        .collect()
}

fn normalized_account(
    evidence: &AccountEvidence,
    known: &BTreeSet<String>,
) -> Option<Option<String>> {
    match (known.len(), evidence) {
        (_, AccountEvidence::Ambiguous) => None,
        (0, AccountEvidence::None) => Some(None),
        (1, AccountEvidence::None) => Some(known.iter().next().cloned()),
        (_, AccountEvidence::One(account)) if known.contains(account) => {
            Some(Some(account.clone()))
        }
        _ => None,
    }
}

fn normalize_live_account(
    account: Option<&str>,
    known: &BTreeSet<String>,
) -> Option<Option<String>> {
    match (known.len(), account) {
        (0, None) => Some(None),
        (1, None) => Some(known.iter().next().cloned()),
        (_, Some(account)) if known.contains(account) => Some(Some(account.to_string())),
        _ => None,
    }
}

fn weighted_turns(rows: Vec<SessionUsageRecord>) -> Vec<WeightedTurn> {
    let mut turns = Vec::new();
    let mut pricing: HashMap<String, Option<ModelPricing>> = HashMap::new();
    for session in rows {
        let hints: Vec<ProviderHint> = session
            .provider_hints_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        let accounts = account_evidence(&session.provider_accounts_json);
        for row in session.turns {
            let Some(at_ms) = row.ts_ms else { continue };
            let model = row.model.as_deref().map(str::trim).unwrap_or_default();
            let model_tokens = ModelTokens {
                input_tokens: row.input_tokens,
                output_tokens: row.output_tokens,
                cache_read_tokens: row.cache_read_tokens,
                cache_creation_tokens: row.cache_write_tokens,
                cache_creation_1h_tokens: 0,
            };
            if !super::has_tokens(&model_tokens) {
                continue;
            }
            let attributed = attribute(
                &session.key.agent,
                BTreeMap::from([(model.to_string(), model_tokens)]),
                &hints,
            );
            let Some((provider, tokens)) = attributed.iter().find_map(|(provider, attributed)| {
                attributed
                    .models
                    .get(model)
                    .map(|tokens| (*provider, tokens))
            }) else {
                continue;
            };
            let rates = pricing
                .entry(model.to_string())
                .or_insert_with(|| lookup_pricing(model));
            let price_weight = rates.as_ref().map(|rates| {
                tokens.input_tokens as f64 * rates.input_cost_per_token
                    + tokens.output_tokens as f64 * rates.output_cost_per_token
                    + tokens.cache_read_tokens as f64 * rates.cache_read_cost_per_token
                    + calculate_cache_write_cost(tokens, rates)
            });
            let token_weight = tokens.input_tokens as f64
                + tokens.output_tokens as f64
                + tokens.cache_read_tokens as f64
                + tokens.cache_creation_tokens as f64;
            turns.push(WeightedTurn {
                key: session.key.clone(),
                wsl_distro: session.wsl_distro.clone(),
                provider,
                account: accounts
                    .get(provider)
                    .cloned()
                    .unwrap_or(AccountEvidence::None),
                at_ms,
                model: model.to_string(),
                price_weight,
                token_weight,
            });
        }
    }
    turns
}

fn parse_at(value: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value, &Rfc3339).ok()
}

fn window_bounds(
    live: &LiveProviderUsage,
    window: &LiveUsageWindow,
    metric: SessionLimitMetric,
    now_ms: i64,
) -> Option<(i64, i64)> {
    if live.freshness != LiveUsageFreshness::Fresh {
        return None;
    }
    let observed = parse_at(&live.observed_at)?;
    let reset = parse_at(window.resets_at.as_deref()?)?;
    let reset_ms = reset.unix_timestamp_nanos() as i64 / 1_000_000;
    if observed >= reset || now_ms >= reset_ms {
        return None;
    }
    let duration = match metric {
        SessionLimitMetric::Weekly => Duration::days(7),
        SessionLimitMetric::FiveHour => Duration::hours(5),
    };
    let start = window
        .starts_at
        .as_deref()
        .and_then(parse_at)
        .unwrap_or(reset - duration);
    if start >= observed || start >= reset {
        return None;
    }
    Some((
        start.unix_timestamp_nanos() as i64 / 1_000_000,
        observed.unix_timestamp_nanos() as i64 / 1_000_000,
    ))
}

fn window_applies(window: &LiveUsageWindow, turn: &WeightedTurn) -> bool {
    let Some(scope) = window.scope_model.as_deref() else {
        return true;
    };
    let scope = super::live::normalize::slugify(scope);
    let model = canonical_model_key(&turn.model);
    if turn.provider == super::providers::GOOGLE {
        return match scope.as_str() {
            "gemini" => super::providers::provider_for_model(&model) == super::providers::GOOGLE,
            "claude-gpt" => matches!(
                super::providers::provider_for_model(&model),
                super::providers::ANTHROPIC | super::providers::OPENAI
            ),
            _ => false,
        };
    }
    scope == super::live::normalize::slugify(&model)
}

fn metric_of(provider: &str, window: &LiveUsageWindow) -> Option<SessionLimitMetric> {
    if window.kind == "weekly" {
        Some(SessionLimitMetric::Weekly)
    } else {
        let five_hour = match provider {
            super::providers::ANTHROPIC => window.id == "five-hour",
            super::providers::OPENAI => window.id == "five-hour" || window.id.ends_with("-300m"),
            super::providers::GOOGLE => matches!(
                window.id.as_str(),
                "antigravity-gemini-5h" | "antigravity-claude-gpt-5h"
            ),
            _ => false,
        };
        five_hour.then_some(SessionLimitMetric::FiveHour)
    }
}

fn distribute<'a>(
    turns: impl Iterator<Item = &'a WeightedTurn>,
    percent: f64,
    basis: WeightBasis,
    output: &mut HashMap<SessionKey, f64>,
) -> bool {
    let turns: Vec<_> = turns.collect();
    if turns.is_empty() {
        return false;
    }
    let total: f64 = turns.iter().filter_map(|turn| turn.weight(basis)).sum();
    if !total.is_finite() || total <= 0.0 || !percent.is_finite() || percent < 0.0 {
        return false;
    }
    let mut by_session: HashMap<SessionKey, f64> = HashMap::new();
    for turn in turns {
        *by_session.entry(turn.key.clone()).or_default() += turn.weight(basis).unwrap_or_default();
    }
    for (key, weight) in by_session {
        *output.entry(key).or_default() += percent * weight / total;
    }
    true
}

fn distribute_window(
    turns: &[&WeightedTurn],
    used_percent: f64,
    start_ms: i64,
    observed_ms: i64,
    samples: &[crate::provider_usage::live::metrics::UsageSample],
    basis: WeightBasis,
) -> (HashMap<SessionKey, f64>, bool) {
    let mut points: BTreeMap<_, _> = samples
        .iter()
        .filter(|sample| sample.freshness == Freshness::Fresh)
        .filter_map(|sample| {
            let percent = sample.used_percent?;
            let at = sample.observed_at.unix_timestamp_nanos() as i64 / 1_000_000;
            (at >= start_ms && at <= observed_ms && percent.is_finite() && percent >= 0.0)
                .then_some((at, percent))
        })
        .collect();
    points.insert(observed_ms, used_percent);
    let points: Vec<_> = points.into_iter().collect();

    if points.len() < 2 || points.windows(2).any(|pair| pair[1].1 < pair[0].1) {
        let mut output = HashMap::new();
        distribute(turns.iter().copied(), used_percent, basis, &mut output);
        return (output, false);
    }

    let mut output = HashMap::new();
    let mut previous_at = start_ms.saturating_sub(1);
    let mut previous_percent = 0.0;
    for (at, percent) in points {
        let delta = percent - previous_percent;
        if delta > 0.0 {
            distribute(
                turns
                    .iter()
                    .copied()
                    .filter(|turn| turn.at_ms > previous_at && turn.at_ms <= at),
                delta,
                basis,
                &mut output,
            );
        }
        previous_at = at;
        previous_percent = percent;
    }
    (output, true)
}

/// Estimate current weekly and five-hour shares from published local turns.
pub fn estimate(
    rows: Vec<SessionUsageRecord>,
    live: &LiveUsageSummary,
    history: &History,
    now_ms: i64,
) -> Vec<SessionLimitAllocation> {
    let turns = weighted_turns(rows);
    let mut known: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for provider in &live.providers {
        if let Some(account) = &provider.account_key {
            known
                .entry(provider.provider.clone())
                .or_default()
                .insert(account.clone());
        }
    }
    for turn in &turns {
        if let AccountEvidence::One(account) = &turn.account {
            known
                .entry(turn.provider.to_string())
                .or_default()
                .insert(account.clone());
        }
    }

    let mut best: HashMap<(SessionKey, SessionLimitMetric), Candidate> = HashMap::new();
    for provider in &live.providers {
        let provider_accounts = known.get(&provider.provider).cloned().unwrap_or_default();
        let Some(account) =
            normalize_live_account(provider.account_key.as_deref(), &provider_accounts)
        else {
            continue;
        };
        for window in &provider.windows {
            let Some(metric) = metric_of(&provider.provider, window) else {
                continue;
            };
            let Some(resets_at) = window.resets_at.clone() else {
                continue;
            };
            let Some(used_percent) = window
                .used_percent
                .filter(|value| value.is_finite() && *value >= 0.0 && *value <= 100.0)
            else {
                continue;
            };
            let Some((start_ms, observed_ms)) = window_bounds(provider, window, metric, now_ms)
            else {
                continue;
            };
            let cohort: Vec<_> = turns
                .iter()
                .filter(|turn| turn.provider == provider.provider)
                .filter(|turn| turn.at_ms >= start_ms && turn.at_ms <= observed_ms)
                .filter(|turn| window_applies(window, turn))
                .filter(|turn| {
                    normalized_account(&turn.account, &provider_accounts) == Some(account.clone())
                })
                .collect();
            if cohort.is_empty() {
                continue;
            }
            // Exact price weights are the best available basis. Complete token
            // weights keep a partially priced cohort useful at lower confidence.
            let basis = if cohort.iter().all(|turn| turn.price_weight.is_some()) {
                WeightBasis::Price
            } else {
                WeightBasis::Tokens
            };
            let samples = history.samples_for(
                &provider.provider,
                provider.account_key.as_deref(),
                &window.id,
            );
            let (shares, uses_history) = distribute_window(
                &cohort,
                used_percent,
                start_ms,
                observed_ms,
                &samples,
                basis,
            );
            let tier = EstimateTier::new(basis, uses_history);
            for turn in &cohort {
                let Some(percent) = shares.get(&turn.key).copied() else {
                    continue;
                };
                if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
                    continue;
                }
                let candidate = Candidate {
                    tier,
                    allocation: SessionLimitAllocation {
                        agent: turn.key.agent.clone(),
                        session_id: turn.key.session_id.clone(),
                        wsl_distro: turn.wsl_distro.clone(),
                        metric,
                        provider: provider.provider.clone(),
                        display_name: provider.display_name.clone(),
                        account_key: account.clone(),
                        window_id: window.id.clone(),
                        resets_at: resets_at.clone(),
                        percent,
                    },
                };
                let key = (turn.key.clone(), metric);
                if best.get(&key).is_none_or(|current| {
                    candidate.tier > current.tier
                        || (candidate.tier == current.tier
                            && candidate.allocation.percent > current.allocation.percent)
                }) {
                    best.insert(key, candidate);
                }
            }
        }
    }
    let mut allocations: Vec<_> = best.into_values().map(|entry| entry.allocation).collect();
    allocations.sort_by(|left, right| {
        left.agent
            .cmp(&right.agent)
            .then_with(|| left.session_id.cmp(&right.session_id))
            .then_with(|| format!("{:?}", left.metric).cmp(&format!("{:?}", right.metric)))
    });
    allocations
}

#[cfg(test)]
mod tests;

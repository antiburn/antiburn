//! Learn the dollars-per-percent limit factor from meter readings and priced
//! turns.
//!
//! See `docs/plans/limit-factor-estimation.md` for the full design. This
//! module writes `store::provider_limit`'s samples and points. Nothing reads
//! them outside tests yet; the session badge still prices from the durable
//! allocator until phase 2.

use std::collections::BTreeMap;

use crate::store::Store;
use crate::store::provider_limit::{
    AttributedSessionDollars, FactorPoint, FactorSample, lane_duration_seconds,
    lane_for_window_role,
};
use crate::store::provider_usage_history::{ProviderUsageObservation, ProviderUsagePeriod};

/// Observation pairs one pass may create or recompute.
///
/// Matches `ledger::reconcile`'s own per-pass ceiling: a bounded amount of
/// durable work per tick, with the remainder picked up on the next one.
const MAX_OBSERVATION_PAIRS: usize = 64;

/// How recently a period must have been observed to enter a pass, and how
/// close to now a sample's `to_epoch` must be to stay open to recompute.
const RECOMPUTE_WINDOW_SECS: i64 = 900;

/// How far back the weighted median looks for delta samples.
const FACTOR_WINDOW_SECS: i64 = 14 * 86_400;

/// How much a newly computed factor must move before it earns a new point.
const FACTOR_CHANGE_THRESHOLD: f64 = 0.02;

/// A candidate factor: its value, method, the samples behind it, the plan it
/// carries, and the epoch it takes effect from — the `to_epoch` of the
/// newest sample it used, so a bootstrapped or backfilled point is dated by
/// the data it came from rather than by the moment it was computed.
type FactorEstimate = (
    f64,
    &'static str,
    usize,
    (Option<String>, Option<String>),
    i64,
);

/// Learn the dollars-per-percent factor from durable meter readings.
///
/// Bounded the same way `ledger::reconcile` is: one shared gate across store
/// clones skips a pass already running, and a fixed ceiling on observation
/// pairs caps the work. Call this wherever `ledger::reconcile` runs today.
pub fn learn(store: &Store, now_epoch: i64) {
    let Some(_in_flight) = store.try_begin_limit_factor_learn() else {
        return;
    };
    let recompute_since = now_epoch - RECOMPUTE_WINDOW_SECS;
    let Ok(periods) = store.provider_limit_candidate_periods(recompute_since) else {
        ::tracing::warn!(event = "limit_factor_candidate_periods_failed");
        return;
    };

    let mut pairs_used = 0usize;
    // `periods` is ordered by `last_observed_epoch` descending, so the first
    // period seen for a lane is already its current one.
    let mut current_period: BTreeMap<(String, String, &'static str), i64> = BTreeMap::new();
    for period in &periods {
        if pairs_used >= MAX_OBSERVATION_PAIRS {
            break;
        }
        let Some(lane) = lane_for_window_role(&period.window_role) else {
            continue;
        };
        let Ok(Some(history)) = store.provider_usage_period_history(period.id) else {
            continue;
        };
        let Ok(existing) = store.factor_samples_for_period(period.id) else {
            continue;
        };
        // Scope "a delta sample already exists" to the account's current
        // plan and tier, so a fresh plan can still seed its own window-start
        // sample instead of being blocked by an older plan's delta history.
        let (plan, plan_tier) = store
            .latest_observation_plan(&period.provider, &period.account_key, lane)
            .ok()
            .flatten()
            .unwrap_or((None, None));
        let has_delta_before = store
            .has_delta_factor_sample(
                &period.provider,
                &period.account_key,
                lane,
                plan.as_deref(),
                plan_tier.as_deref(),
            )
            .unwrap_or(true);
        let pass = SamplePass {
            existing: &existing,
            recompute_since,
            now_epoch,
            budget: MAX_OBSERVATION_PAIRS - pairs_used,
        };
        let (produced, complete) = build_period_samples(
            store,
            period,
            lane,
            &history.observations,
            has_delta_before,
            &pass,
        );
        pairs_used += produced;
        // A pass that stopped mid-period on its budget must not advance the
        // cursor: the period stays a candidate so the next pass finishes it.
        if complete {
            let _ = store.advance_learn_cursor(period.id, period.last_observed_epoch);
        }
        current_period
            .entry((period.provider.clone(), period.account_key.clone(), lane))
            .or_insert(period.id);
    }

    for ((provider, account_key, lane), period_id) in current_period {
        recompute_point(store, &provider, &account_key, lane, now_epoch);
        compute_residual(store, &provider, &account_key, lane, period_id, now_epoch);
    }
}

/// The state one call to [`build_period_samples`] needs beyond the period
/// itself: which pairs already have a sample, and how much work is left.
struct SamplePass<'a> {
    existing: &'a std::collections::HashMap<(i64, i64), FactorSample>,
    recompute_since: i64,
    now_epoch: i64,
    budget: usize,
}

/// Build delta and window-start samples for one period's observations.
///
/// Returns the number of samples this call created or recomputed, and
/// whether it considered every sample the period could yet yield. The
/// second is `false` only when the pass budget cut the delta-pair loop off
/// early; the caller must not advance that period's learn cursor then.
fn build_period_samples(
    store: &Store,
    period: &ProviderUsagePeriod,
    lane: &'static str,
    observations: &[ProviderUsageObservation],
    has_delta_before: bool,
    pass: &SamplePass<'_>,
) -> (usize, bool) {
    let authoritative: Vec<&ProviderUsageObservation> = observations
        .iter()
        .filter(|observation| observation.is_authoritative && observation.used_percent.is_some())
        .collect();
    let pairs = delta_pairs(&authoritative);
    let mut used = 0usize;
    let mut complete = true;

    // A window-start sample only ever forms while no delta sample exists yet
    // for this lane, and only for a period that cannot form one of its own:
    // one with a pair already has a real measurement and needs no fallback.
    let first_positive = authoritative.iter().find(|observation| {
        observation
            .used_percent
            .is_some_and(|percent| percent > 0.0)
    });
    if !has_delta_before
        && pairs.is_empty()
        && used < pass.budget
        && let Some(first_positive) = first_positive
        && let Some(window_start) = window_start_epoch(period, lane)
        && window_start < first_positive.observed_at_epoch
        && should_recompute(
            pass.existing,
            window_start,
            first_positive.observed_at_epoch,
            pass.recompute_since,
        )
        && let Ok(Some(dollars)) = store.attributed_turn_dollars_between(
            &period.provider,
            &period.account_key,
            window_start,
            first_positive.observed_at_epoch,
        )
    {
        let totals = sum_dollars(&dollars);
        let sample = FactorSample {
            provider: period.provider.clone(),
            account_key: period.account_key.clone(),
            lane: lane.to_string(),
            kind: "window_start".to_string(),
            period_id: Some(period.id),
            from_epoch: window_start,
            to_epoch: first_positive.observed_at_epoch,
            from_percent: 0.0,
            to_percent: first_positive.used_percent.unwrap_or(0.0),
            input_usd: totals.0,
            output_usd: totals.1,
            cache_read_usd: totals.2,
            cache_write_usd: totals.3,
            turn_count: totals.4,
            plan: first_positive.plan.clone(),
            plan_tier: first_positive.plan_tier.clone(),
            source_id: first_positive.source_id.clone(),
            computed_at_epoch: pass.now_epoch,
        };
        if store.upsert_factor_sample(&sample).is_ok() {
            used += 1;
        }
    }

    // Delta samples: adjacent readings inside the period, as `delta_pairs`
    // found them.
    for (base, observation) in pairs {
        if used >= pass.budget {
            complete = false;
            break;
        }
        let base_percent = base.used_percent.unwrap_or(f64::NAN);
        let percent = observation.used_percent.unwrap_or(f64::NAN);
        if !should_recompute(
            pass.existing,
            base.observed_at_epoch,
            observation.observed_at_epoch,
            pass.recompute_since,
        ) {
            continue;
        }
        let Ok(Some(dollars)) = store.attributed_turn_dollars_between(
            &period.provider,
            &period.account_key,
            base.observed_at_epoch,
            observation.observed_at_epoch,
        ) else {
            continue;
        };
        let totals = sum_dollars(&dollars);
        let attributed_total = totals.0 + totals.1 + totals.2 + totals.3;
        let kind = if attributed_total > 0.0 {
            "delta"
        } else {
            "unattributed"
        };
        let sample = FactorSample {
            provider: period.provider.clone(),
            account_key: period.account_key.clone(),
            lane: lane.to_string(),
            kind: kind.to_string(),
            period_id: Some(period.id),
            from_epoch: base.observed_at_epoch,
            to_epoch: observation.observed_at_epoch,
            from_percent: base_percent,
            to_percent: percent,
            input_usd: totals.0,
            output_usd: totals.1,
            cache_read_usd: totals.2,
            cache_write_usd: totals.3,
            turn_count: totals.4,
            plan: observation.plan.clone(),
            plan_tier: observation.plan_tier.clone(),
            source_id: observation.source_id.clone(),
            computed_at_epoch: pass.now_epoch,
        };
        if store.upsert_factor_sample(&sample).is_ok() {
            used += 1;
        }
    }
    (used, complete)
}

/// Pair adjacent readings inside one period into delta candidates.
///
/// A later reading strictly greater than the running baseline closes a pair
/// and becomes the new baseline. An equal reading merges into the baseline
/// without closing a pair, so a plateau spans the whole interval once it
/// finally rises. A drop restarts the baseline: it is not a valid delta.
fn delta_pairs<'a>(
    authoritative: &[&'a ProviderUsageObservation],
) -> Vec<(&'a ProviderUsageObservation, &'a ProviderUsageObservation)> {
    let mut pairs = Vec::new();
    let mut baseline: Option<&ProviderUsageObservation> = None;
    for &observation in authoritative {
        let Some(base) = baseline else {
            baseline = Some(observation);
            continue;
        };
        let base_percent = base.used_percent.unwrap_or(f64::NAN);
        let percent = observation.used_percent.unwrap_or(f64::NAN);
        match percent.partial_cmp(&base_percent) {
            Some(std::cmp::Ordering::Greater) => {
                pairs.push((base, observation));
                baseline = Some(observation);
            }
            Some(std::cmp::Ordering::Equal) => {
                // Merge: keep the original baseline in place.
            }
            _ => {
                // A drop mid-period is not a valid delta. Restart from here.
                baseline = Some(observation);
            }
        }
    }
    pairs
}

/// Whether a pair should be (re)computed: it is new, or its close is recent
/// enough that a late turn could still change it.
fn should_recompute(
    existing: &std::collections::HashMap<(i64, i64), FactorSample>,
    from_epoch: i64,
    to_epoch: i64,
    recompute_since: i64,
) -> bool {
    !existing.contains_key(&(from_epoch, to_epoch)) || to_epoch >= recompute_since
}

fn window_start_epoch(period: &ProviderUsagePeriod, lane: &str) -> Option<i64> {
    period.starts_at_epoch.or_else(|| {
        period
            .resets_at_epoch
            .map(|reset| reset - lane_duration_seconds(lane))
    })
}

fn sum_dollars(rows: &[AttributedSessionDollars]) -> (f64, f64, f64, f64, i64) {
    rows.iter()
        .fold((0.0, 0.0, 0.0, 0.0, 0), |mut totals, row| {
            totals.0 += row.input_usd;
            totals.1 += row.output_usd;
            totals.2 += row.cache_read_usd;
            totals.3 += row.cache_write_usd;
            totals.4 += row.turn_count;
            totals
        })
}

/// Recompute the factor for one account and lane, and append a point when it
/// moved enough, the method changed, or the plan changed.
fn recompute_point(store: &Store, provider: &str, account_key: &str, lane: &str, now_epoch: i64) {
    let current_point = store
        .latest_factor_point(provider, account_key, lane)
        .ok()
        .flatten();
    let latest_plan = store
        .latest_observation_plan(provider, account_key, lane)
        .ok()
        .flatten();
    let plan_changed = match (&current_point, &latest_plan) {
        (Some(point), Some((plan, plan_tier))) => {
            point.plan.as_deref() != plan.as_deref()
                || point.plan_tier.as_deref() != plan_tier.as_deref()
        }
        _ => false,
    };

    let recent = store
        .delta_factor_samples_since(provider, account_key, lane, now_epoch - FACTOR_WINDOW_SECS)
        .unwrap_or_default();
    let recent = filter_by_plan(recent, plan_changed, latest_plan.as_ref());

    let computed = if recent.len() >= 3 {
        newest_sample(&recent).map(|newest| {
            (
                weighted_median(&recent, now_epoch),
                "delta",
                recent.len(),
                (newest.plan.clone(), newest.plan_tier.clone()),
                newest.to_epoch,
            )
        })
    } else {
        let all = store
            .all_delta_factor_samples(provider, account_key, lane)
            .unwrap_or_default();
        let all = filter_by_plan(all, plan_changed, latest_plan.as_ref());
        if let Some(newest) = newest_sample(&all) {
            Some((
                weighted_median(&all, now_epoch),
                "delta",
                all.len(),
                (newest.plan.clone(), newest.plan_tier.clone()),
                newest.to_epoch,
            ))
        } else {
            window_start_factor(
                store,
                provider,
                account_key,
                lane,
                plan_changed,
                latest_plan.as_ref(),
            )
        }
    };

    let Some((value, method, sample_count, (plan, plan_tier), effective_at_epoch)) = computed
    else {
        return;
    };
    if !value.is_finite() || value <= 0.0 {
        return;
    }

    let should_append = match &current_point {
        None => true,
        Some(point) => {
            plan_changed
                || point.method != method
                || relative_change(point.usd_per_percent, value) > FACTOR_CHANGE_THRESHOLD
        }
    };
    if !should_append {
        return;
    }
    let _ = store.upsert_factor_point(&FactorPoint {
        id: 0,
        provider: provider.to_string(),
        account_key: account_key.to_string(),
        lane: lane.to_string(),
        effective_at_epoch,
        usd_per_percent: value,
        method: method.to_string(),
        sample_count: sample_count as i64,
        plan,
        plan_tier,
    });
}

fn window_start_factor(
    store: &Store,
    provider: &str,
    account_key: &str,
    lane: &str,
    plan_changed: bool,
    latest_plan: Option<&(Option<String>, Option<String>)>,
) -> Option<FactorEstimate> {
    let sample = store
        .latest_window_start_factor_sample(provider, account_key, lane)
        .ok()
        .flatten()?;
    if plan_changed
        && let Some((plan, plan_tier)) = latest_plan
        && (sample.plan.as_deref() != plan.as_deref()
            || sample.plan_tier.as_deref() != plan_tier.as_deref())
    {
        return None;
    }
    let delta = sample.percent_delta();
    if delta.is_nan() || delta <= 0.0 {
        return None;
    }
    Some((
        sample.total_usd() / delta,
        "window_start",
        1,
        (sample.plan.clone(), sample.plan_tier.clone()),
        sample.to_epoch,
    ))
}

fn relative_change(previous: f64, current: f64) -> f64 {
    if previous.abs() <= f64::EPSILON {
        return f64::INFINITY;
    }
    ((current - previous) / previous).abs()
}

fn filter_by_plan(
    samples: Vec<FactorSample>,
    plan_changed: bool,
    latest_plan: Option<&(Option<String>, Option<String>)>,
) -> Vec<FactorSample> {
    if !plan_changed {
        return samples;
    }
    let Some((plan, plan_tier)) = latest_plan else {
        return samples;
    };
    samples
        .into_iter()
        .filter(|sample| {
            sample.plan.as_deref() == plan.as_deref()
                && sample.plan_tier.as_deref() == plan_tier.as_deref()
        })
        .collect()
}

/// The sample with the latest `to_epoch`, whose plan and close date the
/// point they produce should carry.
fn newest_sample(samples: &[FactorSample]) -> Option<&FactorSample> {
    samples.iter().max_by_key(|sample| sample.to_epoch)
}

/// The weighted median of delta samples' dollars-per-percent values.
///
/// Weight is `percent_delta * 0.5^(age_days / 7)`. A single interval whose
/// dollars include outside use pulls a mean down for every later session;
/// the median ignores it as long as it is not the majority of the weight.
fn weighted_median(samples: &[FactorSample], now_epoch: i64) -> f64 {
    let mut values: Vec<(f64, f64)> = samples
        .iter()
        .filter_map(|sample| {
            let delta = sample.percent_delta();
            if delta.is_nan() || delta <= 0.0 {
                return None;
            }
            let value = sample.total_usd() / delta;
            let age_days = (now_epoch - sample.to_epoch).max(0) as f64 / 86_400.0;
            let weight = delta * 0.5_f64.powf(age_days / 7.0);
            (value.is_finite() && weight > 0.0).then_some((value, weight))
        })
        .collect();
    if values.is_empty() {
        return f64::NAN;
    }
    values.sort_by(|left, right| left.0.total_cmp(&right.0));
    let total_weight: f64 = values.iter().map(|(_, weight)| weight).sum();
    let half = total_weight / 2.0;
    let mut cumulative = 0.0;
    for (value, weight) in &values {
        cumulative += weight;
        if cumulative >= half {
            return *value;
        }
    }
    values.last().map(|(value, _)| *value).unwrap_or(f64::NAN)
}

/// For each account and lane touched this pass, the current period's residual:
/// meter percent against the factor's own estimate for the same span.
fn compute_residual(
    store: &Store,
    provider: &str,
    account_key: &str,
    lane: &str,
    period_id: i64,
    now_epoch: i64,
) {
    let Ok(Some(history)) = store.provider_usage_period_history(period_id) else {
        return;
    };
    let Some(latest) = history
        .observations
        .iter()
        .rev()
        .find(|observation| observation.is_authoritative && observation.used_percent.is_some())
    else {
        return;
    };
    // The point in effect when the meter took this reading, not necessarily
    // the newest point overall: a later plan change should not restate an
    // older period's residual under today's factor.
    let Ok(Some(point)) =
        store.factor_point_at(provider, account_key, lane, latest.observed_at_epoch)
    else {
        return;
    };
    if point.usd_per_percent.is_nan() || point.usd_per_percent <= 0.0 {
        return;
    }
    let Some(window_start) = window_start_epoch(&history.period, lane) else {
        return;
    };
    let Ok(Some(dollars)) = store.attributed_turn_dollars_between(
        provider,
        account_key,
        window_start,
        latest.observed_at_epoch,
    ) else {
        return;
    };
    let totals = sum_dollars(&dollars);
    let attributed_total = totals.0 + totals.1 + totals.2 + totals.3;
    let estimated_percent = attributed_total / point.usd_per_percent;
    let meter_percent = latest.used_percent.unwrap_or(0.0);
    let _ = store.upsert_limit_residual(period_id, now_epoch, meter_percent, estimated_percent);
}

#[cfg(test)]
mod tests;

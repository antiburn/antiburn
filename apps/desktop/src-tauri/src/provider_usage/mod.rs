// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Per-provider usage, derived from the sessions already on this machine.
//!
//! # Where the numbers come from
//!
//! Exactly one place: the local database's cached session analysis. Each
//! analyzed session carries a billable-token breakdown per model; this module
//! attributes those models to providers ([`providers`]), sums them over three
//! calendar windows, prices them against the engine's bundled catalog, and
//! reports how confident the result is.
//!
//! # Where the numbers do *not* come from
//!
//! No credential is read, no provider account API is called, no port is
//! discovered, no socket or loopback connection is opened, and no provider IPC
//! is used. This module's whole input is a `Vec` of rows the caller already
//! read out of the app's own SQLite file, plus a clock. That is what makes the
//! feature honest offline, and it is also its hard ceiling: a transcript
//! records what was *spent*, so an allowance, a percentage, a remaining
//! balance, and a reset time are all unavailable — and are therefore absent
//! from the payload rather than estimated into it.
//!
//! # An honest limitation, surfaced rather than hidden
//!
//! 1. **A session lands in one window: the one its last activity falls in.**
//!    The store keeps a per-session heartbeat, not a per-turn timeline, so a
//!    session that ran across midnight counts entirely in the day it last
//!    touched. The views say so.

pub mod live;
pub mod providers;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};

use antiburn_local::analysis::price_breakdown;
use antiburn_local::pricing::ModelTokens;
use time::{OffsetDateTime, Time, UtcOffset};

use crate::dto::{
    ProviderUsage, ProviderUsageStaleness, ProviderUsageState, ProviderUsageSummary,
    ProviderUsageWindow, ProviderUsageWindows,
};
use crate::store::{UsageEvidenceRecord, iso_from_epoch};

use providers::Route;

/// How old a provider's newest session may be before its totals are marked as
/// describing the past rather than the present.
///
/// A day, because that is the span the narrowest window covers: once the
/// newest evidence is older than that, every rolling figure on the surface is
/// necessarily zero and the reader deserves to know why.
pub const STALE_EVIDENCE_SECS: i64 = 24 * 60 * 60;

/// Widest offset from UTC any real time zone uses (+14:00 / −12:00, clamped
/// symmetrically). Guards the offset the webview supplies, which is input.
const MAX_OFFSET_MINUTES: i32 = 14 * 60;

/// The three windows, as unix-second lower bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowBounds {
    /// Local midnight today.
    pub today_start: i64,
    /// Local midnight six days before today — the trailing seven calendar days.
    pub week_start: i64,
    /// Local midnight on the first of the current month.
    pub month_start: i64,
}

impl WindowBounds {
    /// The earliest instant any window reaches back to.
    fn earliest(&self) -> i64 {
        self.week_start.min(self.month_start)
    }
}

/// The window bounds for `now`, in a reader whose clock is `utc_offset_minutes`
/// from UTC.
///
/// The offset is applied to past dates as well as today's, so a boundary that
/// falls on the far side of a daylight-saving change is off by that change —
/// an hour, once or twice a year, on a figure already labelled an estimate.
/// The alternative is a time-zone database, which is a large dependency for a
/// one-hour edge case.
pub fn window_bounds(now: i64, utc_offset_minutes: i32) -> WindowBounds {
    let offset = local_offset(utc_offset_minutes);
    let at = OffsetDateTime::from_unix_timestamp(now)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH)
        .to_offset(offset);
    let date = at.date();

    let today_start = date
        .with_time(Time::MIDNIGHT)
        .assume_offset(offset)
        .unix_timestamp();
    let month_start = date
        .replace_day(1)
        .unwrap_or(date)
        .with_time(Time::MIDNIGHT)
        .assume_offset(offset)
        .unix_timestamp();

    WindowBounds {
        today_start,
        // Today plus the six days before it. Anchored on local midnight rather
        // than on `now` so the week does not slide through the day, which
        // would make the same session appear and disappear as the clock moves.
        week_start: today_start - 6 * 86_400,
        month_start,
    }
}

/// The earliest heartbeat the aggregation needs to read.
///
/// Exposed so the caller can bound its query instead of loading every retained
/// session and discarding most of them.
pub fn lookback_start(now: i64, utc_offset_minutes: i32) -> i64 {
    window_bounds(now, utc_offset_minutes).earliest()
}

/// A validated [`UtcOffset`], falling back to UTC for anything implausible.
fn local_offset(minutes: i32) -> UtcOffset {
    let minutes = minutes.clamp(-MAX_OFFSET_MINUTES, MAX_OFFSET_MINUTES);
    UtcOffset::from_whole_seconds(minutes * 60).unwrap_or(UtcOffset::UTC)
}

/// Which window a timestamp belongs to. A session can be in more than one.
#[derive(Debug, Clone, Copy)]
struct Membership {
    today: bool,
    week: bool,
    month: bool,
}

impl Membership {
    fn of(epoch: i64, bounds: &WindowBounds) -> Membership {
        Membership {
            today: epoch >= bounds.today_start,
            week: epoch >= bounds.week_start,
            month: epoch >= bounds.month_start,
        }
    }

    /// True when the row falls outside every window, which makes it irrelevant
    /// — a session whose heartbeat is older than the widest bound, or missing
    /// entirely (stored as zero).
    fn is_empty(self) -> bool {
        !self.today && !self.week && !self.month
    }
}

/// Tokens and session count accumulating for one provider in one window.
///
/// Models are keyed in a `BTreeMap` rather than a `HashMap` so the pricing sum
/// below runs in a fixed order: floating-point addition is not associative, and
/// a total that changed in its last digit between runs would be a needless
/// source of flicker.
#[derive(Debug, Default)]
struct Bucket {
    models: BTreeMap<String, ModelTokens>,
    session_count: u32,
}

impl Bucket {
    fn add_tokens(&mut self, model: &str, tokens: &ModelTokens) {
        let entry = self.models.entry(model.to_string()).or_default();
        entry.input_tokens = entry.input_tokens.saturating_add(tokens.input_tokens);
        entry.output_tokens = entry.output_tokens.saturating_add(tokens.output_tokens);
        entry.cache_read_tokens = entry
            .cache_read_tokens
            .saturating_add(tokens.cache_read_tokens);
        entry.cache_creation_tokens = entry
            .cache_creation_tokens
            .saturating_add(tokens.cache_creation_tokens);
        entry.cache_creation_1h_tokens = entry
            .cache_creation_1h_tokens
            .saturating_add(tokens.cache_creation_1h_tokens);
    }
}

/// Everything accumulating for one provider.
#[derive(Debug, Default)]
struct Accumulator {
    today: Bucket,
    week: Bucket,
    month: Bucket,
    /// Every model seen in the covered span, used for the provider's state.
    /// Kept separately because the state describes the provider, not a window.
    all: Bucket,
    last_activity: Option<i64>,
}

/// What pricing could say about a set of models.
#[derive(Debug, Default, Clone, Copy)]
struct Priced {
    usd: f64,
    /// At least one model has a price in the bundled catalog.
    any_priced: bool,
    /// At least one model does not, so `usd` is a floor and not a total.
    any_unpriced: bool,
}

/// Price a bucket's models, one at a time.
///
/// The engine's `price_breakdown` refuses a partial total — one unpriceable
/// model and the whole subject returns `None`, which is right for a session's
/// headline figure. Across a month of many models that rule would blank the
/// entire surface for one unknown name, so this prices model by model and
/// reports the shortfall through [`Priced::any_unpriced`] instead. The view
/// turns that into "some models could not be priced", never into silence.
fn price(models: &BTreeMap<String, ModelTokens>) -> Priced {
    let mut priced = Priced::default();
    for (model, tokens) in models {
        let single = HashMap::from([(model.clone(), tokens.clone())]);
        match price_breakdown(&single) {
            Some(cost) => {
                priced.usd += cost.total_usd;
                priced.any_priced = true;
            }
            None => priced.any_unpriced = true,
        }
    }
    priced
}

/// Turn one bucket into its wire shape.
fn window_of(bucket: &Bucket) -> ProviderUsageWindow {
    let priced = price(&bucket.models);
    let mut window = ProviderUsageWindow {
        session_count: bucket.session_count,
        estimated_usd: priced.any_priced.then_some(priced.usd),
        ..ProviderUsageWindow::default()
    };
    for tokens in bucket.models.values() {
        // Fresh input plus cache writes, matching the engine's `tokens_in`:
        // both are prompt tokens the reader paid to send.
        window.tokens_in = window
            .tokens_in
            .saturating_add(tokens.input_tokens)
            .saturating_add(tokens.cache_creation_tokens);
        window.tokens_out = window.tokens_out.saturating_add(tokens.output_tokens);
        window.cache_read = window.cache_read.saturating_add(tokens.cache_read_tokens);
    }
    window
}

/// The state the covered evidence supports.
fn state_of(all: &Bucket) -> ProviderUsageState {
    if all.models.is_empty() {
        return ProviderUsageState::Unknown;
    }
    let priced = price(&all.models);
    if priced.any_unpriced || !priced.any_priced {
        ProviderUsageState::Observed
    } else {
        ProviderUsageState::Estimated
    }
}

/// Whether the newest evidence still describes now.
fn staleness_of(last_activity: Option<i64>, now: i64) -> ProviderUsageStaleness {
    match last_activity {
        Some(at) if now.saturating_sub(at) < STALE_EVIDENCE_SECS => ProviderUsageStaleness::Fresh,
        Some(_) => ProviderUsageStaleness::Stale,
        None => ProviderUsageStaleness::Unknown,
    }
}

/// The billable tokens one session recorded, per normalized model key.
///
/// Unparseable JSON reads as no evidence rather than as an error: a cache row
/// written by an older build is a state the surface already renders honestly.
fn breakdown_of(record: &UsageEvidenceRecord) -> BTreeMap<String, ModelTokens> {
    let Some(json) = record.model_breakdown_json.as_deref() else {
        return BTreeMap::new();
    };
    serde_json::from_str::<BTreeMap<String, ModelTokens>>(json)
        .unwrap_or_default()
        .into_iter()
        .filter(|(model, _)| !model.trim().is_empty())
        .collect()
}

/// Attribute one session's models to providers.
///
/// A fixed-route agent puts every model under its vendor; a bring-your-own
/// agent splits them, so one session can appear under two providers with the
/// tokens divided between them rather than double-counted.
fn attribute(
    agent: &str,
    breakdown: BTreeMap<String, ModelTokens>,
) -> BTreeMap<&'static str, BTreeMap<String, ModelTokens>> {
    let route = providers::route_for_agent(agent);
    let mut by_provider: BTreeMap<&'static str, BTreeMap<String, ModelTokens>> = BTreeMap::new();
    for (model, tokens) in breakdown {
        let provider = match route {
            Route::Fixed(provider) => provider,
            Route::ByModel => providers::provider_for_model(&model),
        };
        by_provider
            .entry(provider)
            .or_default()
            .insert(model, tokens);
    }
    by_provider
}

/// The provider an unanalyzed session belongs to, when that is knowable.
///
/// A fixed-route agent still names its vendor with no models at all — a Claude
/// Code session that has not been analyzed yet is unambiguously Anthropic's.
/// A bring-your-own session with no models names nothing, so it lands in the
/// unattributed bucket, which is the honest answer rather than a dropped row.
fn provider_without_models(agent: &str) -> &'static str {
    match providers::route_for_agent(agent) {
        Route::Fixed(provider) => provider,
        Route::ByModel => providers::UNKNOWN,
    }
}

/// Aggregate local session evidence into per-provider usage.
///
/// `rows` may hold sessions outside every window — the caller's query is
/// bounded by [`lookback_start`], which is a lower bound, not an exact filter —
/// and those are skipped here.
/// Estimated spend, in USD, across every provider for sessions whose
/// heartbeat falls in `[start, end)`.
///
/// The anomaly monitor's arithmetic, kept next to `summarize` because it
/// reuses the same attribution and per-model pricing. Same floor semantics as
/// the windows: models the catalog cannot price contribute nothing, so the
/// figure can only understate — which is the right direction for a number
/// that decides whether to interrupt someone.
pub fn spend_between(rows: &[UsageEvidenceRecord], start: i64, end: i64) -> f64 {
    let mut models: BTreeMap<String, ModelTokens> = BTreeMap::new();
    for record in rows {
        if record.updated_at_epoch < start || record.updated_at_epoch >= end {
            continue;
        }
        for (_, provider_models) in attribute(&record.agent, breakdown_of(record)) {
            for (model, tokens) in provider_models {
                let entry = models.entry(model).or_default();
                entry.input_tokens = entry.input_tokens.saturating_add(tokens.input_tokens);
                entry.output_tokens = entry.output_tokens.saturating_add(tokens.output_tokens);
                entry.cache_read_tokens = entry
                    .cache_read_tokens
                    .saturating_add(tokens.cache_read_tokens);
                entry.cache_creation_tokens = entry
                    .cache_creation_tokens
                    .saturating_add(tokens.cache_creation_tokens);
                entry.cache_creation_1h_tokens = entry
                    .cache_creation_1h_tokens
                    .saturating_add(tokens.cache_creation_1h_tokens);
            }
        }
    }
    price(&models).usd
}

pub fn summarize(
    rows: &[UsageEvidenceRecord],
    now: i64,
    utc_offset_minutes: i32,
) -> ProviderUsageSummary {
    let bounds = window_bounds(now, utc_offset_minutes);
    let mut accumulators: BTreeMap<&'static str, Accumulator> = BTreeMap::new();

    for record in rows {
        let membership = Membership::of(record.updated_at_epoch, &bounds);
        if membership.is_empty() {
            continue;
        }

        let breakdown = breakdown_of(record);
        let attributed = if breakdown.is_empty() {
            BTreeMap::from([(provider_without_models(&record.agent), BTreeMap::new())])
        } else {
            attribute(&record.agent, breakdown)
        };

        for (provider, models) in attributed {
            let accumulator = accumulators.entry(provider).or_default();
            accumulator.last_activity = Some(
                accumulator
                    .last_activity
                    .map_or(record.updated_at_epoch, |at| {
                        at.max(record.updated_at_epoch)
                    }),
            );

            let mut buckets: Vec<&mut Bucket> = vec![&mut accumulator.all];
            if membership.today {
                buckets.push(&mut accumulator.today);
            }
            if membership.week {
                buckets.push(&mut accumulator.week);
            }
            if membership.month {
                buckets.push(&mut accumulator.month);
            }
            for bucket in buckets {
                bucket.session_count = bucket.session_count.saturating_add(1);
                for (model, tokens) in &models {
                    bucket.add_tokens(model, tokens);
                }
            }
        }
    }

    let mut providers: Vec<ProviderUsage> = accumulators
        .into_iter()
        .map(|(provider, accumulator)| ProviderUsage {
            provider: provider.to_string(),
            display_name: providers::display_name(provider).to_string(),
            state: state_of(&accumulator.all),
            staleness: staleness_of(accumulator.last_activity, now),
            windows: ProviderUsageWindows {
                today: window_of(&accumulator.today),
                week: window_of(&accumulator.week),
                month: window_of(&accumulator.month),
            },
            last_activity_at: accumulator.last_activity.map(|at| iso_from_epoch(Some(at))),
        })
        .collect();

    // Most recently used first, with the id as the tie-break so the order is
    // total and a re-render never reshuffles equal rows. Ranking by size
    // belongs to the views, which know which window the reader is looking at.
    providers.sort_by(|a, b| {
        b.last_activity_at
            .cmp(&a.last_activity_at)
            .then_with(|| a.provider.cmp(&b.provider))
    });

    ProviderUsageSummary {
        providers,
        generated_at: iso_from_epoch(Some(now)),
    }
}

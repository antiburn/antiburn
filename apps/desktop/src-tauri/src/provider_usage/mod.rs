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
//! # Its ceiling, and why
//!
//! This module's whole input is a `Vec` of rows the caller already read out
//! of the app's own SQLite file, plus a clock: it derives spend purely from
//! local transcript rows. It needs no provider figure, no credential, and no
//! call of any kind, because a transcript already records what was *spent*.
//! That is also its hard ceiling: an allowance, a percentage, a remaining
//! balance, and a reset time are none of them things a transcript states, so
//! they are absent from the payload rather than estimated into it.
//!
//! # An honest limitation, surfaced rather than hidden
//!
//! 1. **A session lands in one window: the one its last activity falls in.**
//!    The store keeps a per-session activity timestamp, not a per-turn timeline, so a
//!    session that ran across midnight counts entirely in the day it last
//!    touched. The views say so.

pub(crate) mod allocation;
pub mod live;
pub mod providers;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};

use antiburn_local::analysis::price_breakdown;
use antiburn_local::pricing::ModelTokens;
use time::{OffsetDateTime, Time, UtcOffset};

use crate::dto::{
    ProviderAgentUsage, ProviderUsage, ProviderUsageStaleness, ProviderUsageState,
    ProviderUsageSummary, ProviderUsageWindow, ProviderUsageWindows,
};
use crate::store::{UsageEvidenceRecord, iso_from_epoch};

use antiburn_local::analysis::ProviderHint;
use providers::{HintResolution, Route};
use serde::Deserialize;

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
    /// Local midnight twenty-nine days before today.
    pub last_30_days_start: i64,
}

impl WindowBounds {
    /// The earliest instant any window reaches back to.
    fn earliest(&self) -> i64 {
        self.week_start
            .min(self.month_start)
            .min(self.last_30_days_start)
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
        last_30_days_start: today_start - 29 * 86_400,
    }
}

/// The earliest activity timestamp the aggregation needs to read.
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
    last_30_days: bool,
}

impl Membership {
    fn of(epoch: i64, bounds: &WindowBounds) -> Membership {
        Membership {
            today: epoch >= bounds.today_start,
            week: epoch >= bounds.week_start,
            month: epoch >= bounds.month_start,
            last_30_days: epoch >= bounds.last_30_days_start,
        }
    }

    /// True when the row falls outside every window, which makes it irrelevant
    /// — a session whose activity is older than the widest bound, or missing
    /// entirely (stored as zero).
    fn is_empty(self) -> bool {
        !self.today && !self.week && !self.month && !self.last_30_days
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
    month_to_date: Bucket,
    last_30_days: Bucket,
    /// Every model seen in the covered span, used for the provider's state.
    /// Kept separately because the state describes the provider, not a window.
    all: Bucket,
    last_activity: Option<i64>,
    explicit_provider_detected: bool,
    agents: BTreeMap<String, AgentBuckets>,
}

#[derive(Debug, Default)]
struct AgentBuckets {
    today: Bucket,
    week: Bucket,
    month_to_date: Bucket,
    last_30_days: Bucket,
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
        cost_complete: !priced.any_unpriced,
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
fn state_of(accumulator: &Accumulator) -> ProviderUsageState {
    let has_nonzero_tokens = accumulator.all.models.values().any(has_tokens);
    if !has_nonzero_tokens {
        return if accumulator.explicit_provider_detected {
            ProviderUsageState::Detected
        } else {
            ProviderUsageState::Unknown
        };
    }
    let priced = price(&accumulator.all.models);
    if priced.any_unpriced || !priced.any_priced {
        ProviderUsageState::Observed
    } else {
        ProviderUsageState::Estimated
    }
}

fn has_tokens(tokens: &ModelTokens) -> bool {
    tokens.input_tokens > 0
        || tokens.output_tokens > 0
        || tokens.cache_read_tokens > 0
        || tokens.cache_creation_tokens > 0
        || tokens.cache_creation_1h_tokens > 0
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

fn provider_hints_of(record: &UsageEvidenceRecord) -> Vec<ProviderHint> {
    record
        .provider_hints_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderAccountObservation {
    provider: String,
    account_key: String,
}

fn account_for(record: &UsageEvidenceRecord, provider: &str) -> Option<String> {
    let observations: Vec<ProviderAccountObservation> =
        serde_json::from_str(&record.provider_accounts_json).unwrap_or_default();
    let mut accounts = observations
        .into_iter()
        .filter(|observation| observation.provider == provider)
        .map(|observation| observation.account_key)
        .filter(|account| account.len() == 64);
    let first = accounts.next()?;
    accounts.all(|account| account == first).then_some(first)
}

#[derive(Debug, Default)]
struct Attributed {
    models: BTreeMap<String, ModelTokens>,
    explicit: bool,
}

fn hints_for_model<'a>(model: &str, hints: &'a [ProviderHint]) -> Vec<&'a ProviderHint> {
    let exact: Vec<_> = hints
        .iter()
        .filter(|hint| {
            hint.model
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case(model))
        })
        .collect();
    if exact.is_empty() {
        hints.iter().filter(|hint| hint.model.is_none()).collect()
    } else {
        exact
    }
}

fn explicit_providers<'a>(hints: impl IntoIterator<Item = &'a ProviderHint>) -> Vec<&'static str> {
    let mut providers = Vec::new();
    for hint in hints {
        let provider = match providers::provider_for_hint(&hint.provider) {
            HintResolution::Known(provider) => provider,
            HintResolution::UnknownExplicit => providers::UNKNOWN,
        };
        if !providers.contains(&provider) {
            providers.push(provider);
        }
    }
    providers
}

/// Attribute one session's models to providers.
///
/// A fixed-route agent puts every model under its vendor; a bring-your-own
/// agent splits them, so one session can appear under two providers with the
/// tokens divided between them rather than double-counted.
fn attribute(
    agent: &str,
    breakdown: BTreeMap<String, ModelTokens>,
    hints: &[ProviderHint],
) -> BTreeMap<&'static str, Attributed> {
    let route = providers::route_for_agent(agent);
    let mut by_provider: BTreeMap<&'static str, Attributed> = BTreeMap::new();
    for (model, tokens) in breakdown {
        let provider = match route {
            Route::Fixed(provider) => provider,
            Route::ByModel => {
                let explicit = explicit_providers(hints_for_model(&model, hints));
                if explicit.len() == 1 {
                    explicit[0]
                } else if explicit.len() > 1 {
                    for provider in explicit {
                        by_provider.entry(provider).or_default().explicit = true;
                    }
                    providers::UNKNOWN
                } else {
                    providers::provider_for_model(&model)
                }
            }
        };
        let attributed = by_provider.entry(provider).or_default();
        attributed.explicit |=
            matches!(route, Route::ByModel) && !hints_for_model(&model, hints).is_empty();
        attributed.models.insert(model, tokens);
    }
    if matches!(route, Route::ByModel) {
        for provider in explicit_providers(hints) {
            by_provider.entry(provider).or_default().explicit = true;
        }
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
/// `rows` may hold sessions outside every window. The caller's query uses
/// [`lookback_start`] as a lower bound, so this function skips other rows.
pub fn summarize(
    rows: &[UsageEvidenceRecord],
    now: i64,
    utc_offset_minutes: i32,
) -> ProviderUsageSummary {
    let bounds = window_bounds(now, utc_offset_minutes);
    let mut accumulators: BTreeMap<(&'static str, Option<String>), Accumulator> = BTreeMap::new();

    for record in rows {
        let membership = Membership::of(record.updated_at_epoch, &bounds);
        if membership.is_empty() {
            continue;
        }

        let breakdown = breakdown_of(record);
        let hints = provider_hints_of(record);
        let attributed = if breakdown.is_empty() {
            match providers::route_for_agent(&record.agent) {
                Route::Fixed(provider) => BTreeMap::from([(
                    provider,
                    Attributed {
                        explicit: false,
                        ..Attributed::default()
                    },
                )]),
                Route::ByModel if hints.is_empty() => BTreeMap::from([(
                    provider_without_models(&record.agent),
                    Attributed::default(),
                )]),
                Route::ByModel => explicit_providers(&hints)
                    .into_iter()
                    .map(|provider| {
                        (
                            provider,
                            Attributed {
                                explicit: true,
                                ..Attributed::default()
                            },
                        )
                    })
                    .collect(),
            }
        } else {
            attribute(&record.agent, breakdown, &hints)
        };

        for (provider, attributed) in attributed {
            let account_key = account_for(record, provider);
            let accumulator = accumulators.entry((provider, account_key)).or_default();
            accumulator.explicit_provider_detected |= attributed.explicit;
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
                buckets.push(&mut accumulator.month_to_date);
            }
            if membership.last_30_days {
                buckets.push(&mut accumulator.last_30_days);
            }
            for bucket in buckets {
                bucket.session_count = bucket.session_count.saturating_add(1);
                for (model, tokens) in &attributed.models {
                    bucket.add_tokens(model, tokens);
                }
            }

            let agent = accumulator.agents.entry(record.agent.clone()).or_default();
            let mut agent_buckets: Vec<&mut Bucket> = Vec::new();
            if membership.today {
                agent_buckets.push(&mut agent.today);
            }
            if membership.week {
                agent_buckets.push(&mut agent.week);
            }
            if membership.month {
                agent_buckets.push(&mut agent.month_to_date);
            }
            if membership.last_30_days {
                agent_buckets.push(&mut agent.last_30_days);
            }
            for bucket in agent_buckets {
                bucket.session_count = bucket.session_count.saturating_add(1);
                for (model, tokens) in &attributed.models {
                    bucket.add_tokens(model, tokens);
                }
            }
        }
    }

    let mut providers: Vec<ProviderUsage> = accumulators
        .into_iter()
        .map(|((provider, account_key), accumulator)| ProviderUsage {
            provider: provider.to_string(),
            account_key,
            display_name: providers::display_name(provider).to_string(),
            state: state_of(&accumulator),
            staleness: staleness_of(accumulator.last_activity, now),
            windows: ProviderUsageWindows {
                today: window_of(&accumulator.today),
                week: window_of(&accumulator.week),
                month_to_date: window_of(&accumulator.month_to_date),
                last_30_days: window_of(&accumulator.last_30_days),
            },
            agents: accumulator
                .agents
                .into_iter()
                .map(|(agent, buckets)| ProviderAgentUsage {
                    agent,
                    windows: ProviderUsageWindows {
                        today: window_of(&buckets.today),
                        week: window_of(&buckets.week),
                        month_to_date: window_of(&buckets.month_to_date),
                        last_30_days: window_of(&buckets.last_30_days),
                    },
                })
                .collect(),
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
            .then_with(|| a.account_key.cmp(&b.account_key))
    });

    let totals = providers
        .iter()
        .fold(ProviderUsageWindows::default(), |mut totals, provider| {
            add_window(&mut totals.today, &provider.windows.today);
            add_window(&mut totals.week, &provider.windows.week);
            add_window(&mut totals.month_to_date, &provider.windows.month_to_date);
            add_window(&mut totals.last_30_days, &provider.windows.last_30_days);
            totals
        });
    let mut agents: BTreeMap<String, ProviderUsageWindows> = BTreeMap::new();
    for provider in &providers {
        for entry in &provider.agents {
            let windows = agents.entry(entry.agent.clone()).or_default();
            add_window(&mut windows.today, &entry.windows.today);
            add_window(&mut windows.week, &entry.windows.week);
            add_window(&mut windows.month_to_date, &entry.windows.month_to_date);
            add_window(&mut windows.last_30_days, &entry.windows.last_30_days);
        }
    }

    tracing::debug!(
        providers = providers.len(),
        agents = agents.len(),
        today_tokens = totals.today.tokens_in + totals.today.tokens_out + totals.today.cache_read,
        last_30_day_tokens = totals.last_30_days.tokens_in
            + totals.last_30_days.tokens_out
            + totals.last_30_days.cache_read,
        "computed local usage totals"
    );

    ProviderUsageSummary {
        providers,
        totals,
        agents: agents
            .into_iter()
            .map(|(agent, windows)| ProviderAgentUsage { agent, windows })
            .collect(),
        generated_at: iso_from_epoch(Some(now)),
    }
}

fn add_window(target: &mut ProviderUsageWindow, source: &ProviderUsageWindow) {
    target.tokens_in = target.tokens_in.saturating_add(source.tokens_in);
    target.tokens_out = target.tokens_out.saturating_add(source.tokens_out);
    target.cache_read = target.cache_read.saturating_add(source.cache_read);
    target.session_count = target.session_count.saturating_add(source.session_count);
    target.cost_complete &= source.cost_complete;
    target.estimated_usd = match (target.estimated_usd, source.estimated_usd) {
        (None, None) => None,
        (left, right) => Some(left.unwrap_or(0.0) + right.unwrap_or(0.0)),
    };
}

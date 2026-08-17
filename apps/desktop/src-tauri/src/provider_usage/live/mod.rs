// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Provider-reported usage limits: the half of the picture local evidence
//! cannot supply.
//!
//! # The division of labour with [`super`]
//!
//! The parent module answers *what was spent*, by pricing tokens the sessions
//! on this machine recorded. It can never answer *what remains*, because a
//! transcript has no denominator in it. This module holds the other answer,
//! and it only ever repeats what a provider itself stated: a percentage of an
//! allowance, and the moment that allowance resets.
//!
//! The two never merge. They travel over separate IPC commands
//! ([`crate::commands::get_provider_usage`] and
//! [`crate::commands::get_live_usage`]) so that the estimate payload's
//! structural guarantee — no percentage, no allowance, no reset, enforced by
//! its own test — stays provable no matter what this module does. The views
//! layer them; the shell does not.
//!
//! # Where the numbers come from
//!
//! [`LiveUsageSource`] implementations, registered at startup. By default,
//! [`sources::local_cache`] reads a file another application already wrote —
//! no service of ours involved, no child process — and it works with the
//! machine disconnected, because the figures were fetched by the agent the
//! last time *it* was online, and every window says how old it is rather
//! than pretending to be current. One opt-in setting also registers
//! [`sources::cli_refresh`], which runs the agent itself so that file is
//! newer than whenever the reader last happened to use it — see that module
//! for what running it does, and does not, do.
//!
//! # Fail closed, everywhere
//!
//! A percentage outside `0..=100`, a non-finite number, a timestamp that will
//! not parse — each rejects the whole snapshot rather than contributing a
//! clamped or partial one. A limit surface that is wrong is worse than a
//! limit surface that is absent, because the reader cannot tell which they
//! are looking at.

pub mod anthropic;
pub mod history;
pub mod metrics;
pub mod milestones;
pub mod model;
pub mod normalize;
pub mod sources;

#[cfg(test)]
mod tests;

pub use milestones::{MilestoneContent, MilestoneLedger, milestone_content};
pub use model::{
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, SupportTier, UsageScope,
    UsageWindow, UsageWindowKind, WindowRole,
};

use crate::dto::{
    LiveExtraUsage, LiveProviderUsage, LiveUsageForecast, LiveUsageFreshness, LiveUsageSourceError,
    LiveUsageSummary, LiveUsageSupport, LiveUsageWindow,
};

/// A source of provider-reported usage snapshots.
///
/// Implementations either read a local artefact another application wrote, or
/// — under the D-023 network policy, per-feature opt-in and default off —
/// call a provider endpoint with a credential the reader supplied. Never a
/// private-app endpoint.
///
/// `fetch` returns what the source can prove right now. An empty vector means
/// "nothing to say", which is a normal state and not an error; a source that
/// failed reports that through [`SourceOutcome`] instead, so the monitor can
/// tell "no account here" from "the account is there and we could not read
/// it".
pub trait LiveUsageSource: Send + Sync {
    /// A stable id for this source, used in the error surface and in logs.
    fn id(&self) -> &'static str;

    /// Whether this source may only run behind the online opt-in.
    ///
    /// Declared by the source rather than known by the caller, so adding one
    /// cannot accidentally leave it ungated: a source that causes any traffic
    /// — directly or through a child process — says so here, and
    /// [`sources::collect`] never calls it while the preference is off. The
    /// default is `false`, which is correct for a source that only reads a
    /// file already on the disk.
    fn requires_online_opt_in(&self) -> bool {
        false
    }

    /// Collect whatever this source can currently prove.
    fn fetch(&self) -> SourceOutcome;
}

/// One source's answer for one collection pass.
#[derive(Debug, Default)]
pub struct SourceOutcome {
    /// Snapshots the source could prove. May be empty.
    pub snapshots: Vec<ProviderUsageSnapshot>,
    /// Why the source produced nothing, when the reason is worth surfacing.
    ///
    /// `None` covers both success and the ordinary "this provider is not
    /// configured on this machine" case. Only a genuine failure — an
    /// unreadable file, a rejected credential — belongs here, because every
    /// value in it becomes a line on the reader's screen.
    pub error: Option<ProviderUsageError>,
}

impl SourceOutcome {
    /// A successful pass.
    pub fn found(snapshots: Vec<ProviderUsageSnapshot>) -> SourceOutcome {
        SourceOutcome {
            snapshots,
            error: None,
        }
    }

    /// A pass with nothing to report and nothing wrong.
    pub fn absent() -> SourceOutcome {
        SourceOutcome::default()
    }

    /// A pass that failed in a way the reader should be told about.
    pub fn failed(error: ProviderUsageError) -> SourceOutcome {
        SourceOutcome {
            snapshots: Vec::new(),
            error: Some(error),
        }
    }
}

/// Collect from every registered source, fold in what history says, and shape
/// the result for the views.
///
/// `store` is where the reading series live: every pass appends whatever is
/// new before computing, so a forecast always sees the reading it is about.
/// Passing `None` skips both — used by tests that are only exercising the
/// shaping, and by the fallback path where no store is mounted.
///
/// The ordering is by provider id so a re-render never reshuffles equal rows;
/// within a provider, windows keep the order the parser found them, which is
/// the provider's own order — shortest window first in practice, and not
/// something to second-guess.
pub fn summarize(
    sources: &[Box<dyn LiveUsageSource>],
    store: Option<&crate::store::Store>,
    now: i64,
    utc_offset_minutes: i32,
) -> LiveUsageSummary {
    // Read fresh, and default to *not* acting: an unreadable preference is not
    // permission, the same rule every notifier in this app follows.
    let online = store
        .and_then(|store| store.settings().ok())
        .is_some_and(|settings| settings.live_usage_enabled);
    let collected = sources::collect(sources, online);
    let history = store
        .map(|store| history::record(store, &collected.snapshots))
        .unwrap_or_default();
    let midnight = local_midnight(now, utc_offset_minutes);
    let at =
        time::OffsetDateTime::from_unix_timestamp(now).unwrap_or(time::OffsetDateTime::UNIX_EPOCH);

    let mut providers: Vec<LiveProviderUsage> = collected
        .snapshots
        .into_iter()
        .map(|snapshot| LiveProviderUsage {
            display_name: super::providers::display_name(snapshot.provider).to_string(),
            provider: snapshot.provider.to_string(),
            support: match snapshot.support {
                SupportTier::Live => LiveUsageSupport::Live,
                SupportTier::Estimated => LiveUsageSupport::Estimated,
                SupportTier::Observed => LiveUsageSupport::Observed,
                SupportTier::Detected => LiveUsageSupport::Detected,
            },
            freshness: match snapshot.source.freshness {
                Freshness::Fresh => LiveUsageFreshness::Fresh,
                Freshness::Stale => LiveUsageFreshness::Stale,
            },
            source_label: snapshot.source.label.clone(),
            observed_at: iso(snapshot.observed_at),
            windows: snapshot
                .windows
                .iter()
                .map(|entry| {
                    let samples = history.samples(&history::window_key(&snapshot, &entry.id));
                    window(entry.clone(), &samples, &snapshot, at, midnight)
                })
                .collect(),
            extra_usage: snapshot.supplemental.clone().map(|extra| LiveExtraUsage {
                enabled: extra.enabled,
                used_percent: extra.used_percent,
                used: extra.balance.as_ref().and_then(|balance| balance.used),
                remaining: extra.balance.as_ref().and_then(|balance| balance.remaining),
                limit: extra.balance.as_ref().and_then(|balance| balance.limit),
                currency: extra
                    .balance
                    .as_ref()
                    .and_then(|balance| balance.currency.clone()),
            }),
        })
        .collect();
    providers.sort_by(|a, b| a.provider.cmp(&b.provider));

    LiveUsageSummary {
        providers,
        errors: collected
            .errors
            .into_iter()
            .map(|(source, error)| LiveUsageSourceError {
                source: source.to_string(),
                category: error.category().to_string(),
            })
            .collect(),
        generated_at: crate::store::iso_from_epoch(Some(now)),
    }
}

fn window(
    window: UsageWindow,
    samples: &[metrics::UsageSample],
    snapshot: &ProviderUsageSnapshot,
    now: time::OffsetDateTime,
    local_midnight: time::OffsetDateTime,
) -> LiveUsageWindow {
    let forecast = metrics::usage_forecast(
        samples,
        window.used_percent,
        window.resets_at,
        now,
        snapshot.source.freshness,
        snapshot.source.confidence,
    );
    // Only on a window longer than a day. "How much of your five-hour limit
    // you used today" is a question about a period that has already rolled
    // several times, and the answer would be nonsense dressed as a metric.
    let used_today = matches!(
        window.kind,
        UsageWindowKind::Weekly | UsageWindowKind::Monthly | UsageWindowKind::BillingCycle
    )
    .then(|| metrics::used_since(samples, local_midnight))
    .flatten();

    LiveUsageWindow {
        id: window.id,
        role: match window.role {
            WindowRole::PrimaryShort => "primaryShort".to_string(),
            WindowRole::PrimaryLong => "primaryLong".to_string(),
            WindowRole::Supplemental => "supplemental".to_string(),
            WindowRole::Other(other) => other,
        },
        kind: match window.kind {
            UsageWindowKind::Rolling => "rolling".to_string(),
            UsageWindowKind::Daily => "daily".to_string(),
            UsageWindowKind::Weekly => "weekly".to_string(),
            UsageWindowKind::Monthly => "monthly".to_string(),
            UsageWindowKind::BillingCycle => "billingCycle".to_string(),
            UsageWindowKind::Other(other) => other,
        },
        scope_model: match window.scope {
            UsageScope::Model(model) => Some(model),
            UsageScope::ModelGroup(group) => Some(group),
            UsageScope::Account | UsageScope::Other(_) => None,
        },
        used_percent: window.used_percent,
        starts_at: window.starts_at.map(iso),
        resets_at: window.resets_at.map(iso),
        forecast: LiveUsageForecast {
            unavailable_reason: forecast
                .unavailable_reason
                .map(|reason| reason.as_str().to_string()),
            confidence: forecast.confidence.map(|confidence| {
                match confidence {
                    Confidence::Low => "low",
                    Confidence::Medium => "medium",
                    Confidence::High => "high",
                }
                .to_string()
            }),
            consumption_rate: forecast.consumption_rate,
            pace_ratio: forecast.pace_ratio.filter(|ratio| ratio.is_finite()),
            pace_trend: forecast.pace_trend,
            runway_at: forecast.estimated_exhaustion_at.map(iso),
            used_today,
        },
    }
}

/// The reader's most recent local midnight, as an instant.
///
/// Their calendar day, not UTC's: "used today" is a claim about the day the
/// reader has been having. The offset comes from the webview for the same
/// reason it does on the estimate command — resolving a local offset inside a
/// multi-threaded process is not reliable on every platform.
fn local_midnight(now: i64, utc_offset_minutes: i32) -> time::OffsetDateTime {
    let start = super::window_bounds(now, utc_offset_minutes).today_start;
    time::OffsetDateTime::from_unix_timestamp(start).unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

fn iso(at: time::OffsetDateTime) -> String {
    crate::store::iso_from_epoch(Some(at.unix_timestamp()))
}

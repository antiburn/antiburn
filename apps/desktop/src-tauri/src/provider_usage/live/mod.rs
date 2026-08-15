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
//! [`LiveUsageSource`] implementations, registered at startup. The one that
//! ships today reads a file another application already wrote
//! ([`sources::local_cache`]): no socket, no credential, no child process. It
//! works with the machine disconnected, because the figures were fetched by
//! the agent the last time *it* was online, and every window says how old it
//! is rather than pretending to be current.
//!
//! # Fail closed, everywhere
//!
//! A percentage outside `0..=100`, a non-finite number, a timestamp that will
//! not parse — each rejects the whole snapshot rather than contributing a
//! clamped or partial one. A limit surface that is wrong is worse than a
//! limit surface that is absent, because the reader cannot tell which they
//! are looking at.

pub mod anthropic;
pub mod milestones;
pub mod model;
pub mod normalize;
pub mod sources;

#[cfg(test)]
mod tests;

pub use milestones::{MilestoneContent, MilestoneLedger, milestone_content};
pub use model::{
    Freshness, ProviderUsageError, ProviderUsageSnapshot, SupportTier, UsageScope, UsageWindow,
    UsageWindowKind, WindowRole,
};

use crate::dto::{
    LiveExtraUsage, LiveProviderUsage, LiveUsageFreshness, LiveUsageSourceError, LiveUsageSummary,
    LiveUsageSupport, LiveUsageWindow,
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

/// Collect from every registered source and shape the result for the views.
///
/// The ordering is by provider id so a re-render never reshuffles equal rows;
/// within a provider, windows keep the order the parser found them, which is
/// the provider's own order — shortest window first in practice, and not
/// something to second-guess.
pub fn summarize(sources: &[Box<dyn LiveUsageSource>], now: i64) -> LiveUsageSummary {
    let collected = sources::collect(sources);

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
            source_label: snapshot.source.label,
            observed_at: iso(snapshot.observed_at),
            windows: snapshot.windows.into_iter().map(window).collect(),
            extra_usage: snapshot.supplemental.map(|extra| LiveExtraUsage {
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

fn window(window: UsageWindow) -> LiveUsageWindow {
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
    }
}

fn iso(at: time::OffsetDateTime) -> String {
    crate::store::iso_from_epoch(Some(at.unix_timestamp()))
}

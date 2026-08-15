// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The registered sources of provider-reported usage, and the rule for
//! picking between them.
//!
//! Every source in this module is independently authored for this
//! application. The private app's collectors, credential capture, loopback
//! probes, and API clients are denied by `docs/oss/source-denylist.toml`
//! (rule `provider-usage`); what is shared with it is the domain model those
//! sources produce, which is a separate, allowlisted slice.
//!
//! # The ladder
//!
//! Sources are ranked, not merged. When two of them describe the same
//! provider account, the better one wins outright and the other is discarded
//! — a snapshot is one coherent reading of one moment, and splicing a fresh
//! five-hour window onto a stale weekly one produces a picture that was never
//! true at any instant.

pub mod local_cache;

use super::LiveUsageSource;
use super::model::{Freshness, ProviderUsageSnapshot, SupportTier};

/// Every source this build registers, in no particular order — ranking is
/// [`preferred`]'s job, not registration order's.
///
/// Only one source ships today. The list exists so that adding the opt-in
/// online sources is a push rather than a rewrite, and so the ranking rule
/// below is already exercised by tests before there is a second source to
/// exercise it in the wild.
pub fn registered() -> Vec<Box<dyn LiveUsageSource>> {
    vec![Box::new(local_cache::ClaudeLocalCache::new())]
}

/// Collect from every source and keep the best reading per provider account.
pub fn collect(sources: &[Box<dyn LiveUsageSource>]) -> Collected {
    let mut collected = Collected::default();
    for source in sources {
        let outcome = source.fetch();
        if let Some(error) = outcome.error {
            collected.errors.push((source.id(), error));
        }
        for snapshot in outcome.snapshots {
            collected.push(snapshot);
        }
    }
    collected
}

/// The result of one collection pass across every source.
#[derive(Debug, Default)]
pub struct Collected {
    /// One snapshot per provider account, best reading kept.
    pub snapshots: Vec<ProviderUsageSnapshot>,
    /// Failures worth telling the reader about, by source id.
    pub errors: Vec<(&'static str, super::model::ProviderUsageError)>,
}

impl Collected {
    fn push(&mut self, snapshot: ProviderUsageSnapshot) {
        let key = |snapshot: &ProviderUsageSnapshot| (snapshot.provider, snapshot.account.clone());
        match self
            .snapshots
            .iter_mut()
            .find(|existing| key(existing) == key(&snapshot))
        {
            Some(existing) => {
                if preferred(&snapshot, existing) {
                    *existing = snapshot;
                }
            }
            None => self.snapshots.push(snapshot),
        }
    }
}

/// Whether `candidate` is a better reading of the same account than `current`.
///
/// The order is support tier, then freshness, then observation time. Tier
/// leads because a stale figure the provider stated still beats a current
/// figure we modelled: the first is old news about the truth, the second is
/// fresh news about a guess.
fn preferred(candidate: &ProviderUsageSnapshot, current: &ProviderUsageSnapshot) -> bool {
    let rank = |tier: SupportTier| match tier {
        SupportTier::Live => 3,
        SupportTier::Estimated => 2,
        SupportTier::Observed => 1,
        SupportTier::Detected => 0,
    };
    let fresh = |freshness: Freshness| usize::from(freshness == Freshness::Fresh);

    (
        rank(candidate.support),
        fresh(candidate.source.freshness),
        candidate.observed_at,
    ) > (
        rank(current.support),
        fresh(current.source.freshness),
        current.observed_at,
    )
}

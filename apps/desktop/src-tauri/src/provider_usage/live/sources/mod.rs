//! The registered sources of provider-reported usage, and the rule for
//! picking between them.
//!
//! Each source uses the domain model in the parent module. Source-specific
//! credential access, probes, and API clients stay in this module.
//!
//! # What is registered
//!
//! [`anthropic_fetch`] asks Claude's own usage endpoint with the credential
//! the Claude CLI already keeps on this machine; [`codex_fetch`] does the
//! same for Codex, retrying once with a token it refreshes itself before
//! falling back to [`codex_app_server`] — the Codex CLI's own process, asked
//! over its own protocol — when neither attempt lands. [`antigravity_fetch`]
//! reads Antigravity's provider-owned access token and asks Google Code Assist
//! for the managed project and its four shared quota pools. If that path cannot
//! answer, [`antigravity_local`] probes bounded PID-owned loopback endpoints for
//! a running `agy` or Antigravity IDE language server. All are gated behind
//! [`super::LiveUsageSource::requires_online_opt_in`]: [`collect`] never
//! calls them unless its caller says live usage is active, which folds in
//! Settings → Usage's switch (on by default) *and* onboarding having
//! finished — see [`crate::store::AppSettings::live_usage_active`]. [`http`]
//! is the plumbing they share — one client, one response cap, one mapping
//! from an HTTP status to this module's error taxonomy — and [`cooldown`] is
//! the retry-and-last-good-reading contract both sources are built on.
//!
//! # Picking one reading
//!
//! Sources are ranked, not merged. When two of them describe the same
//! provider account, the better one wins outright and the other is discarded
//! — a snapshot is one coherent reading of one moment, and splicing a fresh
//! five-hour window onto a stale weekly one produces a picture that was never
//! true at any instant. In practice this build registers at most one source
//! per provider, so the rule's real job today is picking between a fresh
//! reading and a cached one — see [`preferred`].

pub mod anthropic_fetch;
pub mod antigravity_fetch;
mod antigravity_local;
mod codex_app_server;
pub mod codex_fetch;
mod cooldown;
pub(crate) mod http;

use std::time::Duration;

use super::LiveUsageSource;
use super::model::{Freshness, ProviderUsageSnapshot};

/// The stable id both [`codex_fetch`] and [`codex_app_server`] stamp their
/// snapshots with. One registered source, two ways of answering for it — the
/// reader sees one row in the source registry either way.
const CODEX_SOURCE_ID: &str = "codex-usage-fetch";
const ANTIGRAVITY_SOURCE_ID: &str = "antigravity-usage-fetch";

/// Every source this build registers, in no particular order — ranking is
/// [`preferred`]'s job, not registration order's.
pub fn registered() -> Vec<Box<dyn LiveUsageSource>> {
    vec![
        Box::new(anthropic_fetch::ClaudeDirectFetch::new()),
        Box::new(antigravity_fetch::AntigravityDirectFetch::new()),
        Box::new(codex_fetch::CodexDirectFetch::new()),
    ]
}

/// Collect from every permitted source and keep the best reading per account.
///
/// `online` is whether live usage is active right now — the reader's switch
/// *and* onboarding having finished, already folded together by the caller
/// (see [`crate::store::AppSettings::live_usage_active`]). A source that
/// declared [`LiveUsageSource::requires_online_opt_in`] is not merely ignored
/// while it is false — it is never called, so nothing it would do can happen.
///
/// `hidden` is the reader's per-provider opt-out — see
/// [`crate::store::AppSettings::live_usage_hidden_providers`]. A hidden
/// provider's source is skipped in the same way, and for the same reason: the
/// reader will not see the figure, so antiburn does not ask for it.
///
/// `max_age` is passed straight through to every source's
/// [`LiveUsageSource::fetch`] — see that method's doc for what it means. One
/// value for the whole pass, because a pass answers one question ("what do
/// the sources say right now, for this caller's freshness need") and every
/// source in it should be answering the same question.
pub fn collect(
    sources: &[Box<dyn LiveUsageSource>],
    online: bool,
    hidden: &crate::store::HiddenMeters,
    max_age: Duration,
) -> Collected {
    let mut collected = Collected::default();
    for source in sources {
        if source.requires_online_opt_in() && !online {
            continue;
        }
        // A hidden meter is not asked. The reader turned the meter off, so
        // there is no figure to show and no reason to spend the request.
        if hidden.contains(source.provider()) {
            continue;
        }
        let outcome = source.fetch(max_age);
        if let Some(error) = outcome.error {
            collected.errors.push(SourceFailure {
                source: source.id(),
                provider: source.provider(),
                error,
            });
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
    /// Failures worth telling the reader about.
    pub errors: Vec<SourceFailure>,
}

/// One source's failure, with the provider it answers for.
///
/// The provider travels with the error because a failed source produces no
/// snapshot: this is the only place the views can learn whose usage is
/// missing.
#[derive(Debug)]
pub struct SourceFailure {
    /// The failed source's stable id.
    pub source: &'static str,
    /// The canonical provider id the source answers for.
    pub provider: &'static str,
    pub error: super::model::ProviderUsageError,
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
/// Freshness leads, then observation time. Every registered source reports
/// provider-stated figures, so there is no second, speculative support tier
/// to rank ahead of recency.
fn preferred(candidate: &ProviderUsageSnapshot, current: &ProviderUsageSnapshot) -> bool {
    let fresh = |freshness: Freshness| usize::from(freshness == Freshness::Fresh);

    (fresh(candidate.source.freshness), candidate.observed_at)
        > (fresh(current.source.freshness), current.observed_at)
}

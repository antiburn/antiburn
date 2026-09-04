//! The value types a provider-reported usage snapshot is made of.
//!
//! Every type here is data, and the only behavior is classification.
//!
//! Two invariants carry the whole module:
//!
//! 1. **A percentage always means capacity *consumed*.** Not remaining. A
//!    presentation layer may show the complement, but nothing stored or
//!    calculated ever does, because one inverted number somewhere in the
//!    middle is the kind of bug that reads as plausible right up until it
//!    matters.
//! 2. **Freshness and confidence are independent.** A figure can be
//!    structurally trustworthy and six hours old; those are different
//!    questions and the surfaces answer them separately.

use time::{Duration, OffsetDateTime};

/// Epistemic strength of a fact, independent of how recently it was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Corroborated evidence, good enough for guarded calculations.
    Medium,
    /// Direct structured evidence from a trusted source.
    High,
}

/// Whether an observation is recent enough to describe the present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Within its source's age budget, or future-dated by clock skew.
    Fresh,
    /// Still useful as last-known data, but not as current evidence.
    Stale,
}

impl Freshness {
    /// Classify an observation's age against an inclusive maximum.
    ///
    /// A future-dated observation counts as fresh: two clocks disagreeing by
    /// a few seconds is ordinary, and treating that as staleness would blank
    /// a perfectly good figure. Freshness says nothing about
    /// [`Confidence`] — an old number can be an exact one.
    pub fn at(observed_at: OffsetDateTime, now: OffsetDateTime, max_age: Duration) -> Freshness {
        if now < observed_at || now - observed_at <= max_age {
            Freshness::Fresh
        } else {
            Freshness::Stale
        }
    }
}

/// Why a payload was rejected outright rather than partly believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaReason {
    /// The input was not valid JSON.
    InvalidJson,
    /// The envelope was missing, or belonged to a different message shape.
    MissingEnvelope,
    /// A fact required for safe interpretation was absent or mistyped.
    MissingRequiredField,
    /// A present value broke its finite, range, or timestamp invariant.
    InvalidValue,
}

/// The failures a source is expected to have.
///
/// Schema errors exist so that a half-understood payload becomes *no*
/// snapshot rather than an invented one. The four variants are also the
/// categories the settings surface groups errors by, which is why they are
/// this coarse: a reader can act on "sign in again" and on "try later", and
/// cannot act on a parser's opinion of byte 402.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderUsageError {
    /// The credential was rejected or is no longer accepted.
    Authentication,
    /// The provider asked us to slow down. Not proof of exhaustion.
    RateLimited,
    /// The payload could not be read without breaking a model invariant.
    Schema(SchemaReason),
    /// The file, process, or endpoint could not be reached at all.
    Unavailable,
}

impl ProviderUsageError {
    /// The coarse category the views group by, as a wire string.
    pub fn category(self) -> &'static str {
        match self {
            ProviderUsageError::Authentication => "authentication",
            ProviderUsageError::RateLimited => "rateLimited",
            ProviderUsageError::Schema(_) => "schema",
            ProviderUsageError::Unavailable => "unavailable",
        }
    }
}

/// How important a window is when several apply at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowRole {
    /// The shorter primary window — a rolling session limit.
    PrimaryShort,
    /// The longer primary window — a weekly or billing-period limit.
    PrimaryLong,
    /// A real secondary limit that must not displace primary capacity.
    Supplemental,
    /// A role the provider named that we decline to guess the meaning of.
    Other(String),
}

/// How an allowance resets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageWindowKind {
    /// A continuously moving interval, not a calendar boundary.
    Rolling,
    /// A weekly allowance, on the provider's own boundary.
    Weekly,
    /// Provider semantics retained without coercion.
    Other(String),
}

/// The population a figure is measured against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageScope {
    /// The authenticated account.
    Account,
    /// A single model, keeping the provider's own display name.
    Model(String),
}

/// One provider allowance.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    /// Stable id within the provider, used to join observations over time.
    pub id: String,
    /// Relative importance among concurrent limits.
    pub role: WindowRole,
    /// Reset behaviour.
    pub kind: UsageWindowKind,
    /// Account or model this limit covers.
    pub scope: UsageScope,
    /// Consumed capacity in `0..=100`. `None` stays unknown; it is never zero.
    pub used_percent: Option<f64>,
    /// Start of the current window, when the provider stated one.
    pub starts_at: Option<OffsetDateTime>,
    /// Next reset, when the provider stated one. Absence stays unknown.
    pub resets_at: Option<OffsetDateTime>,
    /// False when the figure was derived rather than stated. Milestones and
    /// determinate meters both require this to be true.
    pub authoritative: bool,
}

/// A provider-reported credit or monetary balance, separate from the
/// recurring windows above.
#[derive(Debug, Clone, PartialEq)]
pub struct CreditBalance {
    /// Amount consumed, when disclosed.
    pub used: Option<f64>,
    /// Amount still available, when disclosed or safely derivable.
    pub remaining: Option<f64>,
    /// The ceiling, when disclosed. Absence does not imply unlimited.
    pub limit: Option<f64>,
    /// ISO 4217 currency code for monetary amounts, when disclosed.
    pub currency: Option<String>,
}

/// Independent metered usage. Must never replace or downgrade primary quota.
#[derive(Debug, Clone, PartialEq)]
pub struct SupplementalUsage {
    /// Whether the account permits this path. False differs from missing.
    pub enabled: bool,
    /// Consumed supplemental allowance in `0..=100`.
    pub used_percent: Option<f64>,
    /// Raw monetary or credit facts, when disclosed.
    pub balance: Option<CreditBalance>,
}

/// Credits that let the reader reset provider rate limits.
#[derive(Debug, Clone, PartialEq)]
pub struct RateLimitResetCredits {
    /// Credits the provider says remain available.
    pub available_count: u64,
}

/// Where a snapshot's facts came from, and how far they can be trusted.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSource {
    /// The registered source's stable id.
    pub id: &'static str,
    /// A human-readable description. Must carry no account identifier.
    pub label: String,
    /// Structural trust, independent of age.
    pub confidence: Confidence,
    /// Age classification at collection time.
    pub freshness: Freshness,
}

/// One provider account's usage at one observation time.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsageSnapshot {
    /// Canonical provider id, matching the local-estimate path's ids so the
    /// views can join the two without a translation table.
    pub provider: &'static str,
    /// An account discriminator when the source knows one, so two accounts at
    /// one provider never merge. Local-only: nothing is uploaded, so this is
    /// the provider's own id rather than a hash of it.
    pub account: Option<String>,
    /// The provider's raw account UUID for local display, when known.
    pub account_uuid: Option<String>,
    /// The provider's account email for local display, when known.
    pub account_email: Option<String>,
    /// The plan, when the source stated one. Never inferred.
    pub plan: Option<String>,
    /// A finer-grained tier within `plan`, when the source stated one. For
    /// example Claude's `rateLimitTier`. Never inferred.
    pub plan_tier: Option<String>,
    /// When the underlying fact was observed — not when it was read.
    pub observed_at: OffsetDateTime,
    /// Provenance, confidence, and freshness.
    pub source: UsageSource,
    /// The provider's windows, in the order the parser found them.
    pub windows: Vec<UsageWindow>,
    /// Metered usage alongside the windows, when the provider reports it.
    pub supplemental: Option<SupplementalUsage>,
    /// The provider reports manual rate-limit resets here.
    pub reset_credits: Option<RateLimitResetCredits>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(seconds).expect("valid timestamp")
    }

    #[test]
    fn a_future_observation_is_fresh_because_clocks_disagree() {
        // Ten seconds in the future against a one-minute budget: skew, not
        // staleness. Blanking a good figure over this would be the worse bug.
        assert_eq!(
            Freshness::at(at(1_000_010), at(1_000_000), Duration::minutes(1)),
            Freshness::Fresh
        );
    }

    #[test]
    fn the_age_budget_is_inclusive_at_its_boundary() {
        assert_eq!(
            Freshness::at(at(1_000_000), at(1_000_060), Duration::minutes(1)),
            Freshness::Fresh
        );
        assert_eq!(
            Freshness::at(at(1_000_000), at(1_000_061), Duration::minutes(1)),
            Freshness::Stale
        );
    }

    #[test]
    fn every_error_maps_to_a_category_the_views_can_group_by() {
        assert_eq!(
            ProviderUsageError::Authentication.category(),
            "authentication"
        );
        assert_eq!(ProviderUsageError::RateLimited.category(), "rateLimited");
        assert_eq!(
            ProviderUsageError::Schema(SchemaReason::InvalidJson).category(),
            "schema"
        );
        assert_eq!(ProviderUsageError::Unavailable.category(), "unavailable");
    }
}

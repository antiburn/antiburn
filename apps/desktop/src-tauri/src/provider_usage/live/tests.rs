// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Behaviour that spans the live-usage modules: the parser against the shapes
//! the provider actually emits, the source ladder's ranking rule, and the
//! payload the views receive.
//!
//! Every fixture here is hand-authored. `CONTRIBUTING.md` forbids captured
//! machine output as a test input, and a real payload would carry a real
//! account identifier — so these match the observed *shape* with synthetic
//! ids and timestamps, and nothing else.

use time::OffsetDateTime;

use super::anthropic;
use super::model::{
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, SchemaReason, SupportTier,
    UsageScope, UsageSource, UsageWindowKind, WindowRole,
};
use super::{LiveUsageSource, SourceOutcome, sources, summarize};

const NOW: i64 = 1_800_000_000;

fn at(seconds: i64) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(seconds).expect("valid timestamp")
}

/* -------------------------------------------------------------------------
 * The parser
 * ---------------------------------------------------------------------- */

const LIMITS_SHAPE: &str = r#"{
  "limits": [
    {"kind": "session", "group": "session", "percent": 81,
     "severity": "warning", "resets_at": "2027-01-15T02:19:59.917502+00:00",
     "scope": null, "is_active": false},
    {"kind": "weekly_all", "group": "weekly", "percent": 65,
     "severity": "normal", "resets_at": "2027-01-19T17:59:59.917521+00:00",
     "scope": null, "is_active": false},
    {"kind": "weekly_scoped", "group": "weekly", "percent": 95,
     "severity": "critical", "resets_at": "2027-01-19T17:59:59.917847+00:00",
     "scope": {"model": {"id": null, "display_name": "Some Model"}, "surface": null},
     "is_active": true}
  ],
  "extra_usage": {"is_enabled": false, "monthly_limit": null, "used_credits": null,
                  "utilization": null, "currency": null}
}"#;

const LEGACY_SHAPE: &str = r#"{
  "five_hour": {"utilization": 81, "resets_at": "2027-01-15T02:19:59+00:00",
                "limit_dollars": null, "used_dollars": null, "remaining_dollars": null},
  "seven_day": {"utilization": 65, "resets_at": "2027-01-19T17:59:59+00:00",
                "limit_dollars": null, "used_dollars": null, "remaining_dollars": null},
  "seven_day_opus": null
}"#;

#[test]
fn the_limits_array_yields_one_window_per_entry_with_derived_ids() {
    let usage = anthropic::parse_usage(LIMITS_SHAPE).expect("parses");

    let ids: Vec<&str> = usage
        .windows
        .iter()
        .map(|window| window.id.as_str())
        .collect();
    assert_eq!(ids, ["five-hour", "seven-day", "weekly-some-model"]);

    assert_eq!(usage.windows[0].role, WindowRole::PrimaryShort);
    assert_eq!(usage.windows[0].kind, UsageWindowKind::Rolling);
    assert_eq!(usage.windows[0].used_percent, Some(81.0));
    assert_eq!(usage.windows[0].scope, UsageScope::Account);

    assert_eq!(usage.windows[1].role, WindowRole::PrimaryLong);
    assert_eq!(usage.windows[1].kind, UsageWindowKind::Weekly);

    assert_eq!(usage.windows[2].role, WindowRole::Supplemental);
    assert_eq!(
        usage.windows[2].scope,
        UsageScope::Model("Some Model".into())
    );

    // Every window is the provider's own statement, so all of them are
    // authoritative; a window that were not could never fire a milestone.
    assert!(usage.windows.iter().all(|window| window.authoritative));
    // `is_enabled: false` is a fact, not an absence.
    let extra = usage.supplemental.expect("extra usage is reported");
    assert!(!extra.enabled);
    assert_eq!(extra.balance, None);
}

#[test]
fn the_legacy_flat_shape_produces_the_same_two_primary_ids() {
    let usage = anthropic::parse_usage(LEGACY_SHAPE).expect("parses");
    let ids: Vec<&str> = usage
        .windows
        .iter()
        .map(|window| window.id.as_str())
        .collect();
    // Same identities as the array shape, which is what lets a sample history
    // survive a provider changing its payload.
    assert_eq!(ids, ["five-hour", "seven-day"]);
    assert_eq!(usage.windows[0].used_percent, Some(81.0));
    // An explicitly null window is absent, not zero.
    assert_eq!(usage.windows.len(), 2);
}

#[test]
fn a_payload_carrying_both_shapes_does_not_report_a_window_twice() {
    let both = r#"{
      "limits": [{"kind": "session", "percent": 40, "resets_at": null, "scope": null}],
      "five_hour": {"utilization": 40, "resets_at": null},
      "seven_day": {"utilization": 12, "resets_at": null}
    }"#;
    let usage = anthropic::parse_usage(both).expect("parses");
    assert_eq!(usage.windows.len(), 1);
    assert_eq!(usage.windows[0].id, "five-hour");
}

#[test]
fn an_unrecognized_limit_keeps_its_own_name_rather_than_being_guessed_at() {
    let unknown = r#"{"limits": [
      {"kind": "some_future_limit", "percent": 10, "resets_at": null, "scope": null}
    ]}"#;
    let usage = anthropic::parse_usage(unknown).expect("parses");
    assert_eq!(usage.windows[0].id, "some-future-limit");
    assert_eq!(
        usage.windows[0].role,
        WindowRole::Other("some_future_limit".into())
    );
}

#[test]
fn a_percentage_outside_the_domain_rejects_the_whole_payload() {
    // Not clamped to 100. A full meter that is actually a parse failure is
    // indistinguishable from a real one, which is the failure mode this
    // whole module is arranged to avoid.
    let absurd =
        r#"{"limits": [{"kind": "session", "percent": 420, "resets_at": null, "scope": null}]}"#;
    assert_eq!(
        anthropic::parse_usage(absurd),
        Err(ProviderUsageError::Schema(SchemaReason::InvalidValue))
    );
}

#[test]
fn a_missing_percentage_stays_unknown_rather_than_becoming_zero() {
    let silent = r#"{"limits": [{"kind": "session", "resets_at": null, "scope": null}]}"#;
    let usage = anthropic::parse_usage(silent).expect("parses");
    assert_eq!(usage.windows[0].used_percent, None);
}

#[test]
fn an_unparseable_reset_rejects_the_payload_rather_than_dropping_the_field() {
    let bad_reset = r#"{"limits": [{"kind": "session", "percent": 10, "resets_at": "tuesday", "scope": null}]}"#;
    assert_eq!(
        anthropic::parse_usage(bad_reset),
        Err(ProviderUsageError::Schema(SchemaReason::InvalidValue))
    );
}

#[test]
fn an_envelope_with_nothing_in_it_is_an_error_not_an_empty_reading() {
    // The shape changed under us. Reporting "no limits" would look exactly
    // like a reader who has used nothing.
    assert_eq!(
        anthropic::parse_usage("{}"),
        Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField
        ))
    );
    assert_eq!(
        anthropic::parse_usage("[]"),
        Err(ProviderUsageError::Schema(SchemaReason::MissingEnvelope))
    );
    assert_eq!(
        anthropic::parse_usage("not json"),
        Err(ProviderUsageError::Schema(SchemaReason::InvalidJson))
    );
}

#[test]
fn raw_amounts_without_a_currency_fail_closed() {
    // A bare number that might be dollars, cents, or credits has no business
    // next to a spend figure.
    let ambiguous = r#"{"extra_usage": {"is_enabled": true, "used_credits": 500,
                        "monthly_limit": 2000, "utilization": 25, "currency": null}}"#;
    assert_eq!(
        anthropic::parse_usage(ambiguous),
        Err(ProviderUsageError::Schema(
            SchemaReason::MissingRequiredField
        ))
    );

    let priced = r#"{"extra_usage": {"is_enabled": true, "used_credits": 500,
                     "monthly_limit": 2000, "utilization": 25, "currency": "USD"}}"#;
    let usage = anthropic::parse_usage(priced).expect("parses");
    let balance = usage
        .supplemental
        .expect("extra usage")
        .balance
        .expect("balance");
    assert_eq!(balance.used, Some(500.0));
    assert_eq!(balance.limit, Some(2000.0));
    assert_eq!(balance.remaining, Some(1500.0));
    assert_eq!(balance.currency.as_deref(), Some("USD"));
}

/* -------------------------------------------------------------------------
 * The source ladder
 * ---------------------------------------------------------------------- */

struct Fixed(&'static str, Vec<ProviderUsageSnapshot>);

impl LiveUsageSource for Fixed {
    fn id(&self) -> &'static str {
        self.0
    }
    fn fetch(&self) -> SourceOutcome {
        SourceOutcome::found(self.1.clone())
    }
}

struct Broken(&'static str, ProviderUsageError);

impl LiveUsageSource for Broken {
    fn id(&self) -> &'static str {
        self.0
    }
    fn fetch(&self) -> SourceOutcome {
        SourceOutcome::failed(self.1)
    }
}

fn snapshot(
    support: SupportTier,
    freshness: Freshness,
    observed: i64,
    percent: f64,
) -> ProviderUsageSnapshot {
    ProviderUsageSnapshot {
        provider: crate::provider_usage::providers::ANTHROPIC,
        account: Some("account-a".into()),
        plan: None,
        observed_at: at(observed),
        source: UsageSource {
            id: "fixture",
            label: "fixture".into(),
            confidence: Confidence::High,
            freshness,
        },
        support,
        windows: vec![super::UsageWindow {
            id: "five-hour".into(),
            role: WindowRole::PrimaryShort,
            kind: UsageWindowKind::Rolling,
            scope: UsageScope::Account,
            used_percent: Some(percent),
            starts_at: None,
            resets_at: None,
            authoritative: true,
        }],
        supplemental: None,
    }
}

#[test]
fn a_stated_figure_beats_a_fresher_modelled_one() {
    // Old news about the truth beats fresh news about a guess.
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![
        Box::new(Fixed(
            "modelled",
            vec![snapshot(
                SupportTier::Estimated,
                Freshness::Fresh,
                NOW,
                10.0,
            )],
        )),
        Box::new(Fixed(
            "stated",
            vec![snapshot(
                SupportTier::Live,
                Freshness::Stale,
                NOW - 7_200,
                81.0,
            )],
        )),
    ];
    let summary = summarize(&sources, None, NOW, 0);
    assert_eq!(summary.providers.len(), 1);
    assert_eq!(summary.providers[0].windows[0].used_percent, Some(81.0));
}

#[test]
fn between_two_stated_figures_the_fresher_one_wins() {
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![
        Box::new(Fixed(
            "old",
            vec![snapshot(
                SupportTier::Live,
                Freshness::Stale,
                NOW - 7_200,
                40.0,
            )],
        )),
        Box::new(Fixed(
            "new",
            vec![snapshot(SupportTier::Live, Freshness::Fresh, NOW, 81.0)],
        )),
    ];
    let summary = summarize(&sources, None, NOW, 0);
    assert_eq!(summary.providers[0].windows[0].used_percent, Some(81.0));
}

#[test]
fn two_accounts_at_one_provider_never_merge() {
    let mut other = snapshot(SupportTier::Live, Freshness::Fresh, NOW, 12.0);
    other.account = Some("account-b".into());
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Fixed(
        "both",
        vec![
            snapshot(SupportTier::Live, Freshness::Fresh, NOW, 81.0),
            other,
        ],
    ))];
    let collected = sources::collect(&sources);
    assert_eq!(collected.snapshots.len(), 2);
}

#[test]
fn a_failed_source_reports_its_category_without_erasing_a_working_one() {
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![
        Box::new(Fixed(
            "working",
            vec![snapshot(SupportTier::Live, Freshness::Fresh, NOW, 81.0)],
        )),
        Box::new(Broken("signed-out", ProviderUsageError::Authentication)),
    ];
    let summary = summarize(&sources, None, NOW, 0);
    assert_eq!(summary.providers.len(), 1);
    assert_eq!(summary.errors.len(), 1);
    assert_eq!(summary.errors[0].source, "signed-out");
    assert_eq!(summary.errors[0].category, "authentication");
}

#[test]
fn no_sources_is_an_empty_summary_and_not_an_error() {
    let summary = summarize(&[], None, NOW, 0);
    assert!(summary.providers.is_empty());
    assert!(summary.errors.is_empty());
    assert_eq!(summary.generated_at, "2027-01-15T08:00:00Z");
}

/* -------------------------------------------------------------------------
 * The payload the views receive
 * ---------------------------------------------------------------------- */

#[test]
fn the_live_payload_states_percentages_and_says_where_they_came_from() {
    // The mirror image of the estimate payload's guarantee: that one must
    // carry no percentage or reset, and this one must carry them together
    // with the provenance that makes them readable.
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Fixed(
        "fixture",
        vec![snapshot(SupportTier::Live, Freshness::Fresh, NOW, 81.0)],
    ))];
    let json = serde_json::to_string(&summarize(&sources, None, NOW, 0)).unwrap();

    for required in [
        "usedPercent",
        "observedAt",
        "sourceLabel",
        "support",
        "freshness",
    ] {
        assert!(required_present(&json, required), "{required} in {json}");
    }
    assert!(json.contains("\"support\":\"live\""));
    assert!(json.contains("\"freshness\":\"fresh\""));
}

fn required_present(json: &str, field: &str) -> bool {
    json.contains(&format!("\"{field}\":"))
}

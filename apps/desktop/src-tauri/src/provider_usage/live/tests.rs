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
    Confidence, Freshness, ProviderUsageError, ProviderUsageSnapshot, SchemaReason, UsageScope,
    UsageSource, UsageWindowKind, WindowRole,
};
use super::{LiveUsageSource, SourceOutcome, sources, summarize, summarize_collected};

const NOW: i64 = 1_800_000_000;

/// A background-caller-shaped `max_age` for tests that only exercise the
/// source ladder and payload shaping, not the cooldown's freshness budget —
/// `sources/cooldown.rs`'s own suite owns that.
const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(600);

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

#[test]
fn a_fraction_shaped_utilization_is_scaled_the_same_as_an_already_stated_percent() {
    // The endpoint is not consistent about which shape it emits: this fixture
    // states the session window as a `0..=1` fraction and the weekly window
    // as an already-multiplied percent, in the same payload.
    let mixed = r#"{"limits": [
      {"kind": "session", "utilization": 0.42, "resets_at": null, "scope": null},
      {"kind": "weekly_all", "percent": 65, "resets_at": null, "scope": null}
    ]}"#;
    let usage = anthropic::parse_usage(mixed).expect("parses");
    assert_eq!(usage.windows[0].used_percent, Some(42.0));
    assert_eq!(usage.windows[1].used_percent, Some(65.0));
}

#[test]
fn an_explicit_one_percent_reading_stays_one_percent() {
    let current = r#"{
      "limits": [
        {"kind": "session", "percent": 1, "resets_at": null, "scope": null},
        {"kind": "weekly_all", "percent": 37, "resets_at": null, "scope": null}
      ],
      "five_hour": {"utilization": 1.0, "resets_at": null},
      "seven_day": {"utilization": 37.0, "resets_at": null}
    }"#;
    let usage = anthropic::parse_usage(current).expect("parses");
    assert_eq!(usage.windows[0].used_percent, Some(1.0));
    assert_eq!(usage.windows[1].used_percent, Some(37.0));
}

#[test]
fn a_legacy_one_percent_reading_stays_one_percent() {
    let legacy = r#"{
      "five_hour": {"utilization": 1.0, "resets_at": null},
      "seven_day": {"utilization": 37.0, "resets_at": null}
    }"#;
    let usage = anthropic::parse_usage(legacy).expect("parses");
    assert_eq!(usage.windows[0].used_percent, Some(1.0));
    assert_eq!(usage.windows[1].used_percent, Some(37.0));
}

#[test]
fn resets_at_is_read_whether_it_is_epoch_seconds_or_rfc_3339() {
    let mixed = r#"{"limits": [
      {"kind": "session", "percent": 10, "resets_at": 1800003600, "scope": null},
      {"kind": "weekly_all", "percent": 20,
       "resets_at": "2027-01-15T02:19:59+00:00", "scope": null}
    ]}"#;
    let usage = anthropic::parse_usage(mixed).expect("parses");
    assert_eq!(
        usage.windows[0].resets_at,
        OffsetDateTime::from_unix_timestamp(1_800_003_600).ok()
    );
    assert!(usage.windows[1].resets_at.is_some());
}

/* -------------------------------------------------------------------------
 * The source ladder
 * ---------------------------------------------------------------------- */

struct Fixed(&'static str, Vec<ProviderUsageSnapshot>);

impl LiveUsageSource for Fixed {
    fn id(&self) -> &'static str {
        self.0
    }
    fn provider(&self) -> &'static str {
        crate::provider_usage::providers::ANTHROPIC
    }
    fn fetch(&self, _max_age: std::time::Duration) -> SourceOutcome {
        SourceOutcome::found(self.1.clone())
    }
}

struct Broken(&'static str, ProviderUsageError);

impl LiveUsageSource for Broken {
    fn id(&self) -> &'static str {
        self.0
    }
    fn provider(&self) -> &'static str {
        crate::provider_usage::providers::ANTHROPIC
    }
    fn fetch(&self, _max_age: std::time::Duration) -> SourceOutcome {
        SourceOutcome::failed(self.1)
    }
}

fn snapshot(freshness: Freshness, observed: i64, percent: f64) -> ProviderUsageSnapshot {
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
        reset_credits: None,
    }
}

#[test]
fn between_two_stated_figures_the_fresher_one_wins() {
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![
        Box::new(Fixed(
            "old",
            vec![snapshot(Freshness::Stale, NOW - 7_200, 40.0)],
        )),
        Box::new(Fixed("new", vec![snapshot(Freshness::Fresh, NOW, 81.0)])),
    ];
    let summary = summarize(&sources, None, NOW, 0, MAX_AGE);
    assert_eq!(summary.providers[0].windows[0].used_percent, Some(81.0));
}

#[test]
fn two_accounts_at_one_provider_never_merge() {
    let mut other = snapshot(Freshness::Fresh, NOW, 12.0);
    other.account = Some("account-b".into());
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Fixed(
        "both",
        vec![snapshot(Freshness::Fresh, NOW, 81.0), other],
    ))];
    let collected = sources::collect(&sources, true, MAX_AGE);
    assert_eq!(collected.snapshots.len(), 2);
}

#[test]
fn a_failed_source_reports_its_category_without_erasing_a_working_one() {
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![
        Box::new(Fixed(
            "working",
            vec![snapshot(Freshness::Fresh, NOW, 81.0)],
        )),
        Box::new(Broken("signed-out", ProviderUsageError::Authentication)),
    ];
    let summary = summarize(&sources, None, NOW, 0, MAX_AGE);
    assert_eq!(summary.providers.len(), 1);
    assert_eq!(summary.errors.len(), 1);
    assert_eq!(summary.errors[0].source, "signed-out");
    assert_eq!(summary.errors[0].category, "authentication");
    // The error names its provider, so the views can keep a section for a
    // provider the failure left without a snapshot.
    assert_eq!(summary.errors[0].provider, "anthropic");
    assert_eq!(summary.errors[0].display_name, "Claude");
}

#[test]
fn no_sources_is_an_empty_summary_and_not_an_error() {
    let summary = summarize(&[], None, NOW, 0, MAX_AGE);
    assert!(summary.providers.is_empty());
    assert!(summary.errors.is_empty());
    assert_eq!(summary.generated_at, "2027-01-15T08:00:00Z");
}

#[test]
fn a_completed_background_collection_records_history_and_shapes_the_view() {
    let store = crate::store::Store::open_in_memory(std::path::Path::new(
        "/tmp/antiburn-live-usage-background-collection",
    ))
    .expect("in-memory store");
    let reading = snapshot(Freshness::Fresh, NOW, 44.0);
    let key = super::history::window_key(&reading, "five-hour");
    let collected = sources::Collected {
        snapshots: vec![reading],
        errors: Vec::new(),
    };

    let summary = summarize_collected(collected, Some(&store), NOW, 0);

    assert_eq!(summary.providers[0].windows[0].used_percent, Some(44.0));
    assert_eq!(super::history::load(&store).samples(&key).len(), 1);
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
        vec![snapshot(Freshness::Fresh, NOW, 81.0)],
    ))];
    let json = serde_json::to_string(&summarize(&sources, None, NOW, 0, MAX_AGE)).unwrap();

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

#[test]
fn the_live_payload_carries_manual_reset_credits() {
    let mut reading = snapshot(Freshness::Fresh, NOW, 81.0);
    reading.reset_credits = Some(super::model::RateLimitResetCredits { available_count: 1 });
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Fixed("fixture", vec![reading]))];

    let summary = summarize(&sources, None, NOW, 0, MAX_AGE);
    assert_eq!(
        summary.providers[0]
            .reset_credits
            .as_ref()
            .map(|credits| credits.available_count),
        Some(1)
    );
}

fn required_present(json: &str, field: &str) -> bool {
    json.contains(&format!("\"{field}\":"))
}

/* -------------------------------------------------------------------------
 * The online opt-in
 * ---------------------------------------------------------------------- */

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A source that declares it needs the opt-in, and counts how often it is
/// actually asked.
struct Counted(Arc<AtomicUsize>);

impl LiveUsageSource for Counted {
    fn id(&self) -> &'static str {
        "counted"
    }
    fn provider(&self) -> &'static str {
        crate::provider_usage::providers::ANTHROPIC
    }
    fn requires_online_opt_in(&self) -> bool {
        true
    }
    fn fetch(&self, _max_age: std::time::Duration) -> SourceOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        SourceOutcome::found(vec![snapshot(Freshness::Fresh, NOW, 99.0)])
    }
}

#[test]
fn a_gated_source_is_never_called_while_the_opt_in_is_off() {
    // Not merely filtered out afterwards: never called. The gate exists so
    // that nothing the source would *do* — spawn a process, open a socket —
    // can happen, and a design that discarded its results would already have
    // done it.
    let calls = Arc::new(AtomicUsize::new(0));
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![
        Box::new(Fixed(
            "ungated",
            vec![snapshot(Freshness::Fresh, NOW, 40.0)],
        )),
        Box::new(Counted(Arc::clone(&calls))),
    ];

    let off = sources::collect(&sources, false, MAX_AGE);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    // The ungated source still ran, so turning the opt-in off costs the
    // reader the gated source's readings and nothing else.
    assert_eq!(off.snapshots.len(), 1);
    assert_eq!(off.snapshots[0].windows[0].used_percent, Some(40.0));

    sources::collect(&sources, true, MAX_AGE);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn a_source_is_ungated_by_default() {
    // The default is what a new source inherits, so it has to be the right
    // answer for a source that only reads local state and the wrong one
    // loudly for anything that makes a request — which is why every
    // direct-fetch source overrides it explicitly.
    assert!(!Fixed("ungated", Vec::new()).requires_online_opt_in());
}

#[test]
fn a_gated_source_stays_uncalled_before_onboarding_finishes_even_with_the_switch_on() {
    // `liveUsageEnabled` now defaults to true, but the credential read this
    // unlocks — and the macOS Keychain prompt it can trigger — must never
    // land before the reader has gotten through first-run setup. Exercised
    // through `summarize`'s real store, not the bare `online: bool` collect
    // gate, so the onboarding half of `AppSettings::live_usage_active` is
    // actually proven, not merely assumed.
    let calls = Arc::new(AtomicUsize::new(0));
    let sources: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Counted(Arc::clone(&calls)))];
    let store = crate::store::Store::open_in_memory(std::path::Path::new(
        "/tmp/antiburn-live-usage-onboarding-test",
    ))
    .expect("in-memory store");

    store
        .save_settings(&crate::store::AppSettings {
            live_usage_enabled: true,
            onboarding_completed: false,
            ..crate::store::AppSettings::default()
        })
        .unwrap();
    summarize(&sources, Some(&store), NOW, 0, MAX_AGE);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the switch alone is not enough before onboarding completes"
    );

    store
        .save_settings(&crate::store::AppSettings {
            live_usage_enabled: true,
            onboarding_completed: true,
            ..crate::store::AppSettings::default()
        })
        .unwrap();
    summarize(&sources, Some(&store), NOW, 0, MAX_AGE);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "once onboarding is done, the already-on switch is enough on its own"
    );
}

/* -------------------------------------------------------------------------
 * Whether a supplemental, model-scoped window is worth showing
 *
 * `has_nonzero_usage_in_current_period` (see `metrics.rs`) is what the
 * frontend consults to hide a per-model weekly limit until it has something
 * to say. Exercised here end to end, through `summarize`'s real store, so
 * the wiring — not just the arithmetic underneath it — is covered.
 * ---------------------------------------------------------------------- */

use crate::store::Store;

fn weekly_scoped_snapshot(
    observed: i64,
    percent: Option<f64>,
    resets_at: i64,
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
            freshness: Freshness::Fresh,
        },
        windows: vec![super::UsageWindow {
            id: "weekly-some-model".into(),
            role: WindowRole::Supplemental,
            kind: UsageWindowKind::Weekly,
            scope: UsageScope::Model("Some Model".into()),
            used_percent: percent,
            starts_at: None,
            resets_at: Some(at(resets_at)),
            authoritative: true,
        }],
        supplemental: None,
        reset_credits: None,
    }
}

fn in_memory_store() -> Store {
    Store::open_in_memory(std::path::Path::new("/tmp/antiburn-live-usage-test"))
        .expect("in-memory store")
}

#[test]
fn a_supplemental_window_turns_on_and_stays_on_through_the_period() {
    let store = in_memory_store();
    let reset = NOW + 3_600;

    // Sits at 0%, same as most readers who never touch this model.
    let quiet: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Fixed(
        "fixture",
        vec![weekly_scoped_snapshot(NOW - 7_200, Some(0.0), reset)],
    ))];
    let summary = summarize(&quiet, Some(&store), NOW - 7_200, 0, MAX_AGE);
    assert!(!summary.providers[0].windows[0].has_nonzero_usage_in_current_period);

    // A real reading arrives.
    let used: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Fixed(
        "fixture",
        vec![weekly_scoped_snapshot(NOW - 3_600, Some(6.0), reset)],
    ))];
    let summary = summarize(&used, Some(&store), NOW - 3_600, 0, MAX_AGE);
    assert!(summary.providers[0].windows[0].has_nonzero_usage_in_current_period);

    // A later reading with no percentage at all must not erase what the
    // period already proved — sticky for the rest of the period.
    let unknown: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Fixed(
        "fixture",
        vec![weekly_scoped_snapshot(NOW, None, reset)],
    ))];
    let summary = summarize(&unknown, Some(&store), NOW, 0, MAX_AGE);
    assert!(summary.providers[0].windows[0].has_nonzero_usage_in_current_period);
}

#[test]
fn a_reset_clears_the_flag_for_the_new_period() {
    let store = in_memory_store();
    let first_reset = NOW + 3_600;

    let used: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Fixed(
        "fixture",
        vec![weekly_scoped_snapshot(NOW - 3_600, Some(30.0), first_reset)],
    ))];
    let summary = summarize(&used, Some(&store), NOW - 3_600, 0, MAX_AGE);
    assert!(summary.providers[0].windows[0].has_nonzero_usage_in_current_period);

    // The window reset: the percentage dropped, and the provider now states a
    // reset further out than before. Nothing this period has used anything
    // yet, so the flag must not still be reporting last period's usage.
    let second_reset = NOW + 604_800;
    let rolled: Vec<Box<dyn LiveUsageSource>> = vec![Box::new(Fixed(
        "fixture",
        vec![weekly_scoped_snapshot(NOW, Some(0.0), second_reset)],
    ))];
    let summary = summarize(&rolled, Some(&store), NOW, 0, MAX_AGE);
    assert!(!summary.providers[0].windows[0].has_nonzero_usage_in_current_period);
}

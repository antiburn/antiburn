//! Aggregation over synthetic store rows.
//!
//! Every case fixes `now` at a real instant and builds the rows by hand, so
//! nothing here depends on a clock, a scan, or a database. The two things worth
//! stating explicitly: the anchor `1_800_000_000` is `2027-01-15T08:00:00Z`
//! (pinned by a test below, and by `commands::tests`), and the pricing figures
//! come from the engine's bundled catalog rather than from constants repeated
//! here — a catalog revision should move them, not fail these tests.

use super::*;

/// `2027-01-15T08:00:00Z`, mid-month and mid-morning, so day, week, and month
/// boundaries are all distinct and none of them coincides with `now`.
const NOW: i64 = 1_800_000_000;

/// A model the bundled catalog prices.
const PRICED_MODEL: &str = "claude-opus-4-6";
/// A model it does not, and will not: the point is the shape, not the name.
const UNPRICED_MODEL: &str = "some-unreleased-model";

fn tokens(input: u64, output: u64, cache_read: u64, cache_creation: u64) -> ModelTokens {
    ModelTokens {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_creation,
        cache_creation_1h_tokens: 0,
    }
}

/// One analyzed session, with the model breakdown the store would have cached.
fn row(agent: &str, at: i64, models: &[(&str, ModelTokens)]) -> UsageEvidenceRecord {
    let breakdown: BTreeMap<String, ModelTokens> = models
        .iter()
        .map(|(model, tokens)| ((*model).to_string(), tokens.clone()))
        .collect();
    UsageEvidenceRecord {
        agent: agent.to_string(),
        updated_at_epoch: at,
        model_breakdown_json: Some(serde_json::to_string(&breakdown).unwrap()),
    }
}

/// One session discovery has seen but analysis has not reached yet.
fn unanalyzed(agent: &str, at: i64) -> UsageEvidenceRecord {
    UsageEvidenceRecord {
        agent: agent.to_string(),
        updated_at_epoch: at,
        model_breakdown_json: None,
    }
}

fn find<'a>(summary: &'a ProviderUsageSummary, provider: &str) -> &'a ProviderUsage {
    summary
        .providers
        .iter()
        .find(|entry| entry.provider == provider)
        .unwrap_or_else(|| panic!("expected a {provider} row in {:?}", summary.providers))
}

/* -------------------------------------------------------------------------
 * Windows
 * ---------------------------------------------------------------------- */

#[test]
fn the_windows_are_local_calendar_boundaries_around_now() {
    let bounds = window_bounds(NOW, 0);
    // 2027-01-15T00:00:00Z, 2027-01-09T00:00:00Z, 2027-01-01T00:00:00Z.
    assert_eq!(bounds.today_start, 1_799_971_200);
    assert_eq!(bounds.week_start, 1_799_452_800);
    assert_eq!(bounds.month_start, 1_798_761_600);
    // Mid-month, the month reaches further back than the week does — the two
    // windows are independent, not nested.
    assert!(bounds.month_start < bounds.week_start);
    assert_eq!(lookback_start(NOW, 0), bounds.month_start);
}

#[test]
fn early_in_a_month_the_week_reaches_further_back_than_the_month() {
    // 2027-02-02T12:00:00Z: the month is two days old, the week is not.
    let now = 1_801_569_600;
    let bounds = window_bounds(now, 0);
    assert_eq!(bounds.month_start, 1_801_440_000); // 2027-02-01T00:00:00Z
    assert_eq!(bounds.week_start, 1_801_008_000); // 2027-01-27T00:00:00Z
    assert!(bounds.week_start < bounds.month_start);
    assert_eq!(lookback_start(now, 0), bounds.week_start);
}

#[test]
fn a_session_in_last_month_counts_in_the_week_but_not_the_month() {
    let now = 1_801_569_600; // 2027-02-02T12:00:00Z
    // 2027-01-31T12:00:00Z — two days back, but the wrong side of the rollover.
    let rows = [row(
        "claude-code",
        1_801_396_800,
        &[(PRICED_MODEL, tokens(1_000_000, 0, 0, 0))],
    )];
    let summary = summarize(&rows, now, 0);
    let anthropic = find(&summary, providers::ANTHROPIC);

    assert_eq!(anthropic.windows.week.tokens_in, 1_000_000);
    assert_eq!(anthropic.windows.week.session_count, 1);
    assert_eq!(anthropic.windows.month.tokens_in, 0);
    assert_eq!(anthropic.windows.month.session_count, 0);
    assert_eq!(anthropic.windows.month.estimated_usd, None);
    assert_eq!(anthropic.windows.today.session_count, 0);
}

#[test]
fn a_window_includes_its_own_first_second_and_excludes_the_one_before_it() {
    let bounds = window_bounds(NOW, 0);
    let rows = [
        row(
            "claude-code",
            bounds.today_start,
            &[(PRICED_MODEL, tokens(10, 0, 0, 0))],
        ),
        row(
            "claude-code",
            bounds.today_start - 1,
            &[(PRICED_MODEL, tokens(10, 0, 0, 0))],
        ),
    ];
    let anthropic = summarize(&rows, NOW, 0);
    let anthropic = find(&anthropic, providers::ANTHROPIC);

    assert_eq!(
        anthropic.windows.today.session_count, 1,
        "midnight is today"
    );
    assert_eq!(anthropic.windows.week.session_count, 2);
    assert_eq!(anthropic.windows.month.session_count, 2);
}

#[test]
fn a_session_older_than_every_window_is_ignored_entirely() {
    let bounds = window_bounds(NOW, 0);
    let rows = [row(
        "claude-code",
        bounds.month_start - 1,
        &[(PRICED_MODEL, tokens(1_000_000, 0, 0, 0))],
    )];
    assert!(summarize(&rows, NOW, 0).providers.is_empty());
}

#[test]
fn a_session_with_no_heartbeat_is_outside_every_window() {
    // The store writes zero for a session discovery never dated.
    let rows = [unanalyzed("claude-code", 0)];
    assert!(summarize(&rows, NOW, 0).providers.is_empty());
}

#[test]
fn the_readers_own_offset_decides_which_day_a_session_lands_in() {
    // 2027-01-14T22:00:00Z is yesterday in UTC and today in +13:00.
    let at = 1_799_971_200 - 2 * 3_600;
    let rows = [row(
        "claude-code",
        at,
        &[(PRICED_MODEL, tokens(1_000, 0, 0, 0))],
    )];

    let utc = summarize(&rows, NOW, 0);
    assert_eq!(find(&utc, providers::ANTHROPIC).windows.today.tokens_in, 0);

    let auckland = summarize(&rows, NOW, 13 * 60);
    assert_eq!(
        find(&auckland, providers::ANTHROPIC)
            .windows
            .today
            .tokens_in,
        1_000
    );
}

#[test]
fn an_implausible_offset_falls_back_to_utc_rather_than_shifting_wildly() {
    assert_eq!(window_bounds(NOW, 99_999), window_bounds(NOW, 14 * 60));
    assert_eq!(window_bounds(NOW, -99_999), window_bounds(NOW, -14 * 60));
}

/* -------------------------------------------------------------------------
 * States
 * ---------------------------------------------------------------------- */

#[test]
fn a_fully_priced_provider_is_estimated_and_carries_the_engines_figure() {
    let rows = [row(
        "claude-code",
        NOW - 60,
        &[(PRICED_MODEL, tokens(1_000_000, 0, 0, 0))],
    )];
    let summary = summarize(&rows, NOW, 0);
    let anthropic = find(&summary, providers::ANTHROPIC);

    assert_eq!(anthropic.state, ProviderUsageState::Estimated);
    assert_eq!(anthropic.staleness, ProviderUsageStaleness::Fresh);
    // The product, not the corporation: readers recognize "Claude". The id
    // beside it stays `anthropic`, because that is what keys attribution.
    assert_eq!(anthropic.provider, providers::ANTHROPIC);
    assert_eq!(anthropic.display_name, "Claude");
    // A million input tokens of the catalog's Opus rate. The figure belongs to
    // the engine; this only asserts that it arrived intact.
    let expected = price_breakdown(&HashMap::from([(
        PRICED_MODEL.to_string(),
        tokens(1_000_000, 0, 0, 0),
    )]))
    .expect("the catalog prices this model")
    .total_usd;
    assert!(expected > 0.0);
    let today = anthropic.windows.today.estimated_usd.expect("priced");
    assert!((today - expected).abs() < 1e-9);
}

#[test]
fn tokens_split_the_way_the_engine_splits_them() {
    let rows = [row(
        "claude-code",
        NOW - 60,
        &[(PRICED_MODEL, tokens(100, 20, 300, 40))],
    )];
    let summary = summarize(&rows, NOW, 0);
    let today = &find(&summary, providers::ANTHROPIC).windows.today;

    // Effective input is fresh prompt tokens plus cache writes; cache reads are
    // their own line because they are billed at their own rate.
    assert_eq!(today.tokens_in, 140);
    assert_eq!(today.tokens_out, 20);
    assert_eq!(today.cache_read, 300);
}

#[test]
fn an_unpriceable_model_is_observed_with_no_dollar_figure_at_all() {
    let rows = [row(
        "claude-code",
        NOW - 60,
        &[(UNPRICED_MODEL, tokens(1_000, 500, 0, 0))],
    )];
    let summary = summarize(&rows, NOW, 0);
    let anthropic = find(&summary, providers::ANTHROPIC);

    assert_eq!(anthropic.state, ProviderUsageState::Observed);
    assert_eq!(anthropic.windows.today.tokens_in, 1_000);
    assert_eq!(anthropic.windows.today.tokens_out, 500);
    assert_eq!(anthropic.windows.today.estimated_usd, None);
}

#[test]
fn a_partly_priced_provider_reports_the_priced_part_and_says_it_is_observed() {
    let rows = [row(
        "claude-code",
        NOW - 60,
        &[
            (PRICED_MODEL, tokens(1_000_000, 0, 0, 0)),
            (UNPRICED_MODEL, tokens(9_000_000, 0, 0, 0)),
        ],
    )];
    let summary = summarize(&rows, NOW, 0);
    let anthropic = find(&summary, providers::ANTHROPIC);

    // The dollar figure is a floor, not a total — which is exactly what
    // `observed` tells the view, so it can say so instead of implying a total.
    assert_eq!(anthropic.state, ProviderUsageState::Observed);
    let usd = anthropic
        .windows
        .today
        .estimated_usd
        .expect("partly priced");
    assert!(usd > 0.0);
    assert_eq!(anthropic.windows.today.tokens_in, 10_000_000);
}

#[test]
fn a_session_with_no_analysis_yet_is_unknown_rather_than_zero() {
    let rows = [unanalyzed("claude-code", NOW - 60)];
    let summary = summarize(&rows, NOW, 0);
    let anthropic = find(&summary, providers::ANTHROPIC);

    assert_eq!(anthropic.state, ProviderUsageState::Unknown);
    assert_eq!(anthropic.windows.today.session_count, 1);
    assert_eq!(anthropic.windows.today.tokens_in, 0);
    assert_eq!(anthropic.windows.today.estimated_usd, None);
}

#[test]
fn an_unparseable_cache_row_degrades_to_unknown_rather_than_failing() {
    let rows = [UsageEvidenceRecord {
        agent: "claude-code".to_string(),
        updated_at_epoch: NOW - 60,
        model_breakdown_json: Some("not json".to_string()),
    }];
    let summary = summarize(&rows, NOW, 0);
    assert_eq!(
        find(&summary, providers::ANTHROPIC).state,
        ProviderUsageState::Unknown
    );
}

#[test]
fn a_blank_model_key_is_dropped_instead_of_becoming_a_provider() {
    let rows = [row("opencode", NOW - 60, &[("   ", tokens(10, 0, 0, 0))])];
    let summary = summarize(&rows, NOW, 0);
    // Nothing nameable is left, so the session is unattributed rather than
    // filed under a provider called "".
    assert_eq!(summary.providers.len(), 1);
    assert_eq!(summary.providers[0].provider, providers::UNKNOWN);
    assert_eq!(summary.providers[0].state, ProviderUsageState::Unknown);
}

#[test]
fn evidence_older_than_the_threshold_is_stale_and_says_when_it_was() {
    let at = NOW - STALE_EVIDENCE_SECS - 1;
    let rows = [row(
        "claude-code",
        at,
        &[(PRICED_MODEL, tokens(1_000, 0, 0, 0))],
    )];
    let summary = summarize(&rows, NOW, 0);
    let anthropic = find(&summary, providers::ANTHROPIC);

    assert_eq!(anthropic.staleness, ProviderUsageStaleness::Stale);
    assert_eq!(
        anthropic.last_activity_at.as_deref(),
        Some("2027-01-14T07:59:59Z")
    );
    // Still counted — stale evidence is old, not wrong.
    assert_eq!(anthropic.windows.week.tokens_in, 1_000);
    assert_eq!(anthropic.windows.today.tokens_in, 0);
}

#[test]
fn evidence_from_a_moment_ago_is_fresh_right_up_to_the_threshold() {
    let rows = [row(
        "claude-code",
        NOW - STALE_EVIDENCE_SECS + 1,
        &[(PRICED_MODEL, tokens(1, 0, 0, 0))],
    )];
    let summary = summarize(&rows, NOW, 0);
    assert_eq!(
        find(&summary, providers::ANTHROPIC).staleness,
        ProviderUsageStaleness::Fresh
    );
}

/* -------------------------------------------------------------------------
 * Attribution
 * ---------------------------------------------------------------------- */

#[test]
fn a_fixed_route_agent_keeps_its_vendor_whatever_model_ran() {
    // Copilot serves Anthropic models, but GitHub is who bills for them.
    let rows = [row(
        "copilot",
        NOW - 60,
        &[(PRICED_MODEL, tokens(1_000, 0, 0, 0))],
    )];
    let summary = summarize(&rows, NOW, 0);

    assert_eq!(summary.providers.len(), 1);
    let github = find(&summary, providers::GITHUB);
    assert_eq!(github.display_name, "GitHub Copilot");
    assert_eq!(github.windows.today.tokens_in, 1_000);
}

#[test]
fn a_bring_your_own_agent_splits_one_session_across_the_providers_it_used() {
    let rows = [row(
        "opencode",
        NOW - 60,
        &[
            (PRICED_MODEL, tokens(1_000, 0, 0, 0)),
            ("gpt-5.6", tokens(2_000, 0, 0, 0)),
            (UNPRICED_MODEL, tokens(4_000, 0, 0, 0)),
        ],
    )];
    let summary = summarize(&rows, NOW, 0);

    assert_eq!(summary.providers.len(), 3);
    assert_eq!(
        find(&summary, providers::ANTHROPIC).windows.today.tokens_in,
        1_000
    );
    assert_eq!(
        find(&summary, providers::OPENAI).windows.today.tokens_in,
        2_000
    );
    // The unnameable model is not folded into either account.
    let unattributed = find(&summary, providers::UNKNOWN);
    assert_eq!(unattributed.windows.today.tokens_in, 4_000);
    assert_eq!(unattributed.display_name, "Unattributed");

    // The session is counted once under each provider it actually touched, and
    // the tokens are divided rather than duplicated.
    for provider in [providers::ANTHROPIC, providers::OPENAI, providers::UNKNOWN] {
        assert_eq!(find(&summary, provider).windows.today.session_count, 1);
    }
}

#[test]
fn sessions_from_different_agents_merge_when_the_same_vendor_is_billed() {
    let rows = [
        row(
            "claude-code",
            NOW - 60,
            &[(PRICED_MODEL, tokens(1_000, 0, 0, 0))],
        ),
        row(
            "opencode",
            NOW - 120,
            &[(PRICED_MODEL, tokens(500, 0, 0, 0))],
        ),
    ];
    let summary = summarize(&rows, NOW, 0);

    assert_eq!(summary.providers.len(), 1);
    let anthropic = find(&summary, providers::ANTHROPIC);
    assert_eq!(anthropic.windows.today.tokens_in, 1_500);
    assert_eq!(anthropic.windows.today.session_count, 2);
}

#[test]
fn providers_come_back_most_recently_used_first() {
    let rows = [
        row("codex", NOW - 3_600, &[("gpt-5.6", tokens(10, 0, 0, 0))]),
        row(
            "claude-code",
            NOW - 60,
            &[(PRICED_MODEL, tokens(10, 0, 0, 0))],
        ),
        row(
            "cursor",
            NOW - 7_200,
            &[(PRICED_MODEL, tokens(10, 0, 0, 0))],
        ),
    ];
    let summary = summarize(&rows, NOW, 0);
    let order: Vec<&str> = summary
        .providers
        .iter()
        .map(|entry| entry.provider.as_str())
        .collect();
    assert_eq!(
        order,
        vec![providers::ANTHROPIC, providers::OPENAI, providers::CURSOR]
    );
}

/* -------------------------------------------------------------------------
 * The contract the payload has to keep
 * ---------------------------------------------------------------------- */

#[test]
fn an_empty_snapshot_is_dated_and_has_no_providers() {
    let summary = summarize(&[], NOW, 0);
    assert_eq!(summary.generated_at, "2027-01-15T08:00:00Z");
    assert!(summary.providers.is_empty());
}

#[test]
fn the_payload_carries_no_allowance_percentage_or_reset_anywhere() {
    // The guarantee is structural: there is no field to fill, so no future
    // caller can put a made-up denominator on this surface by accident.
    let rows = [
        row(
            "claude-code",
            NOW - 60,
            &[(PRICED_MODEL, tokens(1_000, 10, 20, 30))],
        ),
        unanalyzed("opencode", NOW - 90),
    ];
    let json = serde_json::to_string(&summarize(&rows, NOW, 0)).unwrap();
    for forbidden in [
        "percent",
        "allowance",
        "remaining",
        "reset",
        "quota",
        "limit",
        "plan",
    ] {
        assert!(
            !json.to_ascii_lowercase().contains(forbidden),
            "{forbidden} must not appear in {json}"
        );
    }
}

#[test]
fn the_reserved_states_serialize_the_way_the_views_expect_them_to() {
    // v1 never emits these two, but the wire spelling is part of the contract:
    // the day a reviewed passive source produces one, the view must already
    // render it rather than fall through to an unknown branch.
    for (state, spelling) in [
        (ProviderUsageState::Live, "\"live\""),
        (ProviderUsageState::Estimated, "\"estimated\""),
        (ProviderUsageState::Observed, "\"observed\""),
        (ProviderUsageState::Detected, "\"detected\""),
        (ProviderUsageState::Unknown, "\"unknown\""),
    ] {
        assert_eq!(serde_json::to_string(&state).unwrap(), spelling);
    }
    for (staleness, spelling) in [
        (ProviderUsageStaleness::Fresh, "\"fresh\""),
        (ProviderUsageStaleness::Stale, "\"stale\""),
        (ProviderUsageStaleness::Unknown, "\"unknown\""),
    ] {
        assert_eq!(serde_json::to_string(&staleness).unwrap(), spelling);
    }
}

#[test]
fn session_observations_never_produce_a_live_or_detected_state() {
    // The whole v1 capability claim, asserted rather than asserted-in-prose:
    // whatever the evidence looks like, the aggregation only ever reaches the
    // three states a transcript can support.
    let bounds = window_bounds(NOW, 0);
    let mut rows = vec![
        unanalyzed("claude-code", NOW - 10),
        row("codex", NOW - 20, &[("gpt-5.6", tokens(1, 1, 1, 1))]),
        row(
            "cline",
            bounds.week_start,
            &[(UNPRICED_MODEL, tokens(9, 9, 9, 9))],
        ),
    ];
    for slug in [
        "cursor",
        "windsurf",
        "amp-code",
        "kiro",
        "antigravity",
        "pi",
    ] {
        rows.push(row(slug, NOW - 30, &[(PRICED_MODEL, tokens(5, 5, 5, 5))]));
    }
    for entry in summarize(&rows, NOW, 0).providers {
        assert!(
            matches!(
                entry.state,
                ProviderUsageState::Estimated
                    | ProviderUsageState::Observed
                    | ProviderUsageState::Unknown
            ),
            "{} reached {:?}",
            entry.provider,
            entry.state
        );
    }
}

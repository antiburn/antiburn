use super::*;
use crate::dto::{LiveUsageForecast, LiveUsageSupport, LiveUsageWindow, SessionLimitMetric};
use crate::provider_usage::live::metrics::UsageSample;
use crate::store::SessionUsageTurnRecord;

const NOW: i64 = 1_800_000_000;
const MODEL: &str = "claude-opus-4-6";

fn iso(at: i64) -> String {
    OffsetDateTime::from_unix_timestamp(at)
        .unwrap()
        .format(&Rfc3339)
        .unwrap()
}

fn turn(session: &str, at: i64, tokens: u64, accounts: &[&str]) -> SessionUsageRecord {
    SessionUsageRecord {
        key: SessionKey::new("native", "claude-code", session),
        wsl_distro: None,
        provider_hints_json: None,
        provider_accounts_json: serde_json::to_string(
            &accounts
                .iter()
                .map(|account| {
                    serde_json::json!({
                        "provider": "anthropic",
                        "accountKey": account,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
        turns: vec![SessionUsageTurnRecord {
            ts_ms: Some(at * 1_000),
            model: Some(MODEL.to_string()),
            input_tokens: tokens,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 0,
        }],
    }
}

fn window(metric: SessionLimitMetric, used_percent: f64) -> LiveUsageWindow {
    LiveUsageWindow {
        id: match metric {
            SessionLimitMetric::Weekly => "seven-day",
            SessionLimitMetric::FiveHour => "five-hour",
        }
        .to_string(),
        role: match metric {
            SessionLimitMetric::Weekly => "primaryLong",
            SessionLimitMetric::FiveHour => "primaryShort",
        }
        .to_string(),
        kind: match metric {
            SessionLimitMetric::Weekly => "weekly",
            SessionLimitMetric::FiveHour => "rolling",
        }
        .to_string(),
        scope_model: None,
        used_percent: Some(used_percent),
        starts_at: None,
        resets_at: Some(iso(NOW + 3_600)),
        has_nonzero_usage_in_current_period: true,
        forecast: LiveUsageForecast::default(),
    }
}

fn live(account: Option<&str>, windows: Vec<LiveUsageWindow>) -> LiveUsageSummary {
    live_for("anthropic", "Claude", account, windows)
}

fn live_for(
    provider: &str,
    display_name: &str,
    account: Option<&str>,
    windows: Vec<LiveUsageWindow>,
) -> LiveUsageSummary {
    LiveUsageSummary {
        providers: vec![LiveProviderUsage {
            provider: provider.to_string(),
            account_key: account.map(str::to_string),
            display_name: display_name.to_string(),
            account_uuid: None,
            account_email: None,
            support: LiveUsageSupport::Live,
            freshness: LiveUsageFreshness::Fresh,
            source_label: "test".to_string(),
            observed_at: iso(NOW),
            windows,
            extra_usage: None,
            reset_credits: None,
            plan: None,
        }],
        generated_at: iso(NOW),
        ..LiveUsageSummary::default()
    }
}

fn weekly(rows: &[SessionUsageRecord], live: &LiveUsageSummary) -> Vec<SessionLimitAllocation> {
    estimate(rows.to_vec(), live, &History::default(), NOW * 1_000)
        .into_iter()
        .filter(|entry| entry.metric == SessionLimitMetric::Weekly)
        .collect()
}

fn account(character: char) -> String {
    character.to_string().repeat(64)
}

#[test]
fn one_session_receives_the_reported_share() {
    let allocations = weekly(
        &[turn("one", NOW - 60, 1_000_000, &[])],
        &live(None, vec![window(SessionLimitMetric::Weekly, 40.0)]),
    );
    assert_eq!(allocations.len(), 1);
    assert!((allocations[0].percent - 40.0).abs() < 1e-9);
}

#[test]
fn sessions_divide_the_share_by_priced_turn_weight() {
    let allocations = weekly(
        &[
            turn("one", NOW - 60, 1_000_000, &[]),
            turn("two", NOW - 30, 3_000_000, &[]),
        ],
        &live(None, vec![window(SessionLimitMetric::Weekly, 80.0)]),
    );
    let shares: HashMap<_, _> = allocations
        .into_iter()
        .map(|entry| (entry.session_id, entry.percent))
        .collect();
    assert!((shares["one"] - 20.0).abs() < 1e-9);
    assert!((shares["two"] - 60.0).abs() < 1e-9);
}

#[test]
fn complete_pricing_is_preferred_over_raw_token_weight() {
    let input = turn("input", NOW - 60, 1_000_000, &[]);
    let mut output = turn("output", NOW - 30, 0, &[]);
    output.turns[0].output_tokens = 1_000_000;
    let allocations = weekly(
        &[input, output],
        &live(None, vec![window(SessionLimitMetric::Weekly, 40.0)]),
    );
    let shares: HashMap<_, _> = allocations
        .into_iter()
        .map(|entry| (entry.session_id, entry.percent))
        .collect();

    assert!(shares["output"] > shares["input"]);
    assert!((shares.values().sum::<f64>() - 40.0).abs() < 1e-9);
}

#[test]
fn zero_usage_is_a_valid_zero_estimate() {
    let allocations = weekly(
        &[turn("one", NOW - 60, 1_000_000, &[])],
        &live(None, vec![window(SessionLimitMetric::Weekly, 0.0)]),
    );
    assert_eq!(allocations.len(), 1);
    assert_eq!(allocations[0].percent, 0.0);
}

#[test]
fn turns_use_the_provider_window_boundaries() {
    let mut weekly_window = window(SessionLimitMetric::Weekly, 30.0);
    weekly_window.starts_at = Some(iso(NOW - 100));
    let allocations = weekly(
        &[
            turn("before", NOW - 101, 1_000_000, &[]),
            turn("start", NOW - 100, 1_000_000, &[]),
            turn("observed", NOW, 1_000_000, &[]),
            turn("future", NOW + 1, 1_000_000, &[]),
        ],
        &live(None, vec![weekly_window]),
    );
    let ids: BTreeSet<_> = allocations
        .into_iter()
        .map(|entry| entry.session_id)
        .collect();
    assert_eq!(
        ids,
        BTreeSet::from(["observed".to_string(), "start".to_string()])
    );
}

#[test]
fn five_hour_and_weekly_windows_use_independent_turn_sets() {
    let allocations = estimate(
        vec![
            turn("old", NOW - 6 * 3_600, 1_000_000, &[]),
            turn("new", NOW - 60, 1_000_000, &[]),
        ],
        &live(
            None,
            vec![
                window(SessionLimitMetric::FiveHour, 50.0),
                window(SessionLimitMetric::Weekly, 40.0),
            ],
        ),
        &History::default(),
        NOW * 1_000,
    );
    let get = |session: &str, metric| {
        allocations
            .iter()
            .find(|entry| entry.session_id == session && entry.metric == metric)
            .map(|entry| entry.percent)
    };
    assert_eq!(get("old", SessionLimitMetric::FiveHour), None);
    assert_eq!(get("new", SessionLimitMetric::FiveHour), Some(50.0));
    assert_eq!(get("old", SessionLimitMetric::Weekly), Some(20.0));
    assert_eq!(get("new", SessionLimitMetric::Weekly), Some(20.0));
}

#[test]
fn every_registered_provider_allocates_its_primary_weekly_and_five_hour_windows() {
    let cases = [
        ("claude-code", "anthropic", "Claude", MODEL),
        ("codex", "openai", "OpenAI", "gpt-5.6-sol"),
    ];
    for (agent, provider, display_name, model) in cases {
        let mut row = turn(provider, NOW - 60, 1_000_000, &[]);
        row.key.agent = agent.to_string();
        row.turns[0].model = Some(model.to_string());
        let allocations = estimate(
            vec![row],
            &live_for(
                provider,
                display_name,
                None,
                vec![
                    window(SessionLimitMetric::FiveHour, 25.0),
                    window(SessionLimitMetric::Weekly, 40.0),
                ],
            ),
            &History::default(),
            NOW * 1_000,
        );

        assert_eq!(allocations.len(), 2, "{provider}");
        assert!(allocations.iter().any(|entry| {
            entry.metric == SessionLimitMetric::FiveHour && (entry.percent - 25.0).abs() < 1e-9
        }));
        assert!(allocations.iter().any(|entry| {
            entry.metric == SessionLimitMetric::Weekly && (entry.percent - 40.0).abs() < 1e-9
        }));
    }
}

#[test]
fn antigravity_shared_pools_match_the_models_their_labels_name() {
    let mut gemini = turn("gemini", NOW - 60, 1_000_000, &[]);
    gemini.key.agent = "antigravity".to_string();
    gemini.turns[0].model = Some("gemini-3-pro-high".to_string());
    let mut claude = turn("claude", NOW - 30, 1_000_000, &[]);
    claude.key.agent = "antigravity".to_string();
    claude.turns[0].model = Some("antigravity-claude-opus-4-6-thinking".to_string());
    let mut gemini_weekly = window(SessionLimitMetric::Weekly, 30.0);
    gemini_weekly.id = "antigravity-gemini-weekly".to_string();
    gemini_weekly.scope_model = Some("Gemini".to_string());
    let mut third_party_five_hour = window(SessionLimitMetric::FiveHour, 20.0);
    third_party_five_hour.id = "antigravity-claude-gpt-5h".to_string();
    third_party_five_hour.scope_model = Some("Claude + GPT".to_string());

    let allocations = estimate(
        vec![gemini, claude],
        &live_for(
            "google",
            "Antigravity",
            None,
            vec![gemini_weekly, third_party_five_hour],
        ),
        &History::default(),
        NOW * 1_000,
    );

    assert_eq!(allocations.len(), 2);
    assert!(allocations.iter().any(|entry| {
        entry.session_id == "gemini" && entry.metric == SessionLimitMetric::Weekly
    }));
    assert!(allocations.iter().any(|entry| {
        entry.session_id == "claude" && entry.metric == SessionLimitMetric::FiveHour
    }));
}

#[test]
fn openai_does_not_label_other_short_windows_as_five_hour_limits() {
    let mut row = turn("codex", NOW - 60, 1_000_000, &[]);
    row.key.agent = "codex".to_string();
    row.turns[0].model = Some("gpt-5.6-sol".to_string());
    let mut other_short = window(SessionLimitMetric::FiveHour, 90.0);
    other_short.id = "burst-60m".to_string();
    let allocations = estimate(
        vec![row],
        &live_for(
            "openai",
            "OpenAI",
            None,
            vec![other_short, window(SessionLimitMetric::FiveHour, 25.0)],
        ),
        &History::default(),
        NOW * 1_000,
    );

    assert_eq!(allocations.len(), 1);
    assert!((allocations[0].percent - 25.0).abs() < 1e-9);
}

#[test]
fn fixed_route_turns_without_models_still_support_account_limits() {
    let mut row = turn("one", NOW - 60, 1_000_000, &[]);
    row.turns[0].model = None;

    let allocations = weekly(
        &[row],
        &live(None, vec![window(SessionLimitMetric::Weekly, 40.0)]),
    );

    assert_eq!(allocations.len(), 1);
    assert!((allocations[0].percent - 40.0).abs() < 1e-9);
}

#[test]
fn unattributed_turns_join_the_only_known_account() {
    let account = account('a');
    let allocations = weekly(
        &[turn("one", NOW - 60, 1_000_000, &[])],
        &live(
            Some(&account),
            vec![window(SessionLimitMetric::Weekly, 25.0)],
        ),
    );
    assert_eq!(allocations.len(), 1);
    assert_eq!(
        allocations[0].account_key.as_deref(),
        Some(account.as_str())
    );
}

#[test]
fn an_accountless_reading_joins_the_only_observed_account() {
    let account = account('a');
    let allocations = weekly(
        &[turn("one", NOW - 60, 1_000_000, &[&account])],
        &live(None, vec![window(SessionLimitMetric::Weekly, 25.0)]),
    );

    assert_eq!(allocations.len(), 1);
    assert_eq!(
        allocations[0].account_key.as_deref(),
        Some(account.as_str())
    );
}

#[test]
fn opencode_provider_hints_allocate_openai_weekly_usage() {
    let account = account('o');
    let mut row = turn("opencode", NOW - 60, 1_000_000, &[]);
    row.key.agent = "opencode".to_string();
    row.turns[0].model = Some("gpt-5.6-sol".to_string());
    row.provider_hints_json =
        Some(serde_json::json!([{"provider": "openai", "model": "gpt-5.6-sol"}]).to_string());
    row.provider_accounts_json = serde_json::json!([{
        "provider": "openai",
        "accountKey": account,
    }])
    .to_string();
    let mut summary = live(
        Some(&account),
        vec![window(SessionLimitMetric::Weekly, 54.0)],
    );
    summary.providers[0].provider = "openai".to_string();
    summary.providers[0].display_name = "Codex".to_string();

    let allocations = weekly(&[row], &summary);

    assert_eq!(allocations.len(), 1);
    assert_eq!(allocations[0].provider, "openai");
    assert_eq!(allocations[0].percent, 54.0);
}

#[test]
fn multiple_known_accounts_require_an_exact_session_match() {
    let account_a = account('a');
    let account_b = account('b');
    let mut summary = live(
        Some(&account_a),
        vec![window(SessionLimitMetric::Weekly, 20.0)],
    );
    summary.providers.push(
        live(
            Some(&account_b),
            vec![window(SessionLimitMetric::Weekly, 60.0)],
        )
        .providers
        .remove(0),
    );
    let rows = [
        turn("a", NOW - 60, 1_000_000, &[&account_a]),
        turn("b", NOW - 60, 1_000_000, &[&account_b]),
        turn("none", NOW - 60, 1_000_000, &[]),
    ];
    let allocations = estimate(rows.to_vec(), &summary, &History::default(), NOW * 1_000);
    assert_eq!(allocations.len(), 2);
    assert!(allocations.iter().all(|entry| entry.session_id != "none"));
    assert_eq!(
        allocations
            .iter()
            .find(|entry| entry.session_id == "a")
            .unwrap()
            .percent,
        20.0
    );
    assert_eq!(
        allocations
            .iter()
            .find(|entry| entry.session_id == "b")
            .unwrap()
            .percent,
        60.0
    );
}

#[test]
fn conflicting_session_accounts_never_use_the_single_account_fallback() {
    let account_a = account('a');
    let account_b = account('b');
    let allocations = weekly(
        &[turn(
            "ambiguous",
            NOW - 60,
            1_000_000,
            &[&account_a, &account_b],
        )],
        &live(
            Some(&account_a),
            vec![window(SessionLimitMetric::Weekly, 20.0)],
        ),
    );
    assert!(allocations.is_empty());
}

#[test]
fn an_incomplete_price_cohort_falls_back_to_token_weight() {
    let mut unpriced = turn("unknown", NOW - 60, 1_000_000, &[]);
    unpriced.turns[0].model = Some("unknown-model-with-no-price".to_string());
    let allocations = weekly(
        &[turn("priced", NOW - 60, 3_000_000, &[]), unpriced],
        &live(None, vec![window(SessionLimitMetric::Weekly, 20.0)]),
    );
    let shares: HashMap<_, _> = allocations
        .into_iter()
        .map(|entry| (entry.session_id, entry.percent))
        .collect();
    assert!((shares["priced"] - 15.0).abs() < 1e-9);
    assert!((shares["unknown"] - 5.0).abs() < 1e-9);
}

#[test]
fn a_higher_confidence_window_wins_before_a_larger_percentage() {
    let priced = turn("mixed", NOW - 60, 1_000_000, &[]);
    let mut unpriced = turn("mixed", NOW - 30, 1_000_000, &[]);
    unpriced.turns[0].model = Some("unknown-model-with-no-price".to_string());
    let token_window = window(SessionLimitMetric::Weekly, 80.0);
    let mut price_window = window(SessionLimitMetric::Weekly, 20.0);
    price_window.id = "priced-model-weekly".to_string();
    price_window.scope_model = Some(MODEL.to_string());

    let allocations = weekly(
        &[priced, unpriced],
        &live(None, vec![token_window, price_window]),
    );

    assert_eq!(allocations.len(), 1);
    assert_eq!(allocations[0].window_id, "priced-model-weekly");
    assert_eq!(allocations[0].percent, 20.0);
}

#[test]
fn evidence_without_a_timestamp_or_provider_is_too_weak_to_allocate() {
    let mut missing_time = turn("missing-time", NOW - 60, 1_000_000, &[]);
    missing_time.turns[0].ts_ms = None;
    let mut unknown_provider = turn("unknown-provider", NOW - 60, 1_000_000, &[]);
    unknown_provider.key.agent = "opencode".to_string();
    unknown_provider.turns[0].model = Some("unrecognized-model".to_string());

    assert!(
        weekly(
            &[missing_time, unknown_provider],
            &live(None, vec![window(SessionLimitMetric::Weekly, 20.0)]),
        )
        .is_empty()
    );
}

#[test]
fn stale_and_invalid_live_windows_do_not_allocate() {
    let rows = [turn("one", NOW - 60, 1_000_000, &[])];
    let mut stale = live(None, vec![window(SessionLimitMetric::Weekly, 20.0)]);
    stale.providers[0].freshness = LiveUsageFreshness::Stale;
    assert!(weekly(&rows, &stale).is_empty());

    let mut expired = live(None, vec![window(SessionLimitMetric::Weekly, 20.0)]);
    expired.providers[0].windows[0].resets_at = Some(iso(NOW));
    assert!(weekly(&rows, &expired).is_empty());

    let mut missing = live(None, vec![window(SessionLimitMetric::Weekly, 20.0)]);
    missing.providers[0].windows[0].resets_at = None;
    assert!(weekly(&rows, &missing).is_empty());

    let current = live(None, vec![window(SessionLimitMetric::Weekly, 20.0)]);
    assert!(
        estimate(
            rows.to_vec(),
            &current,
            &History::default(),
            (NOW + 3_601) * 1_000,
        )
        .is_empty()
    );
}

#[test]
fn model_scoped_windows_include_only_matching_turns() {
    let mut scoped = window(SessionLimitMetric::Weekly, 30.0);
    scoped.scope_model = Some("Claude Opus 4.6".to_string());
    let mut other = turn("other", NOW - 60, 1_000_000, &[]);
    other.turns[0].model = Some("claude-sonnet-4-6".to_string());
    let allocations = weekly(
        &[turn("match", NOW - 60, 1_000_000, &[]), other],
        &live(None, vec![scoped]),
    );
    assert_eq!(allocations.len(), 1);
    assert_eq!(allocations[0].session_id, "match");
    assert_eq!(allocations[0].percent, 30.0);
}

#[test]
fn history_deltas_are_allocated_only_to_turns_in_each_interval() {
    let turns = weighted_turns(vec![
        turn("early", NOW - 90, 1_000_000, &[]),
        turn("late", NOW - 10, 1_000_000, &[]),
    ]);
    let refs: Vec<_> = turns.iter().collect();
    let samples = vec![
        UsageSample {
            observed_at: OffsetDateTime::from_unix_timestamp(NOW - 60).unwrap(),
            used_percent: Some(10.0),
            freshness: Freshness::Fresh,
        },
        UsageSample {
            observed_at: OffsetDateTime::from_unix_timestamp(NOW).unwrap(),
            used_percent: Some(30.0),
            freshness: Freshness::Fresh,
        },
    ];
    let (shares, uses_history) = distribute_window(
        &refs,
        30.0,
        (NOW - 100) * 1_000,
        NOW * 1_000,
        &samples,
        WeightBasis::Price,
    );
    assert!(uses_history);
    assert!((shares[&SessionKey::new("native", "claude-code", "early")] - 10.0).abs() < 1e-9);
    assert!((shares[&SessionKey::new("native", "claude-code", "late")] - 20.0).abs() < 1e-9);
}

#[test]
fn decreasing_history_falls_back_to_the_current_window_share() {
    let turns = weighted_turns(vec![
        turn("one", NOW - 90, 1_000_000, &[]),
        turn("two", NOW - 10, 1_000_000, &[]),
    ]);
    let refs: Vec<_> = turns.iter().collect();
    let samples = vec![
        UsageSample {
            observed_at: OffsetDateTime::from_unix_timestamp(NOW - 60).unwrap(),
            used_percent: Some(30.0),
            freshness: Freshness::Fresh,
        },
        UsageSample {
            observed_at: OffsetDateTime::from_unix_timestamp(NOW).unwrap(),
            used_percent: Some(20.0),
            freshness: Freshness::Fresh,
        },
    ];

    let (shares, uses_history) = distribute_window(
        &refs,
        20.0,
        (NOW - 100) * 1_000,
        NOW * 1_000,
        &samples,
        WeightBasis::Price,
    );
    assert!(!uses_history);

    assert!((shares[&SessionKey::new("native", "claude-code", "one")] - 10.0).abs() < 1e-9);
    assert!((shares[&SessionKey::new("native", "claude-code", "two")] - 10.0).abs() < 1e-9);
}

#[test]
fn an_interval_with_no_local_turns_remains_unallocated() {
    let turns = weighted_turns(vec![turn("late", NOW - 10, 1_000_000, &[])]);
    let refs: Vec<_> = turns.iter().collect();
    let samples = vec![
        UsageSample {
            observed_at: OffsetDateTime::from_unix_timestamp(NOW - 60).unwrap(),
            used_percent: Some(10.0),
            freshness: Freshness::Fresh,
        },
        UsageSample {
            observed_at: OffsetDateTime::from_unix_timestamp(NOW).unwrap(),
            used_percent: Some(30.0),
            freshness: Freshness::Fresh,
        },
    ];
    let (shares, uses_history) = distribute_window(
        &refs,
        30.0,
        (NOW - 100) * 1_000,
        NOW * 1_000,
        &samples,
        WeightBasis::Price,
    );
    assert!(uses_history);
    assert!((shares.values().sum::<f64>() - 20.0).abs() < 1e-9);
}

#[test]
fn allocation_is_invariant_to_turn_order_and_uniform_scaling() {
    for scale in [1, 10, 1_000] {
        let first = turn("one", NOW - 60, scale, &[]);
        let second = turn("two", NOW - 30, scale * 3, &[]);
        let expected = weekly(
            &[first.clone(), second.clone()],
            &live(None, vec![window(SessionLimitMetric::Weekly, 80.0)]),
        );
        let reversed = weekly(
            &[second, first],
            &live(None, vec![window(SessionLimitMetric::Weekly, 80.0)]),
        );
        assert_eq!(expected, reversed);
    }
}

#[test]
fn generated_token_fallback_cohorts_are_bounded_and_exhaustive() {
    for seed in 1..=24_u64 {
        let count = (seed % 9 + 1) as usize;
        let mut rows: Vec<_> = (0..count)
            .map(|index| {
                let mut row = turn(
                    &format!("session-{index}"),
                    NOW - index as i64,
                    (seed * (index as u64 + 1) * 97) % 10_000 + 1,
                    &[],
                );
                if index % 2 == 0 {
                    row.turns[0].model = Some(format!("unpriced-{index}"));
                }
                row
            })
            .collect();
        let expected = weekly(
            &rows,
            &live(None, vec![window(SessionLimitMetric::Weekly, 73.0)]),
        );
        rows.reverse();
        let reversed = weekly(
            &rows,
            &live(None, vec![window(SessionLimitMetric::Weekly, 73.0)]),
        );

        assert_eq!(expected, reversed);
        assert_eq!(expected.len(), count);
        assert!(expected.iter().all(|entry| entry.percent.is_finite()));
        assert!(
            expected
                .iter()
                .all(|entry| (0.0..=73.0).contains(&entry.percent))
        );
        assert!((expected.iter().map(|entry| entry.percent).sum::<f64>() - 73.0).abs() < 1e-9);
    }
}

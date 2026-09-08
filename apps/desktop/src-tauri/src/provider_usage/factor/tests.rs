use std::path::Path;

use rusqlite::params;

use super::*;
use crate::store::provider_limit::LANE_FIVE_HOUR;
use crate::store::{SessionKey, SessionRecord};

const PROVIDER: &str = "anthropic";
const AGENT: &str = "claude-code";
const MODEL: &str = "claude-opus-4-6";

fn account() -> String {
    "a".repeat(64)
}

fn memory_store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-limit-factor-test")).expect("opens store")
}

fn observe_account(store: &Store) {
    store
        .lock()
        .execute(
            "INSERT INTO provider_account_seen (agent, provider, account_key,
                 first_seen_epoch, last_seen_epoch)
             VALUES (?1, ?2, ?3, 1, 1)",
            params![AGENT, PROVIDER, account()],
        )
        .expect("records a seen account");
}

fn insert_session(store: &Store, session_id: &str) -> SessionKey {
    let key = SessionKey::new("native", AGENT, session_id);
    store
        .upsert_sessions(
            &[SessionRecord {
                key: key.clone(),
                source_kind: "inline".to_string(),
                source_label: "synthetic".to_string(),
                wsl_distro: None,
                title: None,
                title_source: None,
                cwd: None,
                surface: "unknown".to_string(),
                updated_at_epoch: Some(1),
                activity_cursor: "synthetic".to_string(),
                activity_source: "event".to_string(),
                subagent_count: 0,
                fork_parent_session_id: None,
                source_fingerprint: Some("synthetic".to_string()),
            }],
            &[],
        )
        .expect("stores synthetic session");
    key
}

/// Publish one turn at `ts_ms`, with its session evidence published, so the
/// attribution query's `session_evidence` join matches it.
fn insert_turn(store: &Store, key: &SessionKey, ts_ms: i64, input_tokens: i64) {
    let connection = store.lock();
    connection
        .execute(
            "INSERT OR IGNORE INTO session_evidence (
                 environment_key, agent, session_id, status, published_fence
             ) VALUES (?1, ?2, ?3, 'ready', 1)",
            params![key.environment_key, key.agent, key.session_id],
        )
        .expect("publishes synthetic evidence");
    connection
        .execute(
            "INSERT INTO turn (
                 environment_key, agent, session_id, claim_fence, source_key,
                 thread_id, turn_index, scope, role, ts_ms, model, effort, speed,
                 input_tokens, cache_read_tokens, cache_write_tokens, output_tokens,
                 is_compaction_boundary, message_id, uuid, parent_uuid
             ) VALUES (?1, ?2, ?3, 1, 'synthetic', 'synthetic', 0, 'main', 'assistant',
                       ?4, ?5, NULL, NULL, ?6, 0, 0, 0, 0, NULL, NULL, NULL)",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                ts_ms,
                MODEL,
                input_tokens
            ],
        )
        .expect("stores synthetic turn");
}

fn insert_period(store: &Store, start: i64, reset: i64) -> i64 {
    let connection = store.lock();
    connection
        .execute(
            "INSERT INTO provider_usage_period (
                 provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, duration_seconds, starts_at_epoch,
                 resets_at_epoch, first_observed_epoch, last_observed_epoch
             ) VALUES (?1, ?2, 'five-hour', 'rolling', 'primaryShort',
                       'account', 'account', ?3, ?4, ?5, ?4, ?4)",
            params![PROVIDER, account(), reset - start, start, reset],
        )
        .expect("inserts a synthetic period");
    connection.last_insert_rowid()
}

fn push_observation(
    store: &Store,
    period_id: i64,
    observed_at: i64,
    used_percent: f64,
    plan: Option<&str>,
) {
    push_observation_with_tier(store, period_id, observed_at, used_percent, plan, None);
}

fn push_observation_with_tier(
    store: &Store,
    period_id: i64,
    observed_at: i64,
    used_percent: f64,
    plan: Option<&str>,
    plan_tier: Option<&str>,
) {
    let connection = store.lock();
    connection
        .execute(
            "INSERT INTO provider_usage_observation (
                 period_id, provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                 is_authoritative, confidence, source_id, plan, plan_tier
             ) SELECT id, provider, account_key, window_id, window_kind, window_role,
                      scope_key, scope_label, ?2, ?3, 1, 1, 'high', 'test', ?4, ?5
                 FROM provider_usage_period WHERE id = ?1",
            params![period_id, observed_at, used_percent, plan, plan_tier],
        )
        .expect("stores a synthetic observation");
    connection
        .execute(
            "UPDATE provider_usage_period
                SET first_observed_epoch = MIN(first_observed_epoch, ?2),
                    last_observed_epoch = MAX(last_observed_epoch, ?2)
              WHERE id = ?1",
            params![period_id, observed_at],
        )
        .expect("advances the period's observed range");
}

fn sample_at(total_usd: f64, percent_delta: f64, to_epoch: i64) -> FactorSample {
    FactorSample {
        provider: PROVIDER.to_string(),
        account_key: account(),
        lane: LANE_FIVE_HOUR.to_string(),
        kind: "delta".to_string(),
        period_id: None,
        from_epoch: to_epoch - 100,
        to_epoch,
        from_percent: 0.0,
        to_percent: percent_delta,
        input_usd: total_usd,
        output_usd: 0.0,
        cache_read_usd: 0.0,
        cache_write_usd: 0.0,
        turn_count: 1,
        plan: None,
        plan_tier: None,
        source_id: "test".to_string(),
        computed_at_epoch: to_epoch,
    }
}

#[test]
fn delta_sample_arithmetic_prices_turns_between_two_readings() {
    let store = memory_store();
    observe_account(&store);
    let key = insert_session(&store, "s1");
    insert_turn(&store, &key, 150_000, 200_000); // 200,000 * 5e-6 = $1.00
    let period_id = insert_period(&store, 0, 18_000);
    push_observation(&store, period_id, 100, 10.0, None);
    push_observation(&store, period_id, 200, 15.0, None);

    learn(&store, 300);

    let samples = store
        .all_delta_factor_samples(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap();
    assert_eq!(samples.len(), 1);
    let sample = &samples[0];
    assert_eq!(sample.from_epoch, 100);
    assert_eq!(sample.to_epoch, 200);
    assert!((sample.from_percent - 10.0).abs() < 1e-9);
    assert!((sample.to_percent - 15.0).abs() < 1e-9);
    assert!((sample.total_usd() - 1.0).abs() < 1e-9);
    assert_eq!(sample.turn_count, 1);

    let point = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("a point is learned from the one delta sample");
    assert!((point.usd_per_percent - 0.2).abs() < 1e-9);
    assert_eq!(point.method, "delta");
    assert_eq!(point.sample_count, 1);
}

#[test]
fn equal_readings_merge_into_one_interval_spanning_the_whole_plateau() {
    let store = memory_store();
    observe_account(&store);
    let key = insert_session(&store, "s1");
    insert_turn(&store, &key, 120_000, 100_000); // $0.50, before the plateau's end
    insert_turn(&store, &key, 180_000, 100_000); // $0.50, after the plateau
    let period_id = insert_period(&store, 0, 18_000);
    push_observation(&store, period_id, 100, 10.0, None);
    push_observation(&store, period_id, 150, 10.0, None); // equal: merges with 100
    push_observation(&store, period_id, 200, 15.0, None);

    learn(&store, 300);

    let samples = store
        .all_delta_factor_samples(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap();
    assert_eq!(
        samples.len(),
        1,
        "the plateau produces one interval, not two"
    );
    assert_eq!(samples[0].from_epoch, 100);
    assert_eq!(samples[0].to_epoch, 200);
    assert!((samples[0].total_usd() - 1.0).abs() < 1e-9);
    assert_eq!(samples[0].turn_count, 2);
}

#[test]
fn a_positive_delta_with_no_local_turns_is_stored_as_unattributed() {
    let store = memory_store();
    observe_account(&store);
    let period_id = insert_period(&store, 0, 18_000);
    push_observation(&store, period_id, 100, 10.0, None);
    push_observation(&store, period_id, 200, 20.0, None);

    learn(&store, 300);

    assert!(
        store
            .all_delta_factor_samples(PROVIDER, &account(), LANE_FIVE_HOUR)
            .unwrap()
            .is_empty(),
        "no dollars behind the delta means no 'delta' sample"
    );
    let kind: String = store
        .lock()
        .query_row(
            "SELECT kind FROM provider_limit_factor_sample WHERE from_epoch = 100 AND to_epoch = 200",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "unattributed");
    assert!(
        store
            .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
            .unwrap()
            .is_none(),
        "zero dollars behind every sample earns no factor"
    );
}

#[test]
fn window_start_only_forms_while_no_delta_sample_exists_yet() {
    let store = memory_store();
    observe_account(&store);

    // Period 1: one positive reading, no pair possible. Window start is the
    // only sample this lane can produce yet.
    let key1 = insert_session(&store, "s1");
    insert_turn(&store, &key1, 250_000, 100_000); // $0.50
    let period1 = insert_period(&store, 0, 18_000);
    push_observation(&store, period1, 500, 8.0, None);
    learn(&store, 600);

    let window_start = store
        .latest_window_start_factor_sample(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the first positive reading seeds a window-start sample");
    assert_eq!(window_start.from_epoch, 0);
    assert_eq!(window_start.to_epoch, 500);
    assert!(
        !store
            .has_delta_factor_sample(PROVIDER, &account(), LANE_FIVE_HOUR, None, None)
            .unwrap()
    );
    let point = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("a window-start point seeds the badge before any delta exists");
    assert_eq!(point.method, "window_start");

    // Period 2: two readings in one lane, so a real delta sample appears.
    let key2 = insert_session(&store, "s2");
    insert_turn(&store, &key2, 18_150_000, 100_000); // $0.50
    let period2 = insert_period(&store, 18_000, 36_000);
    push_observation(&store, period2, 18_100, 5.0, None);
    push_observation(&store, period2, 18_200, 10.0, None);
    learn(&store, 18_300);
    assert!(
        store
            .has_delta_factor_sample(PROVIDER, &account(), LANE_FIVE_HOUR, None, None)
            .unwrap()
    );

    // Period 3: another single positive reading. No new window-start sample
    // forms now that a delta sample exists for the lane.
    let period3 = insert_period(&store, 36_000, 54_000);
    push_observation(&store, period3, 36_100, 6.0, None);
    learn(&store, 36_200);

    let still_the_first_window_start = store
        .latest_window_start_factor_sample(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the earlier window-start sample is untouched");
    assert_eq!(still_the_first_window_start.to_epoch, 500);
}

#[test]
fn the_weighted_median_is_not_pulled_by_one_outlier() {
    let now = 1_000;
    let mut samples: Vec<FactorSample> = (0..5).map(|_| sample_at(5.0, 5.0, now)).collect();
    samples.push(sample_at(500.0, 5.0, now)); // value 100.0, same weight as the rest
    let median = weighted_median(&samples, now);
    assert!(
        (median - 1.0).abs() < 1e-9,
        "median should stay at 1.0, not the mean's ~17.5, got {median}"
    );
}

#[test]
fn a_point_is_appended_only_when_the_factor_actually_changes() {
    let store = memory_store();
    observe_account(&store);
    let key = insert_session(&store, "s1");
    insert_turn(&store, &key, 150_000, 200_000); // $1.00
    let period_id = insert_period(&store, 0, 18_000);
    push_observation(&store, period_id, 100, 10.0, None);
    push_observation(&store, period_id, 200, 15.0, None);

    learn(&store, 300);
    assert_eq!(count_points(&store), 1);

    // Recomputing the same pair with nothing new changes nothing.
    learn(&store, 400);
    assert_eq!(
        count_points(&store),
        1,
        "an unchanged factor earns no new point"
    );

    // A late-arriving turn more than triples the dollars behind the same
    // pair, moving the factor well past the 2% threshold. The point is dated
    // by the sample's own to_epoch (200), same as before, so this replaces
    // the existing point rather than appending a second one.
    insert_turn(&store, &key, 180_000, 400_000); // +$2.00
    learn(&store, 500);
    assert_eq!(
        count_points(&store),
        1,
        "a recompute of the same interval replaces its point, not duplicates it"
    );
    let latest = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .unwrap();
    assert!((latest.usd_per_percent - 0.6).abs() < 1e-9);
    assert_eq!(
        latest.effective_at_epoch, 200,
        "the point is dated by the sample's to_epoch, not the moment it was computed"
    );
}

#[test]
fn the_recompute_window_upserts_a_late_arriving_turn_into_the_same_sample() {
    let store = memory_store();
    observe_account(&store);
    let period_id = insert_period(&store, 0, 18_000);
    push_observation(&store, period_id, 100, 10.0, None);
    push_observation(&store, period_id, 200, 15.0, None);

    learn(&store, 300);
    let kind: String = store
        .lock()
        .query_row(
            "SELECT kind FROM provider_limit_factor_sample WHERE from_epoch = 100 AND to_epoch = 200",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "unattributed", "no turns exist for this pair yet");

    let key = insert_session(&store, "s1");
    insert_turn(&store, &key, 150_000, 200_000); // $1.00, arrives late
    learn(&store, 400);

    let row_count: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM provider_limit_factor_sample
              WHERE from_epoch = 100 AND to_epoch = 200",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(row_count, 1, "the pair is upserted, not duplicated");
    let samples = store
        .all_delta_factor_samples(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap();
    assert_eq!(samples.len(), 1);
    assert!((samples[0].total_usd() - 1.0).abs() < 1e-9);
}

#[test]
fn a_zero_percent_reading_never_seeds_a_sample() {
    let store = memory_store();
    observe_account(&store);
    // A Codex reading at 0% with a projected reset carries no meaningful
    // window boundary; `is_sliding_reset_projection` upstream already drops
    // the stated reset. Whether or not a reset survived, a zero reading must
    // never seed a window-start or delta sample.
    let period_id = insert_period(&store, 0, 18_000);
    push_observation(&store, period_id, 100, 0.0, None);

    learn(&store, 200);

    assert!(
        store
            .latest_window_start_factor_sample(PROVIDER, &account(), LANE_FIVE_HOUR)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .all_delta_factor_samples(PROVIDER, &account(), LANE_FIVE_HOUR)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_plan_change_drops_earlier_samples_and_appends_a_point() {
    let store = memory_store();
    observe_account(&store);
    let key = insert_session(&store, "s1");
    let period_id = insert_period(&store, 0, 18_000);

    // Three "pro" delta pairs, $1.00 each for a 5-point delta: factor 0.2.
    push_observation(&store, period_id, 100, 10.0, Some("pro"));
    push_observation(&store, period_id, 200, 15.0, Some("pro"));
    push_observation(&store, period_id, 300, 20.0, Some("pro"));
    push_observation(&store, period_id, 400, 25.0, Some("pro"));
    insert_turn(&store, &key, 150_000, 200_000); // $1.00
    insert_turn(&store, &key, 250_000, 200_000); // $1.00
    insert_turn(&store, &key, 350_000, 200_000); // $1.00
    learn(&store, 450);

    let pro_point = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the pro-plan samples seed a point");
    assert_eq!(pro_point.plan.as_deref(), Some("pro"));
    assert!((pro_point.usd_per_percent - 0.2).abs() < 1e-9);

    // A new "max"-plan reading, priced twice as expensive per token: factor
    // should reflect only this new-plan sample, not blend with the old plan.
    push_observation(&store, period_id, 500, 30.0, Some("max"));
    insert_turn(&store, &key, 450_000, 400_000); // $2.00
    learn(&store, 600);

    let max_point = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the plan change appends a new point");
    assert_eq!(max_point.plan.as_deref(), Some("max"));
    assert!(
        (max_point.usd_per_percent - 0.4).abs() < 1e-9,
        "expected the new-plan sample alone (0.4), got {}",
        max_point.usd_per_percent
    );
    assert_eq!(count_points(&store), 2);
}

#[test]
fn learning_records_one_residual_row_per_period() {
    let store = memory_store();
    observe_account(&store);
    let key = insert_session(&store, "s1");
    insert_turn(&store, &key, 150_000, 200_000); // $1.00
    let period_id = insert_period(&store, 0, 18_000);
    push_observation(&store, period_id, 100, 10.0, None);
    push_observation(&store, period_id, 200, 15.0, None);

    learn(&store, 300);

    let (meter_percent, estimated_percent): (f64, f64) = store
        .lock()
        .query_row(
            "SELECT meter_percent, estimated_percent FROM provider_limit_residual
              WHERE period_id = ?1",
            [period_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("one residual row exists for the period");
    assert!((meter_percent - 15.0).abs() < 1e-9);
    // Attributed dollars from the window start (epoch 0) through the latest
    // observation (epoch 200) is the same $1.00 turn, divided by the 0.2
    // factor: 5.0 estimated percent.
    assert!(
        (estimated_percent - 5.0).abs() < 1e-9,
        "expected 5.0, got {estimated_percent}"
    );

    let row_count: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM provider_limit_residual WHERE period_id = ?1",
            [period_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(row_count, 1);
}

#[test]
fn a_plan_tier_change_with_the_same_plan_drops_earlier_samples_and_appends_a_point() {
    let store = memory_store();
    observe_account(&store);
    let key = insert_session(&store, "s1");
    let period_id = insert_period(&store, 0, 18_000);

    // Three "max" / "standard"-tier delta pairs, $1.00 each for a 5-point
    // delta: factor 0.2. The plan itself never changes.
    push_observation_with_tier(&store, period_id, 100, 10.0, Some("max"), Some("standard"));
    push_observation_with_tier(&store, period_id, 200, 15.0, Some("max"), Some("standard"));
    push_observation_with_tier(&store, period_id, 300, 20.0, Some("max"), Some("standard"));
    push_observation_with_tier(&store, period_id, 400, 25.0, Some("max"), Some("standard"));
    insert_turn(&store, &key, 150_000, 200_000); // $1.00
    insert_turn(&store, &key, 250_000, 200_000); // $1.00
    insert_turn(&store, &key, 350_000, 200_000); // $1.00
    learn(&store, 450);

    let standard_point = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the standard-tier samples seed a point");
    assert_eq!(standard_point.plan.as_deref(), Some("max"));
    assert_eq!(standard_point.plan_tier.as_deref(), Some("standard"));
    assert!((standard_point.usd_per_percent - 0.2).abs() < 1e-9);

    // A tier change alone (plan stays "max") priced twice as expensive per
    // token: the factor should reflect only the new-tier sample.
    push_observation_with_tier(&store, period_id, 500, 30.0, Some("max"), Some("pro"));
    insert_turn(&store, &key, 450_000, 400_000); // $2.00
    learn(&store, 600);

    let pro_point = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the tier change appends a new point");
    assert_eq!(pro_point.plan.as_deref(), Some("max"));
    assert_eq!(pro_point.plan_tier.as_deref(), Some("pro"));
    assert!(
        (pro_point.usd_per_percent - 0.4).abs() < 1e-9,
        "expected the new-tier sample alone (0.4), got {}",
        pro_point.usd_per_percent
    );
    assert_eq!(count_points(&store), 2);
}

#[test]
fn a_point_computed_from_old_observations_is_dated_by_their_epoch_not_by_now() {
    let store = memory_store();
    observe_account(&store);
    let key = insert_session(&store, "s1");
    // Readings from two days ago, as a bootstrap or backfill pass would see.
    let two_days_ago = 2 * 86_400;
    let period_id = insert_period(&store, two_days_ago, two_days_ago + 18_000);
    insert_turn(&store, &key, (two_days_ago + 150) * 1_000, 200_000); // $1.00
    push_observation(&store, period_id, two_days_ago + 100, 10.0, None);
    push_observation(&store, period_id, two_days_ago + 200, 15.0, None);

    // The pass itself runs today, long after the data it is learning from.
    let today = 5 * 86_400;
    learn(&store, today);

    let point = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the old sample seeds a point");
    assert_eq!(
        point.effective_at_epoch,
        two_days_ago + 200,
        "the point is dated by the sample it came from, not by when the pass ran"
    );
}

#[test]
fn a_tier_change_with_no_delta_yet_seeds_a_window_start_sample_and_point() {
    let store = memory_store();
    observe_account(&store);

    // Period 1: two "standard"-tier readings form a delta sample and point.
    let key1 = insert_session(&store, "s1");
    insert_turn(&store, &key1, 150_000, 200_000); // $1.00
    let period1 = insert_period(&store, 0, 18_000);
    push_observation_with_tier(&store, period1, 100, 10.0, Some("max"), Some("standard"));
    push_observation_with_tier(&store, period1, 200, 15.0, Some("max"), Some("standard"));
    learn(&store, 300);
    let standard_point = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the standard-tier delta seeds a point");
    assert_eq!(standard_point.method, "delta");

    // Period 2: the account moves to the "pro" tier. Its first reading has
    // no partner yet, so only a window-start sample can form for this tier
    // — and it can, because the delta check is scoped to (plan, plan_tier).
    let key2 = insert_session(&store, "s2");
    insert_turn(&store, &key2, 18_050_000, 200_000); // $1.00
    let period2 = insert_period(&store, 18_000, 36_000);
    push_observation_with_tier(&store, period2, 18_100, 5.0, Some("max"), Some("pro"));
    learn(&store, 18_200);

    let window_start = store
        .latest_window_start_factor_sample(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the new tier's first positive reading seeds a window-start sample");
    assert_eq!(window_start.plan_tier.as_deref(), Some("pro"));

    let pro_point = store
        .latest_factor_point(PROVIDER, &account(), LANE_FIVE_HOUR)
        .unwrap()
        .expect("the window-start sample seeds a new point");
    assert_eq!(pro_point.method, "window_start");
    assert_eq!(pro_point.plan_tier.as_deref(), Some("pro"));
}

fn count_points(store: &Store) -> i64 {
    store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM provider_limit_factor_point",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

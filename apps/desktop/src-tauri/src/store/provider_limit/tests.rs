use std::path::Path;

use rusqlite::params;

use super::*;
use crate::store::{RETAIN_SESSION_DATA_FOREVER, SessionRecord};

const PROVIDER: &str = "anthropic";
const AGENT: &str = "claude-code";
const MODEL: &str = "claude-opus-4-6";

fn account(character: char) -> String {
    character.to_string().repeat(64)
}

fn memory_store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-provider-limit-test")).expect("opens store")
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

/// Publish one turn at `ts_ms` and mark its session evidence published, so
/// the attribution query's `session_evidence` join matches it.
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

/// Publish one turn whose cache-write tokens are all one-hour writes, so the
/// attribution query's cache-write pricing can be checked at the double rate.
fn insert_turn_with_one_hour_cache_write(
    store: &Store,
    key: &SessionKey,
    ts_ms: i64,
    cache_write_tokens: i64,
) {
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
                 input_tokens, cache_read_tokens, cache_write_tokens,
                 cache_write_1h_tokens, output_tokens,
                 is_compaction_boundary, message_id, uuid, parent_uuid
             ) VALUES (?1, ?2, ?3, 1, 'synthetic', 'synthetic', 0, 'main', 'assistant',
                       ?4, ?5, NULL, NULL, 0, 0, ?6, ?7, 0, 0, NULL, NULL, NULL)",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                ts_ms,
                MODEL,
                cache_write_tokens,
                cache_write_tokens
            ],
        )
        .expect("stores synthetic turn");
}

fn bind_account(store: &Store, key: &SessionKey, account_key: &str) {
    store
        .lock()
        .execute(
            "INSERT INTO session_provider_account (
                 environment_key, agent, session_id, provider, account_key,
                 provenance, confidence, first_seen_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'provider_live', 'direct', '2026-01-01T00:00:00Z')",
            params![
                key.environment_key,
                key.agent,
                key.session_id,
                PROVIDER,
                account_key
            ],
        )
        .expect("binds session account");
}

fn insert_period(
    store: &Store,
    account_key: &str,
    start: i64,
    reset: i64,
    last_observed: i64,
) -> i64 {
    let connection = store.lock();
    connection
        .execute(
            "INSERT INTO provider_usage_period (
                 provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, duration_seconds, starts_at_epoch,
                 resets_at_epoch, first_observed_epoch, last_observed_epoch
             ) VALUES (?1, ?2, 'five-hour', 'rolling', 'primaryShort',
                       'account', 'account', ?3, ?4, ?5, ?6, ?6)",
            params![
                PROVIDER,
                account_key,
                reset - start,
                start,
                reset,
                last_observed
            ],
        )
        .expect("inserts a synthetic period");
    connection.last_insert_rowid()
}

fn observe_account(store: &Store, account_key: &str) {
    store
        .lock()
        .execute(
            "INSERT INTO provider_account_seen (agent, provider, account_key,
                 first_seen_epoch, last_seen_epoch)
             VALUES (?1, ?2, ?3, 1, 1)",
            params![AGENT, PROVIDER, account_key],
        )
        .expect("records a seen account");
}

#[test]
fn an_unbound_session_with_two_known_accounts_attributes_nothing() {
    let store = memory_store();
    let key = insert_session(&store, "session");
    insert_turn(&store, &key, 150_000, 200_000);
    observe_account(&store, &account('a'));
    observe_account(&store, &account('b'));

    let dollars = store
        .attributed_turn_dollars_between(PROVIDER, &account('a'), 0, 1_000, None)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert!(
        dollars.is_empty(),
        "an ambiguous account set attributes no session"
    );
}

#[test]
fn an_unbound_session_with_one_known_account_falls_back_to_it() {
    let store = memory_store();
    let key = insert_session(&store, "session");
    insert_turn(&store, &key, 150_000, 200_000);
    observe_account(&store, &account('a'));

    let dollars = store
        .attributed_turn_dollars_between(PROVIDER, &account('a'), 0, 1_000, None)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert_eq!(dollars.len(), 1);
    assert_eq!(dollars[0].key, key);
    // claude-opus-4-6 test pricing: 5e-6 dollars per input token.
    assert!((dollars[0].input_usd - 1.0).abs() < 1e-9);
    assert_eq!(dollars[0].turn_count, 1);
}

#[test]
fn attribution_prices_one_hour_cache_writes_at_double_the_input_rate() {
    let store = memory_store();
    let key = insert_session(&store, "session");
    insert_turn_with_one_hour_cache_write(&store, &key, 150_000, 100_000);
    observe_account(&store, &account('a'));

    let dollars = store
        .attributed_turn_dollars_between(PROVIDER, &account('a'), 0, 1_000, None)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert_eq!(dollars.len(), 1);
    assert_eq!(dollars[0].key, key);
    // claude-opus-4-6 test pricing: 5e-6 dollars per input token, so the
    // one-hour cache-write subset prices at 1e-5 dollars per token.
    assert!((dollars[0].cache_write_usd - 1.0).abs() < 1e-9);
}

#[test]
fn a_directly_bound_session_ignores_the_single_account_fallback() {
    let store = memory_store();
    let key = insert_session(&store, "session");
    insert_turn(&store, &key, 150_000, 200_000);
    bind_account(&store, &key, &account('a'));
    observe_account(&store, &account('b'));

    let dollars = store
        .attributed_turn_dollars_between(PROVIDER, &account('b'), 0, 1_000, None)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert!(
        dollars.is_empty(),
        "a session bound to a different account never falls back"
    );

    let dollars = store
        .attributed_turn_dollars_between(PROVIDER, &account('a'), 0, 1_000, None)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert_eq!(dollars.len(), 1);
}

#[test]
fn point_lookup_at_an_epoch_uses_the_latest_point_at_or_before_it_else_the_earliest() {
    let store = memory_store();
    let lane = LANE_FIVE_HOUR;
    store
        .upsert_factor_point(&FactorPoint {
            id: 0,
            provider: PROVIDER.to_string(),
            account_key: account('a'),
            lane: lane.to_string(),
            effective_at_epoch: 1_000,
            usd_per_percent: 0.2,
            method: "delta".to_string(),
            sample_count: 3,
            plan: None,
            plan_tier: None,
        })
        .unwrap();
    store
        .upsert_factor_point(&FactorPoint {
            id: 0,
            provider: PROVIDER.to_string(),
            account_key: account('a'),
            lane: lane.to_string(),
            effective_at_epoch: 2_000,
            usd_per_percent: 0.3,
            method: "delta".to_string(),
            sample_count: 4,
            plan: None,
            plan_tier: None,
        })
        .unwrap();

    let before_any = store
        .factor_point_at(PROVIDER, &account('a'), lane, 500)
        .unwrap()
        .expect("falls back to the earliest point");
    assert_eq!(before_any.effective_at_epoch, 1_000);

    let between = store
        .factor_point_at(PROVIDER, &account('a'), lane, 1_500)
        .unwrap()
        .expect("uses the latest point at or before the epoch");
    assert_eq!(between.effective_at_epoch, 1_000);

    let after_both = store
        .factor_point_at(PROVIDER, &account('a'), lane, 5_000)
        .unwrap()
        .expect("uses the newest point");
    assert_eq!(after_both.effective_at_epoch, 2_000);
}

#[test]
fn sample_retention_deletes_old_samples_but_never_points() {
    let store = memory_store();
    let lane = LANE_FIVE_HOUR;
    let sample = FactorSample {
        provider: PROVIDER.to_string(),
        account_key: account('a'),
        lane: lane.to_string(),
        kind: "delta".to_string(),
        period_id: None,
        from_epoch: 0,
        to_epoch: 100,
        from_percent: 0.0,
        to_percent: 5.0,
        input_usd: 1.0,
        output_usd: 0.0,
        cache_read_usd: 0.0,
        cache_write_usd: 0.0,
        turn_count: 1,
        plan: None,
        plan_tier: None,
        source_id: "test".to_string(),
        computed_at_epoch: 100,
    };
    store.upsert_factor_sample(&sample).unwrap();
    store
        .upsert_factor_point(&FactorPoint {
            id: 0,
            provider: PROVIDER.to_string(),
            account_key: account('a'),
            lane: lane.to_string(),
            effective_at_epoch: 100,
            usd_per_percent: 0.2,
            method: "delta".to_string(),
            sample_count: 1,
            plan: None,
            plan_tier: None,
        })
        .unwrap();

    // 20 days elapsed, 30-day retention: the cutoff has not yet reached the
    // sample's to_epoch (100), so nothing is removed yet.
    let now = 20 * 86_400;
    apply_sample_retention_in(&store.lock(), 30, now).unwrap();
    assert_eq!(
        store
            .all_delta_factor_samples(PROVIDER, &account('a'), lane)
            .unwrap()
            .len(),
        1
    );

    // 40 days elapsed, the same 30-day retention: the cutoff has moved past
    // the sample's to_epoch, so the sample goes; the point does not.
    let now = 40 * 86_400;
    apply_sample_retention_in(&store.lock(), 30, now).unwrap();
    assert!(
        store
            .all_delta_factor_samples(PROVIDER, &account('a'), lane)
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .latest_factor_point(PROVIDER, &account('a'), lane)
            .unwrap()
            .is_some(),
        "retention never deletes factor points"
    );
}

/// One sample of `kind`, spanning `from_epoch..to_epoch`, otherwise
/// identical to [`sample_retention_deletes_old_samples_but_never_points`]'s
/// fixture.
fn kind_sample(kind: &str, from_epoch: i64, to_epoch: i64) -> FactorSample {
    FactorSample {
        provider: PROVIDER.to_string(),
        account_key: account('a'),
        lane: LANE_FIVE_HOUR.to_string(),
        kind: kind.to_string(),
        period_id: None,
        from_epoch,
        to_epoch,
        from_percent: 0.0,
        to_percent: 5.0,
        input_usd: 1.0,
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
fn a_rollout_sample_counts_the_same_as_a_delta_sample() {
    let store = memory_store();
    let lane = LANE_FIVE_HOUR;
    store
        .upsert_factor_sample(&kind_sample("delta", 0, 100))
        .unwrap();
    store
        .upsert_factor_sample(&kind_sample("rollout", 100, 200))
        .unwrap();
    store
        .upsert_factor_sample(&kind_sample("unattributed", 200, 300))
        .unwrap();
    store
        .upsert_factor_sample(&kind_sample("window_start", 300, 400))
        .unwrap();

    let all = store
        .all_delta_factor_samples(PROVIDER, &account('a'), lane)
        .unwrap();
    assert_eq!(
        all.iter()
            .map(|sample| sample.kind.as_str())
            .collect::<Vec<_>>(),
        vec!["delta", "rollout"],
        "the median's sample pool takes delta and rollout alike, and nothing else"
    );

    let recent = store
        .delta_factor_samples_since(PROVIDER, &account('a'), lane, 0)
        .unwrap();
    assert_eq!(recent.len(), 2);
}

#[test]
fn a_rollout_only_history_satisfies_has_delta_factor_sample() {
    let store = memory_store();
    let lane = LANE_FIVE_HOUR;
    assert!(
        !store
            .has_delta_factor_sample(PROVIDER, &account('a'), lane, None, None)
            .unwrap()
    );

    store
        .upsert_factor_sample(&kind_sample("rollout", 0, 100))
        .unwrap();

    assert!(
        store
            .has_delta_factor_sample(PROVIDER, &account('a'), lane, None, None)
            .unwrap(),
        "a rollout sample blocks a window-start sample from forming, the same as a delta sample"
    );
}

#[test]
fn a_sample_s_period_reference_is_nulled_before_its_period_is_deleted() {
    let store = memory_store();
    let account_key = account('a');
    let period_id = {
        let connection = store.lock();
        connection
            .execute(
                "INSERT INTO provider_usage_period (
                     provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, duration_seconds, starts_at_epoch,
                     resets_at_epoch, first_observed_epoch, last_observed_epoch
                 ) VALUES (?1, ?2, 'five-hour', 'rolling', 'primaryShort',
                           'account', 'account', 18000, 0, 18000, 10, 10)",
                params![PROVIDER, account_key],
            )
            .unwrap();
        let period_id = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO provider_usage_observation (
                     period_id, provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                     is_authoritative, confidence, source_id
                 ) VALUES (?1, ?2, ?3, 'five-hour', 'rolling', 'primaryShort',
                           'account', 'account', 10, 5.0, 1, 1, 'high', 'test')",
                params![period_id, PROVIDER, account_key],
            )
            .unwrap();
        period_id
    };
    let recent = 190 * 86_400;
    store
        .upsert_factor_sample(&FactorSample {
            provider: PROVIDER.to_string(),
            account_key: account_key.clone(),
            lane: LANE_FIVE_HOUR.to_string(),
            kind: "delta".to_string(),
            period_id: Some(period_id),
            from_epoch: 0,
            to_epoch: recent,
            from_percent: 0.0,
            to_percent: 5.0,
            input_usd: 1.0,
            output_usd: 0.0,
            cache_read_usd: 0.0,
            cache_write_usd: 0.0,
            turn_count: 1,
            plan: None,
            plan_tier: None,
            source_id: "test".to_string(),
            computed_at_epoch: recent,
        })
        .unwrap();

    // A 30-day cutoff removes the old observation and, with it, the now
    // orphaned period. The sample's own `to_epoch` is recent enough to
    // survive that same cutoff, so it must not keep a dangling period id.
    let now = 200 * 86_400;
    Store::apply_provider_usage_retention_in(&store.lock(), 30, now).unwrap();

    let samples = store
        .all_delta_factor_samples(PROVIDER, &account_key, LANE_FIVE_HOUR)
        .unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(
        samples[0].period_id, None,
        "the deleted period's id is nulled, not left dangling"
    );
}

#[test]
fn a_period_with_no_learn_cursor_is_a_candidate_however_old_its_last_reading() {
    let store = memory_store();
    // Last observed two days ago: a "recently observed" rule alone would
    // never pick this up. Bootstrap on upgrade relies on it being a
    // candidate anyway, since it has no cursor row yet.
    let period_id = insert_period(&store, &account('a'), 0, 18_000, 2 * 86_400);

    let now = 3 * 86_400;
    let periods = store
        .provider_limit_candidate_periods(now - 900)
        .expect("query succeeds");
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0].id, period_id);
}

#[test]
fn a_period_whose_cursor_has_caught_up_and_gone_stale_is_not_a_candidate() {
    let store = memory_store();
    let last_observed = 2 * 86_400;
    let period_id = insert_period(&store, &account('a'), 0, 18_000, last_observed);
    store
        .advance_learn_cursor(period_id, last_observed)
        .expect("advances the cursor");

    let now = 3 * 86_400;
    let periods = store
        .provider_limit_candidate_periods(now - 900)
        .expect("query succeeds");
    assert!(
        periods.is_empty(),
        "a cursor already at the period's last reading, outside the recompute window, is not re-read"
    );
}

#[test]
fn a_period_with_a_cursor_behind_its_last_reading_is_a_candidate() {
    let store = memory_store();
    let last_observed = 2 * 86_400;
    let period_id = insert_period(&store, &account('a'), 0, 18_000, last_observed);
    // A backfill (or an earlier partial pass) left the cursor behind the
    // period's newest reading.
    store
        .advance_learn_cursor(period_id, last_observed - 1)
        .expect("advances the cursor");

    let now = 3 * 86_400;
    let periods = store
        .provider_limit_candidate_periods(now - 900)
        .expect("query succeeds");
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0].id, period_id);
}

#[test]
fn a_period_s_learn_cursor_is_deleted_before_its_period_is_deleted() {
    let store = memory_store();
    let account_key = account('a');
    let period_id = {
        let connection = store.lock();
        connection
            .execute(
                "INSERT INTO provider_usage_period (
                     provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, duration_seconds, starts_at_epoch,
                     resets_at_epoch, first_observed_epoch, last_observed_epoch
                 ) VALUES (?1, ?2, 'five-hour', 'rolling', 'primaryShort',
                           'account', 'account', 18000, 0, 18000, 10, 10)",
                params![PROVIDER, account_key],
            )
            .unwrap();
        connection.last_insert_rowid()
    };
    store
        .advance_learn_cursor(period_id, 10)
        .expect("advances the cursor");

    let now = 200 * 86_400;
    Store::apply_provider_usage_retention_in(&store.lock(), 400, now).unwrap();

    let remaining: i64 = store
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM provider_limit_learn_cursor WHERE period_id = ?1",
            [period_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        remaining, 0,
        "the cursor is deleted, not left pointing at a removed period"
    );
}

#[test]
fn forever_retention_keeps_every_provider_usage_row() {
    let store = memory_store();
    let account_key = account('a');
    let period_id = insert_period(&store, &account_key, 0, 18_000, 10);
    store
        .lock()
        .execute(
            "INSERT INTO provider_usage_observation (
                 period_id, provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                 is_authoritative, confidence, source_id
             ) VALUES (?1, ?2, ?3, 'five-hour', 'rolling', 'primaryShort',
                       'account', 'account', 10, 5.0, 1, 1, 'high', 'test')",
            params![period_id, PROVIDER, account_key],
        )
        .unwrap();
    store
        .upsert_factor_sample(&FactorSample {
            provider: PROVIDER.to_string(),
            account_key: account_key.clone(),
            lane: LANE_FIVE_HOUR.to_string(),
            kind: "delta".to_string(),
            period_id: Some(period_id),
            from_epoch: 0,
            to_epoch: 10,
            from_percent: 0.0,
            to_percent: 5.0,
            input_usd: 1.0,
            output_usd: 0.0,
            cache_read_usd: 0.0,
            cache_write_usd: 0.0,
            turn_count: 1,
            plan: None,
            plan_tier: None,
            source_id: "test".to_string(),
            computed_at_epoch: 10,
        })
        .unwrap();
    store
        .upsert_limit_residual(period_id, 10, 5.0, 4.0)
        .unwrap();

    // 200 days old, well past any of the caps this rule used to enforce.
    let now = 200 * 86_400;
    let removed =
        Store::apply_provider_usage_retention_in(&store.lock(), RETAIN_SESSION_DATA_FOREVER, now)
            .unwrap();
    assert_eq!(removed, 0, "forever prunes nothing");

    let connection = store.lock();
    let count = |table: &str| -> i64 {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    assert_eq!(
        count("provider_usage_observation"),
        1,
        "forever keeps the observation"
    );
    assert_eq!(
        count("provider_usage_period"),
        1,
        "forever keeps the period"
    );
    assert_eq!(
        count("provider_limit_factor_sample"),
        1,
        "forever keeps the sample"
    );
    assert_eq!(
        count("provider_limit_residual"),
        1,
        "forever keeps the residual"
    );
}

#[test]
fn finite_retention_prunes_the_residual_and_learn_cursor_with_their_period() {
    let store = memory_store();
    let account_key = account('a');
    let now = 45 * 86_400;
    let old = now - 40 * 86_400;
    let period_id = insert_period(&store, &account_key, old - 18_000, old, old);
    store
        .lock()
        .execute(
            "INSERT INTO provider_usage_observation (
                 period_id, provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                 is_authoritative, confidence, source_id
             ) VALUES (?1, ?2, ?3, 'five-hour', 'rolling', 'primaryShort',
                       'account', 'account', ?4, 5.0, 1, 1, 'high', 'test')",
            params![period_id, PROVIDER, account_key, old],
        )
        .unwrap();
    store
        .advance_learn_cursor(period_id, old)
        .expect("advances the cursor");
    store
        .upsert_limit_residual(period_id, old, 5.0, 4.0)
        .expect("stores the residual");

    // PRAGMA foreign_keys is on for the store; a residual or cursor row left
    // dangling on a deleted period would raise an error here, not just a
    // stale row, so `.unwrap()` below is itself the no-error assertion.
    let removed = Store::apply_provider_usage_retention_in(&store.lock(), 30, now).unwrap();
    assert_eq!(removed, 1, "the 40-day-old observation is removed");

    let connection = store.lock();
    let count = |table: &str| -> i64 {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    };
    assert_eq!(
        count("provider_usage_period"),
        0,
        "the orphaned period is removed"
    );
    assert_eq!(
        count("provider_limit_residual"),
        0,
        "the residual is pruned with its period, not left dangling"
    );
    assert_eq!(
        count("provider_limit_learn_cursor"),
        0,
        "the cursor is pruned with its period"
    );
}

/// A synthetic period with only the fields [`lane_for_period`] reads set;
/// every other field is a harmless placeholder.
fn period_for_lane(
    window_role: &str,
    window_kind: &str,
    window_id: &str,
    scope_key: &str,
    scope_label: &str,
) -> ProviderUsagePeriod {
    ProviderUsagePeriod {
        id: 1,
        provider: PROVIDER.to_string(),
        account_key: account('a'),
        window_id: window_id.to_string(),
        window_kind: window_kind.to_string(),
        window_role: window_role.to_string(),
        scope_key: scope_key.to_string(),
        scope_label: scope_label.to_string(),
        duration_seconds: None,
        starts_at_epoch: None,
        resets_at_epoch: None,
        first_observed_epoch: 0,
        last_observed_epoch: 0,
    }
}

#[test]
fn lane_for_period_maps_the_account_wide_and_model_scoped_branches() {
    assert_eq!(
        lane_for_period(&period_for_lane(
            "primaryShort",
            "rolling",
            "five-hour",
            "account",
            "account"
        )),
        Some(LANE_FIVE_HOUR.to_string())
    );
    assert_eq!(
        lane_for_period(&period_for_lane(
            "primaryLong",
            "weekly",
            "seven-day",
            "account",
            "account"
        )),
        Some(LANE_WEEKLY.to_string())
    );
    // The slug comes from the window id when it has the expected shape.
    assert_eq!(
        lane_for_period(&period_for_lane(
            "supplemental",
            "weekly",
            "weekly-fable",
            "model:Fable",
            "Fable"
        )),
        Some("model:fable".to_string())
    );
    // A window id of another shape falls back to slugifying the scope label.
    assert_eq!(
        lane_for_period(&period_for_lane(
            "supplemental",
            "weekly",
            "unexpected-shape",
            "model:Codex Feature",
            "Codex Feature"
        )),
        Some("model:codex-feature".to_string())
    );
    // Anything else — a role or kind this app does not attribute a factor
    // to — carries no lane.
    assert_eq!(
        lane_for_period(&period_for_lane(
            "other:custom",
            "other:custom",
            "custom",
            "account",
            "account"
        )),
        None
    );
}

#[test]
fn candidate_periods_admit_a_model_scoped_weekly_period_and_exclude_an_account_scoped_supplemental()
{
    let store = memory_store();
    let account_key = account('a');
    let connection = store.lock();
    connection
        .execute(
            "INSERT INTO provider_usage_period (
                 provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, duration_seconds, starts_at_epoch,
                 resets_at_epoch, first_observed_epoch, last_observed_epoch
             ) VALUES (?1, ?2, 'weekly-fable', 'weekly', 'supplemental',
                       'model:Fable', 'Fable', 604800, 0, 604800, 100, 100)",
            params![PROVIDER, account_key],
        )
        .expect("inserts a model-scoped candidate period");
    connection
        .execute(
            "INSERT INTO provider_usage_period (
                 provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, duration_seconds, starts_at_epoch,
                 resets_at_epoch, first_observed_epoch, last_observed_epoch
             ) VALUES (?1, ?2, 'weekly-something', 'weekly', 'supplemental',
                       'account', 'account', 604800, 0, 604800, 100, 100)",
            params![PROVIDER, account_key],
        )
        .expect("inserts an account-scoped supplemental period");
    drop(connection);

    let periods = store
        .provider_limit_candidate_periods(0)
        .expect("query succeeds");
    assert_eq!(periods.len(), 1);
    assert_eq!(periods[0].scope_key, "model:Fable");
}

#[test]
fn latest_observation_plan_for_a_model_lane_reads_the_supplemental_windows_own_reading() {
    let store = memory_store();
    let account_key = account('a');
    let connection = store.lock();
    connection
        .execute(
            "INSERT INTO provider_usage_period (
                 provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, duration_seconds, starts_at_epoch,
                 resets_at_epoch, first_observed_epoch, last_observed_epoch
             ) VALUES (?1, ?2, 'weekly-fable', 'weekly', 'supplemental',
                       'model:Fable', 'Fable', 604800, 0, 604800, 100, 100)",
            params![PROVIDER, account_key],
        )
        .expect("inserts a model-scoped period");
    let model_period_id = connection.last_insert_rowid();
    connection
        .execute(
            "INSERT INTO provider_usage_period (
                 provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, duration_seconds, starts_at_epoch,
                 resets_at_epoch, first_observed_epoch, last_observed_epoch
             ) VALUES (?1, ?2, 'seven-day', 'weekly', 'primaryLong',
                       'account', 'account', 604800, 0, 604800, 100, 100)",
            params![PROVIDER, account_key],
        )
        .expect("inserts an account-wide weekly period");
    let account_period_id = connection.last_insert_rowid();
    connection
        .execute(
            "INSERT INTO provider_usage_observation (
                 period_id, provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                 is_authoritative, confidence, source_id, plan, plan_tier
             ) SELECT id, provider, account_key, window_id, window_kind, window_role,
                      scope_key, scope_label, 100, 5.0, 1, 1, 'high', 'test', 'max', 'model_tier'
                 FROM provider_usage_period WHERE id = ?1",
            params![model_period_id],
        )
        .expect("stores the model window's own reading");
    connection
        .execute(
            "INSERT INTO provider_usage_observation (
                 period_id, provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                 is_authoritative, confidence, source_id, plan, plan_tier
             ) SELECT id, provider, account_key, window_id, window_kind, window_role,
                      scope_key, scope_label, 100, 5.0, 1, 1, 'high', 'test', 'max', 'account_tier'
                 FROM provider_usage_period WHERE id = ?1",
            params![account_period_id],
        )
        .expect("stores the account window's own reading");
    drop(connection);

    let (plan, plan_tier) = store
        .latest_observation_plan(PROVIDER, &account_key, "model:fable")
        .expect("query succeeds")
        .expect("a reading exists");
    assert_eq!(plan.as_deref(), Some("max"));
    assert_eq!(
        plan_tier.as_deref(),
        Some("model_tier"),
        "a model lane reads the supplemental window's own observation, not the account window's"
    );
}

/// A per-step migration test for v50, kept for review; per repo convention
/// it may be deleted after merge, since the ladder test in `store::tests` is
/// the durable coverage.
#[test]
fn v50_widens_the_lane_check_and_keeps_existing_sample_and_point_rows() {
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    for &sql in &crate::store::schema::MIGRATIONS[..49] {
        connection.execute_batch(sql).unwrap();
    }
    connection.pragma_update(None, "user_version", 49).unwrap();
    connection
        .execute(
            "INSERT INTO provider_limit_factor_sample (
                 provider, account_key, lane, kind, from_epoch, to_epoch,
                 from_percent, to_percent, input_usd, output_usd, cache_read_usd,
                 cache_write_usd, turn_count, source_id, computed_at_epoch
             ) VALUES ('anthropic', ?1, 'weekly', 'delta', 0, 100, 0.0, 5.0,
                       1.0, 0.0, 0.0, 0.0, 1, 'test', 100)",
            params![account('a')],
        )
        .expect("inserts a pre-migration sample row");
    connection
        .execute(
            "INSERT INTO provider_limit_factor_point (
                 provider, account_key, lane, effective_at_epoch, usd_per_percent,
                 method, sample_count
             ) VALUES ('anthropic', ?1, 'weekly', 100, 0.2, 'delta', 1)",
            params![account('a')],
        )
        .expect("inserts a pre-migration point row");

    let store = Store::from_connection(connection, Path::new("/tmp/antiburn-v50-test").into())
        .expect("migration reaches the head");
    assert_eq!(store.schema_version().unwrap(), 50);

    let connection = store.lock();
    let samples: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM provider_limit_factor_sample",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(samples, 1, "an existing sample row survives the rebuild");
    let points: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM provider_limit_factor_point",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(points, 1, "an existing point row survives the rebuild");

    connection
        .execute(
            "INSERT INTO provider_limit_factor_sample (
                 provider, account_key, lane, kind, from_epoch, to_epoch,
                 from_percent, to_percent, input_usd, output_usd, cache_read_usd,
                 cache_write_usd, turn_count, source_id, computed_at_epoch
             ) VALUES ('anthropic', ?1, 'model:fable', 'delta', 200, 300, 0.0, 5.0,
                       1.0, 0.0, 0.0, 0.0, 1, 'test', 300)",
            params![account('b')],
        )
        .expect("a model-scoped lane is now allowed by the widened check");

    let rejected = connection.execute(
        "INSERT INTO provider_limit_factor_sample (
             provider, account_key, lane, kind, from_epoch, to_epoch,
             from_percent, to_percent, input_usd, output_usd, cache_read_usd,
             cache_write_usd, turn_count, source_id, computed_at_epoch
         ) VALUES ('anthropic', ?1, 'bogus', 'delta', 400, 500, 0.0, 5.0,
                   1.0, 0.0, 0.0, 0.0, 1, 'test', 500)",
        params![account('c')],
    );
    assert!(
        rejected.is_err(),
        "an unrecognized lane still fails the check"
    );
}

/// Publish one turn at `ts_ms` under a chosen model, so a model-scope filter
/// test can use a model id [`crate::provider_usage::factor::model_matches_scope`]
/// does or does not match.
fn insert_turn_with_model(
    store: &Store,
    key: &SessionKey,
    ts_ms: i64,
    input_tokens: i64,
    model: &str,
) {
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
                model,
                input_tokens
            ],
        )
        .expect("stores synthetic turn");
}

/// A turn claimed under a fence that never became `published_fence`, so the
/// bucketed query's `session_evidence` join excludes it.
fn insert_unpublished_turn(store: &Store, key: &SessionKey, ts_ms: i64, input_tokens: i64) {
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
             ) VALUES (?1, ?2, ?3, 2, 'synthetic', 'synthetic', 0, 'main', 'assistant',
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
        .expect("stores an unpublished synthetic turn");
}

#[test]
fn bucketed_query_groups_turns_by_fifteen_minute_bucket() {
    let store = memory_store();
    let key = insert_session(&store, "session");
    observe_account(&store, &account('a'));
    // Both inside the first bucket (0..900s).
    insert_turn(&store, &key, 100_000, 100_000);
    insert_turn(&store, &key, 800_000, 100_000);
    // The next bucket (900..1800s).
    insert_turn(&store, &key, 900_000 + 1, 100_000);

    let rows = store
        .attributed_turn_dollars_by_bucket(PROVIDER, &account('a'), None, 0, 2_000)
        .expect("query succeeds")
        .expect("stays within the group bound");
    let mut buckets: Vec<i64> = rows.iter().map(|row| row.bucket_start_epoch).collect();
    buckets.sort_unstable();
    assert_eq!(buckets, vec![0, 900]);
    let first_bucket = rows
        .iter()
        .find(|row| row.bucket_start_epoch == 0)
        .expect("first bucket present");
    // Two turns of 100_000 input tokens each (200_000 total), merged into one
    // bucket. claude-opus-4-6 test pricing: 5e-6 dollars per input token.
    assert!((first_bucket.usd - 1.0).abs() < 1e-9);
    assert_eq!(first_bucket.turn_count, 2);
}

#[test]
fn bucketed_query_excludes_a_turn_on_an_unpublished_fence() {
    let store = memory_store();
    let key = insert_session(&store, "session");
    observe_account(&store, &account('a'));
    insert_unpublished_turn(&store, &key, 100_000, 100_000);

    let rows = store
        .attributed_turn_dollars_by_bucket(PROVIDER, &account('a'), None, 0, 2_000)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert!(
        rows.is_empty(),
        "a turn on an unpublished fence never joins session_evidence"
    );
}

#[test]
fn bucketed_query_drops_another_account_and_keeps_unbound_as_unbound() {
    let store = memory_store();
    let bound_to_other = insert_session(&store, "bound-to-other");
    bind_account(&store, &bound_to_other, &account('b'));
    insert_turn(&store, &bound_to_other, 100_000, 100_000);

    let unbound = insert_session(&store, "unbound");
    observe_account(&store, &account('a'));
    observe_account(&store, &account('b'));
    insert_turn(&store, &unbound, 100_000, 200_000);

    let rows = store
        .attributed_turn_dollars_by_bucket(PROVIDER, &account('a'), None, 0, 2_000)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert_eq!(rows.len(), 1, "only the unbound session's row survives");
    assert_eq!(rows[0].key, unbound);
    assert_eq!(rows[0].account, Resolved::Unbound);
}

#[test]
fn bucketed_query_model_scope_filter_excludes_a_non_matching_model() {
    let store = memory_store();
    let key = insert_session(&store, "session");
    observe_account(&store, &account('a'));
    insert_turn_with_model(&store, &key, 100_000, 100_000, "claude-fable-5-1");

    let matching = store
        .attributed_turn_dollars_by_bucket(PROVIDER, &account('a'), Some("Fable"), 0, 2_000)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert_eq!(matching.len(), 1);

    let non_matching = store
        .attributed_turn_dollars_by_bucket(PROVIDER, &account('a'), Some("Opus"), 0, 2_000)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert!(non_matching.is_empty());
}

#[test]
fn quota_periods_for_lane_returns_overlapping_periods_plus_the_anchor_before_the_range() {
    let store = memory_store();
    let account_key = account('a');
    // Anchor: resets well before the query range.
    insert_period(&store, &account_key, 0, 1_000, 1_000);
    // Overlaps the query range.
    insert_period(&store, &account_key, 5_000, 6_000, 6_000);

    let periods = store
        .quota_periods_for_lane(PROVIDER, &account_key, LANE_FIVE_HOUR, 4_000, 7_000)
        .expect("query succeeds");
    let mut resets: Vec<i64> = periods
        .iter()
        .filter_map(|period| period.resets_at_epoch)
        .collect();
    resets.sort_unstable();
    assert_eq!(
        resets,
        vec![1_000, 6_000],
        "the anchor and the overlapping period both come back"
    );
}

#[test]
fn factor_points_for_lane_orders_by_effective_at_epoch() {
    let store = memory_store();
    let lane = LANE_WEEKLY;
    for effective_at_epoch in [2_000, 1_000, 3_000] {
        store
            .upsert_factor_point(&FactorPoint {
                id: 0,
                provider: PROVIDER.to_string(),
                account_key: account('a'),
                lane: lane.to_string(),
                effective_at_epoch,
                usd_per_percent: 0.1,
                method: "delta".to_string(),
                sample_count: 1,
                plan: None,
                plan_tier: None,
            })
            .unwrap();
    }
    let points = store
        .factor_points_for_lane(PROVIDER, &account('a'), lane)
        .expect("query succeeds");
    let epochs: Vec<i64> = points
        .iter()
        .map(|point| point.effective_at_epoch)
        .collect();
    assert_eq!(epochs, vec![1_000, 2_000, 3_000]);
}

#[test]
fn attributed_turn_epochs_rounds_to_the_minute_and_respects_the_two_step_rule() {
    let store = memory_store();
    let bound = insert_session(&store, "bound");
    observe_account(&store, &account('a'));
    insert_turn(&store, &bound, 100_123, 1);
    insert_turn(&store, &bound, 100_456, 1);

    let other = insert_session(&store, "other");
    bind_account(&store, &other, &account('b'));
    insert_turn(&store, &other, 200_000, 1);

    let epochs = store
        .attributed_turn_epochs(PROVIDER, &account('a'), 0, 1_000)
        .expect("query succeeds");
    // Both turns (100_123ms and 100_456ms) round into the same minute
    // (60..120s) and dedupe to one epoch; the session bound to a different
    // account contributes nothing.
    assert_eq!(epochs, vec![60]);
}

#[test]
fn quota_accounts_reports_label_has_factor_and_the_current_open_period() {
    let store = memory_store();
    let account_key = account('a');
    // A weekly period still open at `now`.
    insert_period(&store, &account_key, 0, 10_000, 10_000);
    store
        .lock()
        .execute(
            "UPDATE provider_usage_period SET window_role = 'primaryLong'
              WHERE account_key = ?1",
            params![account_key],
        )
        .unwrap();
    // A five-hour period, inserted after the update above so it keeps its
    // default primaryShort role, and a model-scoped lane, so the account
    // carries all three lane kinds for the ordering assertion below.
    insert_period(&store, &account_key, 0, 18_000, 18_000);
    store
        .lock()
        .execute(
            "INSERT INTO provider_usage_period (
                 provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, duration_seconds, starts_at_epoch,
                 resets_at_epoch, first_observed_epoch, last_observed_epoch
             ) VALUES (?1, ?2, 'weekly-zeta', 'weekly', 'supplemental',
                       'model:zeta', 'Zeta', 604800, 0, 604800, 100, 100)",
            params![PROVIDER, account_key],
        )
        .unwrap();
    store
        .upsert_factor_point(&FactorPoint {
            id: 0,
            provider: PROVIDER.to_string(),
            account_key: account_key.clone(),
            lane: LANE_WEEKLY.to_string(),
            effective_at_epoch: 1,
            usd_per_percent: 0.2,
            method: "delta".to_string(),
            sample_count: 1,
            plan: None,
            plan_tier: None,
        })
        .unwrap();

    let accounts = store.quota_accounts(5_000).expect("query succeeds");
    let account = accounts
        .iter()
        .find(|account| account.account_key == account_key)
        .expect("the account is reported");
    let lane_names: Vec<&str> = account
        .lanes
        .iter()
        .map(|lane| lane.lane.as_str())
        .collect();
    assert_eq!(
        lane_names,
        vec![LANE_WEEKLY, LANE_FIVE_HOUR, "model:zeta"],
        "lanes are ordered weekly, then fiveHour, then model lanes alphabetically"
    );
    let lane = account
        .lanes
        .iter()
        .find(|lane| lane.lane == LANE_WEEKLY)
        .expect("the weekly lane is reported");
    assert_eq!(lane.label, "Weekly");
    assert!(lane.has_factor);
    assert_eq!(
        lane.current_period,
        Some((0, 10_000)),
        "the period has not reset yet"
    );

    // Once `now` passes the reset, the same period no longer counts as open.
    let accounts_after_reset = store.quota_accounts(20_000).expect("query succeeds");
    let lane_after_reset = accounts_after_reset
        .iter()
        .find(|account| account.account_key == account_key)
        .and_then(|account| account.lanes.iter().find(|lane| lane.lane == LANE_WEEKLY))
        .expect("the weekly lane is still reported");
    assert_eq!(
        lane_after_reset.current_period, None,
        "a period that has already reset is not the current one"
    );
}

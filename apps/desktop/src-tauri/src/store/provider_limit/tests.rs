use std::path::Path;

use rusqlite::params;

use super::*;
use crate::store::SessionRecord;

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
        .attributed_turn_dollars_between(PROVIDER, &account('a'), 0, 1_000)
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
        .attributed_turn_dollars_between(PROVIDER, &account('a'), 0, 1_000)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert_eq!(dollars.len(), 1);
    assert_eq!(dollars[0].key, key);
    // claude-opus-4-6 test pricing: 5e-6 dollars per input token.
    assert!((dollars[0].input_usd - 1.0).abs() < 1e-9);
    assert_eq!(dollars[0].turn_count, 1);
}

#[test]
fn a_directly_bound_session_ignores_the_single_account_fallback() {
    let store = memory_store();
    let key = insert_session(&store, "session");
    insert_turn(&store, &key, 150_000, 200_000);
    bind_account(&store, &key, &account('a'));
    observe_account(&store, &account('b'));

    let dollars = store
        .attributed_turn_dollars_between(PROVIDER, &account('b'), 0, 1_000)
        .expect("query succeeds")
        .expect("stays within the group bound");
    assert!(
        dollars.is_empty(),
        "a session bound to a different account never falls back"
    );

    let dollars = store
        .attributed_turn_dollars_between(PROVIDER, &account('a'), 0, 1_000)
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

    // 200 days elapsed, 400-day retention capped at 365: the sample's
    // to_epoch (100) is far short of the cutoff, so nothing is removed yet.
    let now = 200 * 86_400;
    apply_sample_retention_in(&store.lock(), 400, now).unwrap();
    assert_eq!(
        store
            .all_delta_factor_samples(PROVIDER, &account('a'), lane)
            .unwrap()
            .len(),
        1
    );

    // Past the (capped) 365-day cutoff, the sample goes; the point does not.
    let now = 400 * 86_400;
    apply_sample_retention_in(&store.lock(), 400, now).unwrap();
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

    // 90-day observation retention removes the period; the (uncapped-by-this
    // call) 365-day sample cutoff sits before epoch zero, so the sample
    // itself survives and must not keep a dangling period id.
    let now = 200 * 86_400;
    Store::apply_provider_usage_retention_in(&store.lock(), 400, now).unwrap();

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

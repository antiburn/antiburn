//! Lifecycle tests for durable provider-period allocations.

use std::path::Path;

use rusqlite::params;

use super::*;
use crate::store::{SessionKey, SessionRecord, Store};

const ACCOUNT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn memory_store() -> Store {
    Store::open_in_memory(Path::new("/tmp/antiburn-ledger-lifecycle-test"))
        .expect("opens migrated store")
}

fn session(id: &str) -> SessionRecord {
    SessionRecord {
        key: SessionKey::new("native", "claude-code", id),
        source_kind: "inline".to_string(),
        source_label: "synthetic".to_string(),
        wsl_distro: None,
        title: None,
        title_source: None,
        cwd: None,
        surface: "unknown".to_string(),
        updated_at_epoch: Some(1_000),
        activity_cursor: "synthetic".to_string(),
        activity_source: "event".to_string(),
        subagent_count: 0,
        fork_parent_session_id: None,
        source_fingerprint: Some("synthetic".to_string()),
    }
}

fn period(store: &Store, metric: &str, start: i64, reset: i64) -> i64 {
    let (window_id, kind, role) = match metric {
        "weekly" => ("seven-day", "weekly", "primaryLong"),
        "fiveHour" => ("five-hour", "rolling", "primaryShort"),
        _ => panic!("unsupported synthetic metric"),
    };
    let connection = store.lock();
    connection
        .execute(
            "INSERT INTO provider_usage_period (
                 provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, duration_seconds, starts_at_epoch,
                 resets_at_epoch, first_observed_epoch, last_observed_epoch
             ) VALUES ('anthropic', ?1, ?2, ?3, ?4, 'account', 'account',
                       ?5, ?6, ?7, ?6, ?6)",
            params![ACCOUNT, window_id, kind, role, reset - start, start, reset],
        )
        .expect("inserts synthetic period");
    connection.last_insert_rowid()
}

fn allocation(key: &SessionKey, metric: &str, percent: f64) -> SessionPeriodAllocation {
    SessionPeriodAllocation {
        key: key.clone(),
        metric: metric.to_string(),
        percent,
        basis: "tokens".to_string(),
        partial: false,
    }
}

fn queued(store: &Store, period_id: i64) -> DirtyPeriod {
    store
        .provider_usage_allocation_dirty_periods(32)
        .expect("reads dirty periods")
        .into_iter()
        .find(|dirty| dirty.period_id == period_id)
        .expect("queues the period")
}

#[test]
fn allocation_reconcile_gate_is_shared_by_store_clones() {
    let store = memory_store();
    let clone = store.clone();
    let first = store
        .try_begin_allocation_reconcile()
        .expect("claims the first pass");
    assert!(clone.try_begin_allocation_reconcile().is_none());

    drop(first);

    assert!(clone.try_begin_allocation_reconcile().is_some());
}

#[test]
fn retained_adjacent_weekly_and_five_hour_periods_survive_reopen_over_one_hundred_percent() {
    let directory = tempfile::tempdir().expect("creates synthetic state directory");
    let key = session("cumulative").key;
    {
        let store = Store::open(directory.path()).expect("opens store");
        store
            .upsert_sessions(&[session("cumulative")], &[])
            .expect("stores session");
        for (metric, start, reset, percent) in [
            ("weekly", 0, 100, 70.0),
            ("weekly", 100, 200, 60.0),
            ("fiveHour", 0, 100, 55.0),
            ("fiveHour", 100, 200, 50.0),
        ] {
            let period_id = period(&store, metric, start, reset);
            store
                .replace_provider_usage_period_allocations(
                    period_id,
                    &[allocation(&key, metric, percent)],
                    200,
                )
                .expect("stores allocation");
        }
    }

    let reopened = Store::open(directory.path()).expect("reopens store");
    let allocations = reopened
        .cumulative_session_limit_allocations(std::slice::from_ref(&key))
        .expect("reads retained allocations");
    assert_eq!(allocations.len(), 2);
    let weekly = allocations
        .iter()
        .find(|entry| entry.metric == "weekly")
        .expect("retains weekly total");
    assert_eq!(weekly.percent, 130.0);
    assert_eq!(weekly.period_count, 2);
    let five_hour = allocations
        .iter()
        .find(|entry| entry.metric == "fiveHour")
        .expect("retains five-hour total");
    assert_eq!(five_hour.percent, 105.0);
    assert_eq!(five_hour.period_count, 2);
}

#[test]
fn disabled_network_setting_still_recomputes_the_local_dirty_ledger() {
    let store = memory_store();
    store
        .save_settings(&crate::store::AppSettings {
            live_usage_enabled: false,
            onboarding_completed: true,
            ..crate::store::AppSettings::default()
        })
        .expect("disables provider network collection");
    assert!(!store.settings().unwrap().live_usage_active());
    let record = session("many-turns");
    let key = record.key.clone();
    store
        .upsert_sessions(std::slice::from_ref(&record), &[])
        .expect("stores session");
    let period_id = period(&store, "weekly", 1_000, 1_100);
    {
        let connection = store.lock();
        connection
            .execute(
                "INSERT INTO session_evidence (
                     environment_key, agent, session_id, status, published_fence
                 ) VALUES (?1, ?2, ?3, 'ready', 1)",
                params![key.environment_key, key.agent, key.session_id],
            )
            .expect("publishes synthetic evidence");
        connection
            .execute(
                "INSERT INTO session_provider_account (
                     environment_key, agent, session_id, provider, account_key,
                     provenance, confidence, first_seen_at
                 ) VALUES (?1, ?2, ?3, 'anthropic', ?4, 'provider_live', 'direct', 'synthetic')",
                params![key.environment_key, key.agent, key.session_id, ACCOUNT],
            )
            .expect("attributes session to synthetic account");
        for index in 0..128 {
            connection
                .execute(
                    "INSERT INTO turn (
                         environment_key, agent, session_id, claim_fence, source_key,
                         thread_id, turn_index, scope, role, ts_ms, model, effort, speed,
                         input_tokens, cache_read_tokens, cache_write_tokens, output_tokens,
                         is_compaction_boundary, message_id, uuid, parent_uuid
                     ) VALUES (?1, ?2, ?3, 1, 'synthetic', 'synthetic', ?4, 'main',
                               'assistant', ?5, 'claude-opus-4-6', NULL, NULL, 1000, 0, 0,
                               0, 0, NULL, NULL, NULL)",
                    params![
                        key.environment_key,
                        key.agent,
                        key.session_id,
                        index,
                        1_020_000 + index
                    ],
                )
                .expect("stores synthetic turn");
        }
        for (observed_at, percent) in [(1_010, 10.0), (1_090, 40.0)] {
            connection
                .execute(
                    "INSERT INTO provider_usage_observation (
                         period_id, provider, account_key, window_id, window_kind, window_role,
                         scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                         is_authoritative, confidence, source_id, reported_starts_at_epoch,
                         reported_resets_at_epoch
                     ) VALUES (?1, 'anthropic', ?2, 'seven-day', 'weekly', 'primaryLong',
                               'account', 'account', ?3, ?4, 1, 1, 'high', 'synthetic',
                               1000, 1100)",
                    params![period_id, ACCOUNT, observed_at, percent],
                )
                .expect("stores synthetic observation");
        }
    }
    store
        .enqueue_provider_usage_allocation_periods(&[period_id], 1_090)
        .expect("queues period");
    crate::provider_usage::ledger::reconcile(&store, 1_100);

    let connection = store.lock();
    let rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM provider_usage_session_allocation WHERE period_id = ?1",
            [period_id],
            |row| row.get(0),
        )
        .expect("counts ledger rows");
    assert_eq!(rows, 1);
    drop(connection);
    assert!(
        store
            .provider_usage_allocation_dirty_periods(32)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn stale_cas_cannot_overwrite_a_later_generation_or_reuse_an_acknowledged_generation() {
    let store = memory_store();
    let record = session("cas");
    let key = record.key.clone();
    store
        .upsert_sessions(&[record], &[])
        .expect("stores session");
    let period_id = period(&store, "weekly", 0, 100);

    store
        .enqueue_provider_usage_allocation_periods(&[period_id], 1)
        .expect("queues first generation");
    let first = queued(&store, period_id);
    store
        .enqueue_provider_usage_allocation_periods(&[period_id], 2)
        .expect("queues later generation");
    let second = queued(&store, period_id);
    assert!(second.generation > first.generation);
    store
        .replace_provider_usage_period_allocations_and_ack(
            period_id,
            second.generation,
            &[allocation(&key, "weekly", 80.0)],
            2,
        )
        .expect("writes later generation");
    store
        .enqueue_provider_usage_allocation_periods(&[period_id], 3)
        .expect("queues after acknowledgment");
    let after_ack = queued(&store, period_id);
    assert!(after_ack.generation > second.generation);

    store
        .replace_provider_usage_period_allocations_and_ack(
            period_id,
            first.generation,
            &[allocation(&key, "weekly", 10.0)],
            3,
        )
        .expect("ignores stale worker");
    let allocations = store
        .cumulative_session_limit_allocations(&[key])
        .expect("reads later allocation");
    assert_eq!(allocations[0].percent, 80.0);
    assert_eq!(queued(&store, period_id), after_ack);
}

#[test]
fn a_worker_that_started_before_retention_freezes_preserves_the_old_partial_total() {
    let store = memory_store();
    let record = session("frozen");
    let key = record.key.clone();
    store
        .upsert_sessions(&[record], &[])
        .expect("stores session");
    let period_id = period(&store, "weekly", 0, 100);
    store
        .replace_provider_usage_period_allocations(
            period_id,
            &[allocation(&key, "weekly", 40.0)],
            100,
        )
        .expect("stores old allocation");
    store
        .enqueue_provider_usage_allocation_periods(&[period_id], 101)
        .expect("queues worker input");
    let dirty = queued(&store, period_id);
    store
        .lock()
        .execute(
            "UPDATE provider_usage_period SET allocation_frozen = 1 WHERE id = ?1",
            [period_id],
        )
        .expect("simulates retention freeze");

    store
        .replace_provider_usage_period_allocations_and_ack(
            period_id,
            dirty.generation,
            &[allocation(&key, "weekly", 90.0)],
            102,
        )
        .expect("acknowledges frozen worker");
    let allocation = store
        .cumulative_session_limit_allocations(&[key])
        .expect("reads retained allocation")
        .pop()
        .expect("retains old allocation");
    assert_eq!(allocation.percent, 40.0);
    assert!(allocation.partial);
    assert!(
        store
            .provider_usage_allocation_dirty_periods(32)
            .expect("reads acknowledged queue")
            .is_empty()
    );
}

#[test]
fn a_late_account_binding_queues_an_existing_period_without_a_new_provider_observation() {
    let store = memory_store();
    let mut record = session("late-account");
    record.key.agent = "pi".to_string();
    record.updated_at_epoch = Some(1_000);
    store.set_internal_value("internal:providerAccountRolloutV1", "0");
    store
        .upsert_sessions(&[record.clone()], &[])
        .expect("stores session");
    let period_id = period(&store, "weekly", 900, 1_100);

    store
        .observe_provider_account("pi", "anthropic", ACCOUNT, 1_050, "tool_oauth")
        .expect("binds late account");
    assert_eq!(queued(&store, period_id).period_id, period_id);
    let connection = store.lock();
    let bindings: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM session_provider_account
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
                AND provider = 'anthropic' AND account_key = ?4",
            params![
                record.key.environment_key,
                record.key.agent,
                record.key.session_id,
                ACCOUNT
            ],
            |row| row.get(0),
        )
        .expect("counts account binding");
    assert_eq!(bindings, 1);
}

#[test]
fn deleting_a_session_requeues_its_period_and_clear_removes_all_queued_history() {
    let store = memory_store();
    let removed = session("removed");
    let remaining = session("remaining");
    store
        .upsert_sessions(&[removed.clone(), remaining.clone()], &[])
        .expect("stores sessions");
    let period_id = period(&store, "fiveHour", 0, 100);
    store
        .replace_provider_usage_period_allocations(
            period_id,
            &[
                allocation(&removed.key, "fiveHour", 40.0),
                allocation(&remaining.key, "fiveHour", 60.0),
            ],
            100,
        )
        .expect("stores allocations");

    assert!(store.delete_session(&removed.key).expect("deletes session"));
    assert_eq!(queued(&store, period_id).period_id, period_id);
    assert!(
        store
            .cumulative_session_limit_allocations(&[removed.key])
            .expect("reads deleted session")
            .is_empty()
    );

    store.clear_local_session_data().expect("clears local data");
    let connection = store.lock();
    for table in [
        "provider_usage_allocation_dirty",
        "provider_usage_session_allocation",
        "provider_usage_observation",
        "provider_usage_period",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("counts cleared provider table");
        assert_eq!(count, 0, "clears {table}");
    }
}

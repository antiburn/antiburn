#[cfg(test)]
mod history_tests {
    use std::path::Path;

    use time::OffsetDateTime;

    use super::super::*;
    use crate::provider_usage::live::model::{UsageSource, WindowRole};
    use crate::store::{SessionKey, SessionRecord};

    const NOW: i64 = 1_800_000_000;
    const ACCOUNT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ACCOUNT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn store() -> Store {
        Store::open_in_memory(Path::new("/tmp/antiburn-provider-usage-history-test"))
            .expect("opens migrated in-memory store")
    }

    fn snapshot(
        account: &str,
        observed_at: i64,
        window_id: &str,
        starts_at: Option<i64>,
        resets_at: Option<i64>,
        used_percent: Option<f64>,
    ) -> ProviderUsageSnapshot {
        ProviderUsageSnapshot {
            provider: "anthropic",
            account: Some(account.into()),
            account_uuid: None,
            account_email: None,
            plan: None,
            plan_tier: None,
            observed_at: OffsetDateTime::from_unix_timestamp(observed_at).expect("valid time"),
            source: UsageSource {
                id: "synthetic",
                label: "Synthetic source".into(),
                confidence: Confidence::High,
                freshness: Freshness::Fresh,
            },
            windows: vec![UsageWindow {
                id: window_id.into(),
                role: WindowRole::PrimaryShort,
                kind: UsageWindowKind::Rolling,
                scope: UsageScope::Account,
                used_percent,
                starts_at: starts_at
                    .map(|value| OffsetDateTime::from_unix_timestamp(value).expect("valid time")),
                resets_at: resets_at
                    .map(|value| OffsetDateTime::from_unix_timestamp(value).expect("valid time")),
                authoritative: true,
            }],
            supplemental: None,
            reset_credits: None,
        }
    }

    fn session() -> SessionRecord {
        SessionRecord {
            key: SessionKey::new("native", "claude-code", "session"),
            source_kind: "inline".to_string(),
            source_label: "test".to_string(),
            wsl_distro: None,
            title: None,
            title_source: None,
            cwd: None,
            surface: "unknown".to_string(),
            updated_at_epoch: Some(NOW),
            activity_cursor: "test".to_string(),
            activity_source: "event".to_string(),
            subagent_count: 0,
            fork_parent_session_id: None,
            source_fingerprint: Some("test".to_string()),
        }
    }

    #[test]
    fn a_repeated_reading_is_idempotent_and_returns_no_changed_period() {
        let store = store();
        let reading = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 18_000),
            Some(NOW),
            Some(40.0),
        );

        let changed = store
            .record_provider_usage_snapshots(std::slice::from_ref(&reading))
            .unwrap();
        assert_eq!(changed.len(), 1);
        assert!(
            store
                .record_provider_usage_snapshots(&[reading])
                .unwrap()
                .is_empty()
        );

        let history = store
            .provider_usage_period_history(changed[0])
            .unwrap()
            .unwrap();
        assert_eq!(history.observations.len(), 1);
    }

    #[test]
    fn reset_jitter_keeps_one_period_and_raw_observations() {
        let store = store();
        let first = snapshot(
            ACCOUNT_A,
            NOW - 60,
            "five-hour",
            Some(NOW - 18_060),
            Some(NOW),
            Some(20.0),
        );
        let second = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 18_059),
            Some(NOW + 1),
            Some(30.0),
        );

        let first_id = store.record_provider_usage_snapshots(&[first]).unwrap()[0];
        assert_eq!(
            store.record_provider_usage_snapshots(&[second]).unwrap(),
            [first_id]
        );
        let history = store
            .provider_usage_period_history(first_id)
            .unwrap()
            .unwrap();
        assert_eq!(history.period.resets_at_epoch, Some(NOW));
        assert_eq!(history.observations.len(), 2);
        assert_eq!(
            history.observations[1].reported_resets_at_epoch,
            Some(NOW + 1)
        );
    }

    #[test]
    fn a_new_reset_creates_a_new_period_without_merging_a_drop() {
        let store = store();
        let first = snapshot(
            ACCOUNT_A,
            NOW - 20_000,
            "five-hour",
            Some(NOW - 38_000),
            Some(NOW - 20_000),
            Some(95.0),
        );
        let second = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 18_000),
            Some(NOW),
            Some(5.0),
        );

        let first_id = store.record_provider_usage_snapshots(&[first]).unwrap()[0];
        let second_id = store.record_provider_usage_snapshots(&[second]).unwrap()[0];
        assert_ne!(first_id, second_id);
        assert_eq!(
            store
                .provider_usage_period_history(first_id)
                .unwrap()
                .unwrap()
                .observations
                .len(),
            1
        );
        assert_eq!(
            store
                .provider_usage_period_history(second_id)
                .unwrap()
                .unwrap()
                .observations
                .len(),
            1
        );
    }

    #[test]
    fn provider_account_and_window_lanes_stay_isolated() {
        let store = store();
        let a_short = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 18_000),
            Some(NOW),
            Some(10.0),
        );
        let a_week = snapshot(
            ACCOUNT_A,
            NOW,
            "weekly",
            Some(NOW - 604_800),
            Some(NOW),
            Some(20.0),
        );
        let b_short = snapshot(
            ACCOUNT_B,
            NOW,
            "five-hour",
            Some(NOW - 18_000),
            Some(NOW),
            Some(30.0),
        );

        let ids = store
            .record_provider_usage_snapshots(&[a_short, a_week, b_short])
            .unwrap();
        assert_eq!(ids.len(), 3);
        let page = store
            .provider_usage_periods_changed_since(NOW, None, 10)
            .unwrap();
        assert_eq!(page.periods.len(), 3);
        assert_eq!(
            page.periods
                .iter()
                .filter(|period| period.account_key == ACCOUNT_A)
                .count(),
            2
        );
    }

    #[test]
    fn unbounded_readings_persist_without_a_period() {
        let store = store();
        let reading = snapshot(ACCOUNT_A, NOW, "unknown", None, None, Some(50.0));

        assert!(
            store
                .record_provider_usage_snapshots(&[reading])
                .unwrap()
                .is_empty()
        );
        let connection = store.lock();
        let period: Option<i64> = connection
            .query_row(
                "SELECT period_id FROM provider_usage_observation",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(period, None);
    }

    #[test]
    fn lower_quality_duplicate_cannot_replace_provider_evidence() {
        let store = store();
        let first = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 18_000),
            Some(NOW),
            Some(40.0),
        );
        let mut weaker = first.clone();
        weaker.source.freshness = Freshness::Stale;
        weaker.windows[0].authoritative = false;
        weaker.windows[0].used_percent = Some(2.0);

        let id = store.record_provider_usage_snapshots(&[first]).unwrap()[0];
        assert!(
            store
                .record_provider_usage_snapshots(&[weaker])
                .unwrap()
                .is_empty()
        );
        let history = store.provider_usage_period_history(id).unwrap().unwrap();
        assert_eq!(history.observations[0].used_percent, Some(40.0));
        assert!(history.observations[0].is_fresh);
        assert!(history.observations[0].is_authoritative);
    }

    #[test]
    fn equal_quality_enrichment_keeps_the_known_percent() {
        let store = store();
        let mut first = snapshot(ACCOUNT_A, NOW, "five-hour", None, None, Some(40.0));
        first.windows[0].authoritative = false;
        let mut enriched = first.clone();
        enriched.windows[0].used_percent = None;
        enriched.windows[0].starts_at =
            Some(OffsetDateTime::from_unix_timestamp(NOW - 18_000).expect("valid time"));
        enriched.windows[0].resets_at =
            Some(OffsetDateTime::from_unix_timestamp(NOW).expect("valid time"));

        assert!(
            store
                .record_provider_usage_snapshots(&[first])
                .unwrap()
                .is_empty()
        );
        let changed = store.record_provider_usage_snapshots(&[enriched]).unwrap();
        assert_eq!(changed.len(), 1);
        let history = store
            .provider_usage_period_history(changed[0])
            .unwrap()
            .unwrap();
        assert_eq!(history.observations[0].used_percent, Some(40.0));
        assert_eq!(
            history.observations[0].reported_starts_at_epoch,
            Some(NOW - 18_000)
        );
        assert_eq!(history.observations[0].reported_resets_at_epoch, Some(NOW));
    }

    #[test]
    fn history_survives_a_store_reopen_after_the_migration() {
        let directory = tempfile::tempdir().expect("creates data directory");
        let reading = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 18_000),
            Some(NOW),
            Some(40.0),
        );
        let period_id = Store::open(directory.path())
            .expect("opens migrated store")
            .record_provider_usage_snapshots(&[reading])
            .expect("records reading")[0];

        let reopened = Store::open(directory.path()).expect("reopens migrated store");
        assert_eq!(
            reopened
                .provider_usage_period_history(period_id)
                .unwrap()
                .unwrap()
                .observations
                .len(),
            1
        );
    }

    #[test]
    fn corrected_boundary_invalidates_both_the_old_and_new_periods() {
        let store = store();
        let first = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 18_000),
            Some(NOW),
            Some(40.0),
        );
        let old_period = store
            .record_provider_usage_snapshots(std::slice::from_ref(&first))
            .unwrap()[0];
        let mut corrected = first;
        corrected.windows[0].resets_at =
            Some(OffsetDateTime::from_unix_timestamp(NOW + 600).expect("valid time"));
        corrected.windows[0].starts_at =
            Some(OffsetDateTime::from_unix_timestamp(NOW - 17_400).expect("valid time"));
        corrected.windows[0].used_percent = None;

        let changed = store.record_provider_usage_snapshots(&[corrected]).unwrap();
        assert_eq!(changed.len(), 2);
        assert!(changed.contains(&old_period));
        let new_period = *changed.iter().find(|id| **id != old_period).unwrap();
        assert!(
            store
                .provider_usage_period_history(old_period)
                .unwrap()
                .unwrap()
                .observations
                .is_empty()
        );
        assert_eq!(
            store
                .provider_usage_period_history(new_period)
                .unwrap()
                .unwrap()
                .observations
                .len(),
            1
        );
    }

    #[test]
    fn retention_bounds_readings_and_clear_removes_provider_history() {
        let store = store();
        let old = snapshot(
            ACCOUNT_A,
            NOW - 91 * 86_400,
            "five-hour",
            Some(NOW - 91 * 86_400 - 18_000),
            Some(NOW - 91 * 86_400),
            Some(10.0),
        );
        let current = snapshot(
            ACCOUNT_A,
            NOW,
            "weekly",
            Some(NOW - 604_800),
            Some(NOW),
            Some(20.0),
        );
        store
            .record_provider_usage_snapshots(&[old, current])
            .unwrap();

        store.apply_session_retention(NOW).unwrap();
        let connection = store.lock();
        let observations: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM provider_usage_observation",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(observations, 1);
        drop(connection);
        store.clear_local_session_data().unwrap();
        let connection = store.lock();
        let remaining: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM provider_usage_observation",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0);
    }

    /// Retention used to keep an orphaned period alive whenever a
    /// materialized allocation row or a dirty-queue entry still pointed at
    /// it. Both checks are gone with the allocator; a period with no
    /// remaining observations must still disappear on its own.
    #[test]
    fn retention_deletes_an_orphaned_period_and_its_observations_after_the_allocation_checks_are_gone()
     {
        let store = store();
        let old = snapshot(
            ACCOUNT_A,
            NOW - 91 * 86_400,
            "five-hour",
            Some(NOW - 91 * 86_400 - 18_000),
            Some(NOW - 91 * 86_400),
            Some(10.0),
        );
        let changed = store.record_provider_usage_snapshots(&[old]).unwrap();
        let period_id = changed[0];

        store.apply_session_retention(NOW).unwrap();

        let connection = store.lock();
        let observations: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM provider_usage_observation WHERE period_id = ?1",
                [period_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(observations, 0);
        let period_exists: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_usage_period WHERE id = ?1)",
                [period_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!period_exists, "an orphaned period is deleted, not kept");
    }

    #[test]
    fn a_raw_account_key_is_not_persisted() {
        let store = store();
        let reading = snapshot(
            "reader@example.test",
            NOW,
            "five-hour",
            Some(NOW - 18_000),
            Some(NOW),
            Some(10.0),
        );
        assert!(
            store
                .record_provider_usage_snapshots(&[reading])
                .unwrap()
                .is_empty()
        );
        let connection = store.lock();
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM provider_usage_observation",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn complementary_same_time_boundaries_keep_one_period_and_duration() {
        let store = store();
        let first = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 100),
            None,
            Some(20.0),
        );
        let id = store.record_provider_usage_snapshots(&[first]).unwrap()[0];
        let second = snapshot(ACCOUNT_A, NOW, "five-hour", None, Some(NOW + 100), None);
        assert_eq!(
            store.record_provider_usage_snapshots(&[second]).unwrap(),
            [id]
        );
        let history = store.provider_usage_period_history(id).unwrap().unwrap();
        assert_eq!(history.period.duration_seconds, Some(200));
        assert_eq!(history.observations[0].period_id, Some(id));
    }

    #[test]
    fn contradictory_merged_bounds_detach_the_old_period() {
        let store = store();
        let first = snapshot(ACCOUNT_A, NOW, "five-hour", Some(NOW), None, Some(20.0));
        let id = store.record_provider_usage_snapshots(&[first]).unwrap()[0];
        let second = snapshot(ACCOUNT_A, NOW, "five-hour", None, Some(NOW - 1), None);
        assert_eq!(
            store.record_provider_usage_snapshots(&[second]).unwrap(),
            [id]
        );
        assert!(
            store
                .provider_usage_period_history(id)
                .unwrap()
                .unwrap()
                .observations
                .is_empty()
        );
    }

    #[test]
    fn same_time_readings_with_different_scope_or_role_stay_distinct() {
        let store = store();
        let first = snapshot(
            ACCOUNT_A,
            NOW,
            "limit",
            Some(NOW - 100),
            Some(NOW),
            Some(20.0),
        );
        let mut other = first.clone();
        other.windows[0].scope = UsageScope::Model("gpt-test".into());
        other.windows[0].role = WindowRole::Supplemental;
        assert_eq!(
            store
                .record_provider_usage_snapshots(&[first, other])
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn pruned_period_ids_do_not_reuse_an_old_id() {
        let store = store();
        let old = snapshot(
            ACCOUNT_A,
            NOW - 91 * 86_400,
            "five-hour",
            Some(NOW - 91 * 86_400 - 100),
            Some(NOW - 91 * 86_400),
            Some(20.0),
        );
        let old_id = store.record_provider_usage_snapshots(&[old]).unwrap()[0];
        store.apply_session_retention(NOW).unwrap();
        let new = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 100),
            Some(NOW),
            Some(20.0),
        );
        let new_id = store.record_provider_usage_snapshots(&[new]).unwrap()[0];
        assert!(new_id > old_id);
    }

    #[test]
    fn the_plan_and_plan_tier_a_snapshot_states_are_stored_on_its_observation() {
        let store = store();
        store.upsert_sessions(&[session()], &[]).unwrap();
        let mut reading = snapshot(
            ACCOUNT_A,
            NOW,
            "five-hour",
            Some(NOW - 18_000),
            Some(NOW),
            Some(40.0),
        );
        reading.plan = Some("pro".to_string());
        reading.plan_tier = Some("standard".to_string());

        let period_id = store.record_provider_usage_snapshots(&[reading]).unwrap()[0];

        let history = store
            .provider_usage_period_history(period_id)
            .unwrap()
            .unwrap();
        assert_eq!(history.observations.len(), 1);
        assert_eq!(history.observations[0].plan.as_deref(), Some("pro"));
        assert_eq!(
            history.observations[0].plan_tier.as_deref(),
            Some("standard")
        );
    }
}

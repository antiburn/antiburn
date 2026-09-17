#[cfg(test)]
mod history_tests {
    use std::path::Path;

    use time::OffsetDateTime;

    use super::super::*;
    use crate::provider_usage::live::model::{UsageSource, WindowRole};
    use crate::store::{AppSettings, SessionKey, SessionRecord};

    /// A finite retention setting, so a test exercising an age-based cutoff
    /// does not have to reason about the forever default.
    const RETENTION_DAYS: i32 = 90;

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
            refusal_kind: None,
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

    /// The same snapshot in the long window, which is the one a daily
    /// allowance series reads.
    fn weekly_snapshot(
        account: &str,
        observed_at: i64,
        starts_at: Option<i64>,
        resets_at: Option<i64>,
        used_percent: Option<f64>,
    ) -> ProviderUsageSnapshot {
        let mut snapshot = snapshot(
            account,
            observed_at,
            "seven-day",
            starts_at,
            resets_at,
            used_percent,
        );
        for window in &mut snapshot.windows {
            window.role = WindowRole::PrimaryLong;
            window.kind = UsageWindowKind::Weekly;
        }
        snapshot
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
    fn reset_jitter_merges_two_codex_rollout_readings_into_one_period() {
        // Same mechanism as `reset_jitter_keeps_one_period_and_raw_observations`,
        // exercised with a rollout-sourced reading pair: consecutive Codex
        // rollout events routinely restate a reset a couple of seconds apart
        // from the server clock, and phase 4's history import must not split
        // them into separate periods.
        let store = store();
        let mut first = snapshot(
            ACCOUNT_A,
            NOW - 60,
            "five-hour",
            None,
            Some(NOW),
            Some(20.0),
        );
        first.provider = "openai";
        first.source.id = "codex-rollout-backfill";
        let mut second = snapshot(ACCOUNT_A, NOW, "five-hour", None, Some(NOW + 3), Some(30.0));
        second.provider = "openai";
        second.source.id = "codex-rollout-backfill";

        let first_id = store.record_provider_usage_snapshots(&[first]).unwrap()[0];
        assert_eq!(
            store.record_provider_usage_snapshots(&[second]).unwrap(),
            [first_id]
        );
        let history = store
            .provider_usage_period_history(first_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            history.observations.len(),
            2,
            "both readings join the same period despite the 3-second reset drift"
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
        store
            .save_settings(&AppSettings {
                session_data_retention_days: RETENTION_DAYS,
                ..AppSettings::default()
            })
            .unwrap();
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

    /// Retention keeps a period whose readings have expired, because its
    /// rollup is the only record of the peak the reader reached inside it.
    ///
    /// The allocation-era exemptions are gone. A materialized allocation row
    /// and a dirty-queue entry no longer hold a period alive. The rollup is
    /// the one exemption that remains.
    #[test]
    fn retention_keeps_an_expired_period_for_its_rollup() {
        let store = store();
        store
            .save_settings(&AppSettings {
                session_data_retention_days: RETENTION_DAYS,
                ..AppSettings::default()
            })
            .unwrap();
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
        assert!(
            period_exists,
            "a period with a rollup outlives its readings"
        );
        drop(connection);

        let rollups = store.provider_usage_period_rollups(0, 10).unwrap();
        assert_eq!(rollups.len(), 1);
        assert_eq!(rollups[0].period_id, period_id);
        assert_eq!(rollups[0].peak_used_percent, Some(10.0));
        assert_eq!(rollups[0].observation_count, 1);
    }

    #[test]
    fn a_rollup_states_the_peak_the_last_figure_and_the_refusal_count() {
        let store = store();
        let start = NOW - 18_000;
        let low = snapshot(
            ACCOUNT_A,
            start,
            "five-hour",
            Some(start),
            Some(NOW),
            Some(20.0),
        );
        let mut peak = snapshot(
            ACCOUNT_A,
            start + 600,
            "five-hour",
            Some(start),
            Some(NOW),
            Some(100.0),
        );
        peak.refusal_kind = Some("usage_limit_reached".to_string());
        let last = snapshot(
            ACCOUNT_A,
            start + 1_200,
            "five-hour",
            Some(start),
            Some(NOW),
            Some(65.0),
        );

        let changed = store
            .record_provider_usage_snapshots(&[low, peak, last])
            .unwrap();
        assert_eq!(changed.len(), 1);

        let rollups = store.provider_usage_period_rollups(0, 10).unwrap();
        assert_eq!(rollups.len(), 1);
        let rollup = &rollups[0];
        assert_eq!(rollup.period_id, changed[0]);
        assert_eq!(rollup.peak_used_percent, Some(100.0));
        assert_eq!(rollup.last_used_percent, Some(65.0));
        assert_eq!(rollup.observation_count, 3);
        assert_eq!(rollup.refusal_count, 1);
        assert_eq!(rollup.starts_at_epoch, Some(start));
        assert_eq!(rollup.resets_at_epoch, Some(NOW));
    }

    /// A refusal is a fact the first reading of a moment can miss. The
    /// reading that states it must reach the row and the rollup.
    #[test]
    fn a_refusal_reaches_a_stored_reading_of_the_same_moment() {
        let store = store();
        let start = NOW - 18_000;
        let quiet = snapshot(
            ACCOUNT_A,
            start + 600,
            "five-hour",
            Some(start),
            Some(NOW),
            Some(100.0),
        );
        let mut refused = quiet.clone();
        refused.refusal_kind = Some("usage_limit_reached".to_string());

        store.record_provider_usage_snapshots(&[quiet]).unwrap();
        let changed = store.record_provider_usage_snapshots(&[refused]).unwrap();
        assert_eq!(changed.len(), 1);

        let history = store
            .provider_usage_period_history(changed[0])
            .unwrap()
            .expect("the period exists");
        assert_eq!(
            history.observations[0].refusal_kind.as_deref(),
            Some("usage_limit_reached")
        );
        let rollups = store.provider_usage_period_rollups(0, 10).unwrap();
        assert_eq!(rollups[0].refusal_count, 1);
    }

    /// A reading that arrives after retention pruned the earlier ones must
    /// not lower the peak the period already reached.
    #[test]
    fn a_later_reading_never_lowers_a_recorded_peak() {
        let store = store();
        let start = NOW - 91 * 86_400 - 18_000;
        let reset = NOW - 91 * 86_400;
        let peak = snapshot(
            ACCOUNT_A,
            start,
            "five-hour",
            Some(start),
            Some(reset),
            Some(80.0),
        );
        let period_id = store.record_provider_usage_snapshots(&[peak]).unwrap()[0];
        store.apply_session_retention(NOW).unwrap();

        let late = snapshot(
            ACCOUNT_A,
            start + 60,
            "five-hour",
            Some(start),
            Some(reset),
            Some(5.0),
        );
        store.record_provider_usage_snapshots(&[late]).unwrap();

        let rollup = store
            .provider_usage_period_rollups(0, 10)
            .unwrap()
            .into_iter()
            .find(|rollup| rollup.period_id == period_id)
            .expect("the period keeps its rollup");
        assert_eq!(rollup.peak_used_percent, Some(80.0));
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
        store
            .save_settings(&AppSettings {
                session_data_retention_days: RETENTION_DAYS,
                ..AppSettings::default()
            })
            .unwrap();
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

    /// The daily series reads the long window only, in time order, and
    /// leaves out a reading the provider stated no figure for.
    #[test]
    fn readings_come_back_in_time_order_for_the_long_window_only() {
        let store = store();
        let start = NOW - 3 * 86_400;
        store
            .record_provider_usage_snapshots(&[
                weekly_snapshot(ACCOUNT_A, start, Some(start), Some(NOW), Some(10.0)),
                weekly_snapshot(ACCOUNT_A, start + 3_600, Some(start), Some(NOW), Some(28.0)),
                weekly_snapshot(ACCOUNT_A, start + 7_200, Some(start), Some(NOW), None),
                snapshot(
                    ACCOUNT_A,
                    start + 10_800,
                    "five-hour",
                    Some(start),
                    Some(NOW),
                    Some(90.0),
                ),
            ])
            .unwrap();

        let readings = store
            .provider_usage_readings(0, "primaryLong", 100)
            .unwrap();

        let figures: Vec<f64> = readings.iter().map(|row| row.used_percent).collect();
        assert_eq!(figures, vec![10.0, 28.0]);
        assert_eq!(readings[0].observed_at_epoch, start);
        assert_eq!(readings[1].observed_at_epoch, start + 3_600);
        assert_eq!(readings[0].period_id, readings[1].period_id);
    }

    /// The series reads every account, however many readings come before it.
    #[test]
    fn readings_page_past_one_query_and_keep_every_account() {
        // A single capped query answers in account order, so the accounts
        // that sort last fall off the end and get no series at all.
        let store = store();
        let start = NOW - 3 * 86_400;
        let snapshots: Vec<_> = (0i32..4)
            .flat_map(|step| {
                let observed_at = start + i64::from(step) * 3_600;
                [
                    weekly_snapshot(
                        ACCOUNT_A,
                        observed_at,
                        Some(start),
                        Some(NOW),
                        Some(f64::from(step) * 10.0),
                    ),
                    weekly_snapshot(
                        ACCOUNT_B,
                        observed_at,
                        Some(start),
                        Some(NOW),
                        Some(f64::from(step) * 5.0),
                    ),
                ]
            })
            .collect();
        store.record_provider_usage_snapshots(&snapshots).unwrap();

        let readings = store.provider_usage_readings(0, "primaryLong", 3).unwrap();

        assert_eq!(readings.len(), 8);
        let accounts: Vec<&str> = readings
            .iter()
            .map(|row| row.account_key.as_str())
            .collect();
        assert_eq!(
            accounts,
            vec![ACCOUNT_A; 4]
                .into_iter()
                .chain([ACCOUNT_B; 4])
                .collect::<Vec<_>>()
        );
        let times: Vec<i64> = readings
            .iter()
            .take(4)
            .map(|row| row.observed_at_epoch)
            .collect();
        assert_eq!(
            times,
            vec![start, start + 3_600, start + 7_200, start + 10_800]
        );
    }

    /// A reading older than the bound is not one the series asks for.
    #[test]
    fn readings_start_at_the_bound_the_caller_states() {
        let store = store();
        let start = NOW - 10 * 86_400;
        store
            .record_provider_usage_snapshots(&[
                weekly_snapshot(ACCOUNT_A, start, Some(start), Some(NOW), Some(10.0)),
                weekly_snapshot(ACCOUNT_A, NOW - 3_600, Some(start), Some(NOW), Some(40.0)),
            ])
            .unwrap();

        let readings = store
            .provider_usage_readings(NOW - 86_400, "primaryLong", 100)
            .unwrap();

        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].used_percent, 40.0);
    }
}

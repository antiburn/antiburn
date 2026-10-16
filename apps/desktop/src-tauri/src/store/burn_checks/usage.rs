use super::*;

use antiburn_local::analysis::jev::JevUsage;
use anyhow::ensure;

impl Store {
    /// Records installed-check usage once per candidate and numbered attempt.
    /// Known tokens can replace unknown usage. Session validity does not gate incurred usage.
    pub(crate) fn record_burn_check_decision_usage(
        &self,
        candidate_id: &str,
        attempt_number: u8,
        check_id: &str,
        usage: Option<JevUsage>,
        now_epoch: i64,
    ) -> anyhow::Result<()> {
        ensure!(
            candidate_id.len() == 64
                && candidate_id
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
                && (1..=2).contains(&attempt_number)
                && !check_id.is_empty(),
            "invalid installed-check usage identity"
        );
        let attempt_id = format!("{candidate_id}:{attempt_number}");
        let mut connection = self.lock();
        let transaction = connection.transaction()?;
        let previous: Option<bool> = transaction
            .query_row(
                "SELECT usage_known FROM burn_check_decision_usage WHERE attempt_id = ?1",
                [&attempt_id],
                |row| row.get(0),
            )
            .optional()?;
        if previous == Some(true) || (previous.is_some() && usage.is_none()) {
            return Ok(());
        }

        let mut ledger = load_ledger(&transaction)?;
        if let Some(usage) = usage {
            if previous.is_some() {
                ledger.summary.unknown_outcomes = ledger.summary.unknown_outcomes.saturating_sub(1);
            }
            record_confirmed_usage(
                &mut ledger.summary,
                &UsageReservation {
                    id: attempt_id.clone(),
                    session_key: String::new(),
                    provider: "typesafe-systemone".into(),
                    check_id: check_id.into(),
                    model: antiburn_local::analysis::jev::PINNED_MODEL.into(),
                    input_tokens: usage.input_tokens,
                    expires_at_epoch: now_epoch,
                    settled: true,
                    unknown_recorded: false,
                },
                usage.input_tokens,
                usage.output_tokens,
                now_epoch,
            );
        } else {
            ledger.summary.unknown_outcomes = ledger.summary.unknown_outcomes.saturating_add(1);
            ledger.summary.last_unknown_at_epoch = Some(now_epoch);
        }
        transaction.execute(
            "INSERT INTO burn_check_decision_usage (attempt_id, usage_known) VALUES (?1, ?2)
             ON CONFLICT(attempt_id) DO UPDATE SET usage_known = excluded.usage_known",
            rusqlite::params![attempt_id, usage.is_some()],
        )?;
        write_json_setting(&transaction, USAGE_LEDGER_KEY, &ledger)?;
        transaction.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANDIDATE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const USAGE: JevUsage = JevUsage {
        input_tokens: 1_000_000,
        output_tokens: 12,
    };

    fn store() -> Store {
        Store::open_in_memory(std::path::Path::new("installed-check-usage")).unwrap()
    }

    #[test]
    fn authored_check_usage_settles_once_without_native_admission() {
        let store = store();
        let check_id = "custom-check-".repeat(12);
        for _ in 0..2 {
            store
                .record_burn_check_decision_usage(CANDIDATE, 1, &check_id, Some(USAGE), 100)
                .unwrap();
        }
        let summary = store.burn_check_usage_summary().unwrap();
        assert_eq!(summary.input_tokens, USAGE.input_tokens);
        assert_eq!(summary.output_tokens, USAGE.output_tokens);
        assert_eq!(summary.confirmed_calls, 1);
        assert_eq!(summary.unknown_outcomes, 0);
        assert_eq!(summary.estimated_usd.as_deref(), Some("$0.042"));
        let native_rows: i64 = store
            .lock()
            .query_row(
                "SELECT (SELECT COUNT(*) FROM burn_check_assessment)
                    + (SELECT COUNT(*) FROM burn_check_usage_reservation)
                    + (SELECT COUNT(*) FROM burn_check_request_outcome)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(native_rows, 0);
    }

    #[test]
    fn known_usage_replaces_unknown_once_and_attempts_remain_separate() {
        let store = store();
        for _ in 0..2 {
            store
                .record_burn_check_decision_usage(CANDIDATE, 1, "custom-check", None, 100)
                .unwrap();
        }
        let summary = store.burn_check_usage_summary().unwrap();
        assert_eq!(summary.unknown_outcomes, 1);
        assert_eq!(summary.confirmed_calls, 0);
        for usage in [Some(USAGE), Some(USAGE), None] {
            store
                .record_burn_check_decision_usage(CANDIDATE, 1, "custom-check", usage, 101)
                .unwrap();
        }
        store
            .record_burn_check_decision_usage(CANDIDATE, 2, "custom-check", Some(USAGE), 102)
            .unwrap();
        let summary = store.burn_check_usage_summary().unwrap();
        assert_eq!(summary.unknown_outcomes, 0);
        assert_eq!(summary.confirmed_calls, 2);
        assert_eq!(summary.input_tokens, USAGE.input_tokens * 2);
        assert_eq!(summary.output_tokens, USAGE.output_tokens * 2);
    }

    #[test]
    fn marker_and_usage_total_roll_back_together() {
        let store = store();
        store.lock().execute_batch("CREATE TRIGGER reject_usage BEFORE INSERT ON setting WHEN NEW.key = 'internal:burnCheckUsageLedgerV1' BEGIN SELECT RAISE(ABORT, 'injected failure'); END;").unwrap();
        let result =
            store.record_burn_check_decision_usage(CANDIDATE, 1, "custom-check", Some(USAGE), 100);
        assert!(result.is_err());
        assert_eq!(store.burn_check_usage_summary().unwrap().confirmed_calls, 0);
        let markers: i64 = store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_decision_usage",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(markers, 0);
        store
            .lock()
            .execute_batch("DROP TRIGGER reject_usage")
            .unwrap();
        store
            .record_burn_check_decision_usage(CANDIDATE, 1, "custom-check", Some(USAGE), 100)
            .unwrap();
        assert_eq!(store.burn_check_usage_summary().unwrap().confirmed_calls, 1);
    }

    #[test]
    fn accounting_survives_reopen_and_session_deletion_but_clears_with_local_data() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store
            .record_burn_check_decision_usage(CANDIDATE, 1, "custom-check", Some(USAGE), 100)
            .unwrap();
        drop(store);
        let store = Store::open(directory.path()).unwrap();
        store
            .record_burn_check_decision_usage(CANDIDATE, 1, "custom-check", Some(USAGE), 101)
            .unwrap();
        store.lock().execute("INSERT INTO session (environment_key, agent, session_id, source_kind, source_label, first_seen_at, last_seen_at) VALUES ('native', 'fixture', 'session', 'file', 'fixture', 'now', 'now')", []).unwrap();
        store
            .delete_session(&SessionKey::new("native", "fixture", "session"))
            .unwrap();
        assert_eq!(store.burn_check_usage_summary().unwrap().confirmed_calls, 1);
        store.clear_local_session_data().unwrap();
        assert_eq!(store.burn_check_usage_summary().unwrap().confirmed_calls, 0);
        let markers: i64 = store
            .lock()
            .query_row(
                "SELECT COUNT(*) FROM burn_check_decision_usage",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(markers, 0);
    }
}

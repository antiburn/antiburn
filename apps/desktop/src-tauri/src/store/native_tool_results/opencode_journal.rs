//! Append-safe leases and atomic OpenCode journal publication.

use antiburn_local::analysis::{OpenCodeJournalCursor, OpenCodeToolResultPage, PARSER_REVISION};
use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::super::publications::{
    PublicationGapKind, PublicationPolicy, ToolResultBatch, publish_coverage_in,
};
use crate::store::selected_source::{
    LIVE_ACTIVATION_SQL, OPENCODE_V1_SOURCE_SQL, ROOT_SESSION_SQL,
};
use crate::store::{Incarnation, SessionKey, Store};

const JOURNAL_POLL_SECS: i64 = 1;
const DORMANT_POLL_SECS: i64 = 30;
const MAX_READ_ATTEMPTS: i64 = 5;

fn candidates_sql() -> String {
    format!(
        "WITH source_sessions AS (
    SELECT s.*, ({ROOT_SESSION_SQL}) AS main_session
       FROM session s WHERE ({OPENCODE_V1_SOURCE_SQL}) AND s.wsl_distro IS NULL
         AND s.cwd IS NOT NULL AND s.cwd <> ''
         AND ({LIVE_ACTIVATION_SQL})
), candidates AS (
    SELECT s.environment_key, s.agent, s.session_id, s.incarnation, s.source_label,
           s.cwd, s.source_generation, s.updated_at_epoch,
           d.cursor_json, d.baseline_sequence, d.retry_count, d.observed_generation,
           d.pending, d.blocked, d.lease_expires_at_epoch, d.next_attempt_at_epoch,
              d.service_ticket, d.history_gap, d.last_error, s.main_session,
            (d.incarnation IS NULL OR d.incarnation <> s.incarnation
                OR d.source_label <> s.source_label OR d.cwd IS NOT s.cwd
                 OR d.main_session <> s.main_session) AS reset
      FROM source_sessions s LEFT JOIN opencode_journal_delivery d
        ON d.environment_key = s.environment_key AND d.agent = s.agent
       AND d.session_id = s.session_id
      WHERE s.main_session AND (d.parser_revision IS NULL OR d.parser_revision IN (?1, 62, 63, 64, 65, 66))
), scheduled AS (
    SELECT *, CASE WHEN reset THEN ?2 ELSE MAX(COALESCE(lease_expires_at_epoch, ?2),
        CASE WHEN observed_generation <> source_generation THEN ?2
            ELSE COALESCE(next_attempt_at_epoch, ?2) END) END AS due_at
      FROM candidates
     WHERE (reset AND updated_at_epoch >= ?3)
        OR (NOT reset AND blocked = 0 AND (pending = 1
            OR observed_generation <> source_generation OR next_attempt_at_epoch IS NOT NULL))
)"
    )
}

#[derive(Debug, Clone)]
pub(crate) struct OpenCodeJournalClaim {
    pub key: SessionKey,
    incarnation: Incarnation,
    source_label: String,
    cwd: Option<String>,
    source_generation: i64,
    claim_fence: i64,
    cursor_json: Option<String>,
    parser_revision: Option<i64>,
    pub(crate) baseline_sequence: Option<u64>,
    pub(crate) requires_full_read: bool,
    history_gap: bool,
    main_session: bool,
    pub(crate) retry_count: i64,
}

impl OpenCodeJournalClaim {
    pub(crate) fn cursor(&self) -> Result<Option<OpenCodeJournalCursor>> {
        ensure!(
            self.cursor_json
                .as_ref()
                .is_none_or(|json| json.len() <= 1024 * 1024),
            "journal cursor exceeds its state limit"
        );
        self.cursor_json
            .as_deref()
            .map(|encoded| {
                let value: serde_json::Value = serde_json::from_str(encoded)?;
                let parser = value["parser_revision"]
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("journal parser revision is invalid"))?;
                ensure!(
                    self.parser_revision == Some(parser),
                    "journal parser binding mismatch"
                );
                let layout = if value["live"].is_null() {
                    None
                } else {
                    Some(
                        value["live"]["revision"]
                            .as_u64()
                            .ok_or_else(|| anyhow::anyhow!("journal live revision is invalid"))?,
                    )
                };
                ensure!(
                    matches!(
                        (parser, layout),
                        (64, Some(2)) | (62, Some(4)) | (63..=66, Some(5)) | (62..=66, None)
                    ) || (parser == PARSER_REVISION && matches!(layout, None | Some(5))),
                    "journal parser and live layout are incompatible"
                );
                if (parser, layout) == (64, Some(2)) {
                    OpenCodeJournalCursor::decode_main_64_json(encoded.as_bytes())
                } else {
                    Ok(serde_json::from_value(value)?)
                }
            })
            .transpose()
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum JournalFailure {
    SourceChanged,
    IncompleteHistory,
    InvalidCursor,
    Unavailable,
    ReadFailed,
    Cancelled,
}

impl JournalFailure {
    fn label(self) -> &'static str {
        match self {
            Self::SourceChanged => "journal-prefix-changed",
            Self::IncompleteHistory => "journal-history-incomplete",
            Self::InvalidCursor => "journal-cursor-invalid",
            Self::Unavailable => "journal-unavailable",
            Self::ReadFailed => "journal-read-failed",
            Self::Cancelled => "journal-cancelled",
        }
    }

    fn blocked(self) -> bool {
        matches!(self, Self::SourceChanged | Self::InvalidCursor)
    }
}

impl Store {
    pub(crate) fn recover_opencode_journal_claims(&self) -> Result<usize> {
        Ok(self.lock().execute(
            "UPDATE opencode_journal_delivery SET pending = 1,
                 lease_expires_at_epoch = NULL, next_attempt_at_epoch = NULL
             WHERE lease_expires_at_epoch IS NOT NULL",
            [],
        )?)
    }

    pub(crate) fn claim_next_opencode_journal(
        &self,
        now: i64,
        lease_secs: i64,
    ) -> Result<Option<OpenCodeJournalClaim>> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let fair_turn = tx.query_row(
            "SELECT successful_turns % 4 = 3 FROM work_queue_schedule
              WHERE domain = 'native_delivery'",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        let order = if fair_turn {
            "COALESCE(service_ticket, (SELECT next_ticket FROM work_queue_schedule
                WHERE domain = 'native_delivery')),"
        } else {
            "updated_at_epoch IS NULL, updated_at_epoch DESC,"
        };
        let selected = super::next_live_source_in(&tx, now, fair_turn)?;
        if selected
            .as_ref()
            .is_none_or(|(family, _, _, _)| family != "journal")
        {
            return Ok(None);
        }
        let candidate = tx
            .query_row(
                &format!(
                    "{}
            SELECT environment_key, agent, session_id, incarnation, source_label, cwd,
                   source_generation, cursor_json, baseline_sequence,
                    COALESCE(retry_count, 0), reset, observed_generation, history_gap, main_session,
                     COALESCE(last_error NOT IN ('journal-history-incomplete',
                         'journal-main-session-unavailable'), 0),
                     (SELECT d.parser_revision FROM opencode_journal_delivery d WHERE d.environment_key = scheduled.environment_key
                       AND d.agent = scheduled.agent AND d.session_id = scheduled.session_id)
              FROM scheduled WHERE due_at <= ?2
               ORDER BY {order} service_ticket, session_id
             LIMIT 1",
                    candidates_sql()
                ),
                params![PARSER_REVISION, now, now - ACTIVE_SESSION_WINDOW_SECS],
                |row| {
                    let reset: bool = row.get(10)?;
                    let generation = row.get(6)?;
                    Ok(OpenCodeJournalClaim {
                        key: SessionKey::new(
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ),
                        incarnation: Incarnation(row.get(3)?),
                        source_label: row.get(4)?,
                        cwd: row.get(5)?,
                        source_generation: generation,
                        claim_fence: 0,
                        cursor_json: if reset { None } else { row.get(7)? },
                        parser_revision: row.get(15)?,
                        baseline_sequence: if reset { None } else { row.get(8)? },
                        requires_full_read: reset
                            || row.get::<_, Option<i64>>(15)? != Some(PARSER_REVISION)
                            || row.get::<_, Option<i64>>(11)? != Some(generation)
                            || row.get::<_, bool>(14)?,
                        history_gap: if reset { false } else { row.get(12)? },
                        main_session: row.get(13)?,
                        retry_count: if reset || row.get::<_, Option<i64>>(11)? != Some(generation)
                        {
                            0
                        } else {
                            row.get(9)?
                        },
                    })
                },
            )
            .optional()?;
        let Some(mut claim) = candidate else {
            return Ok(None);
        };
        let service_ticket: i64 = tx.query_row(
            "SELECT next_ticket FROM work_queue_schedule WHERE domain = 'native_delivery'",
            [],
            |row| row.get(0),
        )?;
        claim.claim_fence = tx.query_row(
            "UPDATE evidence_claim_fence_seq SET value = value + 1 WHERE id = 1 RETURNING value",
            [],
            |row| row.get(0),
        )?;
        tx.execute("INSERT INTO opencode_journal_delivery (
                environment_key, agent, session_id, incarnation, source_label, cwd,
                parser_revision, cursor_json, baseline_sequence, claim_fence,
                lease_expires_at_epoch, last_attempt_at_epoch, retry_count, history_gap,
                main_session, service_ticket)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                incarnation = excluded.incarnation, source_label = excluded.source_label,
                 cwd = excluded.cwd,
                cursor_json = excluded.cursor_json, baseline_sequence = excluded.baseline_sequence,
                pending = 1, blocked = 0, claim_fence = excluded.claim_fence,
                lease_expires_at_epoch = excluded.lease_expires_at_epoch,
                last_attempt_at_epoch = excluded.last_attempt_at_epoch, retry_count = excluded.retry_count,
                history_gap = excluded.history_gap, main_session = excluded.main_session,
                service_ticket = excluded.service_ticket",
            params![claim.key.environment_key, claim.key.agent, claim.key.session_id,
                claim.incarnation.0, claim.source_label, claim.cwd, PARSER_REVISION,
                claim.cursor_json, claim.baseline_sequence, claim.claim_fence,
                now + lease_secs, now, claim.retry_count, claim.history_gap, claim.main_session,
                service_ticket])?;
        tx.execute(
            "UPDATE work_queue_schedule
                SET next_ticket = next_ticket + 1, successful_turns = successful_turns + 1
              WHERE domain = 'native_delivery'",
            [],
        )?;
        tx.commit()?;
        Ok(Some(claim))
    }

    pub(crate) fn renew_opencode_journal_lease(
        &self,
        claim: &OpenCodeJournalClaim,
        now: i64,
        lease_secs: i64,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        if current_generation(&tx, claim, now)?.is_none() {
            return Ok(false);
        }
        tx.execute(
            "UPDATE opencode_journal_delivery SET lease_expires_at_epoch = ?4
            WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                now + lease_secs
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn publish_opencode_journal_page(
        &self,
        claim: &OpenCodeJournalClaim,
        page: &OpenCodeToolResultPage,
        now: i64,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(generation) = current_generation(&tx, claim, now)? else {
            return Ok(false);
        };
        let cursor_json = serde_json::to_string(&page.cursor)?;
        ensure!(
            cursor_json.len() <= 1024 * 1024,
            "journal cursor exceeds its state limit"
        );
        ensure!(
            !page.has_more || claim.cursor_json.as_ref() != Some(&cursor_json),
            "journal page does not advance its cursor"
        );
        let baseline = page
            .cursor
            .live_baseline()
            .or(claim.baseline_sequence)
            .unwrap_or(page.snapshot_end);
        let history_complete = page.history_complete
            && page.evidence.history_complete
            && !claim.history_gap
            && claim.main_session;
        let unsafe_reason = page.evidence.events.iter().find_map(|event| {
            if let antiburn_local::analysis::NativeEvidenceFact::AdmissionFailure(failure) =
                &event.fact
            {
                super::super::admission::reason(failure.reason)
            } else {
                None
            }
        });
        let gap_reason = if unsafe_reason.is_some() {
            unsafe_reason
        } else if !claim.main_session {
            Some("journal-main-session-unavailable")
        } else {
            (!history_complete).then_some("journal-history-incomplete")
        };
        let consumed = super::super::evidence::reconcile_evidence_in(
            &tx,
            &claim.key,
            generation,
            claim.claim_fence,
            &page.evidence,
            PublicationPolicy {
                invalidation_effects: None,
                baseline: Some(baseline),
                history_complete: Some(history_complete),
                gap_reason,
                gap_kind: if claim.main_session {
                    PublicationGapKind::Partial
                } else {
                    PublicationGapKind::Invalidated
                },
                suppress_coverage: false,
                live_eligible: None,
            },
            now * 1000,
        )?;
        super::super::observers::frontier_in(
            &tx,
            &claim.key,
            &cursor_json,
            consumed
                && unsafe_reason.is_none()
                && !page.has_more
                && generation == claim.source_generation,
            now * 1000,
        )?;
        if !consumed {
            let retry = match claim.cursor()? {
                Some(cursor) if cursor.live_baseline().is_some() => {
                    cursor.main_64_retry_json(&page.cursor)?
                }
                _ => page.cold_retry_json()?,
            };
            let retry_baseline = retry.as_ref().and(page.cursor.live_baseline());
            tx.execute(
                "UPDATE opencode_journal_delivery SET pending = 1, lease_expires_at_epoch = NULL,
                next_attempt_at_epoch = ?4, cursor_json = COALESCE(?5, cursor_json),
                baseline_sequence = CASE WHEN ?5 IS NOT NULL THEN ?7 ELSE baseline_sequence END,
                parser_revision = CASE WHEN ?5 IS NOT NULL THEN ?6 ELSE parser_revision END
                WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    now,
                    retry,
                    PARSER_REVISION,
                    retry_baseline
                ],
            )?;
            tx.commit()?;
            return Ok(true);
        }
        let coverage_reason: Option<String> = tx
            .query_row(
                "SELECT gap_reason FROM native_tool_result_publication
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
               AND source_format = ?4 AND outcome IN ('coverage_gap', 'coverage_complete')
             ORDER BY publication_sequence DESC LIMIT 1",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    serde_json::to_string(
                        &antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1
                    )?
                ],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        let history_complete = history_complete
            && coverage_reason.as_deref() != Some("native-message-removal-unbound");
        let pending = page.has_more || generation != claim.source_generation;
        let next_attempt = if pending {
            now
        } else if claim.cursor_json.as_ref() != Some(&cursor_json) {
            now + JOURNAL_POLL_SECS
        } else {
            next_idle_poll(&tx, &claim.key, now)?
        };
        let updated = tx.execute(
            "UPDATE opencode_journal_delivery
            SET cursor_json = ?4, baseline_sequence = ?5, observed_generation = ?6,
                pending = ?7, lease_expires_at_epoch = NULL,
                next_attempt_at_epoch = ?8, retry_count = 0, last_error = ?9, history_gap = ?12,
                service_ticket = CASE WHEN ?7 THEN service_ticket ELSE NULL END, parser_revision = ?13
            WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3
              AND claim_fence = ?10 AND cursor_json IS ?11",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                cursor_json,
                baseline,
                claim.source_generation,
                pending,
                next_attempt,
                gap_reason,
                claim.claim_fence,
                claim.cursor_json,
                !history_complete,
                PARSER_REVISION
            ],
        )?;
        ensure!(updated == 1, "journal cursor compare-and-swap failed");
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn fail_opencode_journal(
        &self,
        claim: &OpenCodeJournalClaim,
        failure: JournalFailure,
        now: i64,
        retry_at: i64,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(generation) = current_generation(&tx, claim, now)? else {
            return Ok(false);
        };
        let cancelled = matches!(failure, JournalFailure::Cancelled);
        let retries = claim.retry_count + i64::from(!cancelled);
        let pending = !failure.blocked() && retries < MAX_READ_ATTEMPTS;
        if !cancelled {
            super::super::observers::break_continuity_in(&tx, &claim.key)?;
            publish_coverage_in(
                &tx,
                &claim.key,
                generation,
                claim.claim_fence,
                &ToolResultBatch {
                    source_format: antiburn_local::analysis::SourceFormat::OpenCodeSqliteV1,
                    results: Vec::new(),
                    complete: false,
                },
                false,
                Some(failure.label()),
            )?;
        }
        tx.execute(
            "UPDATE opencode_journal_delivery
            SET pending = ?4, blocked = ?5, observed_generation = ?6,
                lease_expires_at_epoch = NULL,
                next_attempt_at_epoch = ?7, retry_count = ?8, last_error = ?9,
                service_ticket = CASE WHEN ?4 THEN service_ticket ELSE NULL END
            WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                pending,
                failure.blocked(),
                claim.source_generation,
                pending.then_some(if cancelled {
                    now + JOURNAL_POLL_SECS
                } else {
                    retry_at
                }),
                retries,
                failure.label()
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn finish_opencode_journal_poll(
        &self,
        claim: &OpenCodeJournalClaim,
        now: i64,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(generation) = current_generation(&tx, claim, now)? else {
            return Ok(false);
        };
        ensure!(
            !claim.requires_full_read,
            "journal recovery requires a validated page"
        );
        if generation == claim.source_generation
            && let Some(receipt) = claim.cursor_json.as_deref()
        {
            super::super::observers::frontier_in(&tx, &claim.key, receipt, true, now * 1000)?;
        }
        let next_attempt = if generation != claim.source_generation {
            now
        } else {
            next_idle_poll(&tx, &claim.key, now)?
        };
        tx.execute(
            "UPDATE opencode_journal_delivery
             SET pending = ?4, observed_generation = ?5,
                 lease_expires_at_epoch = NULL, next_attempt_at_epoch = ?6,
                 service_ticket = CASE WHEN ?4 THEN service_ticket ELSE NULL END
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                generation != claim.source_generation,
                claim.source_generation,
                next_attempt
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }

    pub(crate) fn next_opencode_journal_attempt(&self, now: i64) -> Result<Option<i64>> {
        Ok(self.lock().query_row(
            &format!("{} SELECT MIN(due_at) FROM scheduled", candidates_sql()),
            params![PARSER_REVISION, now, now - ACTIVE_SESSION_WINDOW_SECS],
            |row| row.get(0),
        )?)
    }
}

fn next_idle_poll(tx: &Transaction<'_>, key: &SessionKey, now: i64) -> Result<i64> {
    let active = tx.query_row(
        "SELECT COALESCE(updated_at_epoch >= ?4, 0) FROM session
          WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3",
        params![
            key.environment_key,
            key.agent,
            key.session_id,
            now - ACTIVE_SESSION_WINDOW_SECS
        ],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(now
        + if active {
            JOURNAL_POLL_SECS
        } else {
            DORMANT_POLL_SECS
        })
}

fn current_generation(
    tx: &Transaction<'_>,
    claim: &OpenCodeJournalClaim,
    now: i64,
) -> Result<Option<i64>> {
    Ok(tx.query_row("SELECT s.source_generation FROM opencode_journal_delivery d JOIN session s
        ON s.environment_key = d.environment_key AND s.agent = d.agent AND s.session_id = d.session_id
        WHERE d.environment_key = ?1 AND d.agent = ?2 AND d.session_id = ?3
          AND d.claim_fence = ?4 AND d.cursor_json IS ?5 AND d.lease_expires_at_epoch > ?6
          AND d.incarnation = ?7 AND s.incarnation = ?7
          AND s.source_kind = 'providerDb' AND s.wsl_distro IS NULL AND s.source_label = ?8 AND s.cwd IS ?9
          AND d.source_label = ?8 AND d.cwd IS ?9 AND d.parser_revision IN (?10, 62, 63, 64, 65, 66)
          AND d.main_session = ?11 AND ?11 = (NOT EXISTS (
              SELECT 1 FROM session_relation r WHERE r.environment_key = s.environment_key
                AND r.agent = s.agent AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                  OR (r.related_id = s.session_id AND r.kind = 'subagent'))))",
        params![claim.key.environment_key, claim.key.agent, claim.key.session_id,
            claim.claim_fence, claim.cursor_json, now, claim.incarnation.0,
            claim.source_label, claim.cwd, PARSER_REVISION, claim.main_session], |row| row.get(0)).optional()?)
}

#[cfg(test)]
#[path = "journal/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "journal/cold_retry_tests.rs"]
mod cold_retry_tests;

#[cfg(test)]
#[path = "journal_regression_tests.rs"]
mod regression_tests;

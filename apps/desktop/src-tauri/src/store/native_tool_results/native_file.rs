//! Native file receipts do not depend on metric projections.

use antiburn_local::analysis::file_live::{NativeFileCursor, NativeFileReadError};
use antiburn_local::analysis::{PARSER_REVISION, SourceFormat, ToolResultObservation};
use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::super::publications::PublicationPolicy;
use crate::store::{SessionKey, SessionRecord, Store};

const ACTIVE_POLL_SECS: i64 = 1;
const DORMANT_POLL_SECS: i64 = 30;
const MAX_READ_ATTEMPTS: i64 = 5;
pub(crate) const MAX_CURSOR_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn owns_session(record: &SessionRecord) -> bool {
    record.key.environment_key == "native"
        && record.wsl_distro.is_none()
        && record.source_kind == "file"
        && source_format(&record.key.agent).is_some()
}

pub(crate) fn source_format(agent: &str) -> Option<SourceFormat> {
    match agent {
        "claude-code" => Some(SourceFormat::ClaudeJsonl),
        "codex" => Some(SourceFormat::CodexRolloutJsonl),
        "pi" => Some(SourceFormat::PiV3Jsonl),
        _ => None,
    }
}

// A changed path keeps the cursor so the reader can validate file identity.
const CANDIDATES: &str = "WITH candidates AS (
    SELECT s.environment_key, s.agent, s.session_id, s.incarnation, s.source_label,
            s.cwd, s.source_format, s.source_generation, s.updated_at_epoch, d.cursor_json, d.parser_revision,
           d.retry_count, d.observed_generation, d.pending, d.blocked,
            d.lease_expires_at_epoch, d.next_attempt_at_epoch, d.service_ticket, d.last_error,
           COALESCE(d.active_until_epoch, 0) AS active_until_epoch,
            (d.source_label <> s.source_label OR d.cwd IS NOT s.cwd) AS binding_changed,
            (d.incarnation IS NULL OR d.incarnation <> s.incarnation
              OR ((d.source_label <> s.source_label OR d.cwd IS NOT s.cwd) AND d.cursor_json IS NULL)
              ) AS reset
      FROM session s LEFT JOIN native_file_delivery d USING(environment_key, agent, session_id)
     WHERE s.environment_key = 'native' AND s.wsl_distro IS NULL
        AND s.agent IN ('claude-code', 'codex', 'pi') AND s.source_kind = 'file'
        AND (d.parser_revision IS NULL OR d.parser_revision IN (?1, 62, 63, 64, 65, 66))
        AND s.cwd IS NOT NULL AND s.cwd <> ''
        AND EXISTS(SELECT 1 FROM burn_check_activation a
          WHERE a.enabled = 1 AND json_extract(a.activation_scope_key, '$.environment') = s.environment_key
            AND json_extract(a.activation_scope_key, '$.session_cwd') = s.cwd
            AND json_extract(a.activation_scope_key, '$.run') = 'Live')
       AND NOT EXISTS(SELECT 1 FROM session_relation r
         WHERE r.environment_key = s.environment_key AND r.agent = s.agent
           AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
             OR (r.related_id = s.session_id AND r.kind = 'subagent')))
)";

#[derive(Debug)]
pub(crate) struct NativeFileClaim {
    pub key: SessionKey,
    pub source_label: String,
    pub cursor_json: Option<String>,
    pub retry_count: i64,
    incarnation: i64,
    cwd: String,
    generation: i64,
    fence: i64,
    notify_on_success: bool,
    reset: bool,
    parser_revision: Option<i64>,
    request_origin_boundary: bool,
}

impl NativeFileClaim {
    pub(crate) fn cursor(&self) -> Result<Option<NativeFileCursor>> {
        let Some(encoded) = &self.cursor_json else {
            return Ok(None);
        };
        ensure!(
            encoded.len() <= MAX_CURSOR_BYTES,
            "native file cursor exceeds its limit"
        );
        let value: serde_json::Value = serde_json::from_str(encoded)?;
        let cursor = match (self.parser_revision, value["revision"].as_u64()) {
            (Some(64), Some(1)) => NativeFileCursor::convert_main_64_json(encoded.as_bytes())?,
            (Some(62), Some(4)) => NativeFileCursor::convert_v4_json(encoded.as_bytes())?,
            (Some(revision), Some(5))
                if revision == PARSER_REVISION || matches!(revision, 63..=66) =>
            {
                let cursor: NativeFileCursor = serde_json::from_value(value)?;
                if revision < 65
                    || (self.key.agent == "claude-code" && !self.request_origin_boundary)
                {
                    cursor.migrate_pre_65_claude_requests()?
                } else {
                    cursor
                }
            }
            _ => anyhow::bail!("unsupported native file parser and cursor revisions"),
        };
        Ok(Some(cursor))
    }
}

impl Store {
    pub(crate) fn recover_native_file_claims(&self) -> Result<()> {
        self.lock().execute(
            "UPDATE native_file_delivery SET pending = 1, claim_fence = NULL,
                lease_expires_at_epoch = NULL, next_attempt_at_epoch = NULL
             WHERE claim_fence IS NOT NULL",
            [],
        )?;
        Ok(())
    }

    pub(crate) fn claim_next_native_file(
        &self,
        now: i64,
        lease_secs: i64,
    ) -> Result<Option<NativeFileClaim>> {
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
            .is_none_or(|(family, _, _, _)| family != "file")
        {
            return Ok(None);
        }
        let origin_boundary = super::super::request_origin::boundary_sql("candidates");
        let claim = tx.query_row(
            &format!("{CANDIDATES}
             SELECT environment_key, agent, session_id, incarnation, source_label, cwd,
                    source_generation, CASE WHEN reset THEN NULL ELSE cursor_json END,
                     CASE WHEN reset THEN 0 ELSE COALESCE(retry_count, 0) END,
                      reset OR binding_changed OR pending = 1 OR observed_generation <> source_generation OR last_error IS NOT NULL,
                       reset, parser_revision, {origin_boundary} IS NOT NULL
               FROM candidates
              WHERE ((reset AND updated_at_epoch >= ?3)
                 OR (NOT reset AND blocked = 0 AND (pending = 1
                     OR source_generation <> observed_generation
                     OR next_attempt_at_epoch <= ?2)))
                AND COALESCE(lease_expires_at_epoch, 0) <= ?2
                AND (reset OR source_generation <> observed_generation
                     OR COALESCE(next_attempt_at_epoch, 0) <= ?2)
               ORDER BY {order} service_ticket, agent, session_id LIMIT 1"),
            params![PARSER_REVISION, now, now - ACTIVE_SESSION_WINDOW_SECS],
            |row| Ok((NativeFileClaim {
                key: SessionKey::new(row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?),
                incarnation: row.get(3)?, source_label: row.get(4)?, cwd: row.get(5)?,
                generation: row.get(6)?, cursor_json: row.get(7)?, retry_count: row.get(8)?, fence: 0,
                   notify_on_success: row.get(9)?, reset: row.get(10)?, parser_revision: row.get(11)?,
                   request_origin_boundary: row.get(12)?,
            }, row.get::<_, bool>(10)?)),
        ).optional()?;
        let Some((mut claim, reset)) = claim else {
            return Ok(None);
        };
        let service_ticket: i64 = tx.query_row(
            "SELECT next_ticket FROM work_queue_schedule WHERE domain = 'native_delivery'",
            [],
            |row| row.get(0),
        )?;
        claim.fence = tx.query_row(
            "UPDATE evidence_claim_fence_seq SET value = value + 1 WHERE id = 1 RETURNING value",
            [],
            |row| row.get(0),
        )?;
        let state_reset = if reset {
            "cursor_json = NULL, last_error = NULL,"
        } else {
            ""
        };
        tx.execute(
            &format!("INSERT INTO native_file_delivery (environment_key, agent, session_id, incarnation,
                source_label, cwd, parser_revision, claim_fence,
                lease_expires_at_epoch, last_attempt_at_epoch, retry_count, active_until_epoch,
                last_attempt_order, service_ticket)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?10, ?8, ?12)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                incarnation = excluded.incarnation, source_label = excluded.source_label,
                cwd = excluded.cwd,
                {state_reset} pending = 1, blocked = 0,
                claim_fence = excluded.claim_fence, lease_expires_at_epoch = excluded.lease_expires_at_epoch,
                last_attempt_at_epoch = excluded.last_attempt_at_epoch, retry_count = excluded.retry_count,
                 last_attempt_order = excluded.last_attempt_order,
                 service_ticket = excluded.service_ticket"),
            params![claim.key.environment_key, claim.key.agent, claim.key.session_id, claim.incarnation,
                claim.source_label, claim.cwd, PARSER_REVISION, claim.fence,
                now + lease_secs, now, claim.retry_count, service_ticket],
        )?;
        tx.execute(
            "UPDATE work_queue_schedule
                SET next_ticket = next_ticket + 1, successful_turns = successful_turns + 1
              WHERE domain = 'native_delivery'",
            [],
        )?;
        tx.commit()?;
        Ok(Some(claim))
    }

    /// Returns whether a committed page changes report or source progress.
    pub(crate) fn publish_native_file_page(
        &self,
        claim: &NativeFileClaim,
        page: &antiburn_local::analysis::file_live::NativeFilePage,
        _results: &[ToolResultObservation],
        now: i64,
    ) -> Result<bool> {
        let cursor_json = serde_json::to_string(&page.cursor)?;
        ensure!(
            cursor_json.len() <= MAX_CURSOR_BYTES,
            "native file cursor exceeds its limit"
        );
        let has_more = page.has_more;
        let history_complete = page.history_complete;
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(generation) = current_generation(&tx, claim, now)? else {
            return Ok(false);
        };
        ensure!(
            Some(page.evidence.source_format) == source_format(&claim.key.agent),
            "native file evidence source mismatch"
        );
        ensure!(
            !has_more || claim.cursor_json.as_deref() != Some(cursor_json.as_str()),
            "file page does not advance"
        );
        let claude = claim.key.agent == "claude-code";
        let origin_boundary = super::super::request_origin::ensure_boundary_in(
            &tx,
            &claim.key,
            generation,
            claim.fence,
        )?;
        let origin_upgrade = claude
            && (origin_boundary
                || !matches!(claim.parser_revision, Some(65 | 66 | PARSER_REVISION)));
        let mut unsafe_reason = if claim.reset {
            None
        } else {
            super::super::admission::source_reason_in(&tx, &claim.key)?
        };
        let mut has_admission_failure = false;
        if let Some(failure) = &page.admission_failure {
            ensure!(
                failure.source_format == page.evidence.source_format
                    && failure.session_id == claim.key.session_id,
                "unsafe admission source mismatch"
            );
            unsafe_reason = Some(
                super::super::admission::reason(failure.reason)
                    .ok_or_else(|| anyhow::anyhow!("admission failure must have an unsafe reason"))?
                    .into(),
            );
        }
        for event in &page.evidence.events {
            if let antiburn_local::analysis::NativeEvidenceFact::AdmissionFailure(failure) =
                &event.fact
            {
                has_admission_failure = true;
                ensure!(
                    failure.source_format == page.evidence.source_format
                        && failure.session_id == claim.key.session_id,
                    "unsafe admission source mismatch"
                );
                unsafe_reason = Some(
                    super::super::admission::reason(failure.reason)
                        .ok_or_else(|| {
                            anyhow::anyhow!("admission failure must have an unsafe reason")
                        })?
                        .into(),
                );
            }
        }
        if let Some(reason) = &unsafe_reason
            && !has_admission_failure
        {
            super::super::admission::invalidate_in(&tx, &claim.key, reason)?;
        } else if claim.reset {
            tx.execute(
                "UPDATE native_file_delivery SET last_error = NULL WHERE environment_key = ?1
                AND agent = ?2 AND session_id = ?3 AND claim_fence = ?4",
                params![
                    claim.key.environment_key,
                    claim.key.agent,
                    claim.key.session_id,
                    claim.fence
                ],
            )?;
        }
        let consumed = super::super::evidence::reconcile_evidence_in(
            &tx,
            &claim.key,
            generation,
            claim.fence,
            &page.evidence,
            PublicationPolicy {
                invalidation_effects: None,
                baseline: Some(page.baseline_offset),
                history_complete: Some(history_complete && unsafe_reason.is_none()),
                gap_reason: unsafe_reason
                    .as_deref()
                    .or((!history_complete).then_some("file-history-incomplete")),
                gap_kind: if unsafe_reason.is_some() {
                    super::super::publications::PublicationGapKind::Invalidated
                } else {
                    super::super::publications::PublicationGapKind::Partial
                },
                suppress_coverage: origin_upgrade,
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
                && !page.pending_tail
                && page.baseline_reached
                && generation == claim.generation,
            now * 1000,
        )?;
        if !consumed {
            // Retain the old read position. Saved outputs make the next reconciliation advance.
            let retry_cursor = claim
                .cursor()?
                .or_else(|| page.initial_cursor.clone())
                .context("native file page has no retry state")?
                .retry_json_after_page(&page.cursor)?;
            tx.execute("UPDATE native_file_delivery SET pending = 1, claim_fence = NULL, lease_expires_at_epoch = NULL,
                next_attempt_at_epoch = ?4, cursor_json = ?5, parser_revision = ?6
                WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?7",
                params![claim.key.environment_key, claim.key.agent, claim.key.session_id, now,
                    retry_cursor, PARSER_REVISION, claim.fence])?;
            tx.commit()?;
            return Ok(true);
        }
        let changed = claim.cursor_json.as_deref() != Some(cursor_json.as_str());
        let cursor_update = if changed { "cursor_json = ?4," } else { "" };
        tx.execute(
            &format!(
                "UPDATE native_file_delivery SET {cursor_update} observed_generation = ?5,
                pending = ?6, claim_fence = NULL, lease_expires_at_epoch = NULL,
                  retry_count = 0, last_error = ?15, parser_revision = ?14,
                service_ticket = CASE WHEN ?6 THEN service_ticket ELSE NULL END,
                active_until_epoch = CASE WHEN ?7 THEN ?8 ELSE active_until_epoch END,
                next_attempt_at_epoch = CASE WHEN ?6 THEN ?9
                  WHEN ?7 OR active_until_epoch >= ?9 OR EXISTS(SELECT 1 FROM session s
                    WHERE s.environment_key = ?1 AND s.agent = ?2 AND s.session_id = ?3
                      AND s.updated_at_epoch >= ?10) THEN ?11 ELSE ?12 END
              WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?13"
            ),
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                changed.then_some(cursor_json.as_str()),
                claim.generation,
                has_more || generation != claim.generation,
                changed,
                now + ACTIVE_SESSION_WINDOW_SECS,
                now,
                now - ACTIVE_SESSION_WINDOW_SECS,
                now + ACTIVE_POLL_SECS,
                now + DORMANT_POLL_SECS,
                claim.fence,
                PARSER_REVISION,
                unsafe_reason
            ],
        )?;
        tx.commit()?;
        Ok(origin_boundary || changed || claim.notify_on_success || generation != claim.generation)
    }

    pub(crate) fn fail_native_file(
        &self,
        claim: &NativeFileClaim,
        failure: NativeFileReadError,
        now: i64,
        retry_at: i64,
    ) -> Result<bool> {
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if current_generation(&tx, claim, now)?.is_none() {
            return Ok(false);
        }
        let (blocked, last_error) = match failure {
            NativeFileReadError::SourceChanged => (true, Some("file-source-changed")),
            NativeFileReadError::InvalidCursor => (true, Some("file-cursor-invalid")),
            NativeFileReadError::Unsupported => (true, Some("file-unsupported")),
            NativeFileReadError::Unavailable => (false, Some("file-unavailable")),
            NativeFileReadError::ReadFailed => (false, Some("file-read-failed")),
            NativeFileReadError::Cancelled => (false, None),
        };
        let cancelled = failure == NativeFileReadError::Cancelled;
        if !cancelled {
            super::super::observers::break_continuity_in(&tx, &claim.key)?;
        }
        let retry_count = claim.retry_count + i64::from(!cancelled);
        let retry = !blocked && (cancelled || retry_count < MAX_READ_ATTEMPTS);
        tx.execute(
            "UPDATE native_file_delivery SET pending = ?4, blocked = ?5,
                observed_generation = ?6, claim_fence = NULL, lease_expires_at_epoch = NULL,
                retry_count = ?7, next_attempt_at_epoch = ?8,
                service_ticket = CASE WHEN ?4 THEN service_ticket ELSE NULL END,
                 last_error = CASE WHEN ?11 OR last_error IN ('native-unsafe-identity', 'native-unsafe-order', 'native-unsafe-ownership') THEN last_error ELSE ?9 END
             WHERE environment_key = ?1 AND agent = ?2 AND session_id = ?3 AND claim_fence = ?10",
            params![
                claim.key.environment_key,
                claim.key.agent,
                claim.key.session_id,
                retry,
                blocked,
                claim.generation,
                retry_count,
                retry.then_some(if cancelled {
                    now + ACTIVE_POLL_SECS
                } else {
                    retry_at
                }),
                last_error,
                claim.fence,
                cancelled,
            ],
        )?;
        tx.commit()?;
        Ok(true)
    }
}

fn current_generation(
    tx: &Transaction<'_>,
    claim: &NativeFileClaim,
    now: i64,
) -> Result<Option<i64>> {
    Ok(tx.query_row(
        "SELECT s.source_generation FROM native_file_delivery d JOIN session s USING(environment_key, agent, session_id)
         WHERE d.environment_key = ?1 AND d.agent = ?2 AND d.session_id = ?3
           AND d.claim_fence = ?4 AND d.cursor_json IS ?5 AND d.lease_expires_at_epoch > ?6
           AND d.incarnation = ?7 AND s.incarnation = ?7 AND d.source_label = ?8
           AND s.source_label = ?8 AND s.source_kind = 'file' AND s.wsl_distro IS NULL
            AND d.cwd = ?9 AND s.cwd = ?9 AND d.parser_revision IN (?10, 62, 63, 64, 65, 66)
           AND NOT EXISTS(SELECT 1 FROM session_relation r
             WHERE r.environment_key = s.environment_key AND r.agent = s.agent
               AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                 OR (r.related_id = s.session_id AND r.kind = 'subagent')))",
        params![claim.key.environment_key, claim.key.agent, claim.key.session_id, claim.fence,
            claim.cursor_json, now, claim.incarnation, claim.source_label, claim.cwd, PARSER_REVISION],
        |row| row.get(0),
    ).optional()?)
}

#[cfg(test)]
#[path = "native_file_tests.rs"]
mod tests;

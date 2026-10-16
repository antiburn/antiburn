//! Native file receipts do not depend on metric projections.

use antiburn_local::analysis::file_live::NativeFileReadError;
use antiburn_local::analysis::{PARSER_REVISION, SourceFormat, ToolResultObservation};
use antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS;
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};

use super::{PublicationPolicy, ToolResultBatch, reconcile_with_policy};
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
           s.cwd, s.source_generation, s.updated_at_epoch, d.cursor_json,
           d.retry_count, d.observed_generation, d.pending, d.blocked,
            d.lease_expires_at_epoch, d.next_attempt_at_epoch, d.last_attempt_order, d.last_error,
           COALESCE(d.active_until_epoch, 0) AS active_until_epoch,
            (d.source_label <> s.source_label OR d.cwd IS NOT s.cwd) AS binding_changed,
            (d.incarnation IS NULL OR d.incarnation <> s.incarnation
              OR ((d.source_label <> s.source_label OR d.cwd IS NOT s.cwd) AND d.cursor_json IS NULL)
             OR d.parser_revision <> ?1) AS reset
      FROM session s LEFT JOIN native_file_delivery d USING(environment_key, agent, session_id)
     WHERE s.environment_key = 'native' AND s.wsl_distro IS NULL
       AND s.agent IN ('claude-code', 'codex', 'pi') AND s.source_kind = 'file'
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
        let claim = tx.query_row(
            &format!("{CANDIDATES}
             SELECT environment_key, agent, session_id, incarnation, source_label, cwd,
                    source_generation, CASE WHEN reset THEN NULL ELSE cursor_json END,
                     CASE WHEN reset THEN 0 ELSE COALESCE(retry_count, 0) END,
                      reset OR binding_changed OR pending = 1 OR observed_generation <> source_generation OR last_error IS NOT NULL,
                     reset
               FROM candidates
              WHERE ((reset AND updated_at_epoch >= ?3)
                 OR (NOT reset AND blocked = 0 AND (pending = 1
                     OR source_generation <> observed_generation
                     OR next_attempt_at_epoch <= ?2)))
                AND COALESCE(lease_expires_at_epoch, 0) <= ?2
                AND (reset OR source_generation <> observed_generation
                     OR COALESCE(next_attempt_at_epoch, 0) <= ?2)
              ORDER BY COALESCE(last_attempt_order, 0), updated_at_epoch DESC, agent, session_id LIMIT 1"),
            params![PARSER_REVISION, now, now - ACTIVE_SESSION_WINDOW_SECS],
            |row| Ok((NativeFileClaim {
                key: SessionKey::new(row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?),
                incarnation: row.get(3)?, source_label: row.get(4)?, cwd: row.get(5)?,
                generation: row.get(6)?, cursor_json: row.get(7)?, retry_count: row.get(8)?, fence: 0,
                notify_on_success: row.get(9)?,
            }, row.get::<_, bool>(10)?)),
        ).optional()?;
        let Some((mut claim, reset)) = claim else {
            return Ok(None);
        };
        claim.fence = tx.query_row(
            "UPDATE evidence_claim_fence_seq SET value = value + 1 WHERE id = 1 RETURNING value",
            [],
            |row| row.get(0),
        )?;
        let cursor_reset = if reset { "cursor_json = NULL," } else { "" };
        tx.execute(
            &format!("INSERT INTO native_file_delivery (environment_key, agent, session_id, incarnation,
                source_label, cwd, parser_revision, claim_fence,
                lease_expires_at_epoch, last_attempt_at_epoch, retry_count, active_until_epoch, last_attempt_order)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?10, ?8)
             ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                incarnation = excluded.incarnation, source_label = excluded.source_label,
                cwd = excluded.cwd, parser_revision = excluded.parser_revision,
                {cursor_reset} pending = 1, blocked = 0,
                claim_fence = excluded.claim_fence, lease_expires_at_epoch = excluded.lease_expires_at_epoch,
                last_attempt_at_epoch = excluded.last_attempt_at_epoch, retry_count = excluded.retry_count,
                last_attempt_order = excluded.last_attempt_order"),
            params![claim.key.environment_key, claim.key.agent, claim.key.session_id, claim.incarnation,
                claim.source_label, claim.cwd, PARSER_REVISION, claim.fence,
                now + lease_secs, now, claim.retry_count],
        )?;
        tx.commit()?;
        Ok(Some(claim))
    }

    /// Returns whether a committed page changes report or source progress.
    pub(crate) fn publish_native_file_page(
        &self,
        claim: &NativeFileClaim,
        page: &antiburn_local::analysis::file_live::NativeFilePage,
        results: &[ToolResultObservation],
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
            !has_more || claim.cursor_json.as_deref() != Some(cursor_json.as_str()),
            "file page does not advance"
        );
        let format = source_format(&claim.key.agent).expect("file claim has a supported agent");
        reconcile_with_policy(
            &tx,
            &claim.key,
            generation,
            claim.fence,
            &ToolResultBatch {
                source_format: format,
                results: results.to_vec(),
                complete: false,
            },
            PublicationPolicy {
                baseline: Some(page.baseline_offset),
                history_complete: Some(history_complete),
                gap_reason: (!history_complete).then_some("file-history-incomplete"),
            },
        )?;
        let changed = claim.cursor_json.as_deref() != Some(cursor_json.as_str());
        let cursor_update = if changed { "cursor_json = ?4," } else { "" };
        tx.execute(
            &format!(
                "UPDATE native_file_delivery SET {cursor_update} observed_generation = ?5,
                pending = ?6, claim_fence = NULL, lease_expires_at_epoch = NULL,
                retry_count = 0, last_error = NULL,
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
                claim.fence
            ],
        )?;
        tx.commit()?;
        Ok(changed || claim.notify_on_success || generation != claim.generation)
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
        let retry_count = claim.retry_count + i64::from(!cancelled);
        let retry = !blocked && (cancelled || retry_count < MAX_READ_ATTEMPTS);
        tx.execute(
            "UPDATE native_file_delivery SET pending = ?4, blocked = ?5,
                observed_generation = ?6, claim_fence = NULL, lease_expires_at_epoch = NULL,
                retry_count = ?7, next_attempt_at_epoch = ?8,
                last_error = CASE WHEN ?11 THEN last_error ELSE ?9 END
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
           AND d.cwd = ?9 AND s.cwd = ?9 AND d.parser_revision = ?10
           AND NOT EXISTS(SELECT 1 FROM session_relation r
             WHERE r.environment_key = s.environment_key AND r.agent = s.agent
               AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
                 OR (r.related_id = s.session_id AND r.kind = 'subagent')))",
        params![claim.key.environment_key, claim.key.agent, claim.key.session_id, claim.fence,
            claim.cursor_json, now, claim.incarnation, claim.source_label, claim.cwd, PARSER_REVISION],
        |row| row.get(0),
    ).optional()?)
}

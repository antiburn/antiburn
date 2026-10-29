//! Source-owned claims, leases, and publication receipts.

use crate::store::selected_source::{
    LIVE_ACTIVATION_SQL, OPENCODE_V1_SOURCE_SQL, ROOT_SESSION_SQL,
};
use anyhow::Result;
use rusqlite::{OptionalExtension, Transaction, params};

#[path = "native_file.rs"]
pub(crate) mod native_file;
#[path = "opencode_journal.rs"]
pub(crate) mod opencode_journal;

pub(crate) fn next_live_source_in(
    transaction: &Transaction<'_>,
    now: i64,
    fair_turn: bool,
) -> Result<Option<(String, String, String, String)>> {
    let order = if fair_turn {
        "COALESCE(service_ticket, (SELECT next_ticket FROM work_queue_schedule
            WHERE domain = 'native_delivery')), environment_key, agent, session_id"
    } else {
        "updated_at_epoch IS NULL, updated_at_epoch DESC, environment_key, agent, session_id"
    };
    Ok(transaction
        .query_row(
            &format!(
                "WITH file_rows AS (
                     SELECT 'file' AS family, s.environment_key, s.agent, s.session_id,
                            s.updated_at_epoch, d.service_ticket,
                            (d.incarnation IS NULL OR d.incarnation <> s.incarnation
                             OR ((d.source_label <> s.source_label OR d.cwd IS NOT s.cwd)
                                 AND d.cursor_json IS NULL)
                              ) AS reset,
                            d.pending, d.blocked, d.observed_generation,
                            d.next_attempt_at_epoch, d.lease_expires_at_epoch,
                            s.source_generation
                       FROM session s LEFT JOIN native_file_delivery d
                         USING(environment_key, agent, session_id)
                      WHERE s.environment_key = 'native' AND s.wsl_distro IS NULL
                         AND s.agent IN ('claude-code', 'codex', 'pi') AND s.source_kind = 'file'
                         AND (d.parser_revision IS NULL OR d.parser_revision IN (?1, 62, 63, 64, 65, 66))
                        AND s.cwd IS NOT NULL AND s.cwd <> ''
                        AND ({LIVE_ACTIVATION_SQL}) AND ({ROOT_SESSION_SQL})
                 ), journal_rows AS (
                     SELECT 'journal' AS family, s.environment_key, s.agent, s.session_id,
                            s.updated_at_epoch, d.service_ticket,
                            (d.incarnation IS NULL OR d.incarnation <> s.incarnation
                             OR d.source_label <> s.source_label OR d.cwd IS NOT s.cwd
                              OR d.main_session <> 1) AS reset,
                            d.pending, d.blocked, d.observed_generation,
                            d.next_attempt_at_epoch, d.lease_expires_at_epoch,
                            s.source_generation
                       FROM session s LEFT JOIN opencode_journal_delivery d
                         USING(environment_key, agent, session_id)
                        WHERE ({OPENCODE_V1_SOURCE_SQL}) AND s.wsl_distro IS NULL
                        AND (d.parser_revision IS NULL OR d.parser_revision IN (?1, 62, 63, 64, 65, 66))
                        AND s.cwd IS NOT NULL AND s.cwd <> ''
                        AND ({ROOT_SESSION_SQL}) AND ({LIVE_ACTIVATION_SQL})
                 ), eligible AS (
                     SELECT family, environment_key, agent, session_id,
                            updated_at_epoch, service_ticket FROM file_rows
                      WHERE ((reset AND updated_at_epoch >= ?3)
                         OR (NOT reset AND blocked = 0 AND (pending = 1
                             OR source_generation <> observed_generation
                             OR next_attempt_at_epoch <= ?2)))
                        AND COALESCE(lease_expires_at_epoch, 0) <= ?2
                        AND (reset OR source_generation <> observed_generation
                             OR COALESCE(next_attempt_at_epoch, 0) <= ?2)
                     UNION ALL
                     SELECT family, environment_key, agent, session_id,
                            updated_at_epoch, service_ticket FROM journal_rows
                      WHERE ((reset AND updated_at_epoch >= ?3)
                          OR (NOT reset AND blocked = 0 AND (pending = 1
                              OR source_generation <> observed_generation
                              OR next_attempt_at_epoch IS NOT NULL)))
                        AND (reset OR MAX(COALESCE(lease_expires_at_epoch, ?2),
                            CASE WHEN observed_generation <> source_generation THEN ?2
                                 ELSE COALESCE(next_attempt_at_epoch, ?2) END) <= ?2)
                 )
                 SELECT family, environment_key, agent, session_id FROM eligible
                  ORDER BY {order} LIMIT 1"
            ),
            params![
                antiburn_local::analysis::PARSER_REVISION,
                now,
                now - antiburn_local::discovery::ACTIVE_SESSION_WINDOW_SECS
            ],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?)
}
#[path = "opencode_v2.rs"]
pub(crate) mod opencode_v2;

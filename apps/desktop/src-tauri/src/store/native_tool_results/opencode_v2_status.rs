//! Closed source diagnostics for native OpenCode V2 capture.

use anyhow::{Result, bail, ensure};
use rusqlite::{TransactionBehavior, params};

use crate::store::{SessionRecord, Store};

const MAX_STATUS_RECORDS: usize = 1024;

pub(crate) const SOURCE_SQL: &str = "s.environment_key = 'native' AND s.agent = 'opencode'
    AND s.source_kind = 'providerDb' AND s.wsl_distro IS NULL
    AND s.source_label = 'opencode:' || s.session_id || ':open_code_v2_sqlite'
    AND s.cwd IS NOT NULL AND s.cwd <> ''
    AND NOT EXISTS(SELECT 1 FROM session_relation r
      WHERE r.environment_key = s.environment_key AND r.agent = s.agent
        AND ((r.session_id = s.session_id AND r.kind = 'forkParent')
          OR (r.related_id = s.session_id AND r.kind = 'subagent')))";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenCodeV2SourceStatus {
    ServiceUnavailable,
    AuthenticationRejected,
    SourceMismatch,
    VersionUnsupported,
    CaptureGap,
    SourceLimit,
    Live,
}

impl OpenCodeV2SourceStatus {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::ServiceUnavailable => "service-unavailable",
            Self::AuthenticationRejected => "authentication-rejected",
            Self::SourceMismatch => "source-mismatch",
            Self::VersionUnsupported => "version-unsupported",
            Self::CaptureGap => "capture-gap",
            Self::SourceLimit => "source-limit",
            Self::Live => "live",
        }
    }

    pub(crate) fn from_code(code: &str) -> Result<Self> {
        Ok(match code {
            "service-unavailable" => Self::ServiceUnavailable,
            "authentication-rejected" => Self::AuthenticationRejected,
            "source-mismatch" => Self::SourceMismatch,
            "version-unsupported" => Self::VersionUnsupported,
            "capture-gap" => Self::CaptureGap,
            "source-limit" => Self::SourceLimit,
            "live" => Self::Live,
            _ => bail!("invalid OpenCode V2 source status code"),
        })
    }
}

pub(crate) fn live_capture_sql(now: &str) -> String {
    // Content appends can advance source_generation without replacing this source.
    format!(
        "({SOURCE_SQL}) AND EXISTS(SELECT 1 FROM opencode_v2_capture d
          WHERE d.environment_key = s.environment_key AND d.agent = s.agent
            AND d.session_id = s.session_id AND d.incarnation = s.incarnation
            AND d.source_label = s.source_label AND d.cwd = s.cwd
            AND d.lease_expires_at_epoch > ({now}))"
    )
}

pub(crate) fn source_status_sql(now: &str) -> String {
    let live = live_capture_sql(now);
    format!(
        "CASE WHEN ({SOURCE_SQL}) THEN CASE WHEN ({live}) THEN
            CASE WHEN EXISTS(SELECT 1 FROM opencode_v2_capture d
              WHERE d.environment_key = s.environment_key AND d.agent = s.agent
                AND d.session_id = s.session_id AND d.history_gap = 1)
              THEN 'capture-gap' ELSE 'live' END
          ELSE COALESCE((SELECT NULLIF(d.code, 'live') FROM opencode_v2_source_status d
            WHERE d.environment_key = s.environment_key AND d.agent = s.agent
              AND d.session_id = s.session_id AND d.incarnation = s.incarnation
              AND d.source_label = s.source_label AND d.cwd = s.cwd),
            CASE WHEN EXISTS(SELECT 1 FROM opencode_v2_capture d
              WHERE d.environment_key = s.environment_key AND d.agent = s.agent
                AND d.session_id = s.session_id AND d.incarnation = s.incarnation
                AND d.source_label = s.source_label AND d.cwd = s.cwd)
              THEN 'capture-gap' ELSE 'service-unavailable' END) END END"
    )
}

impl Store {
    /// Saves a closed status code for the current native V2 source and CWD.
    /// A live capture lease takes priority when the report reads this status.
    pub(crate) fn set_opencode_v2_source_status(
        &self,
        records: &[SessionRecord],
        status: OpenCodeV2SourceStatus,
        now: i64,
    ) -> Result<usize> {
        ensure!(
            records.len() <= MAX_STATUS_RECORDS,
            "too many V2 source statuses"
        );
        ensure!(now >= 0, "V2 source status time must be non-negative");
        ensure!(
            records.iter().all(|record| {
                record.key.environment_key == "native"
                    && record.key.agent == "opencode"
                    && record.source_kind == "providerDb"
                    && record.source_label
                        == format!("opencode:{}:open_code_v2_sqlite", record.key.session_id)
                    && record.wsl_distro.is_none()
                    && record
                        .cwd
                        .as_ref()
                        .is_some_and(|cwd| !cwd.is_empty() && cwd.len() <= 4096)
            }),
            "invalid V2 source status record"
        );
        let mut connection = self.lock();
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut changed = 0;
        for record in records {
            changed += tx.execute(
                &format!(
                    "INSERT INTO opencode_v2_source_status
                      (environment_key, agent, session_id, incarnation, source_generation,
                       source_label, cwd, code, observed_at_epoch)
                     SELECT s.environment_key, s.agent, s.session_id, s.incarnation,
                        s.source_generation, s.source_label, s.cwd, ?6, ?7 FROM session s
                     WHERE ({SOURCE_SQL}) AND s.environment_key = ?1 AND s.agent = ?2
                        AND s.session_id = ?3 AND s.source_label = ?4 AND s.cwd IS ?5
                     ON CONFLICT(environment_key, agent, session_id) DO UPDATE SET
                       incarnation = excluded.incarnation, source_generation = excluded.source_generation,
                       source_label = excluded.source_label, cwd = excluded.cwd,
                       code = excluded.code, observed_at_epoch = excluded.observed_at_epoch
                     WHERE opencode_v2_source_status.incarnation <> excluded.incarnation
                       OR opencode_v2_source_status.source_label <> excluded.source_label
                       OR opencode_v2_source_status.cwd <> excluded.cwd
                       OR opencode_v2_source_status.observed_at_epoch <= excluded.observed_at_epoch"
                ),
                params![
                    record.key.environment_key,
                    record.key.agent,
                    record.key.session_id,
                    record.source_label,
                    record.cwd,
                    status.code(),
                    now,
                ],
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }
}

#[cfg(test)]
#[path = "opencode_v2_status_tests.rs"]
mod tests;

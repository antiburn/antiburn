//! Closed source diagnostics for native OpenCode V2 capture.

use anyhow::{Result, bail, ensure};
use rusqlite::{Connection, TransactionBehavior, params};

use crate::store::selected_source::native_opencode_sqlite_format;
use crate::store::{SessionKey, SessionRecord, Store};
use antiburn_local::analysis::SourceFormat;

const MAX_STATUS_RECORDS: usize = 1024;

pub(crate) use crate::store::selected_source::OPENCODE_V2_SOURCE_SQL as SOURCE_SQL;

pub(crate) const CURRENT_SOURCE_MATCH_SQL: &str = "d.environment_key = s.environment_key
    AND d.agent = s.agent AND d.session_id = s.session_id
    AND d.incarnation = s.incarnation AND d.source_label = s.source_label AND d.cwd = s.cwd";

pub(crate) fn capture_match_sql() -> String {
    format!("{CURRENT_SOURCE_MATCH_SQL} AND d.capture_epoch <> '' AND d.claim_fence > 0")
}

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
    format!("({SOURCE_SQL}) AND ({})", live_lease_sql(now))
}

fn live_lease_sql(now: &str) -> String {
    let capture_match = capture_match_sql();
    format!(
        "EXISTS(SELECT 1 FROM opencode_v2_capture d
        WHERE {capture_match} AND d.lease_expires_at_epoch > ({now}))"
    )
}

pub(crate) fn source_status_sql(now: &str) -> String {
    let live = live_lease_sql(now);
    let capture_match = capture_match_sql();
    // History gaps limit evidence, not the current capture lease.
    format!(
        "CASE WHEN ({SOURCE_SQL}) THEN CASE WHEN ({live}) THEN 'live'
          ELSE COALESCE((SELECT NULLIF(d.code, 'live') FROM opencode_v2_source_status d
            WHERE {CURRENT_SOURCE_MATCH_SQL}),
            CASE WHEN EXISTS(SELECT 1 FROM opencode_v2_capture d
              WHERE {capture_match})
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
                native_opencode_sqlite_format(
                    &record.key,
                    &record.source_kind,
                    &record.source_label,
                ) == Some(SourceFormat::OpenCodeSqliteV2)
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
            changed += upsert_source_status_in(
                &tx,
                &record.key,
                None,
                &record.source_label,
                record.cwd.as_deref(),
                status,
                now,
            )?;
        }
        tx.commit()?;
        Ok(changed)
    }
}

pub(crate) fn upsert_source_status_in(
    connection: &Connection,
    key: &SessionKey,
    incarnation: Option<crate::store::Incarnation>,
    source_label: &str,
    cwd: Option<&str>,
    status: OpenCodeV2SourceStatus,
    now: i64,
) -> Result<usize> {
    Ok(connection.execute(
        &format!(
            "INSERT INTO opencode_v2_source_status
          (environment_key, agent, session_id, incarnation, source_generation,
           source_label, cwd, code, observed_at_epoch)
         SELECT s.environment_key, s.agent, s.session_id, s.incarnation,
            s.source_generation, s.source_label, s.cwd, ?6, ?7 FROM session s
         WHERE ({SOURCE_SQL}) AND s.environment_key = ?1 AND s.agent = ?2
            AND s.session_id = ?3 AND s.source_label = ?4 AND s.cwd IS ?5
            AND (?8 IS NULL OR s.incarnation = ?8)
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
            key.environment_key,
            key.agent,
            key.session_id,
            source_label,
            cwd,
            status.code(),
            now,
            incarnation.map(|value| value.0)
        ],
    )?)
}

#[cfg(test)]
#[path = "opencode_v2_status_tests.rs"]
mod tests;

//! Durable provider usage readings and their stated allowance periods.
//!
//! This data is separate from the small forecast cache. It keeps the provider
//! facts the limit factor learner needs without guessing a reset boundary
//! when the provider did not state one.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::provider_usage::live::model::{
    Confidence, Freshness, ProviderUsageSnapshot, UsageScope, UsageWindow, UsageWindowKind,
    WindowRole,
};

use super::Store;

const RESET_JITTER_SECS: i64 = 5;
const RETENTION_DAYS: i64 = 90;

/// A provider-stated allowance period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderUsagePeriod {
    pub id: i64,
    pub provider: String,
    pub account_key: String,
    pub window_id: String,
    pub window_kind: String,
    pub window_role: String,
    pub scope_key: String,
    pub scope_label: String,
    pub duration_seconds: Option<i64>,
    pub starts_at_epoch: Option<i64>,
    pub resets_at_epoch: Option<i64>,
    pub first_observed_epoch: i64,
    pub last_observed_epoch: i64,
}

/// One provider-reported usage reading.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsageObservation {
    pub id: i64,
    pub period_id: Option<i64>,
    pub provider: String,
    pub account_key: String,
    pub window_id: String,
    pub window_kind: String,
    pub window_role: String,
    pub scope_key: String,
    pub scope_label: String,
    pub observed_at_epoch: i64,
    pub used_percent: Option<f64>,
    pub is_fresh: bool,
    pub is_authoritative: bool,
    pub confidence: String,
    pub source_id: String,
    pub reported_starts_at_epoch: Option<i64>,
    pub reported_resets_at_epoch: Option<i64>,
    pub plan: Option<String>,
    pub plan_tier: Option<String>,
}

/// A complete period and its ordered readings.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsagePeriodHistory {
    pub period: ProviderUsagePeriod,
    pub observations: Vec<ProviderUsageObservation>,
}

/// One bounded page of period metadata changed after a cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderUsagePeriodPage {
    pub periods: Vec<ProviderUsagePeriod>,
    pub next_after_id: Option<i64>,
}

#[derive(Debug, Clone)]
struct Reading<'a> {
    provider: &'a str,
    account_key: &'a str,
    window_id: &'a str,
    window_kind: String,
    window_role: String,
    scope_key: String,
    scope_label: String,
    duration_seconds: Option<i64>,
    observed_at_epoch: i64,
    used_percent: Option<f64>,
    is_fresh: bool,
    is_authoritative: bool,
    confidence: String,
    source_id: &'a str,
    starts_at_epoch: Option<i64>,
    resets_at_epoch: Option<i64>,
    plan: Option<&'a str>,
    plan_tier: Option<&'a str>,
}

#[derive(Debug, Clone, Copy)]
struct PeriodAssignment {
    id: Option<i64>,
    detaches_existing: bool,
}

impl Store {
    /// Persist opaque-account readings and return periods whose inputs changed.
    ///
    /// A snapshot without an opaque account key stays transient. Durable data
    /// cannot safely join a provider's ambiguous account reading over time.
    pub fn record_provider_usage_snapshots(
        &self,
        snapshots: &[ProviderUsageSnapshot],
    ) -> Result<Vec<i64>> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let mut changed_periods = Vec::new();

        for snapshot in snapshots {
            let Some(account_key) = snapshot.account.as_deref().filter(is_opaque_account_key)
            else {
                continue;
            };
            for window in &snapshot.windows {
                let reading = Reading::from_snapshot(snapshot, account_key, window);
                let existing = existing_observation(&tx, &reading)?;
                if !should_replace(existing.as_ref(), &reading) {
                    continue;
                }
                let reading = merge_boundary_evidence(reading, existing.as_ref());
                let assignment = period_for(&tx, &reading)?;
                let old_period_id = existing.as_ref().and_then(|row| row.period_id);
                let period_id = assignment.id.or(old_period_id);
                write_observation(&tx, &reading, assignment.id, assignment.detaches_existing)?;
                if let Some(period_id) = period_id {
                    changed_periods.push(period_id);
                }
                if let Some(old_period_id) = old_period_id
                    && Some(old_period_id) != assignment.id
                {
                    changed_periods.push(old_period_id);
                }
            }
        }

        changed_periods.sort_unstable();
        changed_periods.dedup();
        tx.commit()?;
        Ok(changed_periods)
    }

    /// Load one complete period without reconstructing a current live snapshot.
    pub fn provider_usage_period_history(
        &self,
        period_id: i64,
    ) -> Result<Option<ProviderUsagePeriodHistory>> {
        let connection = self.lock();
        let period = query_period(&connection, period_id)?;
        let Some(period) = period else {
            return Ok(None);
        };
        let observations = query_observations(&connection, period_id)?;
        Ok(Some(ProviderUsagePeriodHistory {
            period,
            observations,
        }))
    }

    /// List period metadata with an observation at or after a cursor.
    ///
    /// This is an observation-time cursor, not a mutation log. A correction to
    /// an old observation does not appear here unless its period also has a
    /// newer observation.
    pub fn provider_usage_periods_changed_since(
        &self,
        since_epoch: i64,
        after_id: Option<i64>,
        limit: usize,
    ) -> Result<ProviderUsagePeriodPage> {
        let connection = self.lock();
        let limit = i64::try_from(limit.clamp(1, 500)).expect("bounded page fits i64");
        let mut statement = connection.prepare(
            "SELECT id, provider, account_key, window_id, window_kind, window_role,
                    scope_key, scope_label, duration_seconds, starts_at_epoch,
                    resets_at_epoch, first_observed_epoch, last_observed_epoch
               FROM provider_usage_period
              WHERE last_observed_epoch >= ?1
                AND id > ?2
              ORDER BY id
              LIMIT ?3",
        )?;
        let periods = statement
            .query_map(
                params![since_epoch, after_id.unwrap_or(0), limit],
                row_to_period,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_after_id = periods.last().map(|period| period.id);
        Ok(ProviderUsagePeriodPage {
            periods,
            next_after_id,
        })
    }

    /// Remove expired readings and orphaned periods.
    pub(crate) fn apply_provider_usage_retention_in(
        connection: &Connection,
        retention_days: i32,
        now_epoch: i64,
    ) -> Result<usize> {
        let cutoff = bounded_retention_cutoff(retention_days, now_epoch);
        let removed = connection.execute(
            "DELETE FROM provider_usage_observation WHERE observed_at_epoch < ?1",
            [cutoff],
        )?;
        // A sample references its period only for provenance. Null the
        // reference before the period disappears, so the deletion below stays
        // exactly the query it was before samples existed.
        super::provider_limit::detach_samples_pending_period_deletion_in(connection)?;
        connection.execute(
            "DELETE FROM provider_usage_period
              WHERE NOT EXISTS (
                    SELECT 1 FROM provider_usage_observation
                     WHERE provider_usage_observation.period_id = provider_usage_period.id
              )",
            [],
        )?;
        super::provider_limit::apply_sample_retention_in(connection, retention_days, now_epoch)?;
        // A completed rollout file's checkpoint outlives the observations it
        // produced only until they themselves expire. Once they are gone, a
        // later append to the same file is cheap to re-read from byte zero,
        // so nothing is lost by dropping the checkpoint too.
        connection.execute(
            "DELETE FROM provider_usage_rollout_checkpoint
              WHERE status = 'complete' AND completed_at_epoch < ?1",
            [cutoff],
        )?;
        Ok(removed)
    }

    /// The oldest observation the durable retention keeps: the session-data
    /// retention setting, capped at 90 days.
    ///
    /// Shared with [`crate::provider_usage::codex_rollout_history`], so a
    /// rollout reading older than what retention would keep is never
    /// imported only to be deleted on the next pass.
    pub(crate) fn provider_usage_retention_cutoff_epoch(&self, now_epoch: i64) -> Result<i64> {
        let retention_days = self.settings()?.session_data_retention_days;
        Ok(bounded_retention_cutoff(retention_days, now_epoch))
    }
}

/// The retention setting, capped at [`RETENTION_DAYS`], turned into a cutoff
/// epoch: an observation strictly before it is out of scope for retention.
fn bounded_retention_cutoff(retention_days: i32, now_epoch: i64) -> i64 {
    let bounded_days = match retention_days {
        days if days > 0 => i64::from(days).min(RETENTION_DAYS),
        _ => RETENTION_DAYS,
    };
    now_epoch.saturating_sub(bounded_days.saturating_mul(86_400))
}

impl<'a> Reading<'a> {
    fn from_snapshot(
        snapshot: &'a ProviderUsageSnapshot,
        account_key: &'a str,
        window: &'a UsageWindow,
    ) -> Reading<'a> {
        let starts_at_epoch = window.starts_at.map(|value| value.unix_timestamp());
        let resets_at_epoch = window.resets_at.map(|value| value.unix_timestamp());
        Reading {
            provider: snapshot.provider,
            account_key,
            window_id: &window.id,
            window_kind: window_kind(&window.kind),
            window_role: window_role(&window.role),
            scope_key: scope_key(&window.scope),
            scope_label: scope_label(&window.scope),
            duration_seconds: match (starts_at_epoch, resets_at_epoch) {
                (Some(start), Some(reset)) if reset > start => Some(reset - start),
                _ => None,
            },
            observed_at_epoch: snapshot.observed_at.unix_timestamp(),
            used_percent: window.used_percent,
            is_fresh: snapshot.source.freshness == Freshness::Fresh,
            is_authoritative: window.authoritative,
            confidence: confidence(snapshot.source.confidence).to_string(),
            source_id: snapshot.source.id,
            starts_at_epoch,
            resets_at_epoch,
            plan: snapshot.plan.as_deref(),
            plan_tier: snapshot.plan_tier.as_deref(),
        }
    }
}

fn period_for(connection: &Transaction<'_>, reading: &Reading<'_>) -> Result<PeriodAssignment> {
    if !has_valid_period_boundary(reading) {
        return Ok(PeriodAssignment {
            id: None,
            detaches_existing: has_invalid_period_boundary(reading),
        });
    }

    let (boundary_sql, boundary_params): (&str, Vec<i64>) =
        match (reading.resets_at_epoch, reading.starts_at_epoch) {
            (Some(reset), Some(start)) => (
                "AND (resets_at_epoch BETWEEN ?7 AND ?8 OR starts_at_epoch BETWEEN ?9 AND ?10)",
                vec![
                    reset - RESET_JITTER_SECS,
                    reset + RESET_JITTER_SECS,
                    start - RESET_JITTER_SECS,
                    start + RESET_JITTER_SECS,
                ],
            ),
            (Some(reset), None) => (
                "AND resets_at_epoch BETWEEN ?7 AND ?8",
                vec![reset - RESET_JITTER_SECS, reset + RESET_JITTER_SECS],
            ),
            (None, Some(start)) => (
                "AND starts_at_epoch BETWEEN ?7 AND ?8",
                vec![start - RESET_JITTER_SECS, start + RESET_JITTER_SECS],
            ),
            (None, None) => unreachable!("valid period boundary is present"),
        };
    let sql = format!(
        "SELECT id, provider, account_key, window_id, window_kind, window_role,
                scope_key, scope_label, duration_seconds, starts_at_epoch,
                resets_at_epoch, first_observed_epoch, last_observed_epoch
           FROM provider_usage_period
          WHERE provider = ?1 AND account_key = ?2 AND window_id = ?3
            AND window_kind = ?4 AND window_role = ?5 AND scope_key = ?6
            {boundary_sql}
          ORDER BY last_observed_epoch DESC
          LIMIT 16"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut values = vec![
        rusqlite::types::Value::from(reading.provider.to_string()),
        rusqlite::types::Value::from(reading.account_key.to_string()),
        rusqlite::types::Value::from(reading.window_id.to_string()),
        rusqlite::types::Value::from(reading.window_kind.clone()),
        rusqlite::types::Value::from(reading.window_role.clone()),
        rusqlite::types::Value::from(reading.scope_key.clone()),
    ];
    values.extend(
        boundary_params
            .into_iter()
            .map(rusqlite::types::Value::from),
    );
    let candidates = statement
        .query_map(rusqlite::params_from_iter(values), row_to_period)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);

    if let Some(period) = candidates.iter().find(|period| compatible(period, reading)) {
        connection.execute(
            "UPDATE provider_usage_period
                SET duration_seconds = COALESCE(duration_seconds, ?1),
                    starts_at_epoch = COALESCE(starts_at_epoch, ?2),
                    resets_at_epoch = COALESCE(resets_at_epoch, ?3)
              WHERE id = ?4",
            params![
                reading.duration_seconds,
                reading.starts_at_epoch,
                reading.resets_at_epoch,
                period.id,
            ],
        )?;
        return Ok(PeriodAssignment {
            id: Some(period.id),
            detaches_existing: false,
        });
    }

    connection.execute(
        "INSERT INTO provider_usage_period (
                provider, account_key, window_id, window_kind, window_role,
                scope_key, scope_label, duration_seconds, starts_at_epoch,
                resets_at_epoch, first_observed_epoch, last_observed_epoch
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            reading.provider,
            reading.account_key,
            reading.window_id,
            reading.window_kind,
            reading.window_role,
            reading.scope_key,
            reading.scope_label,
            reading.duration_seconds,
            reading.starts_at_epoch,
            reading.resets_at_epoch,
            reading.observed_at_epoch,
        ],
    )?;
    Ok(PeriodAssignment {
        id: Some(connection.last_insert_rowid()),
        detaches_existing: false,
    })
}

fn compatible(period: &ProviderUsagePeriod, reading: &Reading<'_>) -> bool {
    let reset_matches = matches!(
        (period.resets_at_epoch, reading.resets_at_epoch),
        (Some(previous), Some(incoming)) if (previous - incoming).abs() <= RESET_JITTER_SECS
    );
    let start_matches = matches!(
        (period.starts_at_epoch, reading.starts_at_epoch),
        (Some(previous), Some(incoming)) if (previous - incoming).abs() <= RESET_JITTER_SECS
    );
    (reset_matches || start_matches)
        && compatible_boundary(period.resets_at_epoch, reading.resets_at_epoch)
        && compatible_boundary(period.starts_at_epoch, reading.starts_at_epoch)
}

fn compatible_boundary(previous: Option<i64>, incoming: Option<i64>) -> bool {
    match (previous, incoming) {
        (Some(previous), Some(incoming)) => (previous - incoming).abs() <= RESET_JITTER_SECS,
        _ => true,
    }
}

fn has_valid_period_boundary(reading: &Reading<'_>) -> bool {
    (reading.starts_at_epoch.is_some() || reading.resets_at_epoch.is_some())
        && !has_invalid_period_boundary(reading)
}

fn has_invalid_period_boundary(reading: &Reading<'_>) -> bool {
    matches!(
        (reading.starts_at_epoch, reading.resets_at_epoch),
        (Some(start), Some(reset)) if start >= reset
    )
}

fn existing_observation(
    connection: &Transaction<'_>,
    reading: &Reading<'_>,
) -> Result<Option<ProviderUsageObservation>> {
    connection
        .query_row(
            "SELECT id, period_id, provider, account_key, window_id, window_kind, window_role,
                    scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                    is_authoritative, confidence, source_id, reported_starts_at_epoch,
                    reported_resets_at_epoch, plan, plan_tier
               FROM provider_usage_observation
              WHERE provider = ?1 AND account_key = ?2 AND window_id = ?3
                AND window_kind = ?4 AND window_role = ?5 AND scope_key = ?6
                AND observed_at_epoch = ?7",
            params![
                reading.provider,
                reading.account_key,
                reading.window_id,
                reading.window_kind,
                reading.window_role,
                reading.scope_key,
                reading.observed_at_epoch,
            ],
            row_to_observation,
        )
        .optional()
        .map_err(Into::into)
}

fn should_replace(existing: Option<&ProviderUsageObservation>, reading: &Reading<'_>) -> bool {
    let Some(existing) = existing else {
        return true;
    };
    quality(reading) > quality_of(existing)
        || quality(reading) == quality_of(existing) && is_more_complete(reading, existing)
}

fn is_more_complete(reading: &Reading<'_>, existing: &ProviderUsageObservation) -> bool {
    existing.used_percent.is_none() && reading.used_percent.is_some()
        || existing.reported_starts_at_epoch.is_none() && reading.starts_at_epoch.is_some()
        || existing.reported_resets_at_epoch.is_none() && reading.resets_at_epoch.is_some()
        || existing.reported_starts_at_epoch.is_some()
            && reading.starts_at_epoch.is_some()
            && existing.reported_starts_at_epoch != reading.starts_at_epoch
        || existing.reported_resets_at_epoch.is_some()
            && reading.resets_at_epoch.is_some()
            && existing.reported_resets_at_epoch != reading.resets_at_epoch
}

fn merge_boundary_evidence<'a>(
    mut reading: Reading<'a>,
    existing: Option<&ProviderUsageObservation>,
) -> Reading<'a> {
    let Some(existing) = existing else {
        return reading;
    };
    reading.starts_at_epoch = reading
        .starts_at_epoch
        .or(existing.reported_starts_at_epoch);
    reading.resets_at_epoch = reading
        .resets_at_epoch
        .or(existing.reported_resets_at_epoch);
    reading.duration_seconds = match (reading.starts_at_epoch, reading.resets_at_epoch) {
        (Some(start), Some(reset)) if reset > start => Some(reset - start),
        _ => None,
    };
    reading
}

fn write_observation(
    connection: &Transaction<'_>,
    reading: &Reading<'_>,
    period_id: Option<i64>,
    detaches_existing: bool,
) -> Result<()> {
    connection.execute(
        "INSERT INTO provider_usage_observation (
                period_id, provider, account_key, window_id, window_kind, window_role,
                scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                is_authoritative, confidence, source_id, reported_starts_at_epoch,
                reported_resets_at_epoch, plan, plan_tier
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?18, ?19)
            ON CONFLICT (
                provider, account_key, window_id, window_kind, window_role, scope_key,
                observed_at_epoch
            ) DO UPDATE SET
                period_id = CASE WHEN ?17 THEN excluded.period_id
                    ELSE COALESCE(excluded.period_id, provider_usage_observation.period_id)
                END,
                scope_label = excluded.scope_label,
                used_percent = COALESCE(excluded.used_percent, provider_usage_observation.used_percent),
                is_fresh = excluded.is_fresh,
                is_authoritative = excluded.is_authoritative,
                confidence = excluded.confidence,
                source_id = excluded.source_id,
                reported_starts_at_epoch = CASE
                    WHEN excluded.reported_starts_at_epoch IS NULL
                        THEN provider_usage_observation.reported_starts_at_epoch
                    ELSE excluded.reported_starts_at_epoch
                END,
                reported_resets_at_epoch = CASE
                    WHEN excluded.reported_resets_at_epoch IS NULL
                        THEN provider_usage_observation.reported_resets_at_epoch
                    ELSE excluded.reported_resets_at_epoch
                END,
                plan = excluded.plan,
                plan_tier = excluded.plan_tier",
        params![
            period_id,
            reading.provider,
            reading.account_key,
            reading.window_id,
            reading.window_kind,
            reading.window_role,
            reading.scope_key,
            reading.scope_label,
            reading.observed_at_epoch,
            reading.used_percent,
            i64::from(reading.is_fresh),
            i64::from(reading.is_authoritative),
            reading.confidence,
            reading.source_id,
            reading.starts_at_epoch,
            reading.resets_at_epoch,
            i64::from(detaches_existing),
            reading.plan,
            reading.plan_tier,
        ],
    )?;
    if let Some(period_id) = period_id {
        connection.execute(
            "UPDATE provider_usage_period
                SET first_observed_epoch = MIN(first_observed_epoch, ?1),
                    last_observed_epoch = MAX(last_observed_epoch, ?1)
              WHERE id = ?2",
            params![reading.observed_at_epoch, period_id],
        )?;
    }
    Ok(())
}

fn quality(reading: &Reading<'_>) -> u8 {
    u8::from(reading.is_authoritative) * 4
        + u8::from(reading.is_fresh) * 2
        + u8::from(reading.confidence == "high")
}

fn quality_of(observation: &ProviderUsageObservation) -> u8 {
    u8::from(observation.is_authoritative) * 4
        + u8::from(observation.is_fresh) * 2
        + u8::from(observation.confidence == "high")
}

fn query_period(connection: &Connection, period_id: i64) -> Result<Option<ProviderUsagePeriod>> {
    Ok(connection
        .query_row(
            "SELECT id, provider, account_key, window_id, window_kind, window_role,
                    scope_key, scope_label, duration_seconds, starts_at_epoch,
                    resets_at_epoch, first_observed_epoch, last_observed_epoch
               FROM provider_usage_period WHERE id = ?1",
            [period_id],
            row_to_period,
        )
        .optional()?)
}

fn query_observations(
    connection: &Connection,
    period_id: i64,
) -> Result<Vec<ProviderUsageObservation>> {
    let mut statement = connection.prepare(
        "SELECT id, period_id, provider, account_key, window_id, window_kind, window_role,
                scope_key, scope_label, observed_at_epoch, used_percent, is_fresh,
                is_authoritative, confidence, source_id, reported_starts_at_epoch,
                reported_resets_at_epoch, plan, plan_tier
           FROM provider_usage_observation
          WHERE period_id = ?1
          ORDER BY observed_at_epoch, id",
    )?;
    Ok(statement
        .query_map([period_id], row_to_observation)?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn row_to_period(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderUsagePeriod> {
    Ok(ProviderUsagePeriod {
        id: row.get(0)?,
        provider: row.get(1)?,
        account_key: row.get(2)?,
        window_id: row.get(3)?,
        window_kind: row.get(4)?,
        window_role: row.get(5)?,
        scope_key: row.get(6)?,
        scope_label: row.get(7)?,
        duration_seconds: row.get(8)?,
        starts_at_epoch: row.get(9)?,
        resets_at_epoch: row.get(10)?,
        first_observed_epoch: row.get(11)?,
        last_observed_epoch: row.get(12)?,
    })
}

fn row_to_observation(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderUsageObservation> {
    Ok(ProviderUsageObservation {
        id: row.get(0)?,
        period_id: row.get(1)?,
        provider: row.get(2)?,
        account_key: row.get(3)?,
        window_id: row.get(4)?,
        window_kind: row.get(5)?,
        window_role: row.get(6)?,
        scope_key: row.get(7)?,
        scope_label: row.get(8)?,
        observed_at_epoch: row.get(9)?,
        used_percent: row.get(10)?,
        is_fresh: row.get::<_, i64>(11)? != 0,
        is_authoritative: row.get::<_, i64>(12)? != 0,
        confidence: row.get(13)?,
        source_id: row.get(14)?,
        reported_starts_at_epoch: row.get(15)?,
        reported_resets_at_epoch: row.get(16)?,
        plan: row.get(17)?,
        plan_tier: row.get(18)?,
    })
}

fn confidence(value: Confidence) -> &'static str {
    match value {
        Confidence::Medium => "medium",
        Confidence::High => "high",
    }
}

fn window_kind(value: &UsageWindowKind) -> String {
    match value {
        UsageWindowKind::Rolling => "rolling".into(),
        UsageWindowKind::Weekly => "weekly".into(),
        UsageWindowKind::Other(value) => format!("other:{value}"),
    }
}

fn window_role(value: &WindowRole) -> String {
    match value {
        WindowRole::PrimaryShort => "primaryShort".into(),
        WindowRole::PrimaryLong => "primaryLong".into(),
        WindowRole::Supplemental => "supplemental".into(),
        WindowRole::Other(value) => format!("other:{value}"),
    }
}

fn scope_key(value: &UsageScope) -> String {
    match value {
        UsageScope::Account => "account".into(),
        UsageScope::Model(value) => format!("model:{value}"),
    }
}

fn scope_label(value: &UsageScope) -> String {
    match value {
        UsageScope::Account => "account".into(),
        UsageScope::Model(value) => value.clone(),
    }
}

fn is_opaque_account_key(value: &&str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests;

//! Storage for the learned dollars-per-percent limit factor.
//!
//! [`crate::provider_usage::factor`] is this module's only caller. It reads
//! priced, account-attributed turn dollars and account bindings from here,
//! and writes factor samples, factor points, and residual rows here.
//!
//! # Account resolution
//!
//! One rule serves every query in this module. A session belongs to an
//! account when it has exactly one [`session_provider_account`] row for the
//! provider, or, absent that, when [`provider_account_seen`] holds exactly
//! one account for the session's agent and provider. This is the rule from
//! branch `fix/single-account-allocation-fallback` (`provider_known_accounts`),
//! lifted here so learning and reading cannot disagree about an account.
//!
//! [`session_provider_account`]: super::schema
//! [`provider_account_seen`]: super::schema

use std::collections::{BTreeSet, HashMap};

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;

use antiburn_local::analysis::{ProviderHint, lookup_turn_pricing};
use antiburn_local::pricing::ModelTokens;
use antiburn_local::pricing::calc::calculate_cache_write_cost;

use crate::provider_usage::{attribute, has_tokens};

use super::provider_usage_history::ProviderUsagePeriod;
use super::{SessionKey, Store};

/// Session/model/speed groups one attribution query may return before it
/// gives up rather than load an unbounded result.
const MAX_ATTRIBUTION_GROUPS: usize = 20_000;

/// Provider periods one candidate scan may return.
const MAX_CANDIDATE_PERIODS: usize = 64;

/// Ceiling on sample retention, independent of the session-data setting.
const SAMPLE_RETENTION_DAYS_CAP: i64 = 365;

/// The two lanes the learner and the badge track. Anthropic's supplemental
/// per-model windows, and any role a provider stated that this app declines
/// to guess the meaning of, carry no factor.
pub(crate) const LANE_FIVE_HOUR: &str = "fiveHour";
pub(crate) const LANE_WEEKLY: &str = "weekly";

/// A lane's nominal length, used to derive a window start the provider did
/// not state directly.
pub(crate) fn lane_duration_seconds(lane: &str) -> i64 {
    if lane == LANE_WEEKLY { 604_800 } else { 18_000 }
}

/// Map a period's stated role to the lane the learner tracks, or `None` for a
/// role this app does not attribute a factor to.
pub(crate) fn lane_for_window_role(window_role: &str) -> Option<&'static str> {
    match window_role {
        "primaryShort" => Some(LANE_FIVE_HOUR),
        "primaryLong" => Some(LANE_WEEKLY),
        _ => None,
    }
}

/// The stated role that carries one lane's observations. The inverse of
/// [`lane_for_window_role`].
fn window_role_for_lane(lane: &str) -> &'static str {
    if lane == LANE_WEEKLY {
        "primaryLong"
    } else {
        "primaryShort"
    }
}

/// Priced, provider- and account-attributed turn dollars for one session,
/// inside a queried interval.
///
/// Kept split by session rather than summed so a later contribution chart can
/// read the same query unsummed; the learner sums these rows itself.
#[derive(Debug, Clone, PartialEq)]
pub struct AttributedSessionDollars {
    pub key: SessionKey,
    pub input_usd: f64,
    pub output_usd: f64,
    pub cache_read_usd: f64,
    pub cache_write_usd: f64,
    pub turn_count: i64,
}

impl AttributedSessionDollars {
    fn empty(key: SessionKey) -> Self {
        AttributedSessionDollars {
            key,
            input_usd: 0.0,
            output_usd: 0.0,
            cache_read_usd: 0.0,
            cache_write_usd: 0.0,
            turn_count: 0,
        }
    }
}

/// One measurement of the factor: a meter delta, or a first window reading,
/// with the priced dollars behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct FactorSample {
    pub provider: String,
    pub account_key: String,
    pub lane: String,
    pub kind: String,
    pub period_id: Option<i64>,
    pub from_epoch: i64,
    pub to_epoch: i64,
    pub from_percent: f64,
    pub to_percent: f64,
    pub input_usd: f64,
    pub output_usd: f64,
    pub cache_read_usd: f64,
    pub cache_write_usd: f64,
    pub turn_count: i64,
    pub plan: Option<String>,
    pub plan_tier: Option<String>,
    pub source_id: String,
    pub computed_at_epoch: i64,
}

impl FactorSample {
    pub fn total_usd(&self) -> f64 {
        self.input_usd + self.output_usd + self.cache_read_usd + self.cache_write_usd
    }

    pub fn percent_delta(&self) -> f64 {
        self.to_percent - self.from_percent
    }
}

/// The factor in effect from `effective_at_epoch`, for one account and lane.
#[derive(Debug, Clone, PartialEq)]
pub struct FactorPoint {
    pub id: i64,
    pub provider: String,
    pub account_key: String,
    pub lane: String,
    pub effective_at_epoch: i64,
    pub usd_per_percent: f64,
    pub method: String,
    pub sample_count: i64,
    pub plan: Option<String>,
    pub plan_tier: Option<String>,
}

const ATTRIBUTED_TURN_SQL: &str = "SELECT t.environment_key, t.agent, t.session_id,
            a.provider_hints_json,
            COALESCE((
                SELECT json_group_array(json_object(
                    'provider', spa.provider,
                    'accountKey', spa.account_key
                ))
                  FROM session_provider_account spa
                 WHERE spa.environment_key = s.environment_key
                   AND spa.agent = s.agent AND spa.session_id = s.session_id
                   AND spa.provider = ?4
            ), '[]'),
            t.model, t.speed,
            SUM(t.input_tokens), SUM(t.cache_read_tokens), SUM(t.cache_write_tokens),
            SUM(t.output_tokens), COUNT(*)
       FROM turn t INDEXED BY turn_usage_timestamp
       JOIN session_evidence e
         ON e.environment_key = t.environment_key
        AND e.agent = t.agent AND e.session_id = t.session_id
        AND e.published_fence = t.claim_fence
       JOIN session s
         ON s.environment_key = t.environment_key
        AND s.agent = t.agent AND s.session_id = t.session_id
       LEFT JOIN session_analysis a
         ON a.environment_key = s.environment_key
        AND a.agent = s.agent AND a.session_id = s.session_id
      WHERE t.ts_ms > ?1 AND t.ts_ms <= ?2
      GROUP BY t.environment_key, t.agent, t.session_id, t.model, t.speed
      LIMIT ?3";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountObservation {
    account_key: String,
}

/// Resolve one session's account for `provider` under the two-step rule.
fn resolve_account(bound_accounts_json: &str, known: Option<&BTreeSet<String>>) -> Option<String> {
    let bound: BTreeSet<String> =
        serde_json::from_str::<Vec<AccountObservation>>(bound_accounts_json)
            .unwrap_or_default()
            .into_iter()
            .map(|observation| observation.account_key)
            .filter(|account| account.len() == 64)
            .collect();
    if bound.len() == 1 {
        return bound.into_iter().next();
    }
    if !bound.is_empty() {
        return None;
    }
    let known = known?;
    if known.len() == 1 {
        known.iter().next().cloned()
    } else {
        None
    }
}

impl Store {
    /// Every account this app has observed for a provider, by agent.
    ///
    /// The two-step account rule falls back to this when a session has no
    /// direct `session_provider_account` binding for the provider.
    pub(crate) fn provider_known_accounts(
        &self,
        provider: &str,
    ) -> Result<HashMap<String, BTreeSet<String>>> {
        let connection = self.lock();
        let mut statement = connection
            .prepare("SELECT agent, account_key FROM provider_account_seen WHERE provider = ?1")?;
        let mut known: HashMap<String, BTreeSet<String>> = HashMap::new();
        let mut rows = statement.query([provider])?;
        while let Some(row) = rows.next()? {
            let agent: String = row.get(0)?;
            let account_key: String = row.get(1)?;
            known.entry(agent).or_default().insert(account_key);
        }
        Ok(known)
    }

    /// Priced, attributed turn dollars for one account, grouped by session.
    ///
    /// The range is `(from_epoch, to_epoch]`. Dollars are priced through
    /// [`antiburn_local::analysis::lookup_turn_pricing`], the same catalog
    /// [`crate::provider_usage::allocation`] uses. `None` means the bounded
    /// group limit overflowed; the caller tries again on a later pass.
    pub(crate) fn attributed_turn_dollars_between(
        &self,
        provider: &str,
        account_key: &str,
        from_epoch: i64,
        to_epoch: i64,
    ) -> Result<Option<Vec<AttributedSessionDollars>>> {
        if to_epoch <= from_epoch {
            return Ok(Some(Vec::new()));
        }
        let known = self.provider_known_accounts(provider)?;
        let connection = self.lock();
        attributed_turn_dollars_between_in(
            &connection,
            provider,
            account_key,
            from_epoch,
            to_epoch,
            &known,
        )
    }

    /// Active provider periods a factor-learning pass should examine.
    ///
    /// A period qualifies when it carries a primary lane and received an
    /// observation at or after `since_epoch`. Bootstrapping a new period and
    /// recomputing a recent one are the same query: both leave a fresh
    /// `last_observed_epoch`.
    pub(crate) fn provider_limit_candidate_periods(
        &self,
        since_epoch: i64,
    ) -> Result<Vec<ProviderUsagePeriod>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT id, provider, account_key, window_id, window_kind, window_role,
                    scope_key, scope_label, duration_seconds, starts_at_epoch,
                    resets_at_epoch, first_observed_epoch, last_observed_epoch
               FROM provider_usage_period
              WHERE scope_key = 'account'
                AND window_role IN ('primaryShort', 'primaryLong')
                AND last_observed_epoch >= ?1
              ORDER BY last_observed_epoch DESC
              LIMIT ?2",
        )?;
        let periods = statement
            .query_map(
                params![since_epoch, MAX_CANDIDATE_PERIODS as i64],
                row_to_period,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(periods)
    }

    /// Insert or replace one factor sample, keyed by its interval.
    pub(crate) fn upsert_factor_sample(&self, sample: &FactorSample) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO provider_limit_factor_sample (
                    provider, account_key, lane, kind, period_id, from_epoch, to_epoch,
                    from_percent, to_percent, input_usd, output_usd, cache_read_usd,
                    cache_write_usd, turn_count, plan, plan_tier, source_id, computed_at_epoch
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
                ON CONFLICT (provider, account_key, lane, from_epoch, to_epoch) DO UPDATE SET
                    kind = excluded.kind,
                    period_id = excluded.period_id,
                    from_percent = excluded.from_percent,
                    to_percent = excluded.to_percent,
                    input_usd = excluded.input_usd,
                    output_usd = excluded.output_usd,
                    cache_read_usd = excluded.cache_read_usd,
                    cache_write_usd = excluded.cache_write_usd,
                    turn_count = excluded.turn_count,
                    plan = excluded.plan,
                    plan_tier = excluded.plan_tier,
                    source_id = excluded.source_id,
                    computed_at_epoch = excluded.computed_at_epoch",
            params![
                sample.provider,
                sample.account_key,
                sample.lane,
                sample.kind,
                sample.period_id,
                sample.from_epoch,
                sample.to_epoch,
                sample.from_percent,
                sample.to_percent,
                sample.input_usd,
                sample.output_usd,
                sample.cache_read_usd,
                sample.cache_write_usd,
                sample.turn_count,
                sample.plan,
                sample.plan_tier,
                sample.source_id,
                sample.computed_at_epoch,
            ],
        )?;
        Ok(())
    }

    /// Samples already recorded for one period, keyed by their interval.
    pub(crate) fn factor_samples_for_period(
        &self,
        period_id: i64,
    ) -> Result<HashMap<(i64, i64), FactorSample>> {
        let connection = self.lock();
        let mut statement =
            connection.prepare(&format!("{FACTOR_SAMPLE_SELECT} WHERE period_id = ?1"))?;
        let samples = statement
            .query_map([period_id], row_to_sample)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(samples
            .into_iter()
            .map(|sample| ((sample.from_epoch, sample.to_epoch), sample))
            .collect())
    }

    /// Whether a delta sample exists at all for one account and lane.
    ///
    /// A window-start sample is only ever taken while this is false.
    pub(crate) fn has_delta_factor_sample(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
    ) -> Result<bool> {
        let connection = self.lock();
        Ok(connection.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM provider_limit_factor_sample
                  WHERE provider = ?1 AND account_key = ?2 AND lane = ?3 AND kind = 'delta'
             )",
            params![provider, account_key, lane],
            |row| row.get::<_, i64>(0),
        )? != 0)
    }

    /// Delta samples for one account and lane, at or after `since_epoch`,
    /// ordered oldest first.
    pub(crate) fn delta_factor_samples_since(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
        since_epoch: i64,
    ) -> Result<Vec<FactorSample>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "{FACTOR_SAMPLE_SELECT} WHERE provider = ?1 AND account_key = ?2 AND lane = ?3
               AND kind = 'delta' AND to_epoch >= ?4
              ORDER BY to_epoch"
        ))?;
        let samples = statement
            .query_map(
                params![provider, account_key, lane, since_epoch],
                row_to_sample,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(samples)
    }

    /// Every delta sample for one account and lane, ordered oldest first.
    ///
    /// Used only when fewer than three fall inside the recent window.
    pub(crate) fn all_delta_factor_samples(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
    ) -> Result<Vec<FactorSample>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "{FACTOR_SAMPLE_SELECT} WHERE provider = ?1 AND account_key = ?2 AND lane = ?3
               AND kind = 'delta'
              ORDER BY to_epoch"
        ))?;
        let samples = statement
            .query_map(params![provider, account_key, lane], row_to_sample)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(samples)
    }

    /// The most recent window-start sample for one account and lane.
    pub(crate) fn latest_window_start_factor_sample(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
    ) -> Result<Option<FactorSample>> {
        let connection = self.lock();
        connection
            .query_row(
                &format!(
                    "{FACTOR_SAMPLE_SELECT} WHERE provider = ?1 AND account_key = ?2 AND lane = ?3
                       AND kind = 'window_start'
                      ORDER BY to_epoch DESC LIMIT 1"
                ),
                params![provider, account_key, lane],
                row_to_sample,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Append or replace the factor point effective at `effective_at_epoch`.
    pub(crate) fn upsert_factor_point(&self, point: &FactorPoint) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO provider_limit_factor_point (
                    provider, account_key, lane, effective_at_epoch, usd_per_percent,
                    method, sample_count, plan, plan_tier
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                ON CONFLICT (provider, account_key, lane, effective_at_epoch) DO UPDATE SET
                    usd_per_percent = excluded.usd_per_percent,
                    method = excluded.method,
                    sample_count = excluded.sample_count,
                    plan = excluded.plan,
                    plan_tier = excluded.plan_tier",
            params![
                point.provider,
                point.account_key,
                point.lane,
                point.effective_at_epoch,
                point.usd_per_percent,
                point.method,
                point.sample_count,
                point.plan,
                point.plan_tier,
            ],
        )?;
        Ok(())
    }

    /// The newest factor point for one account and lane.
    pub(crate) fn latest_factor_point(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
    ) -> Result<Option<FactorPoint>> {
        let connection = self.lock();
        connection
            .query_row(
                &format!(
                    "{FACTOR_POINT_SELECT} WHERE provider = ?1 AND account_key = ?2 AND lane = ?3
                      ORDER BY effective_at_epoch DESC LIMIT 1"
                ),
                params![provider, account_key, lane],
                row_to_point,
            )
            .optional()
            .map_err(Into::into)
    }

    /// The factor point in effect at `at_epoch`: the latest point at or
    /// before it, else the earliest point of all.
    pub(crate) fn factor_point_at(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
        at_epoch: i64,
    ) -> Result<Option<FactorPoint>> {
        let connection = self.lock();
        let current = connection
            .query_row(
                &format!(
                    "{FACTOR_POINT_SELECT} WHERE provider = ?1 AND account_key = ?2 AND lane = ?3
                       AND effective_at_epoch <= ?4
                      ORDER BY effective_at_epoch DESC LIMIT 1"
                ),
                params![provider, account_key, lane, at_epoch],
                row_to_point,
            )
            .optional()?;
        if current.is_some() {
            return Ok(current);
        }
        connection
            .query_row(
                &format!(
                    "{FACTOR_POINT_SELECT} WHERE provider = ?1 AND account_key = ?2 AND lane = ?3
                      ORDER BY effective_at_epoch ASC LIMIT 1"
                ),
                params![provider, account_key, lane],
                row_to_point,
            )
            .optional()
            .map_err(Into::into)
    }

    /// The plan and plan tier the newest observation reported for one
    /// account and lane, so the learner can notice a plan change.
    pub(crate) fn latest_observation_plan(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
    ) -> Result<Option<(Option<String>, Option<String>)>> {
        let connection = self.lock();
        connection
            .query_row(
                "SELECT plan, plan_tier FROM provider_usage_observation
                  WHERE provider = ?1 AND account_key = ?2 AND window_role = ?3
                    AND used_percent IS NOT NULL
                  ORDER BY observed_at_epoch DESC LIMIT 1",
                params![provider, account_key, window_role_for_lane(lane)],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
    }

    /// Replace one period's residual: meter percent against the factor's own
    /// estimate for the same span.
    pub(crate) fn upsert_limit_residual(
        &self,
        period_id: i64,
        computed_at_epoch: i64,
        meter_percent: f64,
        estimated_percent: f64,
    ) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO provider_limit_residual (
                    period_id, computed_at_epoch, meter_percent, estimated_percent
                ) VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT (period_id) DO UPDATE SET
                    computed_at_epoch = excluded.computed_at_epoch,
                    meter_percent = excluded.meter_percent,
                    estimated_percent = excluded.estimated_percent",
            params![
                period_id,
                computed_at_epoch,
                meter_percent,
                estimated_percent
            ],
        )?;
        Ok(())
    }
}

/// Null a sample's `period_id` before its period is deleted by retention.
///
/// The period-deletion query in [`super::provider_usage_history`] stays
/// exactly as it was before samples existed: this runs first, in the same
/// transaction, against the identical set of about-to-be-removed periods.
pub(crate) fn detach_samples_pending_period_deletion_in(connection: &Connection) -> Result<()> {
    connection.execute(
        "UPDATE provider_limit_factor_sample
            SET period_id = NULL
          WHERE period_id IN (
              SELECT id FROM provider_usage_period p
               WHERE NOT EXISTS (
                       SELECT 1 FROM provider_usage_observation o WHERE o.period_id = p.id
                   )
                 AND NOT EXISTS (
                       SELECT 1 FROM provider_usage_session_allocation a WHERE a.period_id = p.id
                   )
                 AND NOT EXISTS (
                       SELECT 1 FROM provider_usage_allocation_dirty d WHERE d.period_id = p.id
                   )
          )",
        [],
    )?;
    Ok(())
}

/// Delete factor samples older than the session-data retention setting,
/// capped at 365 days. Points are never deleted.
pub(crate) fn apply_sample_retention_in(
    connection: &Connection,
    retention_days: i32,
    now_epoch: i64,
) -> Result<()> {
    let bounded_days = match retention_days {
        days if days > 0 => i64::from(days).min(SAMPLE_RETENTION_DAYS_CAP),
        _ => SAMPLE_RETENTION_DAYS_CAP,
    };
    let cutoff = now_epoch.saturating_sub(bounded_days.saturating_mul(86_400));
    connection.execute(
        "DELETE FROM provider_limit_factor_sample WHERE to_epoch < ?1",
        [cutoff],
    )?;
    Ok(())
}

const FACTOR_SAMPLE_SELECT: &str = "SELECT provider, account_key, lane, kind, period_id,
            from_epoch, to_epoch, from_percent, to_percent, input_usd, output_usd,
            cache_read_usd, cache_write_usd, turn_count, plan, plan_tier, source_id,
            computed_at_epoch
       FROM provider_limit_factor_sample";

const FACTOR_POINT_SELECT: &str = "SELECT id, provider, account_key, lane, effective_at_epoch,
            usd_per_percent, method, sample_count, plan, plan_tier
       FROM provider_limit_factor_point";

fn row_to_sample(row: &rusqlite::Row<'_>) -> rusqlite::Result<FactorSample> {
    Ok(FactorSample {
        provider: row.get(0)?,
        account_key: row.get(1)?,
        lane: row.get(2)?,
        kind: row.get(3)?,
        period_id: row.get(4)?,
        from_epoch: row.get(5)?,
        to_epoch: row.get(6)?,
        from_percent: row.get(7)?,
        to_percent: row.get(8)?,
        input_usd: row.get(9)?,
        output_usd: row.get(10)?,
        cache_read_usd: row.get(11)?,
        cache_write_usd: row.get(12)?,
        turn_count: row.get(13)?,
        plan: row.get(14)?,
        plan_tier: row.get(15)?,
        source_id: row.get(16)?,
        computed_at_epoch: row.get(17)?,
    })
}

fn row_to_point(row: &rusqlite::Row<'_>) -> rusqlite::Result<FactorPoint> {
    Ok(FactorPoint {
        id: row.get(0)?,
        provider: row.get(1)?,
        account_key: row.get(2)?,
        lane: row.get(3)?,
        effective_at_epoch: row.get(4)?,
        usd_per_percent: row.get(5)?,
        method: row.get(6)?,
        sample_count: row.get(7)?,
        plan: row.get(8)?,
        plan_tier: row.get(9)?,
    })
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

fn attributed_turn_dollars_between_in(
    connection: &Connection,
    provider: &str,
    account_key: &str,
    from_epoch: i64,
    to_epoch: i64,
    known_accounts: &HashMap<String, BTreeSet<String>>,
) -> Result<Option<Vec<AttributedSessionDollars>>> {
    let start_ms = from_epoch.saturating_mul(1_000).saturating_add(1);
    let end_ms = to_epoch.saturating_mul(1_000);
    let mut statement = connection.prepare(ATTRIBUTED_TURN_SQL)?;
    let mut rows = statement.query(params![
        start_ms,
        end_ms,
        (MAX_ATTRIBUTION_GROUPS + 1) as i64,
        provider,
    ])?;
    let mut by_session: HashMap<SessionKey, AttributedSessionDollars> = HashMap::new();
    let mut group_count = 0usize;
    while let Some(row) = rows.next()? {
        group_count += 1;
        if group_count > MAX_ATTRIBUTION_GROUPS {
            return Ok(None);
        }
        let key = SessionKey::new(
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        );
        let hints_json: Option<String> = row.get(3)?;
        let accounts_json: String = row.get(4)?;
        let model: Option<String> = row.get(5)?;
        let speed: Option<String> = row.get(6)?;
        let input_tokens: i64 = row.get(7)?;
        let cache_read_tokens: i64 = row.get(8)?;
        let cache_write_tokens: i64 = row.get(9)?;
        let output_tokens: i64 = row.get(10)?;
        let turn_count: i64 = row.get(11)?;

        let resolved = resolve_account(&accounts_json, known_accounts.get(&key.agent));
        if resolved.as_deref() != Some(account_key) {
            continue;
        }
        let Some(model) = model
            .as_deref()
            .map(str::trim)
            .filter(|model| !model.is_empty())
        else {
            continue;
        };
        let tokens = ModelTokens {
            input_tokens: input_tokens.max(0) as u64,
            output_tokens: output_tokens.max(0) as u64,
            cache_read_tokens: cache_read_tokens.max(0) as u64,
            cache_creation_tokens: cache_write_tokens.max(0) as u64,
            cache_creation_1h_tokens: 0,
        };
        if !has_tokens(&tokens) {
            continue;
        }
        let hints: Vec<ProviderHint> = hints_json
            .as_deref()
            .and_then(|json| serde_json::from_str(json).ok())
            .unwrap_or_default();
        let attributed = attribute(
            &key.agent,
            std::collections::BTreeMap::from([(model.to_string(), tokens)]),
            &hints,
        );
        let Some(model_tokens) = attributed
            .get(provider)
            .and_then(|entry| entry.models.get(model))
        else {
            continue;
        };
        let rates = lookup_turn_pricing(model, speed.as_deref());
        let totals = by_session
            .entry(key.clone())
            .or_insert_with(|| AttributedSessionDollars::empty(key.clone()));
        totals.turn_count += turn_count;
        if let Some(rates) = rates {
            totals.input_usd += model_tokens.input_tokens as f64 * rates.input_cost_per_token;
            totals.output_usd += model_tokens.output_tokens as f64 * rates.output_cost_per_token;
            totals.cache_read_usd +=
                model_tokens.cache_read_tokens as f64 * rates.cache_read_cost_per_token;
            totals.cache_write_usd += calculate_cache_write_cost(model_tokens, &rates);
        }
    }
    Ok(Some(by_session.into_values().collect()))
}

#[cfg(test)]
mod tests;

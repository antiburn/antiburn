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

/// How far back the diagnostics export counts recent delta and unattributed
/// samples, matching the factor's own weighted-median lookback.
const RECENT_SAMPLE_WINDOW_SECS: i64 = 14 * 86_400;

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
    resolve_bound_account(Some(&bound), known)
}

/// Resolve one session's account for one provider under the two-step rule,
/// from already-loaded account sets rather than a JSON column.
///
/// A session with exactly one bound account for the provider uses it. A
/// session with no bound account falls back to the agent's single known
/// account for the provider, when it has exactly one. Every other case —
/// more than one bound account, or more than one known account and none
/// bound — is unattributed.
pub(crate) fn resolve_bound_account(
    bound: Option<&BTreeSet<String>>,
    known: Option<&BTreeSet<String>>,
) -> Option<String> {
    match bound {
        Some(accounts) if accounts.len() == 1 => accounts.iter().next().cloned(),
        Some(accounts) if !accounts.is_empty() => None,
        _ => match known {
            Some(known) if known.len() == 1 => known.iter().next().cloned(),
            _ => None,
        },
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

    /// Every direct account binding for the given sessions, across every
    /// provider, in one query rather than one per session.
    ///
    /// The two-step account rule resolves each `(session key, provider)`
    /// entry against [`Store::provider_known_accounts`] through
    /// [`resolve_bound_account`].
    pub(crate) fn session_bound_accounts(
        &self,
        keys: &[SessionKey],
    ) -> Result<HashMap<(SessionKey, String), BTreeSet<String>>> {
        let mut bound: HashMap<(SessionKey, String), BTreeSet<String>> = HashMap::new();
        if keys.is_empty() {
            return Ok(bound);
        }
        let mut clauses = Vec::with_capacity(keys.len());
        let mut values = Vec::with_capacity(keys.len() * 3);
        for key in keys.iter().take(500) {
            clauses.push("(environment_key = ? AND agent = ? AND session_id = ?)");
            values.push(rusqlite::types::Value::from(key.environment_key.clone()));
            values.push(rusqlite::types::Value::from(key.agent.clone()));
            values.push(rusqlite::types::Value::from(key.session_id.clone()));
        }
        let sql = format!(
            "SELECT environment_key, agent, session_id, provider, account_key
               FROM session_provider_account
              WHERE {}",
            clauses.join(" OR ")
        );
        let connection = self.lock();
        let mut statement = connection.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(values))?;
        while let Some(row) = rows.next()? {
            let key = SessionKey::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            );
            let provider: String = row.get(3)?;
            let account_key: String = row.get(4)?;
            if account_key.len() == 64 {
                bound
                    .entry((key, provider))
                    .or_default()
                    .insert(account_key);
            }
        }
        Ok(bound)
    }

    /// Priced, attributed turn dollars for one account, grouped by session.
    ///
    /// The range is `(from_epoch, to_epoch]`. Dollars are priced through
    /// [`antiburn_local::analysis::lookup_turn_pricing`]. `None` means the
    /// bounded group limit overflowed; the caller tries again on a later pass.
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
    /// A period qualifies when it carries a primary lane and either: has no
    /// learn cursor yet, has a reading newer than its cursor, or was
    /// observed at or after `since_epoch`. The first two admit a period
    /// whose readings are old — from bootstrap on upgrade or from a
    /// backfill — that a "recently observed" rule alone would never pick up;
    /// the third keeps a just-updated period in the recompute window even
    /// once its cursor catches up to it.
    pub(crate) fn provider_limit_candidate_periods(
        &self,
        since_epoch: i64,
    ) -> Result<Vec<ProviderUsagePeriod>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT p.id, p.provider, p.account_key, p.window_id, p.window_kind, p.window_role,
                    p.scope_key, p.scope_label, p.duration_seconds, p.starts_at_epoch,
                    p.resets_at_epoch, p.first_observed_epoch, p.last_observed_epoch
               FROM provider_usage_period p
               LEFT JOIN provider_limit_learn_cursor c ON c.period_id = p.id
              WHERE p.scope_key = 'account'
                AND p.window_role IN ('primaryShort', 'primaryLong')
                AND (c.period_id IS NULL
                     OR p.last_observed_epoch > c.learned_through_epoch
                     OR p.last_observed_epoch >= ?1)
              ORDER BY p.last_observed_epoch DESC
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

    /// Record how far one period's observations have been read into samples.
    ///
    /// Call this only after a pass considers every sample the period could
    /// yet produce; a pass that stops mid-period on its budget must leave
    /// the cursor where it was, so the period stays a candidate next time.
    pub(crate) fn advance_learn_cursor(
        &self,
        period_id: i64,
        learned_through_epoch: i64,
    ) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "INSERT INTO provider_limit_learn_cursor (period_id, learned_through_epoch)
                 VALUES (?1, ?2)
                 ON CONFLICT (period_id) DO UPDATE SET
                     learned_through_epoch = excluded.learned_through_epoch",
            params![period_id, learned_through_epoch],
        )?;
        Ok(())
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

    /// Whether a delta or rollout sample exists for one account and lane
    /// under the given plan and plan tier.
    ///
    /// A window-start sample is only ever taken while this is false. Scoping
    /// to the plan pair lets a fresh plan or tier form its own window-start
    /// sample rather than being blocked by an older plan's history. A
    /// rollout sample counts the same as a delta sample: both are a real
    /// meter delta, priced the same way, differing only in provenance.
    pub(crate) fn has_delta_factor_sample(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
        plan: Option<&str>,
        plan_tier: Option<&str>,
    ) -> Result<bool> {
        let connection = self.lock();
        Ok(connection.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM provider_limit_factor_sample
                  WHERE provider = ?1 AND account_key = ?2 AND lane = ?3
                    AND kind IN ('delta', 'rollout')
                    AND plan IS ?4 AND plan_tier IS ?5
             )",
            params![provider, account_key, lane, plan, plan_tier],
            |row| row.get::<_, i64>(0),
        )? != 0)
    }

    /// Delta and rollout samples for one account and lane, at or after
    /// `since_epoch`, ordered oldest first.
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
               AND kind IN ('delta', 'rollout') AND to_epoch >= ?4
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

    /// Every delta and rollout sample for one account and lane, ordered
    /// oldest first.
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
               AND kind IN ('delta', 'rollout')
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

/// One `(provider, account, lane)` factor for the diagnostics export: its
/// current point, how many points and recent samples support it, and its
/// latest residual, if the lane has one.
///
/// [`crate::diagnostics_export`] is the only reader. It groups these rows by
/// `(provider, lane)`, replaces `account_key` with a stable per-export
/// ordinal, and never carries `account_key` itself into the document.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LimitFactorDiagnostics {
    pub provider: String,
    pub account_key: String,
    pub lane: String,
    pub usd_per_percent: f64,
    pub method: String,
    pub sample_count: i64,
    pub point_count: i64,
    pub delta_sample_count: i64,
    pub unattributed_sample_count: i64,
    pub plan: Option<String>,
    pub plan_tier: Option<String>,
    pub residual: Option<LimitResidualDiagnostics>,
}

/// One lane's latest residual, for [`LimitFactorDiagnostics`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LimitResidualDiagnostics {
    pub meter_percent: f64,
    pub estimated_percent: f64,
    pub computed_at_epoch: i64,
}

/// A `(provider, account_key, lane)` group key, shared by the diagnostics
/// queries below so their results can be joined in memory.
type DiagnosticsGroupKey = (String, String, String);

/// One entry per `(provider, account, lane)` that carries at least one factor
/// point, for the diagnostics export.
///
/// Reads directly from a connection rather than through [`Store::lock`]: the
/// export builds its document from its own pinned read-only snapshot, not
/// from a live [`Store`].
pub(crate) fn limit_factor_diagnostics_in(
    connection: &Connection,
    now_epoch: i64,
) -> Result<Vec<LimitFactorDiagnostics>> {
    let mut statement = connection.prepare(
        "SELECT provider, account_key, lane, usd_per_percent, method, sample_count,
                plan, plan_tier
           FROM provider_limit_factor_point p
          WHERE effective_at_epoch = (
                    SELECT MAX(effective_at_epoch)
                      FROM provider_limit_factor_point latest
                     WHERE latest.provider = p.provider
                       AND latest.account_key = p.account_key
                       AND latest.lane = p.lane
                )
          ORDER BY provider, account_key, lane",
    )?;
    let latest_points = statement
        .query_map([], |row| {
            Ok((
                (
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ),
                row.get::<_, f64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let point_counts = grouped_counts(
        connection,
        "SELECT provider, account_key, lane, COUNT(*) FROM provider_limit_factor_point
          GROUP BY provider, account_key, lane",
        [],
    )?;
    let since_epoch = now_epoch - RECENT_SAMPLE_WINDOW_SECS;
    let delta_counts = grouped_counts(
        connection,
        "SELECT provider, account_key, lane, COUNT(*) FROM provider_limit_factor_sample
          WHERE kind = ?1 AND to_epoch >= ?2
          GROUP BY provider, account_key, lane",
        params!["delta", since_epoch],
    )?;
    let unattributed_counts = grouped_counts(
        connection,
        "SELECT provider, account_key, lane, COUNT(*) FROM provider_limit_factor_sample
          WHERE kind = ?1 AND to_epoch >= ?2
          GROUP BY provider, account_key, lane",
        params!["unattributed", since_epoch],
    )?;
    let residuals = latest_residuals_by_lane(connection)?;

    Ok(latest_points
        .into_iter()
        .map(
            |(key, usd_per_percent, method, sample_count, plan, plan_tier)| {
                let (provider, account_key, lane) = key.clone();
                LimitFactorDiagnostics {
                    point_count: point_counts.get(&key).copied().unwrap_or(0),
                    delta_sample_count: delta_counts.get(&key).copied().unwrap_or(0),
                    unattributed_sample_count: unattributed_counts.get(&key).copied().unwrap_or(0),
                    residual: residuals.get(&key).copied(),
                    provider,
                    account_key,
                    lane,
                    usd_per_percent,
                    method,
                    sample_count,
                    plan,
                    plan_tier,
                }
            },
        )
        .collect())
}

/// Run one `GROUP BY (provider, account_key, lane)` count query.
///
/// A shared shape for the point-count, delta-sample-count, and
/// unattributed-sample-count queries [`limit_factor_diagnostics_in`] needs,
/// so the three read alike rather than each hand-rolling row decoding.
fn grouped_counts(
    connection: &Connection,
    sql: &str,
    query_params: impl rusqlite::Params,
) -> Result<HashMap<DiagnosticsGroupKey, i64>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement
        .query_map(query_params, |row| {
            Ok((
                (
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ),
                row.get::<_, i64>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows.into_iter().collect())
}

/// The latest residual for each `(provider, account, lane)`, resolving each
/// residual row's period to the lane it belongs to.
///
/// `provider_limit_residual` is keyed by `period_id`, not by lane directly,
/// so this joins through `provider_usage_period` to read the period's own
/// provider, account, and window role.
fn latest_residuals_by_lane(
    connection: &Connection,
) -> Result<HashMap<DiagnosticsGroupKey, LimitResidualDiagnostics>> {
    let mut statement = connection.prepare(
        "SELECT pu.provider, pu.account_key, pu.window_role,
                r.meter_percent, r.estimated_percent, r.computed_at_epoch
           FROM provider_limit_residual r
           JOIN provider_usage_period pu ON pu.id = r.period_id
          WHERE r.computed_at_epoch = (
                    SELECT MAX(r2.computed_at_epoch)
                      FROM provider_limit_residual r2
                      JOIN provider_usage_period pu2 ON pu2.id = r2.period_id
                     WHERE pu2.provider = pu.provider
                       AND pu2.account_key = pu.account_key
                       AND pu2.window_role = pu.window_role
                )",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, f64>(4)?,
            row.get::<_, i64>(5)?,
        ))
    })?;
    let mut result = HashMap::new();
    for row in rows {
        let (
            provider,
            account_key,
            window_role,
            meter_percent,
            estimated_percent,
            computed_at_epoch,
        ) = row?;
        let Some(lane) = lane_for_window_role(&window_role) else {
            continue;
        };
        result.insert(
            (provider, account_key, lane.to_string()),
            LimitResidualDiagnostics {
                meter_percent,
                estimated_percent,
                computed_at_epoch,
            },
        );
    }
    Ok(result)
}

/// Null a sample's `period_id`, and delete its learn cursor, before its
/// period is deleted by retention.
///
/// The period-deletion query in [`super::provider_usage_history`] stays
/// exactly as it was before samples existed: this runs first, in the same
/// transaction, against the identical set of about-to-be-removed periods.
pub(crate) fn detach_samples_pending_period_deletion_in(connection: &Connection) -> Result<()> {
    const PENDING_DELETION: &str = "
              SELECT id FROM provider_usage_period p
               WHERE NOT EXISTS (
                       SELECT 1 FROM provider_usage_observation o WHERE o.period_id = p.id
                   )";
    connection.execute(
        &format!(
            "UPDATE provider_limit_factor_sample
                SET period_id = NULL
              WHERE period_id IN ({PENDING_DELETION})"
        ),
        [],
    )?;
    connection.execute(
        &format!("DELETE FROM provider_limit_learn_cursor WHERE period_id IN ({PENDING_DELETION})"),
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

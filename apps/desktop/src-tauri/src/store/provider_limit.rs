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

use crate::provider_usage::factor::model_matches_scope;
use crate::provider_usage::live::normalize::slugify;
use crate::provider_usage::providers::display_name;
use crate::provider_usage::{attribute, has_tokens};

use super::provider_usage_history::{ProviderUsageObservation, ProviderUsagePeriod};
use super::{SessionKey, Store};

/// Session/model/speed groups one attribution query may return before it
/// gives up rather than load an unbounded result.
const MAX_ATTRIBUTION_GROUPS: usize = 20_000;

/// Provider periods one candidate scan may return.
const MAX_CANDIDATE_PERIODS: usize = 64;

/// How far back the diagnostics export counts recent delta and unattributed
/// samples, matching the factor's own weighted-median lookback.
const RECENT_SAMPLE_WINDOW_SECS: i64 = 14 * 86_400;

/// The two account-wide lanes the learner and the badge track, plus any
/// number of `model:<slug>` lanes for a provider's supplemental per-model
/// weekly windows, such as Anthropic's "Fable" limit. Any other role a
/// provider stated that this app declines to guess the meaning of carries no
/// factor.
pub(crate) const LANE_FIVE_HOUR: &str = "fiveHour";
pub(crate) const LANE_WEEKLY: &str = "weekly";

/// Prefix for a lane keyed by a supplemental weekly window scoped to one
/// model, rather than to the whole account.
pub(crate) const MODEL_LANE_PREFIX: &str = "model:";

/// Turn timestamps land in a 15-minute bucket for the per-session-per-bucket
/// contribution query, keyed the same way the quota screen buckets a chart.
pub(crate) const CONTRIBUTION_BUCKET_SECS: i64 = 900;

/// A lane's nominal length, used to derive a window start the provider did
/// not state directly. A model-scoped lane is weekly, the only period length
/// this app has seen a supplemental model window carry.
pub(crate) fn lane_duration_seconds(lane: &str) -> i64 {
    if lane == LANE_WEEKLY || lane.starts_with(MODEL_LANE_PREFIX) {
        604_800
    } else {
        18_000
    }
}

/// Map a period's role, kind, id, and scope to the lane the learner tracks,
/// or `None` for a period this app does not attribute a factor to.
///
/// The two account-wide windows map to the two fixed lanes. A supplemental
/// weekly window scoped to one model maps to its own `model:<slug>` lane,
/// keyed by the slug the window id already carries so the lane survives a
/// display-name repunctuation the provider might make later. A window id of
/// another shape falls back to slugifying the scope label directly.
pub(crate) fn lane_for_period(period: &ProviderUsagePeriod) -> Option<String> {
    match period.window_role.as_str() {
        "primaryShort" if period.scope_key == "account" => Some(LANE_FIVE_HOUR.to_string()),
        "primaryLong" if period.scope_key == "account" => Some(LANE_WEEKLY.to_string()),
        "supplemental"
            if period.window_kind == "weekly"
                && period.scope_key.starts_with(MODEL_LANE_PREFIX) =>
        {
            let slug = period
                .window_id
                .strip_prefix("weekly-")
                .map(str::to_string)
                .unwrap_or_else(|| slugify(&period.scope_label));
            Some(format!("{MODEL_LANE_PREFIX}{slug}"))
        }
        _ => None,
    }
}

/// The model name to filter a lane's dollars by, for a model-scoped lane.
///
/// `None` for the two account-wide lanes, whose query prices every model.
pub(crate) fn model_scope_for_period(period: &ProviderUsagePeriod) -> Option<&str> {
    period
        .scope_key
        .starts_with(MODEL_LANE_PREFIX)
        .then_some(period.scope_label.as_str())
}

/// The observation filter that carries one lane's readings: the stated
/// window role for the two fixed lanes, or the stated role together with the
/// exact supplemental window id for a model-scoped lane. The inverse of
/// [`lane_for_period`].
pub(crate) fn observation_filter_for_lane(lane: &str) -> (&'static str, Option<String>) {
    match lane.strip_prefix(MODEL_LANE_PREFIX) {
        Some(slug) => ("supplemental", Some(format!("weekly-{slug}"))),
        None if lane == LANE_WEEKLY => ("primaryLong", None),
        None => ("primaryShort", None),
    }
}

/// The reader-facing name for a lane: "Weekly" and "5-hour" for the two
/// fixed lanes, else the model-scoped period's own scope label (Anthropic's
/// supplemental window is currently labelled "Fable").
pub(crate) fn lane_label(lane: &str, period: &ProviderUsagePeriod) -> String {
    match lane {
        LANE_WEEKLY => "Weekly".to_string(),
        LANE_FIVE_HOUR => "5-hour".to_string(),
        _ => period.scope_label.clone(),
    }
}

/// Sort key for a quota account's lanes: `weekly` first, then `fiveHour`,
/// then every `model:` lane alphabetically by its own name.
fn lane_sort_key(lane: &str) -> (u8, &str) {
    match lane {
        LANE_WEEKLY => (0, lane),
        LANE_FIVE_HOUR => (1, lane),
        _ => (2, lane),
    }
}

/// A lane's most recent period whose reset is after `now`, with any boundary
/// the provider did not state derived the same way
/// [`crate::provider_usage::quota::resolve_periods`] derives it: reported,
/// else the other boundary offset by `lane_duration`.
///
/// `None` when every period for the lane has already reset, or carries
/// neither boundary. Lets a reader define "this week" as the account's
/// actual current window rather than a calendar week.
fn current_period_for_lane(
    periods: &[&ProviderUsagePeriod],
    lane_duration: i64,
    now_epoch: i64,
) -> Option<(i64, i64)> {
    periods
        .iter()
        .filter_map(|period| {
            let resets_at_epoch = period
                .resets_at_epoch
                .or_else(|| period.starts_at_epoch.map(|start| start + lane_duration))?;
            if resets_at_epoch <= now_epoch {
                return None;
            }
            let starts_at_epoch = period
                .starts_at_epoch
                .unwrap_or(resets_at_epoch - lane_duration);
            Some((period.last_observed_epoch, starts_at_epoch, resets_at_epoch))
        })
        .max_by_key(|(last_observed_epoch, ..)| *last_observed_epoch)
        .map(|(_, starts_at_epoch, resets_at_epoch)| (starts_at_epoch, resets_at_epoch))
}

/// One lane a quota account carries: its reader-facing label, whether it has
/// a learned factor yet, and its current window, when one is open.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuotaAccountLane {
    pub lane: String,
    pub label: String,
    pub has_factor: bool,
    /// `(starts_at_epoch, resets_at_epoch)` of the lane's open window, when
    /// one exists.
    pub current_period: Option<(i64, i64)>,
    /// The earliest reading this lane holds. A range that ends before it
    /// has no data.
    pub first_observed_epoch: i64,
}

/// One `(provider, account)` this app has observed at least one quota period
/// for, with every lane it carries.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuotaAccount {
    pub provider: String,
    pub display_name: String,
    pub account_key: String,
    pub lanes: Vec<QuotaAccountLane>,
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

/// Which account, if any, [`price_turn_row`] resolved a priced group to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// The session resolved to exactly this account.
    Bound(String),
    /// The session has no resolved account for the provider.
    Unbound,
}

/// Priced, attributed turn dollars for one session inside one 15-minute
/// bucket, for the quota screen's per-session contribution chart.
///
/// A row resolved to a different account than the one asked for never
/// becomes one of these: [`Store::attributed_turn_dollars_by_bucket`] drops
/// it. A row with no resolved account keeps its bucket's dollars under
/// [`Resolved::Unbound`], so the caller can report an "unattributed" total
/// rather than silently dropping spend nobody could be credited with.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BucketedSessionDollars {
    pub key: SessionKey,
    pub bucket_start_epoch: i64,
    pub usd: f64,
    pub turn_count: i64,
    pub account: Resolved,
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

/// The scan groups turns first, before any per-session join runs. The inner
/// query `g` scans `turn` and `session_evidence` and groups by session, model,
/// and speed. The outer query then joins `session` and `session_analysis`,
/// and evaluates the account subquery, once per group in `g`, not once per
/// turn. This cuts a 30-day, 240k-row scan from about 2.6 s to about 0.5 s on
/// a 3,900-group result, with the same rows out.
const ATTRIBUTED_TURN_SQL: &str = "SELECT g.environment_key, g.agent, g.session_id,
            a.provider_hints_json,
            COALESCE((
                SELECT json_group_array(json_object(
                    'provider', spa.provider,
                    'accountKey', spa.account_key
                ))
                  FROM session_provider_account spa
                 WHERE spa.environment_key = g.environment_key
                   AND spa.agent = g.agent AND spa.session_id = g.session_id
                   AND spa.provider = ?4
            ), '[]'),
            g.model, g.speed,
            g.input_tokens, g.cache_read_tokens, g.cache_write_tokens,
            g.output_tokens, g.turn_count, g.cache_write_1h_tokens
       FROM (
            SELECT t.environment_key, t.agent, t.session_id, t.model, t.speed,
                   SUM(t.input_tokens) AS input_tokens,
                   SUM(t.cache_read_tokens) AS cache_read_tokens,
                   SUM(t.cache_write_tokens) AS cache_write_tokens,
                   SUM(t.output_tokens) AS output_tokens,
                   COUNT(*) AS turn_count,
                   SUM(t.cache_write_1h_tokens) AS cache_write_1h_tokens
              FROM turn t INDEXED BY turn_usage_timestamp
              JOIN session_evidence e
                ON e.environment_key = t.environment_key
               AND e.agent = t.agent AND e.session_id = t.session_id
               AND e.published_fence = t.claim_fence
             WHERE t.ts_ms > ?1 AND t.ts_ms <= ?2
             GROUP BY t.environment_key, t.agent, t.session_id, t.model, t.speed
       ) g
       JOIN session s
         ON s.environment_key = g.environment_key
        AND s.agent = g.agent AND s.session_id = g.session_id
       LEFT JOIN session_analysis a
         ON a.environment_key = g.environment_key
        AND a.agent = g.agent AND a.session_id = g.session_id
      LIMIT ?3";

/// [`ATTRIBUTED_TURN_SQL`], grouped further by 15-minute bucket, for the
/// quota screen's per-session-per-bucket contribution chart. Same
/// group-first shape, joins, fences, limit, and params; the inner scan
/// additionally groups by bucket, and the outer query carries it through.
const ATTRIBUTED_TURN_BUCKET_SQL: &str = "SELECT g.environment_key, g.agent, g.session_id,
            a.provider_hints_json,
            COALESCE((
                SELECT json_group_array(json_object(
                    'provider', spa.provider,
                    'accountKey', spa.account_key
                ))
                  FROM session_provider_account spa
                 WHERE spa.environment_key = g.environment_key
                   AND spa.agent = g.agent AND spa.session_id = g.session_id
            ), '[]'),
            g.model, g.speed,
            g.input_tokens, g.cache_read_tokens, g.cache_write_tokens,
            g.output_tokens, g.turn_count, g.cache_write_1h_tokens,
            g.bucket
       FROM (
            SELECT t.environment_key, t.agent, t.session_id, t.model, t.speed,
                   SUM(t.input_tokens) AS input_tokens,
                   SUM(t.cache_read_tokens) AS cache_read_tokens,
                   SUM(t.cache_write_tokens) AS cache_write_tokens,
                   SUM(t.output_tokens) AS output_tokens,
                   COUNT(*) AS turn_count,
                   SUM(t.cache_write_1h_tokens) AS cache_write_1h_tokens,
                   t.ts_ms / 900000 AS bucket
              FROM turn t INDEXED BY turn_usage_timestamp
              JOIN session_evidence e
                ON e.environment_key = t.environment_key
               AND e.agent = t.agent AND e.session_id = t.session_id
               AND e.published_fence = t.claim_fence
             WHERE t.ts_ms > ?1 AND t.ts_ms <= ?2
             GROUP BY t.environment_key, t.agent, t.session_id, t.model, t.speed, bucket
       ) g
       JOIN session s
         ON s.environment_key = g.environment_key
        AND s.agent = g.agent AND s.session_id = g.session_id
       LEFT JOIN session_analysis a
         ON a.environment_key = g.environment_key
        AND a.agent = g.agent AND a.session_id = g.session_id
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
    ///
    /// `model_scope`, when given, keeps only turns whose model matches it —
    /// the period's scope label for a model-scoped lane, `None` for an
    /// account-wide one.
    pub(crate) fn attributed_turn_dollars_between(
        &self,
        provider: &str,
        account_key: &str,
        from_epoch: i64,
        to_epoch: i64,
        model_scope: Option<&str>,
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
            model_scope,
            &known,
        )
    }

    /// Priced, attributed turn dollars for one account, grouped by session
    /// and 15-minute bucket, for the quota screen's contribution chart.
    ///
    /// The range is `(from_epoch, to_epoch]`, same as
    /// [`Store::attributed_turn_dollars_between`]. A row resolved to a
    /// different account is dropped; a row resolved to no account at all is
    /// kept under [`Resolved::Unbound`] rather than dropped, so the caller
    /// can report unattributed spend instead of losing it silently. `None`
    /// means the bounded group limit overflowed.
    pub(crate) fn attributed_turn_dollars_by_bucket(
        &self,
        provider: &str,
        account_key: &str,
        model_scope: Option<&str>,
        from_epoch: i64,
        to_epoch: i64,
    ) -> Result<Option<Vec<BucketedSessionDollars>>> {
        Ok(self.quota_turn_input(from_epoch, to_epoch)?.map(|input| {
            input
                .for_account(provider, account_key, model_scope)
                .by_bucket(model_scope)
        }))
    }

    pub(crate) fn quota_turn_input(
        &self,
        from_epoch: i64,
        to_epoch: i64,
    ) -> Result<Option<QuotaTurnInput>> {
        if to_epoch <= from_epoch {
            return Ok(Some(QuotaTurnInput::default()));
        }
        let started = std::time::Instant::now();
        let connection = self.lock();
        let wait_ms = started.elapsed().as_millis() as u64;
        let read_started = std::time::Instant::now();
        let input = read_quota_turn_input(&connection, from_epoch, to_epoch)?;
        drop(connection);
        tracing::debug!(
            wait_ms,
            read_ms = read_started.elapsed().as_millis() as u64,
            groups = input.as_ref().map(|input| input.rows.len()),
            "read quota turn input"
        );
        Ok(input)
    }

    /// Ascending, minute-rounded epochs of every turn in `(from_epoch,
    /// to_epoch]` whose session resolves to `account_key`, under the same
    /// two-step rule as every other query in this module.
    ///
    /// Feeds the five-hour lane's turn-gap resolver
    /// ([`crate::provider_usage::quota::resolve_periods`]): account binding
    /// is a per-session fact, so this reads every turn from a matching
    /// session regardless of model or provider attribution.
    ///
    /// Scans the minute-epoch rows once, then resolves each distinct
    /// session's account with one small point query against
    /// `session_provider_account` (indexed by `session_provider_account_lookup`).
    /// A range with 240k turn rows carries only a few hundred distinct
    /// sessions, so this replaces a second full-range scan with a point
    /// lookup per session.
    pub(crate) fn attributed_turn_epochs(
        &self,
        provider: &str,
        account_key: &str,
        from_epoch: i64,
        to_epoch: i64,
    ) -> Result<Vec<i64>> {
        Ok(self
            .attributed_turn_minutes(from_epoch, to_epoch)?
            .for_account(provider, account_key))
    }

    /// The same scan [`Store::attributed_turn_epochs`] ran per account,
    /// shared across every account a caller resolves from it.
    ///
    /// Scans the minute-epoch rows once, resolves each distinct session's
    /// account bindings once across every provider (not filtered to one, the
    /// way a single [`Store::attributed_turn_epochs`] call was), and reads
    /// [`Store::provider_known_accounts`]'s fallback map once for every
    /// provider `provider_account_seen` has, the same single query
    /// [`read_quota_turn_input`] already runs for [`QuotaTurnInput`].
    /// [`TurnMinutes::for_account`] then resolves one account from this scan
    /// with no further store access, so a caller that needs several accounts
    /// over the same range pays for the scan once, not once per account.
    pub(crate) fn attributed_turn_minutes(
        &self,
        from_epoch: i64,
        to_epoch: i64,
    ) -> Result<TurnMinutes> {
        if to_epoch <= from_epoch {
            return Ok(TurnMinutes::default());
        }
        let start_ms = from_epoch.saturating_mul(1_000).saturating_add(1);
        let end_ms = to_epoch.saturating_mul(1_000);
        let connection = self.lock();

        let mut minutes_by_session: HashMap<SessionKey, BTreeSet<i64>> = HashMap::new();
        {
            let mut statement = connection.prepare(
                "SELECT DISTINCT t.environment_key, t.agent, t.session_id,
                        (t.ts_ms / 60000) * 60 AS minute_epoch
                   FROM turn t INDEXED BY turn_usage_timestamp
                   JOIN session_evidence e
                     ON e.environment_key = t.environment_key
                    AND e.agent = t.agent AND e.session_id = t.session_id
                    AND e.published_fence = t.claim_fence
                  WHERE t.ts_ms > ?1 AND t.ts_ms <= ?2",
            )?;
            let mut rows = statement.query(params![start_ms, end_ms])?;
            while let Some(row) = rows.next()? {
                let key = SessionKey::new(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                );
                let minute_epoch: i64 = row.get(3)?;
                minutes_by_session
                    .entry(key)
                    .or_default()
                    .insert(minute_epoch);
            }
        }

        let mut accounts_by_session: HashMap<SessionKey, String> = HashMap::new();
        if !minutes_by_session.is_empty() {
            let mut account_statement = connection.prepare(
                "SELECT COALESCE((
                     SELECT json_group_array(json_object(
                         'provider', spa.provider,
                         'accountKey', spa.account_key
                     ))
                       FROM session_provider_account spa
                      WHERE spa.environment_key = ?1
                        AND spa.agent = ?2 AND spa.session_id = ?3
                 ), '[]')",
            )?;
            for key in minutes_by_session.keys() {
                let accounts_json: String = account_statement.query_row(
                    params![key.environment_key, key.agent, key.session_id],
                    |row| row.get(0),
                )?;
                accounts_by_session.insert(key.clone(), accounts_json);
            }
        }

        let mut known_accounts: HashMap<(String, String), BTreeSet<String>> = HashMap::new();
        {
            let mut statement = connection
                .prepare("SELECT provider, agent, account_key FROM provider_account_seen")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                known_accounts
                    .entry((row.get(0)?, row.get(1)?))
                    .or_default()
                    .insert(row.get(2)?);
            }
        }

        Ok(TurnMinutes {
            minutes_by_session,
            accounts_by_session,
            known_accounts,
        })
    }

    /// The epoch span, in seconds, of one session's own published turns:
    /// `(earliest, latest)`. `None` when the session has no timestamped,
    /// published turn.
    ///
    /// Bounds the range [`Store::quota_periods_for_lane`] and
    /// [`Store::attributed_turn_epochs`] need to cover a session's activity
    /// for `get_session_quota`.
    pub(crate) fn session_turn_epoch_bounds(&self, key: &SessionKey) -> Result<Option<(i64, i64)>> {
        let connection = self.lock();
        connection
            .query_row(
                "SELECT MIN(t.ts_ms), MAX(t.ts_ms)
                   FROM turn t
                   JOIN session_evidence e
                     ON e.environment_key = t.environment_key
                    AND e.agent = t.agent AND e.session_id = t.session_id
                    AND e.published_fence = t.claim_fence
                  WHERE t.environment_key = ?1 AND t.agent = ?2 AND t.session_id = ?3
                    AND t.ts_ms IS NOT NULL",
                params![key.environment_key, key.agent, key.session_id],
                |row| {
                    let min_ms: Option<i64> = row.get(0)?;
                    let max_ms: Option<i64> = row.get(1)?;
                    Ok(min_ms.zip(max_ms))
                },
            )
            .map(|bounds| bounds.map(|(min_ms, max_ms)| (min_ms / 1_000, max_ms / 1_000)))
            .map_err(Into::into)
    }

    /// Every stored factor point for one account and lane, oldest first.
    ///
    /// The quota screen loads a lane's whole point series once per call and
    /// binary-searches it per bucket, rather than querying
    /// [`Store::factor_point_at`] once per bucket.
    pub(crate) fn factor_points_for_lane(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
    ) -> Result<Vec<FactorPoint>> {
        let connection = self.lock();
        let mut statement = connection.prepare(&format!(
            "{FACTOR_POINT_SELECT} WHERE provider = ?1 AND account_key = ?2 AND lane = ?3
              ORDER BY effective_at_epoch"
        ))?;
        let points = statement
            .query_map(params![provider, account_key, lane], row_to_point)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(points)
    }

    /// Observed periods for one lane overlapping `[range_start, range_end)`,
    /// plus the most recent period before the range, which anchors weekly
    /// cadence extrapolation in
    /// [`crate::provider_usage::quota::resolve_periods`].
    ///
    /// A period's boundary is derived the same way the resolver derives it
    /// (`COALESCE` against the lane's nominal duration) so a period missing
    /// one stated boundary is not missed here only to be picked up, or
    /// double counted, by the resolver's own fallback.
    pub(crate) fn quota_periods_for_lane(
        &self,
        provider: &str,
        account_key: &str,
        lane: &str,
        range_start: i64,
        range_end: i64,
    ) -> Result<Vec<ProviderUsagePeriod>> {
        let (window_role, window_id) = observation_filter_for_lane(lane);
        let lane_duration = lane_duration_seconds(lane);
        let connection = self.lock();
        let mut statement = connection.prepare(QUOTA_PERIODS_FOR_LANE_SQL)?;
        let periods = statement
            .query_map(
                params![
                    provider,
                    account_key,
                    window_role,
                    window_id,
                    range_start,
                    range_end,
                    lane_duration
                ],
                row_to_period,
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(periods)
    }

    /// One period's readings, oldest first. A thin name for
    /// [`Store::provider_usage_period_history`] at the quota screen's call
    /// sites, which want only the readings, never the period row again.
    pub(crate) fn quota_period_samples(
        &self,
        period_id: i64,
    ) -> Result<Vec<ProviderUsageObservation>> {
        Ok(self
            .provider_usage_period_history(period_id)?
            .map(|history| history.observations)
            .unwrap_or_default())
    }

    /// Several periods' readings in one round trip. A thin name over
    /// [`Store::provider_usage_observations_for`] for the quota screen's
    /// call sites that already have every period id they need, the way
    /// [`Store::quota_period_samples`] is one for a single period.
    pub(crate) fn quota_period_samples_for(
        &self,
        period_ids: &[i64],
    ) -> Result<HashMap<i64, Vec<ProviderUsageObservation>>> {
        self.provider_usage_observations_for(period_ids)
    }

    /// Every `(provider, account)` this app has observed a quota period for,
    /// with the lanes each one carries.
    pub(crate) fn quota_accounts(&self, now_epoch: i64) -> Result<Vec<QuotaAccount>> {
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT id, provider, account_key, window_id, window_kind, window_role,
                    scope_key, scope_label, duration_seconds, starts_at_epoch,
                    resets_at_epoch, first_observed_epoch, last_observed_epoch
               FROM provider_usage_period",
        )?;
        let periods = statement
            .query_map([], row_to_period)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);

        let mut by_account: std::collections::BTreeMap<
            (String, String),
            std::collections::BTreeMap<String, Vec<&ProviderUsagePeriod>>,
        > = Default::default();
        for period in &periods {
            let Some(lane) = lane_for_period(period) else {
                continue;
            };
            by_account
                .entry((period.provider.clone(), period.account_key.clone()))
                .or_default()
                .entry(lane)
                .or_default()
                .push(period);
        }

        let mut accounts = Vec::with_capacity(by_account.len());
        for ((provider, account_key), lanes_by_name) in by_account {
            let mut lanes = Vec::with_capacity(lanes_by_name.len());
            for (lane, lane_periods) in lanes_by_name {
                let label = lane_periods
                    .first()
                    .map(|period| lane_label(&lane, period))
                    .unwrap_or_else(|| lane.clone());
                let has_factor = connection.query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM provider_limit_factor_point
                          WHERE provider = ?1 AND account_key = ?2 AND lane = ?3
                     )",
                    params![provider, account_key, lane],
                    |row| row.get::<_, i64>(0),
                )? != 0;
                let current_period =
                    current_period_for_lane(&lane_periods, lane_duration_seconds(&lane), now_epoch);
                let first_observed_epoch = lane_periods
                    .iter()
                    .map(|period| period.first_observed_epoch)
                    .min()
                    .unwrap_or(now_epoch);
                lanes.push(QuotaAccountLane {
                    lane,
                    label,
                    has_factor,
                    current_period,
                    first_observed_epoch,
                });
            }
            lanes.sort_by(|left, right| lane_sort_key(&left.lane).cmp(&lane_sort_key(&right.lane)));
            accounts.push(QuotaAccount {
                display_name: display_name(&provider).to_string(),
                provider,
                account_key,
                lanes,
            });
        }
        accounts.sort_by(|left, right| {
            (&left.provider, &left.account_key).cmp(&(&right.provider, &right.account_key))
        });
        Ok(accounts)
    }

    /// Active provider periods a factor-learning pass should examine.
    ///
    /// A period qualifies when it carries an account-wide primary lane or a
    /// supplemental model-scoped weekly lane, and either: has no learn
    /// cursor yet, has a reading newer than its cursor, or was observed at
    /// or after `since_epoch`. The first two admit a period
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
              WHERE (
                        (p.scope_key = 'account' AND p.window_role IN ('primaryShort', 'primaryLong'))
                     OR (p.window_role = 'supplemental' AND p.window_kind = 'weekly'
                         AND p.scope_key LIKE 'model:%')
                    )
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
        let (window_role, window_id) = observation_filter_for_lane(lane);
        match window_id {
            Some(window_id) => connection
                .query_row(
                    "SELECT plan, plan_tier FROM provider_usage_observation
                      WHERE provider = ?1 AND account_key = ?2 AND window_role = ?3
                        AND window_id = ?4 AND used_percent IS NOT NULL
                      ORDER BY observed_at_epoch DESC LIMIT 1",
                    params![provider, account_key, window_role, window_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(Into::into),
            None => connection
                .query_row(
                    "SELECT plan, plan_tier FROM provider_usage_observation
                      WHERE provider = ?1 AND account_key = ?2 AND window_role = ?3
                        AND used_percent IS NOT NULL
                      ORDER BY observed_at_epoch DESC LIMIT 1",
                    params![provider, account_key, window_role],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(Into::into),
        }
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

    /// Whether one closed quota window already reported its
    /// `antiburn.quota_window_closed` accuracy event.
    ///
    /// A period reports at most once, ever: this is the durable half of that
    /// promise, checked before analytics decides to report a period again.
    #[cfg(feature = "analytics")]
    pub(crate) fn quota_window_already_reported(&self, period_id: i64) -> Result<bool> {
        let connection = self.lock();
        Ok(connection
            .query_row(
                "SELECT 1 FROM quota_window_reported WHERE period_id = ?1",
                [period_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Mark one closed quota window as reported. `period_id` is the table's
    /// primary key, so a marker already present from an earlier pass is left
    /// alone rather than duplicated.
    #[cfg(feature = "analytics")]
    pub(crate) fn mark_quota_window_reported(
        &self,
        period_id: i64,
        reported_at_epoch: i64,
    ) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "INSERT OR IGNORE INTO quota_window_reported (period_id, reported_at_epoch)
                VALUES (?1, ?2)",
            params![period_id, reported_at_epoch],
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
    // Rollout-history readings count as delta samples here: both are
    // attributed observation-to-observation deltas, differing only in
    // where the reading came from.
    let delta_counts = grouped_counts(
        connection,
        "SELECT provider, account_key, lane, COUNT(*) FROM provider_limit_factor_sample
          WHERE kind IN ('delta', 'rollout') AND to_epoch >= ?1
          GROUP BY provider, account_key, lane",
        params![since_epoch],
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
/// `provider_limit_residual` is keyed by `period_id`, not by lane directly.
/// A model-scoped lane needs more than the window role to identify it — two
/// different model windows can share `window_role = 'supplemental'` — so
/// this reads every period field [`lane_for_period`] needs and resolves the
/// lane, and keeps the newest reading, in Rust rather than in SQL.
fn latest_residuals_by_lane(
    connection: &Connection,
) -> Result<HashMap<DiagnosticsGroupKey, LimitResidualDiagnostics>> {
    let mut statement = connection.prepare(
        "SELECT pu.provider, pu.account_key, pu.window_id, pu.window_kind, pu.window_role,
                pu.scope_key, pu.scope_label,
                r.meter_percent, r.estimated_percent, r.computed_at_epoch
           FROM provider_limit_residual r
           JOIN provider_usage_period pu ON pu.id = r.period_id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, f64>(7)?,
            row.get::<_, f64>(8)?,
            row.get::<_, i64>(9)?,
        ))
    })?;
    let mut result: HashMap<DiagnosticsGroupKey, LimitResidualDiagnostics> = HashMap::new();
    for row in rows {
        let (
            provider,
            account_key,
            window_id,
            window_kind,
            window_role,
            scope_key,
            scope_label,
            meter_percent,
            estimated_percent,
            computed_at_epoch,
        ) = row?;
        // Only the fields `lane_for_period` reads matter here; the rest of a
        // real period row plays no part in a residual's lane identity.
        let period = ProviderUsagePeriod {
            id: 0,
            provider: provider.clone(),
            account_key: account_key.clone(),
            window_id,
            window_kind,
            window_role,
            scope_key,
            scope_label,
            duration_seconds: None,
            starts_at_epoch: None,
            resets_at_epoch: None,
            first_observed_epoch: 0,
            last_observed_epoch: 0,
        };
        let Some(lane) = lane_for_period(&period) else {
            continue;
        };
        let key = (provider, account_key, lane);
        let is_newer = result
            .get(&key)
            .is_none_or(|existing| computed_at_epoch > existing.computed_at_epoch);
        if is_newer {
            result.insert(
                key,
                LimitResidualDiagnostics {
                    meter_percent,
                    estimated_percent,
                    computed_at_epoch,
                },
            );
        }
    }
    Ok(result)
}

/// Null a sample's `period_id`, delete its learn cursor, and delete its
/// residual and its quota-window-closed marker, before its period is deleted
/// by retention.
///
/// The period-deletion query in [`super::provider_usage_history`] stays
/// exactly as it was before samples existed: this runs first, in the same
/// transaction, against the identical set of about-to-be-removed periods.
/// `provider_limit_residual` and `quota_window_reported` both key their row
/// on `period_id` alone, so a pending period never leaves one behind: each is
/// deleted here, not nulled.
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
    connection.execute(
        &format!("DELETE FROM provider_limit_residual WHERE period_id IN ({PENDING_DELETION})"),
        [],
    )?;
    // The quota-window-closed marker keys on `period_id` too, so it goes with
    // its period the same way the residual row does.
    connection.execute(
        &format!("DELETE FROM quota_window_reported WHERE period_id IN ({PENDING_DELETION})"),
        [],
    )?;
    Ok(())
}

/// Delete factor samples older than the session-data retention setting, with
/// no cap. Points are never deleted.
///
/// The caller already turned a forever setting into an early return, so
/// `retention_days` here is always positive.
pub(crate) fn apply_sample_retention_in(
    connection: &Connection,
    retention_days: i32,
    now_epoch: i64,
) -> Result<()> {
    let cutoff = now_epoch.saturating_sub(i64::from(retention_days).saturating_mul(86_400));
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

/// Periods for [`Store::quota_periods_for_lane`]: every period matching the
/// lane's observation filter whose derived `[start, reset)` overlaps the
/// range (`?5, ?6`), unioned with the single most recent matching period
/// that resets at or before the range starts — the cadence anchor. `?4` is
/// the lane's window id, `NULL` for the two fixed lanes, in which case the
/// query falls back to `scope_key = 'account'` to stay within one lane's
/// rows. `?7` is the lane's nominal duration, used only to derive whichever
/// boundary a period did not state.
const QUOTA_PERIODS_FOR_LANE_SQL: &str = "SELECT id, provider, account_key, window_id, window_kind,
            window_role, scope_key, scope_label, duration_seconds, starts_at_epoch,
            resets_at_epoch, first_observed_epoch, last_observed_epoch
       FROM provider_usage_period
      WHERE provider = ?1 AND account_key = ?2 AND window_role = ?3
        AND (?4 IS NULL OR window_id = ?4) AND (?4 IS NOT NULL OR scope_key = 'account')
        AND COALESCE(resets_at_epoch, starts_at_epoch + ?7) > ?5
        AND COALESCE(starts_at_epoch, resets_at_epoch - ?7) < ?6
      UNION
      SELECT * FROM (
          SELECT id, provider, account_key, window_id, window_kind, window_role,
                 scope_key, scope_label, duration_seconds, starts_at_epoch,
                 resets_at_epoch, first_observed_epoch, last_observed_epoch
            FROM provider_usage_period
           WHERE provider = ?1 AND account_key = ?2 AND window_role = ?3
             AND (?4 IS NULL OR window_id = ?4) AND (?4 IS NOT NULL OR scope_key = 'account')
             AND COALESCE(resets_at_epoch, starts_at_epoch + ?7) <= ?5
           ORDER BY COALESCE(resets_at_epoch, starts_at_epoch + ?7) DESC
           LIMIT 1
      )
      ORDER BY id";

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

/// One priced `(session, model, speed)` group: the account the session
/// resolved to, if any, and the dollars its tokens price to.
///
/// Shared by [`attributed_turn_dollars_between_in`] and
/// [`QuotaTurnInput::for_account`], so learning and the quota
/// screen's contribution chart cannot disagree about how a group is priced
/// or which account it belongs to.
struct PricedTurnGroup {
    resolved_account: Option<String>,
    input_usd: f64,
    output_usd: f64,
    cache_read_usd: f64,
    cache_write_usd: f64,
    turn_count: i64,
}

/// One `(session, model, speed[, bucket])` row from [`ATTRIBUTED_TURN_SQL`]
/// or [`ATTRIBUTED_TURN_BUCKET_SQL`], decoded but not yet priced.
///
/// Bundles [`price_turn_row`]'s per-row fields into one value so the
/// function itself stays under a handful of parameters.
struct TurnGroupRow<'a> {
    agent: &'a str,
    resolved_account: Option<String>,
    hints_json: Option<&'a str>,
    model: Option<&'a str>,
    speed: Option<&'a str>,
    input_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    cache_write_1h_tokens: i64,
    turn_count: i64,
}

/// Price one group's tokens, or return `None` when it
/// carries no model, no billable tokens, or a model outside `model_scope`.
///
/// `None` here means "this row prices to nothing": the caller drops it
/// regardless of which account it resolved to. Every caller must still apply
/// its own keep-or-drop policy over `resolved_account` — this function
/// never filters by account itself, since the two callers disagree on which
/// accounts to keep.
fn price_turn_row(
    provider: &str,
    model_scope: Option<&str>,
    row: &TurnGroupRow<'_>,
) -> Option<PricedTurnGroup> {
    let resolved_account = row.resolved_account.clone();
    let model = row.model.map(str::trim).filter(|model| !model.is_empty())?;
    if let Some(scope) = model_scope
        && !model_matches_scope(model, scope)
    {
        return None;
    }
    let tokens = ModelTokens {
        input_tokens: row.input_tokens.max(0) as u64,
        output_tokens: row.output_tokens.max(0) as u64,
        cache_read_tokens: row.cache_read_tokens.max(0) as u64,
        cache_creation_tokens: row.cache_write_tokens.max(0) as u64,
        cache_creation_1h_tokens: row.cache_write_1h_tokens.max(0) as u64,
    };
    if !has_tokens(&tokens) {
        return None;
    }
    let hints: Vec<ProviderHint> = row
        .hints_json
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let attributed = attribute(
        row.agent,
        std::collections::BTreeMap::from([(model.to_string(), tokens)]),
        &hints,
    );
    let model_tokens = attributed
        .get(provider)
        .and_then(|entry| entry.models.get(model))?;
    let (input_usd, output_usd, cache_read_usd, cache_write_usd) =
        match lookup_turn_pricing(model, row.speed) {
            Some(rates) => (
                model_tokens.input_tokens as f64 * rates.input_cost_per_token,
                model_tokens.output_tokens as f64 * rates.output_cost_per_token,
                model_tokens.cache_read_tokens as f64 * rates.cache_read_cost_per_token,
                calculate_cache_write_cost(model_tokens, &rates),
            ),
            None => (0.0, 0.0, 0.0, 0.0),
        };
    Some(PricedTurnGroup {
        resolved_account,
        input_usd,
        output_usd,
        cache_read_usd,
        cache_write_usd,
        turn_count: row.turn_count,
    })
}

fn attributed_turn_dollars_between_in(
    connection: &Connection,
    provider: &str,
    account_key: &str,
    from_epoch: i64,
    to_epoch: i64,
    model_scope: Option<&str>,
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
        let cache_write_1h_tokens: i64 = row.get(12)?;

        let Some(priced) = price_turn_row(
            provider,
            model_scope,
            &TurnGroupRow {
                agent: &key.agent,
                resolved_account: resolve_account(&accounts_json, known_accounts.get(&key.agent)),
                hints_json: hints_json.as_deref(),
                model: model.as_deref(),
                speed: speed.as_deref(),
                input_tokens,
                cache_read_tokens,
                cache_write_tokens,
                output_tokens,
                cache_write_1h_tokens,
                turn_count,
            },
        ) else {
            continue;
        };
        if priced.resolved_account.as_deref() != Some(account_key) {
            continue;
        }
        let totals = by_session
            .entry(key.clone())
            .or_insert_with(|| AttributedSessionDollars::empty(key.clone()));
        totals.turn_count += priced.turn_count;
        totals.input_usd += priced.input_usd;
        totals.output_usd += priced.output_usd;
        totals.cache_read_usd += priced.cache_read_usd;
        totals.cache_write_usd += priced.cache_write_usd;
    }
    Ok(Some(by_session.into_values().collect()))
}

// Keep model and speed groups until pricing and lane selection finish.
#[derive(Default)]
pub(crate) struct QuotaTurnInput {
    rows: Vec<BucketTurnRow>,
    known_accounts: HashMap<(String, String), BTreeSet<String>>,
}

struct BucketTurnRow {
    key: SessionKey,
    hints_json: Option<String>,
    accounts_json: String,
    model: Option<String>,
    speed: Option<String>,
    input_tokens: i64,
    cache_read_tokens: i64,
    cache_write_tokens: i64,
    output_tokens: i64,
    turn_count: i64,
    cache_write_1h_tokens: i64,
    bucket_start_epoch: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderAccountObservation {
    provider: String,
    account_key: String,
}

pub(crate) struct AccountTurnDollars {
    rows: Vec<(Option<String>, BucketedSessionDollars)>,
}

/// [`Store::attributed_turn_minutes`]'s shared scan: every session's turn
/// minutes in the requested range, its unfiltered account bindings, and the
/// per-provider known-account fallback map, all read once.
#[derive(Default)]
pub(crate) struct TurnMinutes {
    minutes_by_session: HashMap<SessionKey, BTreeSet<i64>>,
    accounts_by_session: HashMap<SessionKey, String>,
    known_accounts: HashMap<(String, String), BTreeSet<String>>,
}

impl TurnMinutes {
    /// Exactly what [`Store::attributed_turn_epochs`] returned before this
    /// scan was shared across accounts: this account's ascending,
    /// minute-rounded turn epochs.
    pub(crate) fn for_account(&self, provider: &str, account_key: &str) -> Vec<i64> {
        let mut epochs: BTreeSet<i64> = BTreeSet::new();
        for (key, minute_epochs) in &self.minutes_by_session {
            let accounts_json = self
                .accounts_by_session
                .get(key)
                .map(String::as_str)
                .unwrap_or("[]");
            let bound: BTreeSet<String> =
                serde_json::from_str::<Vec<ProviderAccountObservation>>(accounts_json)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|observation| {
                        observation.provider == provider && observation.account_key.len() == 64
                    })
                    .map(|observation| observation.account_key)
                    .collect();
            let resolved = resolve_bound_account(
                Some(&bound),
                self.known_accounts
                    .get(&(provider.to_string(), key.agent.clone())),
            );
            if resolved.as_deref() == Some(account_key) {
                epochs.extend(minute_epochs.iter().copied());
            }
        }
        epochs.into_iter().collect()
    }
}

impl QuotaTurnInput {
    pub(crate) fn for_account(
        &self,
        provider: &str,
        account_key: &str,
        model_scope: Option<&str>,
    ) -> AccountTurnDollars {
        let mut rows = Vec::new();
        for row in &self.rows {
            let bound = serde_json::from_str::<Vec<ProviderAccountObservation>>(&row.accounts_json)
                .unwrap_or_default()
                .into_iter()
                .filter(|observation| {
                    observation.provider == provider && observation.account_key.len() == 64
                })
                .map(|observation| observation.account_key)
                .collect();
            let resolved = resolve_bound_account(
                Some(&bound),
                self.known_accounts
                    .get(&(provider.to_string(), row.key.agent.clone())),
            );
            let account = match resolved.as_deref() {
                Some(value) if value == account_key => Resolved::Bound(value.to_string()),
                Some(_) => continue,
                None => Resolved::Unbound,
            };
            let Some(priced) = price_turn_row(
                provider,
                model_scope,
                &TurnGroupRow {
                    agent: &row.key.agent,
                    resolved_account: resolved,
                    hints_json: row.hints_json.as_deref(),
                    model: row.model.as_deref(),
                    speed: row.speed.as_deref(),
                    input_tokens: row.input_tokens,
                    cache_read_tokens: row.cache_read_tokens,
                    cache_write_tokens: row.cache_write_tokens,
                    output_tokens: row.output_tokens,
                    cache_write_1h_tokens: row.cache_write_1h_tokens,
                    turn_count: row.turn_count,
                },
            ) else {
                continue;
            };
            rows.push((
                row.model.clone(),
                BucketedSessionDollars {
                    key: row.key.clone(),
                    bucket_start_epoch: row.bucket_start_epoch,
                    usd: priced.input_usd
                        + priced.output_usd
                        + priced.cache_read_usd
                        + priced.cache_write_usd,
                    turn_count: priced.turn_count,
                    account,
                },
            ));
        }
        AccountTurnDollars { rows }
    }
}

impl AccountTurnDollars {
    pub(crate) fn by_bucket(&self, model_scope: Option<&str>) -> Vec<BucketedSessionDollars> {
        let mut buckets: HashMap<(SessionKey, i64), BucketedSessionDollars> = HashMap::new();
        for (model, row) in &self.rows {
            if let Some(scope) = model_scope
                && !model
                    .as_deref()
                    .is_some_and(|model| model_matches_scope(model.trim(), scope))
            {
                continue;
            }
            let entry = buckets
                .entry((row.key.clone(), row.bucket_start_epoch))
                .or_insert_with(|| BucketedSessionDollars {
                    usd: 0.0,
                    turn_count: 0,
                    ..row.clone()
                });
            entry.usd += row.usd;
            entry.turn_count += row.turn_count;
        }
        buckets.into_values().collect()
    }
}

fn read_quota_turn_input(
    connection: &Connection,
    from_epoch: i64,
    to_epoch: i64,
) -> Result<Option<QuotaTurnInput>> {
    let start_ms = from_epoch.saturating_mul(1_000).saturating_add(1);
    let end_ms = to_epoch.saturating_mul(1_000);
    let mut statement = connection.prepare(ATTRIBUTED_TURN_BUCKET_SQL)?;
    let mut query = statement.query(params![
        start_ms,
        end_ms,
        (MAX_ATTRIBUTION_GROUPS + 1) as i64
    ])?;
    let mut rows = Vec::new();
    while let Some(row) = query.next()? {
        if rows.len() == MAX_ATTRIBUTION_GROUPS {
            return Ok(None);
        }
        rows.push(BucketTurnRow {
            key: SessionKey::new(
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ),
            hints_json: row.get(3)?,
            accounts_json: row.get(4)?,
            model: row.get(5)?,
            speed: row.get(6)?,
            input_tokens: row.get(7)?,
            cache_read_tokens: row.get(8)?,
            cache_write_tokens: row.get(9)?,
            output_tokens: row.get(10)?,
            turn_count: row.get(11)?,
            cache_write_1h_tokens: row.get(12)?,
            bucket_start_epoch: row.get::<_, i64>(13)? * CONTRIBUTION_BUCKET_SECS,
        });
    }
    let mut known_accounts: HashMap<(String, String), BTreeSet<String>> = HashMap::new();
    let mut statement =
        connection.prepare("SELECT provider, agent, account_key FROM provider_account_seen")?;
    let mut query = statement.query([])?;
    while let Some(row) = query.next()? {
        known_accounts
            .entry((row.get(0)?, row.get(1)?))
            .or_default()
            .insert(row.get(2)?);
    }
    Ok(Some(QuotaTurnInput {
        rows,
        known_accounts,
    }))
}

#[cfg(test)]
mod tests;

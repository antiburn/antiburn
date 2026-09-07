//! Durable, compact per-session allowance contributions.
//!
//! A row is one session's share of one stated provider period. It stores the
//! result, not a contribution timeline, so observation cleanup cannot erase a
//! completed session total.

use std::collections::HashMap;

use anyhow::Result;
use rusqlite::{OptionalExtension, Transaction, params, params_from_iter};

use super::SessionKey;

/// One contribution computed for a provider allowance period.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionPeriodAllocation {
    pub key: SessionKey,
    pub metric: String,
    pub percent: f64,
    pub basis: String,
    pub partial: bool,
}

#[cfg(test)]
#[path = "provider_usage_ledger/lifecycle_tests.rs"]
mod lifecycle_tests;

/// The cumulative value for one session and one provider allowance class.
#[derive(Debug, Clone, PartialEq)]
pub struct CumulativeSessionAllocation {
    pub key: SessionKey,
    pub wsl_distro: Option<String>,
    pub metric: String,
    pub provider: String,
    pub account_key: String,
    pub window_id: String,
    pub percent: f64,
    pub partial: bool,
    pub period_count: u32,
}

/// A dirty period and the generation that a reconciler observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyPeriod {
    pub period_id: i64,
    pub generation: i64,
}

#[derive(Debug, Clone)]
struct PeriodContribution {
    allocation: CumulativeSessionAllocation,
    period_id: i64,
    starts_at_epoch: Option<i64>,
    resets_at_epoch: Option<i64>,
    duration_seconds: Option<i64>,
    window_role: String,
    scope_key: String,
}

impl super::Store {
    /// Queue changed periods for a bounded background allocation pass.
    pub fn enqueue_provider_usage_allocation_periods(
        &self,
        period_ids: &[i64],
        requested_at_epoch: i64,
    ) -> Result<()> {
        if period_ids.is_empty() {
            return Ok(());
        }
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        enqueue_in(&tx, period_ids, requested_at_epoch)?;
        tx.commit()?;
        Ok(())
    }

    /// Queue every stated period after a pricing catalog update.
    ///
    /// SQLite performs the bounded metadata update in one statement. It does
    /// not load every period id into the application.
    pub fn enqueue_all_provider_usage_allocation_periods(
        &self,
        requested_at_epoch: i64,
    ) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        tx.execute(
            "UPDATE provider_usage_allocation_revision SET value = value + 1 WHERE id = 1",
            [],
        )?;
        tx.execute(
            "INSERT INTO provider_usage_allocation_dirty (period_id, requested_at_epoch, generation)
             SELECT p.id, ?1, r.value
               FROM provider_usage_period p
               JOIN provider_usage_allocation_revision r ON r.id = 1
              WHERE p.resets_at_epoch IS NOT NULL
             ON CONFLICT(period_id) DO UPDATE SET
                 requested_at_epoch = MIN(
                     provider_usage_allocation_dirty.requested_at_epoch,
                     excluded.requested_at_epoch
                 ),
                 generation = excluded.generation",
            [requested_at_epoch],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Read at most `limit` dirty periods without removing their durable work.
    pub fn provider_usage_allocation_dirty_periods(
        &self,
        limit: usize,
    ) -> Result<Vec<DirtyPeriod>> {
        let limit = i64::try_from(limit.clamp(1, 32)).expect("bounded limit fits i64");
        let recent_limit = (limit * 3 + 3) / 4;
        let older_limit = limit - recent_limit;
        let connection = self.lock();
        let mut statement = connection.prepare(
            "WITH recent AS (
                SELECT d.period_id, d.generation
                  FROM provider_usage_allocation_dirty d
                  JOIN provider_usage_period p ON p.id = d.period_id
                 ORDER BY p.resets_at_epoch DESC, d.period_id DESC
                 LIMIT ?1
             ), older AS (
                SELECT d.period_id, d.generation
                  FROM provider_usage_allocation_dirty d
                 WHERE NOT EXISTS (
                    SELECT 1 FROM recent WHERE recent.period_id = d.period_id
                 )
                 ORDER BY d.requested_at_epoch, d.period_id
                 LIMIT ?2
             )
             SELECT period_id, generation FROM recent
             UNION ALL
             SELECT period_id, generation FROM older",
        )?;
        let ids = statement
            .query_map(params![recent_limit, older_limit], |row| {
                Ok(DirtyPeriod {
                    period_id: row.get(0)?,
                    generation: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// Replace a period's materialized contributions in one transaction.
    pub fn replace_provider_usage_period_allocations(
        &self,
        period_id: i64,
        allocations: &[SessionPeriodAllocation],
        computed_at_epoch: i64,
    ) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        replace_in(&tx, period_id, allocations, computed_at_epoch)?;
        tx.commit()?;
        Ok(())
    }

    /// Replace a period's rows and clear only the generation that was read.
    pub fn replace_provider_usage_period_allocations_and_ack(
        &self,
        period_id: i64,
        generation: i64,
        allocations: &[SessionPeriodAllocation],
        computed_at_epoch: i64,
    ) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let current = tx
            .query_row(
                "SELECT d.generation, p.allocation_frozen
                   FROM provider_usage_allocation_dirty d
                   JOIN provider_usage_period p ON p.id = d.period_id
                  WHERE d.period_id = ?1",
                [period_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? != 0)),
            )
            .optional()?;
        if current.as_ref().map(|(value, _)| *value) != Some(generation) {
            tx.commit()?;
            return Ok(());
        }
        if current.is_some_and(|(_, frozen)| frozen) {
            tx.execute(
                "UPDATE provider_usage_session_allocation SET partial = 1 WHERE period_id = ?1",
                [period_id],
            )?;
            tx.execute(
                "DELETE FROM provider_usage_allocation_dirty
                  WHERE period_id = ?1 AND generation = ?2",
                params![period_id, generation],
            )?;
            tx.commit()?;
            return Ok(());
        }
        replace_in(&tx, period_id, allocations, computed_at_epoch)?;
        tx.execute(
            "DELETE FROM provider_usage_allocation_dirty
              WHERE period_id = ?1 AND generation = ?2",
            params![period_id, generation],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Acknowledge a deleted period without writing a contribution row.
    pub fn acknowledge_provider_usage_allocation_period(
        &self,
        period_id: i64,
        generation: i64,
    ) -> Result<()> {
        let connection = self.lock();
        connection.execute(
            "DELETE FROM provider_usage_allocation_dirty
              WHERE period_id = ?1 AND generation = ?2",
            params![period_id, generation],
        )?;
        Ok(())
    }

    /// Retain an older estimate but mark it partial when bounded aggregation overflows.
    pub fn retain_partial_provider_usage_period_allocations_and_ack(
        &self,
        period_id: i64,
        generation: i64,
    ) -> Result<()> {
        let mut connection = self.lock();
        let tx = connection.transaction()?;
        let current = tx
            .query_row(
                "SELECT generation FROM provider_usage_allocation_dirty WHERE period_id = ?1",
                [period_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if current == Some(generation) {
            tx.execute(
                "UPDATE provider_usage_session_allocation SET partial = 1 WHERE period_id = ?1",
                [period_id],
            )?;
            tx.execute(
                "DELETE FROM provider_usage_allocation_dirty
                  WHERE period_id = ?1 AND generation = ?2",
                params![period_id, generation],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Read materialized totals for the sessions currently displayed.
    pub fn cumulative_session_limit_allocations(
        &self,
        keys: &[SessionKey],
    ) -> Result<Vec<CumulativeSessionAllocation>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let reader = self.open_allocation_reader()?;
        if let Some(reader) = reader.as_ref() {
            return cumulative_session_limit_allocations_in(reader, keys);
        }
        let connection = self.lock();
        cumulative_session_limit_allocations_in(&connection, keys)
    }
}

fn cumulative_session_limit_allocations_in(
    connection: &rusqlite::Connection,
    keys: &[SessionKey],
) -> Result<Vec<CumulativeSessionAllocation>> {
    let mut clauses = Vec::with_capacity(keys.len());
    let mut values = Vec::with_capacity(keys.len() * 3);
    for key in keys.iter().take(500) {
        clauses.push("(a.environment_key = ? AND a.agent = ? AND a.session_id = ?)");
        values.push(rusqlite::types::Value::from(key.environment_key.clone()));
        values.push(rusqlite::types::Value::from(key.agent.clone()));
        values.push(rusqlite::types::Value::from(key.session_id.clone()));
    }
    let sql = format!(
        "SELECT a.environment_key, a.agent, a.session_id, s.wsl_distro, a.metric,
                p.provider, p.account_key, p.window_id, a.percent, a.partial,
                a.period_id, p.starts_at_epoch, p.resets_at_epoch, p.duration_seconds,
                p.window_role, p.scope_key
           FROM provider_usage_session_allocation a
           JOIN provider_usage_period p ON p.id = a.period_id
           JOIN session s ON s.environment_key = a.environment_key
             AND s.agent = a.agent AND s.session_id = a.session_id
          WHERE {}",
        clauses.join(" OR ")
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(params_from_iter(values), |row| {
            Ok(PeriodContribution {
                allocation: CumulativeSessionAllocation {
                    key: SessionKey::new(
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ),
                    wsl_distro: row.get(3)?,
                    metric: row.get(4)?,
                    provider: row.get(5)?,
                    account_key: row.get(6)?,
                    window_id: row.get(7)?,
                    percent: row.get(8)?,
                    partial: row.get::<_, i64>(9)? != 0,
                    period_count: 1,
                },
                period_id: row.get(10)?,
                starts_at_epoch: row.get(11)?,
                resets_at_epoch: row.get(12)?,
                duration_seconds: row.get(13)?,
                window_role: row.get(14)?,
                scope_key: row.get(15)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut by_account: HashMap<_, Vec<_>> = HashMap::new();
    for contribution in rows {
        let allocation = &contribution.allocation;
        by_account
            .entry((
                allocation.key.clone(),
                allocation.metric.clone(),
                allocation.provider.clone(),
                allocation.account_key.clone(),
            ))
            .or_default()
            .push(contribution);
    }

    let mut best: HashMap<(SessionKey, String), CumulativeSessionAllocation> = HashMap::new();
    for (_, contributions) in by_account {
        let Some(allocation) = cumulative_lane_allocation(contributions) else {
            continue;
        };
        let key = (allocation.key.clone(), allocation.metric.clone());
        if best
            .get(&key)
            .is_none_or(|current| allocation.percent > current.percent)
        {
            best.insert(key, allocation);
        }
    }
    let mut allocations: Vec<_> = best.into_values().collect();
    allocations.sort_by(|left, right| {
        left.key
            .agent
            .cmp(&right.key.agent)
            .then_with(|| left.key.session_id.cmp(&right.key.session_id))
            .then_with(|| left.metric.cmp(&right.metric))
    });
    Ok(allocations)
}

fn cumulative_lane_allocation(
    contributions: Vec<PeriodContribution>,
) -> Option<CumulativeSessionAllocation> {
    let primary_lane = contributions
        .iter()
        .filter(|entry| is_account_primary(entry))
        .map(|entry| entry.allocation.window_id.as_str())
        .min_by_key(|window_id| lane_rank(window_id));
    let fallback_lane = contributions
        .iter()
        .filter(|entry| !is_account_primary(entry))
        .min_by_key(|entry| {
            (
                lane_rank(&entry.allocation.window_id),
                entry.scope_key.as_str(),
                entry.window_role.as_str(),
            )
        })
        .map(|entry| {
            (
                entry.allocation.window_id.as_str(),
                entry.scope_key.as_str(),
                entry.window_role.as_str(),
            )
        });
    let lane = primary_lane.or_else(|| fallback_lane.map(|lane| lane.0))?;

    let primary_intervals: Vec<_> = contributions
        .iter()
        .filter(|entry| is_account_primary(entry))
        .filter_map(period_interval)
        .collect();
    let uses_primary = primary_lane.is_some();
    let selected: Vec<_> = contributions
        .iter()
        .filter(|entry| {
            if is_account_primary(entry) {
                return entry.allocation.window_id == lane;
            }
            let matches_fallback = fallback_lane.is_some_and(|fallback| {
                entry.allocation.window_id == fallback.0
                    && entry.scope_key == fallback.1
                    && entry.window_role == fallback.2
            });
            matches_fallback
                && (!uses_primary
                    || period_interval(entry)
                        .map(|interval| {
                            !primary_intervals
                                .iter()
                                .any(|primary| overlaps(*primary, interval))
                        })
                        .unwrap_or(false))
        })
        .collect();
    let first = selected.first()?.allocation.clone();
    let mut period_ids = std::collections::HashSet::new();
    let mut partial = false;
    let mut percent = 0.0;
    for entry in selected {
        period_ids.insert(entry.period_id);
        partial |= entry.allocation.partial || !is_account_primary(entry);
        percent += entry.allocation.percent;
    }
    percent.is_finite().then_some(CumulativeSessionAllocation {
        key: first.key,
        wsl_distro: first.wsl_distro,
        metric: first.metric,
        provider: first.provider,
        account_key: first.account_key,
        window_id: lane.to_string(),
        percent,
        partial,
        period_count: period_ids.len().try_into().unwrap_or(u32::MAX),
    })
}

fn is_account_primary(entry: &PeriodContribution) -> bool {
    entry.scope_key == "account"
        && matches!(entry.window_role.as_str(), "primaryShort" | "primaryLong")
}

fn lane_rank(window_id: &str) -> (u8, &str) {
    let rank = match window_id {
        "seven-day" | "five-hour" => 0,
        id if id.ends_with("-10080m") || id.ends_with("-300m") => 1,
        _ => 2,
    };
    (rank, window_id)
}

fn period_interval(entry: &PeriodContribution) -> Option<(i64, i64)> {
    let reset = entry.resets_at_epoch?;
    let duration = entry.duration_seconds.unwrap_or_else(|| {
        if entry.allocation.metric == "weekly" {
            7 * 86_400
        } else {
            5 * 3_600
        }
    });
    let start = entry
        .starts_at_epoch
        .unwrap_or_else(|| reset.saturating_sub(duration));
    (start < reset).then_some((start, reset))
}

fn overlaps(left: (i64, i64), right: (i64, i64)) -> bool {
    left.0 < right.1 && right.0 < left.1
}

pub(crate) fn enqueue_in(
    connection: &Transaction<'_>,
    period_ids: &[i64],
    requested_at_epoch: i64,
) -> Result<()> {
    if period_ids.is_empty() {
        return Ok(());
    }
    connection.execute(
        "UPDATE provider_usage_allocation_revision SET value = value + 1 WHERE id = 1",
        [],
    )?;
    let generation = connection.query_row(
        "SELECT value FROM provider_usage_allocation_revision WHERE id = 1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    for period_id in period_ids {
        connection.execute(
            "INSERT INTO provider_usage_allocation_dirty (period_id, requested_at_epoch, generation)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(period_id) DO UPDATE SET
                 requested_at_epoch = MIN(
                     provider_usage_allocation_dirty.requested_at_epoch,
                     excluded.requested_at_epoch
                 ),
                 generation = excluded.generation",
            params![period_id, requested_at_epoch, generation],
        )?;
    }
    Ok(())
}

/// Queue only provider periods that published turns in `key` can affect.
pub(crate) fn enqueue_session_periods_in(
    connection: &Transaction<'_>,
    key: &SessionKey,
    requested_at_epoch: i64,
) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT p.id
           FROM provider_usage_period p
           JOIN session_provider_account spa
             ON spa.environment_key = ?1 AND spa.agent = ?2 AND spa.session_id = ?3
            AND spa.provider = p.provider AND spa.account_key = p.account_key
          WHERE p.resets_at_epoch IS NOT NULL
            AND EXISTS (
                SELECT 1
                  FROM turn t
                  JOIN session_evidence e
                    ON e.environment_key = t.environment_key
                   AND e.agent = t.agent AND e.session_id = t.session_id
                   AND e.published_fence = t.claim_fence
                 WHERE t.environment_key = ?1 AND t.agent = ?2 AND t.session_id = ?3
                   AND t.ts_ms >= (
                       COALESCE(
                           p.starts_at_epoch,
                           p.resets_at_epoch - CASE p.window_kind
                               WHEN 'weekly' THEN 604800 ELSE 18000 END
                       ) * 1000
                   )
                   AND t.ts_ms < p.resets_at_epoch * 1000
            )
         UNION
         SELECT DISTINCT a.period_id
           FROM provider_usage_session_allocation a
          WHERE a.environment_key = ?1 AND a.agent = ?2 AND a.session_id = ?3",
    )?;
    let period_ids = statement
        .query_map(
            params![key.environment_key, key.agent, key.session_id],
            |row| row.get::<_, i64>(0),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    enqueue_in(connection, &period_ids, requested_at_epoch)
}

fn replace_in(
    connection: &Transaction<'_>,
    period_id: i64,
    allocations: &[SessionPeriodAllocation],
    computed_at_epoch: i64,
) -> Result<()> {
    connection.execute(
        "DELETE FROM provider_usage_session_allocation WHERE period_id = ?1",
        [period_id],
    )?;
    for allocation in allocations {
        connection.execute(
            "INSERT INTO provider_usage_session_allocation (
                    period_id, environment_key, agent, session_id, metric,
                    percent, basis, partial, computed_at_epoch
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                period_id,
                allocation.key.environment_key,
                allocation.key.agent,
                allocation.key.session_id,
                allocation.metric,
                allocation.percent,
                allocation.basis,
                i64::from(allocation.partial),
                computed_at_epoch,
            ],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::store::{SessionRecord, Store};

    fn contribution(
        period_id: i64,
        window_id: &str,
        role: &str,
        scope: &str,
        start: i64,
        reset: i64,
        percent: f64,
    ) -> PeriodContribution {
        PeriodContribution {
            allocation: CumulativeSessionAllocation {
                key: SessionKey::new("environment", "codex", "session"),
                wsl_distro: None,
                metric: "weekly".to_string(),
                provider: "openai".to_string(),
                account_key: "account".to_string(),
                window_id: window_id.to_string(),
                percent,
                partial: false,
                period_count: 1,
            },
            period_id,
            starts_at_epoch: Some(start),
            resets_at_epoch: Some(reset),
            duration_seconds: Some(reset - start),
            window_role: role.to_string(),
            scope_key: scope.to_string(),
        }
    }

    #[test]
    fn sums_adjacent_primary_periods_in_one_canonical_lane() {
        let allocation = cumulative_lane_allocation(vec![
            contribution(1, "seven-day", "primaryLong", "account", 0, 100, 12.5),
            contribution(2, "seven-day", "primaryLong", "account", 100, 200, 7.5),
        ])
        .expect("primary periods allocate");

        assert_eq!(allocation.percent, 20.0);
        assert_eq!(allocation.period_count, 2);
        assert!(!allocation.partial);
    }

    #[test]
    fn scoped_period_that_bridges_primary_resets_does_not_add_capacity() {
        let allocation = cumulative_lane_allocation(vec![
            contribution(1, "seven-day", "primaryLong", "account", 0, 100, 12.5),
            contribution(2, "seven-day", "primaryLong", "account", 100, 200, 7.5),
            contribution(
                3,
                "model-weekly",
                "supplemental",
                "model:gpt-5",
                0,
                200,
                80.0,
            ),
        ])
        .expect("primary periods allocate");

        assert_eq!(allocation.percent, 20.0);
        assert_eq!(allocation.period_count, 2);
    }

    #[test]
    fn primary_period_replaces_an_overlapping_fallback_idempotently() {
        let contributions = vec![
            contribution(
                1,
                "model-weekly",
                "supplemental",
                "model:gpt-5",
                0,
                100,
                80.0,
            ),
            contribution(2, "seven-day", "primaryLong", "account", 0, 100, 12.5),
        ];

        let first = cumulative_lane_allocation(contributions.clone()).expect("allocation");
        let second = cumulative_lane_allocation(contributions).expect("allocation");
        assert_eq!(first.percent, 12.5);
        assert_eq!(first.period_count, 1);
        assert_eq!(first, second);
    }

    #[test]
    fn changed_session_requeues_a_period_it_previously_contributed_to() {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-ledger-test")).unwrap();
        let key = SessionKey::new("native", "claude-code", "session");
        store
            .upsert_sessions(
                &[SessionRecord {
                    key: key.clone(),
                    source_kind: "inline".to_string(),
                    source_label: "test".to_string(),
                    wsl_distro: None,
                    title: None,
                    title_source: None,
                    cwd: None,
                    surface: "unknown".to_string(),
                    updated_at_epoch: Some(100),
                    activity_cursor: "test".to_string(),
                    activity_source: "event".to_string(),
                    subagent_count: 0,
                    fork_parent_session_id: None,
                    source_fingerprint: Some("test".to_string()),
                }],
                &[],
            )
            .unwrap();
        {
            let connection = store.lock();
            connection
                .execute(
                    "INSERT INTO provider_usage_period (
                         provider, account_key, window_id, window_kind, window_role,
                         scope_key, scope_label, duration_seconds, starts_at_epoch,
                         resets_at_epoch, first_observed_epoch, last_observed_epoch
                     ) VALUES ('anthropic', 'account', 'five-hour', 'rolling', 'primaryShort',
                               'account', 'account', 18000, 0, 18000, 1, 1)",
                    [],
                )
                .unwrap();
        }
        store
            .replace_provider_usage_period_allocations(
                1,
                &[SessionPeriodAllocation {
                    key: key.clone(),
                    metric: "fiveHour".to_string(),
                    percent: 10.0,
                    basis: "tokens".to_string(),
                    partial: false,
                }],
                100,
            )
            .unwrap();
        {
            let mut connection = store.lock();
            let tx = connection.transaction().unwrap();
            enqueue_session_periods_in(&tx, &key, 101).unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            store
                .provider_usage_allocation_dirty_periods(8)
                .unwrap()
                .len(),
            1
        );
    }
}

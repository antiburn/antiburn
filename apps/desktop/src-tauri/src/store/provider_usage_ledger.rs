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

    /// Read at most `limit` dirty periods without removing their durable work.
    pub fn provider_usage_allocation_dirty_periods(&self, limit: usize) -> Result<Vec<DirtyPeriod>> {
        let limit = i64::try_from(limit.clamp(1, 32)).expect("bounded limit fits i64");
        let connection = self.lock();
        let mut statement = connection.prepare(
            "SELECT period_id, generation FROM provider_usage_allocation_dirty
              ORDER BY requested_at_epoch, period_id LIMIT ?1",
        )?;
        let ids = statement
            .query_map([limit], |row| {
                Ok(DirtyPeriod { period_id: row.get(0)?, generation: row.get(1)? })
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
        let current = tx
            .query_row(
                "SELECT generation FROM provider_usage_allocation_dirty WHERE period_id = ?1",
                [period_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        if current != Some(generation) {
            tx.commit()?;
            return Ok(());
        }
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

    /// Read materialized totals for the sessions currently displayed.
    pub fn cumulative_session_limit_allocations(
        &self,
        keys: &[SessionKey],
    ) -> Result<Vec<CumulativeSessionAllocation>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let connection = self.lock();
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
                    p.provider, p.account_key, p.window_id,
                    SUM(a.percent), MAX(a.partial), COUNT(DISTINCT a.period_id)
               FROM provider_usage_session_allocation a
               JOIN provider_usage_period p ON p.id = a.period_id
               JOIN session s ON s.environment_key = a.environment_key
                 AND s.agent = a.agent AND s.session_id = a.session_id
              WHERE {}
              GROUP BY a.environment_key, a.agent, a.session_id, a.metric,
                       s.wsl_distro, p.provider, p.account_key, p.window_id",
            clauses.join(" OR ")
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement
            .query_map(params_from_iter(values), |row| {
                Ok(CumulativeSessionAllocation {
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
                    period_count: row.get::<_, i64>(10)?.try_into().unwrap_or(u32::MAX),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let mut best: HashMap<(SessionKey, String), CumulativeSessionAllocation> = HashMap::new();
        for allocation in rows {
            let key = (allocation.key.clone(), allocation.metric.clone());
            if best.get(&key).is_none_or(|current| allocation.percent > current.percent) {
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
}

pub(crate) fn enqueue_in(
    connection: &Transaction<'_>,
    period_ids: &[i64],
    requested_at_epoch: i64,
) -> Result<()> {
    for period_id in period_ids {
        connection.execute(
            "INSERT INTO provider_usage_allocation_dirty (period_id, requested_at_epoch)
             VALUES (?1, ?2)
             ON CONFLICT(period_id) DO UPDATE SET
                 requested_at_epoch = excluded.requested_at_epoch,
                 generation = provider_usage_allocation_dirty.generation + 1",
            params![period_id, requested_at_epoch],
        )?;
    }
    Ok(())
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

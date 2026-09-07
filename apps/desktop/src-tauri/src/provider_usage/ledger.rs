//! Bounded reconciliation of durable provider periods into session totals.

use crate::provider_usage::allocation;
use crate::store::Store;

/// The number of periods one refresh may materialize.
///
/// Each period query is constrained to its own half-open time range. The
/// limit keeps a launch or a delayed provider response from monopolizing the
/// store lock; unclaimed rows remain in the durable queue for the next pass.
const PERIOD_BATCH: usize = 8;

/// Queue changed provider periods and materialize one bounded batch.
pub fn enqueue_and_reconcile(store: &Store, period_ids: &[i64], now_epoch: i64) {
    if let Err(error) = store.enqueue_provider_usage_allocation_periods(period_ids, now_epoch) {
        ::tracing::warn!(event = "provider_usage_allocation_enqueue_failed", error = %error);
        return;
    }
    reconcile(store, now_epoch);
}

/// Materialize a small durable queue batch outside the popover read path.
pub fn reconcile(store: &Store, now_epoch: i64) {
    let Ok(periods) = store.provider_usage_allocation_dirty_periods(PERIOD_BATCH) else {
        ::tracing::warn!(event = "provider_usage_allocation_claim_failed");
        return;
    };
    for dirty in periods {
        let result = reconcile_period(store, dirty, now_epoch);
        if let Err(error) = result {
            ::tracing::warn!(event = "provider_usage_allocation_reconcile_failed", error = %error);
        }
    }
}

fn reconcile_period(
    store: &Store,
    dirty: crate::store::provider_usage_ledger::DirtyPeriod,
    now_epoch: i64,
) -> anyhow::Result<()> {
    let period_id = dirty.period_id;
    let Some(history) = store.provider_usage_period_history(period_id)? else {
        return Ok(());
    };
    let Some(reset) = history.period.resets_at_epoch else {
        return Ok(());
    };
    let duration = history.period.duration_seconds.unwrap_or_else(|| {
        if history.period.window_kind == "weekly" {
            7 * 86_400
        } else {
            5 * 3_600
        }
    });
    let start = history.period.starts_at_epoch.unwrap_or_else(|| reset.saturating_sub(duration));
    let turns = store.session_usage_turns_between(
        start.saturating_mul(1_000),
        reset.saturating_mul(1_000),
    )?;
    let Some((_, mut allocations)) = allocation::estimate_period(turns, &history) else {
        return Ok(());
    };
    if history
        .observations
        .iter()
        .any(|observation| observation.source_id.ends_with("-backfill"))
    {
        for allocation in &mut allocations {
            allocation.partial = true;
        }
    }
    store.replace_provider_usage_period_allocations_and_ack(
        period_id,
        dirty.generation,
        &allocations,
        now_epoch,
    )
}

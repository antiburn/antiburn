//! Bounded reconciliation of durable provider periods into session totals.

use crate::provider_usage::allocation;
use crate::store::Store;

/// The number of periods one refresh may materialize.
///
/// Each period query is constrained to its own half-open time range. The
/// limit keeps a launch or a delayed provider response from monopolizing the
/// store lock; unclaimed rows remain in the durable queue for the next pass.
const PERIOD_BATCH: usize = 8;
const MAX_OBSERVATION_INTERVALS: usize = 96;
const MAX_TURN_GROUPS: usize = 50_000;

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
    if store.provider_usage_period_allocation_frozen(period_id)? {
        return store
            .retain_partial_provider_usage_period_allocations_and_ack(period_id, dirty.generation);
    }
    let Some(history) =
        store.provider_usage_period_history_for_allocation(period_id, MAX_OBSERVATION_INTERVALS)?
    else {
        return store.acknowledge_provider_usage_allocation_period(period_id, dirty.generation);
    };
    if history.observations.is_empty() {
        return store.replace_provider_usage_period_allocations_and_ack(
            period_id,
            dirty.generation,
            &[],
            now_epoch,
        );
    }
    let Some(reset) = history.period.resets_at_epoch else {
        return store.replace_provider_usage_period_allocations_and_ack(
            period_id,
            dirty.generation,
            &[],
            now_epoch,
        );
    };
    let duration = history.period.duration_seconds.unwrap_or_else(|| {
        if history.period.window_kind == "weekly" {
            7 * 86_400
        } else {
            5 * 3_600
        }
    });
    let start = history
        .period
        .starts_at_epoch
        .unwrap_or_else(|| reset.saturating_sub(duration));
    let interval_ends =
        allocation::period_observation_interval_ends(&history, MAX_OBSERVATION_INTERVALS);
    let Some(observed_at_ms) = interval_ends.last().copied() else {
        return store.replace_provider_usage_period_allocations_and_ack(
            period_id,
            dirty.generation,
            &[],
            now_epoch,
        );
    };
    let end_ms = observed_at_ms.saturating_add(1);
    let Some(turns) = store.session_usage_turns_grouped_between(
        start.saturating_mul(1_000),
        end_ms,
        &interval_ends,
        MAX_TURN_GROUPS,
    )?
    else {
        ::tracing::warn!(
            event = "provider_usage_allocation_group_limit",
            period_id,
            max_groups = MAX_TURN_GROUPS
        );
        return store
            .retain_partial_provider_usage_period_allocations_and_ack(period_id, dirty.generation);
    };
    let Some((_, mut allocations)) = allocation::estimate_period(turns, &history) else {
        return store.replace_provider_usage_period_allocations_and_ack(
            period_id,
            dirty.generation,
            &[],
            now_epoch,
        );
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

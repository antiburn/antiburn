//! The Overview allowance snapshot from shared quota periods and refusals.

use std::collections::BTreeMap;

use tauri::Manager;

use super::{CommandResult, fail, run_blocking};
use crate::dto::{
    AllowanceDay, AllowanceOverage, AllowanceUsageAccount, AllowanceUsageSummary,
    AllowanceUtilization, QuotaPeriodPayload, QuotaUsageRequest,
};
use crate::provider_usage;
use crate::store::Store;

const OVERAGE_SPAN_DAYS: i64 = 30;
const UTILIZATION_SPAN_DAYS: u32 = 60;
const WEEKLY_LEAD_SECS: i64 = 7 * provider_usage::DAY;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AccountLane {
    Weekly,
    FiveHour,
}

fn account_lane(lane: &str) -> Option<AccountLane> {
    match lane {
        crate::store::provider_limit::LANE_WEEKLY => Some(AccountLane::Weekly),
        crate::store::provider_limit::LANE_FIVE_HOUR => Some(AccountLane::FiveHour),
        _ => None,
    }
}

#[tauri::command]
pub async fn get_allowance_usage(
    app: tauri::AppHandle,
    utc_offset_minutes: Option<i32>,
) -> CommandResult<AllowanceUsageSummary> {
    let now = crate::scan::unix_now();
    let store = app.state::<Store>().inner().clone();
    let offset_minutes = utc_offset_minutes.unwrap_or(0);
    run_blocking(move || allowance_usage_for_store(&store, now, offset_minutes)).await
}

pub(super) fn allowance_usage_for_store(
    store: &Store,
    now: i64,
    offset_minutes: i32,
) -> CommandResult<AllowanceUsageSummary> {
    let bounds = provider_usage::window_bounds(now, offset_minutes);
    let offset = provider_usage::local_offset(offset_minutes);
    let series_start = bounds.previous_30_days_start;
    let incidents = store.quota_incidents(series_start).map_err(fail)?;
    let blocks =
        provider_usage::allowance::account_blocks(&incidents, series_start.saturating_mul(1_000));
    let overage_since_ms = now
        .saturating_sub(OVERAGE_SPAN_DAYS * provider_usage::DAY)
        .saturating_mul(1_000);
    let overages = provider_usage::allowance::account_overages(&incidents, overage_since_ms);
    let mut accounts: BTreeMap<(String, String), AllowanceUsageAccount> = BTreeMap::new();
    for account in store.quota_accounts(now).map_err(fail)? {
        let id = (account.provider.clone(), account.account_key.clone());
        let mut long_periods: Vec<QuotaPeriodPayload> = Vec::new();
        let mut short_periods: Vec<QuotaPeriodPayload> = Vec::new();
        for lane in &account.lanes {
            let target = match account_lane(&lane.lane) {
                Some(AccountLane::Weekly) => &mut long_periods,
                Some(AccountLane::FiveHour) => &mut short_periods,
                None => continue,
            };
            let usage = super::quota::quota_usage_for_store(
                store,
                now,
                QuotaUsageRequest {
                    provider: account.provider.clone(),
                    account_key: account.account_key.clone(),
                    lane: lane.lane.clone(),
                    range_start_epoch: series_start.saturating_sub(WEEKLY_LEAD_SECS),
                    range_end_epoch: now.saturating_add(1),
                },
            )?;
            target.extend(usage.periods.into_iter().filter(|period| {
                period.resets_at_epoch > series_start && period.starts_at_epoch <= now
            }));
        }
        let consumption = provider_usage::allowance::consumption(&long_periods);
        let (days, previous_days) = allowance_days(
            &consumption,
            blocks.get(&id).map_or(&[][..], Vec::as_slice),
            &bounds,
            offset,
        );
        let overage = overages.get(&id).cloned().unwrap_or_default();
        accounts.insert(
            id,
            AllowanceUsageAccount {
                provider: account.provider,
                display_name: account.display_name,
                account_key: account.account_key,
                utilization: provider_usage::allowance::utilization(&long_periods, "weekly")
                    .map(utilization_payload),
                burst: provider_usage::allowance::utilization(&short_periods, "rolling")
                    .map(utilization_payload),
                overage: overage_payload(overage),
                days,
                previous_days,
            },
        );
    }
    for (id, overage) in overages {
        accounts.entry(id.clone()).or_insert_with(|| {
            let (days, previous_days) = allowance_days(
                &provider_usage::allowance::AccountConsumption::default(),
                blocks.get(&id).map_or(&[][..], Vec::as_slice),
                &bounds,
                offset,
            );
            AllowanceUsageAccount {
                display_name: provider_usage::providers::display_name(&id.0).to_string(),
                provider: id.0,
                account_key: id.1,
                utilization: None,
                burst: None,
                overage: overage_payload(overage),
                days,
                previous_days,
            }
        });
    }
    Ok(AllowanceUsageSummary {
        accounts: accounts.into_values().collect(),
        overage_span_days: OVERAGE_SPAN_DAYS as u32,
        utilization_span_days: UTILIZATION_SPAN_DAYS,
        generated_at: crate::store::iso_from_epoch(Some(now)),
    })
}

fn overage_payload(overage: provider_usage::allowance::Overage) -> AllowanceOverage {
    AllowanceOverage {
        block_count: u32::try_from(overage.block_count).unwrap_or(u32::MAX),
        waited_seconds: overage.waited_ms / 1_000,
        blocks_without_wait: u32::try_from(overage.blocks_without_wait).unwrap_or(u32::MAX),
        last_block_at: overage
            .last_block_at_ms
            .map(|at_ms| crate::store::iso_from_epoch(Some(at_ms / 1_000))),
    }
}

fn allowance_days(
    consumption: &provider_usage::allowance::AccountConsumption,
    blocks: &[provider_usage::allowance::Block],
    bounds: &provider_usage::WindowBounds,
    offset: time::UtcOffset,
) -> (Vec<AllowanceDay>, Vec<AllowanceDay>) {
    const SLOTS: usize = 2 * provider_usage::SERIES_DAYS;
    let start = bounds.previous_30_days_start;
    let slot_of = |epoch: i64| -> Option<usize> {
        usize::try_from((epoch - start).div_euclid(provider_usage::DAY))
            .ok()
            .filter(|slot| *slot < SLOTS)
    };
    let mut consumed = [0.0_f64; SLOTS];
    let mut known = [false; SLOTS];
    let mut block_counts = [0_u32; SLOTS];
    for entry in &consumption.consumed {
        if let Some(slot) = slot_of(entry.at_epoch) {
            consumed[slot] += entry.percent;
        }
    }
    for (slot, known) in known.iter_mut().enumerate() {
        let day_start = start + slot as i64 * provider_usage::DAY;
        *known = consumption.covers(day_start, day_start + provider_usage::DAY - 1);
    }
    for block in blocks {
        if let Some(slot) = slot_of(block.started_at_ms / 1_000) {
            block_counts[slot] = block_counts[slot].saturating_add(1);
        }
    }
    let day = |slot: usize| AllowanceDay {
        local_date: provider_usage::local_date(start + slot as i64 * provider_usage::DAY, offset),
        used_percent: known[slot].then_some(consumed[slot]),
        block_count: block_counts[slot],
    };
    let previous_days = (0..provider_usage::SERIES_DAYS).map(day).collect();
    let days = (provider_usage::SERIES_DAYS..SLOTS).map(day).collect();
    (days, previous_days)
}

fn utilization_payload(
    utilization: provider_usage::allowance::Utilization,
) -> AllowanceUtilization {
    AllowanceUtilization {
        typical_percent: utilization.typical_percent,
        peak_percent: utilization.peak_percent,
        average_percent: utilization.average_percent,
        period_count: u32::try_from(utilization.period_count).unwrap_or(u32::MAX),
        maxed_period_count: u32::try_from(utilization.maxed_period_count).unwrap_or(u32::MAX),
        window_kind: utilization.window_kind,
        first_period_at: crate::store::iso_from_epoch(Some(utilization.first_period_at_epoch)),
        last_period_at: crate::store::iso_from_epoch(Some(utilization.last_period_at_epoch)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_scoped_lane_does_not_enter_account_allowance() {
        assert_eq!(account_lane("weekly"), Some(AccountLane::Weekly));
        assert_eq!(account_lane("fiveHour"), Some(AccountLane::FiveHour));
        assert_eq!(account_lane("model:fable"), None);
    }

    #[test]
    fn shared_bucket_days_follow_the_overview_local_calendar() {
        let now = 70 * provider_usage::DAY;
        let bounds = provider_usage::window_bounds(now, 0);
        let first = bounds.previous_30_days_start;
        let consumption = provider_usage::allowance::AccountConsumption {
            consumed: vec![
                provider_usage::allowance::Consumed {
                    at_epoch: first + provider_usage::DAY,
                    percent: 12.0,
                },
                provider_usage::allowance::Consumed {
                    at_epoch: first + 30 * provider_usage::DAY,
                    percent: 25.0,
                },
            ],
            covered: vec![
                (first + provider_usage::DAY, first + provider_usage::DAY),
                (
                    first + 30 * provider_usage::DAY,
                    first + 30 * provider_usage::DAY,
                ),
            ],
        };
        let (days, previous) =
            allowance_days(&consumption, &[], &bounds, provider_usage::local_offset(0));
        assert_eq!(previous[0].used_percent, None);
        assert_eq!(previous[1].used_percent, Some(12.0));
        assert_eq!(days[0].used_percent, Some(25.0));
        assert_eq!(days[1].used_percent, None);
    }
}

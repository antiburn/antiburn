//! The Overview allowance chart: one account's rolling subscription
//! utilization, built from the same shared quota periods Limits uses.

use std::time::Instant;

use tauri::Manager;

use super::{CommandResult, fail, log_overview_read_timing, run_blocking};
use crate::UiReadStore;
use crate::dto::{
    AllowanceChart, AllowanceLevelPoint, AllowanceRollingPoint, AllowanceUsageAccount,
    AllowanceUsageSummary, AllowanceUtilization, AllowanceWindowLevels, AllowanceWindowPeak,
    QuotaPeriodPayload, QuotaUsageRequest,
};
use crate::provider_usage;
use crate::provider_usage::allowance::{
    POOL_SPAN_SECS, PoolWindow, SHORT_POOL_WEIGHT, WEEKLY_POOL_WEIGHT,
};
use crate::store::Store;

/// The trailing span the headline `utilization` pools, and the visible
/// chart's own width.
const UTILIZATION_SPAN_DAYS: u32 = 28;

/// How far the fetch reaches behind the visible chart range: the rolling
/// pool's own 28-day span, plus the seven days a weekly window can run
/// before the close that would land it inside that span. Provably covers
/// what [`provider_usage::allowance::rolling_utilization`] needs, because the
/// fetch filter below uses this same constant as its lower bound.
const ROLLING_LOOKBACK_SECS: i64 = POOL_SPAN_SECS + 7 * provider_usage::DAY;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AccountLane {
    Weekly,
    FiveHour,
    /// A supplemental per-model weekly window, such as Anthropic's "Fable"
    /// limit. It draws as its own weekly-style area and pools into the
    /// headline alongside the two account-wide lanes.
    Model,
}

fn account_lane(lane: &str) -> Option<AccountLane> {
    match lane {
        crate::store::provider_limit::LANE_WEEKLY => Some(AccountLane::Weekly),
        crate::store::provider_limit::LANE_FIVE_HOUR => Some(AccountLane::FiveHour),
        lane if lane.starts_with(crate::store::provider_limit::MODEL_LANE_PREFIX) => {
            Some(AccountLane::Model)
        }
        _ => None,
    }
}

#[tauri::command]
pub async fn get_allowance_usage(
    app: tauri::AppHandle,
    utc_offset_minutes: Option<i32>,
) -> CommandResult<AllowanceUsageSummary> {
    let now = crate::scan::unix_now();
    let store = app.state::<UiReadStore>().0.clone();
    let offset_minutes = utc_offset_minutes.unwrap_or(0);
    run_blocking(move || {
        let started = Instant::now();
        let result = allowance_usage_for_store(&store, now, offset_minutes);
        log_overview_read_timing("get_allowance_usage", started.elapsed());
        result
    })
    .await
}

pub(super) fn allowance_usage_for_store(
    store: &Store,
    now: i64,
    offset_minutes: i32,
) -> CommandResult<AllowanceUsageSummary> {
    let bounds = provider_usage::window_bounds(now, offset_minutes);
    let range_start = bounds.last_30_days_start;
    let fetch_start = range_start - ROLLING_LOOKBACK_SECS;
    let mut accounts = Vec::new();
    let quota_accounts = store.quota_accounts(now).map_err(fail)?;
    let mut turn_input = None;
    // One scan for every account's five-hour lane, instead of the full
    // range-and-resolve `attributed_turn_epochs` query this request would
    // otherwise run once per account. Deferred until the first five-hour
    // lane needs it, so a request with no five-hour lane skips the scan.
    let mut turn_minutes: Option<crate::store::provider_limit::TurnMinutes> = None;
    for account in quota_accounts {
        let input = match &mut turn_input {
            Some(input) => input,
            empty => empty.insert(
                store
                    .quota_turn_input(fetch_start, now.saturating_add(1))
                    .map_err(fail)?
                    .ok_or_else(|| "too much turn activity in this range".to_string())?,
            ),
        };
        let dollars = input.for_account(&account.provider, &account.account_key, None);
        let mut weekly_periods: Vec<QuotaPeriodPayload> = Vec::new();
        let mut short_periods: Vec<QuotaPeriodPayload> = Vec::new();
        // Each model-scoped lane keeps its own slug, since a multi-model
        // account draws one area per model rather than one pooled area.
        let mut model_periods: Vec<(String, QuotaPeriodPayload)> = Vec::new();
        for lane in &account.lanes {
            let Some(kind) = account_lane(&lane.lane) else {
                continue;
            };
            if lane.lane == crate::store::provider_limit::LANE_FIVE_HOUR && turn_minutes.is_none() {
                turn_minutes = Some(
                    store
                        .attributed_turn_minutes(fetch_start, now.saturating_add(1))
                        .map_err(fail)?,
                );
            }
            let usage = super::quota::quota_usage_with_turn_dollars(
                store,
                now,
                QuotaUsageRequest {
                    provider: account.provider.clone(),
                    account_key: account.account_key.clone(),
                    lane: lane.lane.clone(),
                    range_start_epoch: fetch_start,
                    range_end_epoch: now.saturating_add(1),
                },
                &dollars,
                turn_minutes.as_ref(),
            )?;
            let periods = usage.periods.into_iter().filter(|period| {
                period.resets_at_epoch > fetch_start && period.starts_at_epoch <= now
            });
            match kind {
                AccountLane::Weekly => weekly_periods.extend(periods),
                AccountLane::FiveHour => short_periods.extend(periods),
                AccountLane::Model => {
                    let lane_id = lane.lane.clone();
                    model_periods.extend(periods.map(|period| (lane_id.clone(), period)));
                }
            }
        }

        let pool_windows: Vec<PoolWindow> = weekly_periods
            .iter()
            .chain(model_periods.iter().map(|(_, period)| period))
            .filter_map(|period| pool_window(period, now, WEEKLY_POOL_WEIGHT))
            .chain(
                short_periods
                    .iter()
                    .filter_map(|period| pool_window(period, now, SHORT_POOL_WEIGHT)),
            )
            .collect();
        let rolling =
            provider_usage::allowance::rolling_utilization(&pool_windows, now, range_start);
        let utilization =
            rolling
                .last()
                .and_then(|last| last.percent)
                .map(|percent| AllowanceUtilization {
                    utilization_percent: percent,
                    weekly_window_count: pooled_window_count(weekly_periods.iter(), now),
                    short_window_count: pooled_window_count(short_periods.iter(), now),
                    model_window_count: pooled_window_count(
                        model_periods.iter().map(|(_, period)| period),
                        now,
                    ),
                });

        let short_windows: Vec<AllowanceWindowPeak> = short_periods
            .iter()
            .filter(|period| overlaps_visible_range(period, range_start, now))
            .filter_map(|period| {
                provider_usage::allowance::window_peak(period).map(|peak_percent| {
                    AllowanceWindowPeak {
                        starts_at_epoch: period.starts_at_epoch,
                        resets_at_epoch: period.resets_at_epoch,
                        peak_percent,
                    }
                })
            })
            .collect();

        let weekly_windows: Vec<AllowanceWindowLevels> = weekly_periods
            .iter()
            .map(|period| ("weekly".to_string(), period))
            .chain(
                model_periods
                    .iter()
                    .map(|(lane, period)| (lane.clone(), period)),
            )
            .filter(|(_, period)| overlaps_visible_range(period, range_start, now))
            .filter_map(|(lane, period)| {
                provider_usage::allowance::weekly_levels(period, now).map(|points| {
                    AllowanceWindowLevels {
                        lane,
                        starts_at_epoch: period.starts_at_epoch,
                        resets_at_epoch: period.resets_at_epoch,
                        points: points
                            .into_iter()
                            .map(|point| AllowanceLevelPoint {
                                at_epoch: point.at_epoch,
                                percent: point.percent,
                            })
                            .collect(),
                    }
                })
            })
            .collect();

        let plan = super::quota::account_plan(
            store,
            &account.provider,
            &account.account_key,
            &account.lanes,
        )?;
        accounts.push(AllowanceUsageAccount {
            provider: account.provider,
            display_name: account.display_name,
            account_key: account.account_key,
            plan,
            utilization,
            chart: AllowanceChart {
                short_windows,
                weekly_windows,
                rolling: rolling
                    .into_iter()
                    .map(|point| AllowanceRollingPoint {
                        at_epoch: point.at_epoch,
                        percent: point.percent,
                    })
                    .collect(),
            },
        });
    }
    Ok(AllowanceUsageSummary {
        accounts,
        utilization_span_days: UTILIZATION_SPAN_DAYS,
        range_start_epoch: range_start,
        range_end_epoch: now,
        generated_at: crate::store::iso_from_epoch(Some(now)),
    })
}

/// A period as the rolling pool reduces it: its start, its close, its own
/// [`provider_usage::allowance::window_peak`], whether it is still open at
/// `now`, and how many times its percent counts in the pool. `None` when the
/// period has no estimate to pool.
fn pool_window(period: &QuotaPeriodPayload, now: i64, weight: usize) -> Option<PoolWindow> {
    provider_usage::allowance::window_peak(period).map(|percent| PoolWindow {
        starts_at_epoch: period.starts_at_epoch,
        resets_at_epoch: period.resets_at_epoch,
        percent,
        open: period.resets_at_epoch > now,
        weight,
    })
}

/// How many of `periods` belong to the headline's pool at `now`, by the same
/// rule [`provider_usage::allowance::rolling_utilization`] uses for its last
/// point — so a lane's count and its share of the pooled figure never
/// disagree.
fn pooled_window_count<'a>(periods: impl Iterator<Item = &'a QuotaPeriodPayload>, now: i64) -> u32 {
    periods
        .filter(|period| {
            period.estimated_percent.is_some()
                && provider_usage::allowance::pooled_at(
                    period.resets_at_epoch,
                    period.resets_at_epoch > now,
                    now,
                    now,
                )
        })
        .count() as u32
}

/// Whether a window belongs on the visible chart: it overlaps
/// `[range_start, now]`, the same rule the old day series used.
fn overlaps_visible_range(period: &QuotaPeriodPayload, range_start: i64, now: i64) -> bool {
    period.resets_at_epoch > range_start && period.starts_at_epoch <= now
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use rusqlite::params;

    use super::*;

    const PROVIDER: &str = "anthropic";
    const ACCOUNT_KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn model_scoped_lane_classifies_for_pooling() {
        assert_eq!(account_lane("weekly"), Some(AccountLane::Weekly));
        assert_eq!(account_lane("fiveHour"), Some(AccountLane::FiveHour));
        // A model-scoped lane classifies on its own, so it never lands in
        // the weekly or five-hour vec that the chart's own layers read.
        assert_eq!(account_lane("model:fable"), Some(AccountLane::Model));
        assert_ne!(account_lane("model:fable"), account_lane("weekly"));
        assert_eq!(account_lane("unknown"), None);
    }

    /// An account with only a weekly period has no five-hour lane, so the
    /// shared turn-minutes scan has nothing to fill. The request still
    /// resolves and reports an empty five-hour payload for that account,
    /// instead of failing or fabricating a short window.
    #[test]
    fn no_five_hour_lane_returns_empty_short_windows_without_a_scan() {
        let store = Store::open_in_memory(Path::new("/tmp/antiburn-usage-commands-test"))
            .expect("opens store");
        let now = 1_800_000_000_i64;
        let resets_at = now + 7 * 24 * 60 * 60;
        store
            .lock()
            .execute(
                "INSERT INTO provider_usage_period (
                     provider, account_key, window_id, window_kind, window_role,
                     scope_key, scope_label, duration_seconds, starts_at_epoch,
                     resets_at_epoch, first_observed_epoch, last_observed_epoch
                 ) VALUES (?1, ?2, 'weekly', 'weekly', 'primaryLong',
                           'account', 'account', NULL, NULL, ?3, ?3, ?3)",
                params![PROVIDER, ACCOUNT_KEY, resets_at],
            )
            .expect("inserts a synthetic weekly period");

        let summary = allowance_usage_for_store(&store, now, 0).expect("resolves the summary");

        assert_eq!(summary.accounts.len(), 1);
        assert!(summary.accounts[0].chart.short_windows.is_empty());
    }
}

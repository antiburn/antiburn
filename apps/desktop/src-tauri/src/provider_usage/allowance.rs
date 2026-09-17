//! Reduce observed evidence to the two allowance figures the Overview shows.
//!
//! # Two numbers, neither derived from the other
//!
//! **Utilization** is supply consumed: the share of the plan's allowance the
//! provider's own meter reports. **Overage** is demand refused: how often the
//! provider turned a request away, and how long the reader then waited. A
//! week can close at 62% and still contain a refusal, because the refusal
//! came from a different, shorter window. So the surface states both.
//!
//! # What counts as a refusal
//!
//! Only the provider saying it refused a request. A meter reading of 100% is
//! not a refusal. Claude states a refusal as a limit error in the transcript;
//! Codex states one on the `rate_limits` object of a reading.

use std::collections::BTreeMap;

use antiburn_local::analysis::{QuotaIncident, QuotaLimitKind, QuotaResetClock};
use serde::Deserialize;

use crate::store::QuotaIncidentRecord;
use crate::store::provider_usage_history::{ProviderUsagePeriodRollup, ProviderUsageReading};

/// The gap that separates two refusals.
///
/// A refused request is retried, and each retry is refused again while the
/// window stays closed. Every refusal inside this gap belongs to one block.
const STORM_GAP_MS: i64 = 3 * 60 * 60 * 1000;

/// How long the rolling window lasts. Claude calls it the session limit.
const ROLLING_WINDOW_MS: i64 = 5 * 60 * 60 * 1000;

/// How long the weekly window lasts.
const WEEKLY_WINDOW_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// One block: a refusal and the wait it caused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    /// When the provider first refused.
    pub started_at_ms: i64,
    /// When the window reopened, when a refusal in this block states a
    /// reset the window length can hold.
    pub reset_at_ms: Option<i64>,
}

impl Block {
    /// How long the reader waited: the whole block, not the last retry.
    ///
    /// A block that states no usable reset contributes to the count and not
    /// to the wait.
    pub fn waited_ms(&self) -> Option<i64> {
        self.reset_at_ms
            .map(|reset_at_ms| reset_at_ms - self.started_at_ms)
    }
}

/// Every block over one span, already reduced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overage {
    pub block_count: usize,
    /// The total wait across the blocks that state one.
    pub waited_ms: i64,
    /// How many blocks state no usable reset, so state no wait.
    pub blocks_without_wait: usize,
    pub last_block_at_ms: Option<i64>,
}

/// How long the window of `kind` lasts.
///
/// `UsageLimit` names no window, and `RateLimit` is the fallback for text
/// that names none either. Both take the rolling window, the shorter of the
/// two, so a wait that only a weekly window could hold is discarded.
fn window_ms(kind: QuotaLimitKind) -> i64 {
    match kind {
        QuotaLimitKind::Weekly => WEEKLY_WINDOW_MS,
        QuotaLimitKind::RollingWindow
        | QuotaLimitKind::ModelSpecific
        | QuotaLimitKind::WeightedUsage
        | QuotaLimitKind::RateLimit
        | QuotaLimitKind::UsageLimit => ROLLING_WINDOW_MS,
    }
}

/// The instant a stated local reset clock names, after `ts_ms`.
///
/// The text states a wall-clock time and a zone, and no date. The reset is
/// therefore the next time that clock comes around in that zone. A zone the
/// database does not know gives no instant at all.
fn resolve_reset_clock(ts_ms: i64, clock: &QuotaResetClock) -> Option<i64> {
    let zone = jiff::tz::TimeZone::get(&clock.zone).ok()?;
    let at = jiff::Timestamp::from_millisecond(ts_ms)
        .ok()?
        .to_zoned(zone);
    let same_day = at
        .with()
        .hour(i8::try_from(clock.hour).ok()?)
        .minute(i8::try_from(clock.minute).ok()?)
        .second(0)
        .subsec_nanosecond(0)
        .build()
        .ok()?;
    let reset = if same_day > at {
        same_day
    } else {
        same_day.checked_add(jiff::Span::new().days(1)).ok()?
    };
    Some(reset.timestamp().as_millisecond())
}

/// When the window this refusal hit reopens.
///
/// The wait it implies must fit inside the window that refused. A stated
/// clock carries no date, so reading it as the next occurrence gives a
/// 23-hour wait when the time has already passed today. A five-hour window
/// cannot produce that. Such a reset is discarded and never clamped: a
/// clamped value would read as a real five-hour outage.
fn reset_at_ms(incident: &QuotaIncident) -> Option<i64> {
    let reset_ms = match (incident.reset_ts_ms, incident.reset_clock.as_ref()) {
        (Some(reset_ms), _) => reset_ms,
        (None, Some(clock)) => resolve_reset_clock(incident.ts_ms, clock)?,
        (None, None) => return None,
    };
    let waited = reset_ms.checked_sub(incident.ts_ms)?;
    (waited > 0 && waited <= window_ms(incident.limit_kind)).then_some(reset_ms)
}

/// The blocks the refusals at or after `since_ms` make.
///
/// The span drops the refusals before it, not the blocks that start before
/// it. A refusal up to the storm gap before the span merges with the first
/// refusal inside it, and the merged block keeps the earlier start. Dropping
/// that block loses a refusal the reader met inside the span.
fn blocks_since(incidents: &[QuotaIncident], since_ms: i64) -> Vec<Block> {
    blocks_from(
        incidents
            .iter()
            .filter(|incident| incident.ts_ms >= since_ms)
            .collect(),
    )
}

/// Collapse a retry storm into the blocks behind it.
///
/// Incidents arrive from every session, so one block can appear in two
/// transcripts that ran at once. Grouping by time rather than by session is
/// what makes the count a count of blocks.
fn blocks_from(mut sorted: Vec<&QuotaIncident>) -> Vec<Block> {
    sorted.sort_by_key(|incident| (incident.ts_ms, incident.limit_kind));
    // One open block for each limit kind, and the last refusal that kind saw.
    //
    // A five-hour limit and a weekly limit are two different windows. They
    // refuse at their own times and state their own resets, so a refusal of
    // one kind must never extend a block of the other. Merging them let a
    // weekly reset state the wait for a five-hour block.
    let mut open: Vec<(QuotaLimitKind, usize, i64)> = Vec::new();
    let mut blocks: Vec<Block> = Vec::new();
    for incident in sorted {
        let reset_at_ms = reset_at_ms(incident);
        let same_kind = open
            .iter_mut()
            .find(|(kind, _, _)| *kind == incident.limit_kind);
        match same_kind {
            // The gap that opens a new block is the gap between two
            // refusals, not the span since the block started. A window that
            // stays closed longer than the gap is still one block while the
            // retries keep arriving inside it.
            //
            // A retry can state the reset the first refusal missed, so the
            // latest stated reset wins.
            Some((_, index, last_ts_ms)) if incident.ts_ms - *last_ts_ms <= STORM_GAP_MS => {
                let block = &mut blocks[*index];
                block.reset_at_ms = block.reset_at_ms.max(reset_at_ms);
                *last_ts_ms = incident.ts_ms;
            }
            Some((_, index, last_ts_ms)) => {
                *index = blocks.len();
                *last_ts_ms = incident.ts_ms;
                blocks.push(Block {
                    started_at_ms: incident.ts_ms,
                    reset_at_ms,
                });
            }
            None => {
                open.push((incident.limit_kind, blocks.len(), incident.ts_ms));
                blocks.push(Block {
                    started_at_ms: incident.ts_ms,
                    reset_at_ms,
                });
            }
        }
    }
    blocks.sort_by_key(|block| block.started_at_ms);
    blocks
}

/// Reduce every refusal at or after `since_ms` to one overage figure.
pub fn overage(incidents: &[QuotaIncident], since_ms: i64) -> Overage {
    let mut overage = Overage::default();
    for block in blocks_since(incidents, since_ms) {
        overage.block_count += 1;
        match block.waited_ms() {
            Some(waited) => overage.waited_ms += waited,
            None => overage.blocks_without_wait += 1,
        }
        overage.last_block_at_ms = Some(
            overage
                .last_block_at_ms
                .map_or(block.started_at_ms, |last| last.max(block.started_at_ms)),
        );
    }
    overage
}

/// The fewest periods that make a median a typical value.
///
/// The midpoint of two numbers is not typical of anything. Below this count
/// the peak stands on its own, which is honest at any sample size.
pub const TYPICAL_MIN_PERIODS: usize = 6;

/// The figure a window reaches when the provider has nothing left to give.
///
/// A refusal happens here and at nothing less. Across the five-hour windows
/// antiburn has recorded, windows peaking at 96% and 99% refused nothing. So
/// this surface names no warning threshold below it.
pub const MAXED_PERCENT: f64 = 100.0;

/// How much of the allowance the reader consumed, across whole periods.
#[derive(Debug, Clone, PartialEq)]
pub struct Utilization {
    /// The median peak across the periods, or `None` while the sample is
    /// too small for a median to mean anything.
    pub typical_percent: Option<f64>,
    /// The highest figure any one period reached.
    pub peak_percent: f64,
    /// The mean peak across every period the store holds. It answers "how
    /// much of the plan do I use", where the peak answers "can the plan
    /// hold me". An idle period pulls this figure down and moves the
    /// median above far less.
    pub average_percent: f64,
    pub period_count: usize,
    /// How many periods reached [`MAXED_PERCENT`].
    pub maxed_period_count: usize,
    /// The window the periods measure, as the store names it: `weekly`,
    /// `rolling`, or the provider's own word. The reader must know which
    /// window a figure covers, because the windows answer different
    /// questions.
    pub window_kind: String,
    pub first_period_at_epoch: i64,
    pub last_period_at_epoch: i64,
}

/// Reduce one account's periods in one window to its utilization.
///
/// A period the provider never reported a figure for is left out. It is not
/// a zero: an unknown period reads as unknown, never as idle.
pub fn utilization(rollups: &[ProviderUsagePeriodRollup]) -> Option<Utilization> {
    let mut peaks: Vec<(i64, f64, &str)> = rollups
        .iter()
        .filter_map(|rollup| {
            Some((
                rollup.last_observed_epoch,
                rollup.peak_used_percent?,
                rollup.window_kind.as_str(),
            ))
        })
        .collect();
    if peaks.is_empty() {
        return None;
    }
    // The newest period names the window. A provider that renames a window
    // must not make the older name outlive it.
    let window_kind = peaks
        .iter()
        .max_by_key(|entry| entry.0)
        .expect("the list is not empty")
        .2
        .to_owned();
    let total: f64 = peaks.iter().map(|entry| entry.1).sum();
    peaks.sort_by(|left, right| left.1.total_cmp(&right.1));
    let period_count = peaks.len();
    let typical_percent =
        (period_count >= TYPICAL_MIN_PERIODS).then(|| median(&peaks[..], |entry| entry.1));
    Some(Utilization {
        window_kind,
        typical_percent,
        peak_percent: peaks[period_count - 1].1,
        average_percent: total / period_count as f64,
        period_count,
        maxed_period_count: peaks
            .iter()
            .filter(|entry| entry.1 >= MAXED_PERCENT)
            .count(),
        first_period_at_epoch: peaks
            .iter()
            .map(|entry| entry.0)
            .min()
            .expect("the list is not empty"),
        last_period_at_epoch: peaks
            .iter()
            .map(|entry| entry.0)
            .max()
            .expect("the list is not empty"),
    })
}

/// The middle value of an already-sorted list.
///
/// The typical figure is a median and not a mean. An unspent allowance
/// expires, so an idle period is a real zero that drags a mean toward a
/// number no period resembles.
fn median<T>(sorted: &[T], value: impl Fn(&T) -> f64) -> f64 {
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        value(&sorted[middle])
    } else {
        f64::midpoint(value(&sorted[middle - 1]), value(&sorted[middle]))
    }
}

/// The window that measures plan fit: the long one, usually weekly.
const PRIMARY_LONG: &str = "primaryLong";

/// The window that refuses the reader: the short rolling one.
const PRIMARY_SHORT: &str = "primaryShort";

/// The whole-account window. A per-model window measures something else.
const ACCOUNT_SCOPE: &str = "account";

/// One provider account, named the way the store names it.
pub type AccountId = (String, String);

/// Everything one account's allowance numbers rest on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountAllowance {
    /// The long window across periods: how well the plan fits.
    pub utilization: Option<Utilization>,
    /// The short window across periods: what refuses the reader. It is the
    /// cause of the blocks, not a second plan-fit figure.
    pub burst: Option<Utilization>,
    pub overage: Overage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountObservation {
    provider: String,
    account_key: String,
}

/// Reduce the stored rollups and incidents to one figure for each account.
///
/// `since_ms` bounds the blocks only. The utilization figures cover every
/// period the rollups still hold, because "your busiest week" means the
/// busiest week there is, not the busiest inside a trailing window.
pub fn account_allowances(
    rollups: &[ProviderUsagePeriodRollup],
    incidents: &[QuotaIncidentRecord],
    since_ms: i64,
) -> BTreeMap<AccountId, AccountAllowance> {
    let mut accounts: BTreeMap<AccountId, AccountAllowance> = BTreeMap::new();
    let mut lanes: BTreeMap<(AccountId, &str), Vec<ProviderUsagePeriodRollup>> = BTreeMap::new();
    for rollup in rollups
        .iter()
        .filter(|rollup| rollup.scope_key == ACCOUNT_SCOPE)
    {
        let role = match rollup.window_role.as_str() {
            PRIMARY_LONG => PRIMARY_LONG,
            PRIMARY_SHORT => PRIMARY_SHORT,
            _ => continue,
        };
        let id = (rollup.provider.clone(), rollup.account_key.clone());
        lanes.entry((id, role)).or_default().push(rollup.clone());
    }
    for ((id, role), lane) in lanes {
        let entry = accounts.entry(id).or_default();
        match role {
            PRIMARY_LONG => entry.utilization = utilization(&lane),
            _ => entry.burst = utilization(&lane),
        }
    }

    for (id, incidents) in incidents_by_account(incidents) {
        accounts.entry(id).or_default().overage = overage(&incidents, since_ms);
    }
    accounts
}

/// What one reading added to its period, and when the provider stated it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Consumed {
    pub at_epoch: i64,
    /// Percentage points of the allowance the reader consumed since the
    /// reading before this one.
    pub percent: f64,
}

/// The span one account's readings cover, and what each reading consumed.
///
/// A day inside the span but without a reading of its own consumed nothing:
/// the meter stood still between two readings that bracket it. A day outside
/// the span is unknown, because no reading speaks for it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountConsumption {
    pub consumed: Vec<Consumed>,
    /// The spans the readings cover, as inclusive epoch pairs. One span for
    /// each period, because a period with one reading covers one instant.
    pub covered: Vec<(i64, i64)>,
}

impl AccountConsumption {
    /// True when a reading speaks for any part of this span.
    ///
    /// The provider states a running total, so two readings that bracket a
    /// day say what that day consumed even when neither falls inside it.
    pub fn covers(&self, from_epoch: i64, to_epoch: i64) -> bool {
        self.covered
            .iter()
            .any(|(from, to)| *from <= to_epoch && *to >= from_epoch)
    }
}

/// Turn ordered meter readings into what each one consumed.
///
/// The provider states a running total for the period, not a per-day figure.
/// What a day consumed is the rise from the reading before it. The first
/// reading of a period carries the whole rise from the period's start.
///
/// A fall inside a period counts as nothing consumed. The provider restated
/// a lower figure; the reader did not give allowance back.
pub fn consumption(readings: &[ProviderUsageReading]) -> BTreeMap<AccountId, AccountConsumption> {
    let mut by_account: BTreeMap<AccountId, AccountConsumption> = BTreeMap::new();
    let mut previous: Option<(i64, f64)> = None;
    for reading in readings {
        let id = (reading.provider.clone(), reading.account_key.clone());
        let entry = by_account.entry(id).or_default();
        let percent = match previous {
            Some((period_id, before)) if period_id == reading.period_id => {
                (reading.used_percent - before).max(0.0)
            }
            _ => reading.used_percent,
        };
        let starts_period = previous.is_none_or(|(period_id, _)| period_id != reading.period_id);
        if starts_period {
            entry
                .covered
                .push((reading.observed_at_epoch, reading.observed_at_epoch));
        } else if let Some(span) = entry.covered.last_mut() {
            span.1 = reading.observed_at_epoch;
        }
        entry.consumed.push(Consumed {
            at_epoch: reading.observed_at_epoch,
            percent,
        });
        previous = Some((reading.period_id, reading.used_percent));
    }
    by_account
}

/// The blocks each account met inside the span, in the order they happened.
///
/// The Overview marks the days that carry a block, so it needs the blocks
/// themselves and not only how many there were.
pub fn account_blocks(
    incidents: &[QuotaIncidentRecord],
    since_ms: i64,
) -> BTreeMap<AccountId, Vec<Block>> {
    incidents_by_account(incidents)
        .into_iter()
        .map(|(id, incidents)| (id, blocks_since(&incidents, since_ms)))
        .collect()
}

/// Gather every session's incidents under the accounts that session used.
///
/// A session states which provider accounts it touched. Grouping by account
/// rather than by session is what lets one block that two transcripts both
/// recorded count once.
fn incidents_by_account(
    records: &[QuotaIncidentRecord],
) -> BTreeMap<AccountId, Vec<QuotaIncident>> {
    let mut by_account: BTreeMap<AccountId, Vec<QuotaIncident>> = BTreeMap::new();
    for record in records {
        let Ok(incidents) = serde_json::from_str::<Vec<QuotaIncident>>(&record.incidents_json)
        else {
            continue;
        };
        let observations: Vec<AccountObservation> =
            serde_json::from_str(&record.provider_accounts_json).unwrap_or_default();
        for observation in observations {
            by_account
                .entry((observation.provider, observation.account_key))
                .or_default()
                .extend(incidents.iter().cloned());
        }
    }
    by_account
}

#[cfg(test)]
mod tests;

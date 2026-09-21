import type { HudSpendRate } from "./hudIpc"
import type { LiveUsageSummaryPayload } from "./providerUsageIpc"

/**
 * The blink period of the HUD's live LED, from the spend rate.
 *
 * Fast means concerning. A quiet session ticks at the slow end, which is the
 * fixed period the LED had before the rate existed. A burst strobes at the
 * fast end, which stays under the flash-safety band: a 6px dot at 3.3 Hz.
 * Do not raise the fast end without revisiting that reason.
 */
export const PERIOD_SLOW_MS = 3_000
export const PERIOD_FAST_MS = 300

/** Dollars per minute at or below which the LED ticks at the slow end. */
const SPEND_FLOOR_USD_PER_MIN = 0.05
/** Dollars per minute at or above which the LED strobes at the fast end. */
export const SPEND_CEIL_USD_PER_MIN = 2

/** Allowance per hour, in percentage points, at the slow end. */
const CONSUMPTION_FLOOR_PCT_PER_HOUR = 5
/** Allowance per hour, in percentage points, at the fast end. */
const CONSUMPTION_CEIL_PCT_PER_HOUR = 100

/**
 * Eight geometric rungs between the slow and fast ends, in milliseconds.
 * WebKit restarts a CSS animation when its duration changes, so the period
 * moves in gear changes instead of on every poll.
 */
const SLOWEST_PERIOD_MS = 3_000
const FASTEST_PERIOD_MS = 300
export const LED_PERIOD_RUNGS_MS: readonly number[] = [
  SLOWEST_PERIOD_MS,
  2_100,
  1_480,
  1_040,
  730,
  510,
  360,
  FASTEST_PERIOD_MS,
]

/** Where the LED's period comes from, best first. */
type BlinkSource = "spend" | "usage" | "fixed"

export type BlinkPeriod = {
  /** Milliseconds per blink cycle. */
  periodMs: number
  source: BlinkSource
}

/**
 * Map a rate onto the rungs. `floor` and below give the slow end; `ceil`
 * and above give the fast end. Between them the map is geometric, so each
 * rung covers the same multiple of the rate.
 */
function rungFor(rate: number, floor: number, ceil: number): number {
  if (!Number.isFinite(rate) || rate <= floor) return SLOWEST_PERIOD_MS
  if (rate >= ceil) return FASTEST_PERIOD_MS
  const t = Math.log(rate / floor) / Math.log(ceil / floor)
  const index = Math.round(t * (LED_PERIOD_RUNGS_MS.length - 1))
  return LED_PERIOD_RUNGS_MS[index] ?? SLOWEST_PERIOD_MS
}

/** The blink period for a spend rate in dollars per minute, or null for none. */
export function ledPeriodMs(usdPerMinute: number | null): number | null {
  if (usdPerMinute == null) return null
  return rungFor(usdPerMinute, SPEND_FLOOR_USD_PER_MIN, SPEND_CEIL_USD_PER_MIN)
}

/** The blink period for an allowance rate in percentage points per hour. */
export function ledPeriodFromConsumption(pctPerHour: number | null): number | null {
  if (pctPerHour == null) return null
  return rungFor(pctPerHour, CONSUMPTION_FLOOR_PCT_PER_HOUR, CONSUMPTION_CEIL_PCT_PER_HOUR)
}

/** The fastest allowance consumption rate the usage payload reports. */
function peakConsumptionRate(usage: LiveUsageSummaryPayload | null): number | null {
  let peak: number | null = null
  for (const provider of usage?.providers ?? []) {
    for (const window of provider.windows) {
      const rate = window.forecast.consumptionRate
      if (rate == null) continue
      peak = peak == null ? rate : Math.max(peak, rate)
    }
  }
  return peak
}

/**
 * Pick the period on the degradation ladder: priced spend, then allowance
 * consumption, then the fixed slow period.
 *
 * A spend with nothing priced never reaches the slow end. Slow claims that
 * the machine is quiet, and an unpriced model cannot support that claim, so
 * the ladder steps down to the allowance rate instead.
 */
export function blinkPeriod(
  spend: HudSpendRate | null,
  usage: LiveUsageSummaryPayload | null,
): BlinkPeriod {
  if (spend && spend.pricedShare > 0) {
    return { periodMs: ledPeriodMs(spend.usdPerMinute) ?? PERIOD_SLOW_MS, source: "spend" }
  }
  const consumption = ledPeriodFromConsumption(peakConsumptionRate(usage))
  if (consumption != null) return { periodMs: consumption, source: "usage" }
  return { periodMs: PERIOD_SLOW_MS, source: "fixed" }
}

/** State the spend rate in words, for readers who see no motion. */
export function describeSpend(spend: HudSpendRate | null): string | null {
  if (!spend) return null
  if (spend.pricedShare === 0) return "Spend unknown: no priced model in the window."
  const rate = spend.usdPerMinute
  const dollars = rate >= 1 ? rate.toFixed(2) : rate.toFixed(3)
  const hedge = spend.pricedShare < 1 ? " or more" : ""
  return `Spending about $${dollars}/min${hedge}.`
}

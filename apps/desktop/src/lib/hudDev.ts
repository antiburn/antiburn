import type { HudSpendRate } from "./hudIpc"
import type { UsageBarItem } from "./usageBars"

/**
 * One override from the tray's "HUD Dev" submenu. The shell sends these only
 * in debug builds, so a release HUD never sees one.
 */
export type HudDevOverride =
  | { kind: "spend"; usdPerMinute: number | null }
  | { kind: "block"; secs: number }
  | { kind: "celebrate" }

/** A fully priced spend rate at `usdPerMinute`, or null to use the real one. */
export function devSpendRate(
  usdPerMinute: number | null,
  windowSecs: number,
): HudSpendRate | null {
  if (usdPerMinute == null) return null
  return { usdPerMinute, windowSecs, pricedShare: 1 }
}

/**
 * Hold the first bar at its limit until `blockUntil`, so the countdown, the
 * fresh read and the reset celebration all run. Past the time the bars come
 * back unchanged. Pure.
 */
export function withDevBlock(
  bars: readonly UsageBarItem[],
  blockUntil: number,
  now: number,
): UsageBarItem[] {
  const [first, ...rest] = bars
  if (!first || now >= blockUntil) return [...bars]
  return [{ ...first, percent: 100, resetsAt: new Date(blockUntil) }, ...rest]
}

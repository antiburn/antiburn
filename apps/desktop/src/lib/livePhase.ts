import type { CSSProperties } from "react"

/**
 * One phase for every animation that shows a live session.
 *
 * A CSS animation starts when the browser applies it, so an element that
 * mounts later starts its cycle later. The shimmer on a running session's
 * title and the sweep on that session's meters then move apart, in one window
 * and between the popover and the HUD.
 *
 * A negative delay of the wall clock within the cycle puts every element at
 * the same point of the cycle, whenever the element mounts. The cycle is
 * `--activity-row-shimmer-cycle` in `session-rows.css` and `--led-sweep-cycle`
 * in `hud.css`; keep the three numbers equal.
 */
export const LIVE_CYCLE_MS = 4000

/** The custom properties the stylesheets read the shared phase from. */
type LivePhaseVariable = "--led-sweep-delay" | "--activity-row-shimmer-delay"

/** The `animation-delay` that puts an element at the shared phase. */
export function livePhaseDelay(now: number = Date.now()): string {
  return `-${now % LIVE_CYCLE_MS}ms`
}

/** The style that gives one animation the shared phase. */
export function livePhaseStyle(variable: LivePhaseVariable, now?: number): CSSProperties {
  return { [variable]: livePhaseDelay(now) } as CSSProperties
}

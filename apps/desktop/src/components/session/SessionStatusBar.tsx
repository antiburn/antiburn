import { cn } from "../../lib/cn"
import type { MockSessionHygieneCheck } from "../../lib/presentation/mockSessionHygiene"
import { relativeTime } from "../../lib/presentation/relativeTime"
import { Tooltip } from "../presentation/Tooltip"
import { SessionCostBadge, type SessionCostBadgeProps } from "./metrics/SessionCostBadge"

export interface SessionStatusBarProps {
  checks: MockSessionHygieneCheck[]
  /** Display values for the cost figure; omit when nothing priced the session. */
  cost?: SessionCostBadgeProps | null | undefined
  /**
   * ISO timestamp of the session's most recent activity. It shows only
   * while the pointer is over the host `group` row.
   */
  timestamp?: string | undefined
}

/**
 * The verdict line across the top of a session row: the pass count, the
 * last-activity time, and the session cost.
 *
 * The verdict is always plain monospace text — never a badge. Severity
 * lives in the ink, on a traffic-light ramp: green when every check
 * passes, then orange at the first failure, and on to red as the
 * failures accumulate. The mix runs in oklch, so the ramp keeps its
 * saturation across the hue travel. The tooltip names each failed
 * check. The host row places the provider mark in a column of its own,
 * left of this line.
 */

/**
 * The verdict ink for a failure share. A passing row is green. A
 * failing row mixes the legible red into the legible orange in
 * proportion to the failures, so one failure of six reads orange and a
 * full sweep reads red.
 */
function verdictInk(failedShare: number): string {
  if (failedShare === 0) return "var(--color-system-green)"
  const pct = Math.round(failedShare * 100)
  return `color-mix(in oklch, var(--color-system-red-text) ${pct}%, var(--color-system-orange))`
}

export function SessionStatusBar({ checks, cost, timestamp }: SessionStatusBarProps) {
  const failed = checks.filter((check) => !check.passed)
  const allPassed = failed.length === 0
  const passedCount = checks.length - failed.length
  const failedShare = checks.length === 0 ? 0 : failed.length / checks.length
  const verdictLabel = allPassed
    ? "All checks pass"
    : `${failed.length} of ${checks.length} checks failed`
  const tooltip = allPassed
    ? verdictLabel
    : `${verdictLabel}: ${failed.map((check) => check.title).join(", ")}`

  return (
    // The row is exactly as tall as the provider mark in the column beside
    // it. That height also equalizes the two gaps in the card: the gap under
    // the verdict and the gap under the title both measure 9.3px from the
    // lowest glyph to the next cap (measured 2026-08-27).
    <div className="flex h-[14px] w-full items-center gap-1.5 text-label-secondary">
      <Tooltip label={tooltip} delayMs={150}>
        <span
          // The important modifiers beat the weight and the letter
          // spacing baked into .type-footnote. That spacing opens the
          // text up for the sans face; the monospace face is already
          // wide enough. The monospace space is as wide as a digit, so
          // the negative word spacing keeps the count as one unit.
          aria-label={verdictLabel}
          className="font-mono type-footnote font-medium! tracking-tight! [word-spacing:-2px] leading-[13px] tabular-nums"
          style={{ color: verdictInk(failedShare) }}
        >
          {passedCount}/{checks.length} checks pass
        </span>
      </Tooltip>

      {timestamp && (
        <time
          dateTime={timestamp}
          aria-label={`Last activity ${relativeTime(timestamp)}`}
          // Hidden at rest; the host row's `group` hover reveals it. The
          // element keeps its width, so the cost figure does not move.
          className="ml-auto shrink-0 type-callout tabular-nums text-label-tertiary opacity-0 transition-opacity duration-[var(--duration-fast)] group-hover:opacity-100"
        >
          {relativeTime(timestamp)}
        </time>
      )}

      {cost && (
        <SessionCostBadge
          {...cost}
          // A usual cost stays bare; only a hot cost earns the pill.
          appearance={cost.isHighCost ? "pill" : "bare"}
          className={cn(!timestamp && "ml-auto")}
        />
      )}
    </div>
  )
}

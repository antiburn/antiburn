import { cn } from "../../lib/cn"
import type { SessionHygieneEvidenceState } from "../../lib/insightsIpc"
import {
  sessionHygieneStateLabel,
  type SessionHygieneCheck,
} from "../../lib/presentation/sessionHygiene"
import { relativeTime } from "../../lib/presentation/relativeTime"
import { Tooltip } from "../presentation/Tooltip"
import { SessionCostBadge, type SessionCostBadgeProps } from "./metrics/SessionCostBadge"

export interface SessionStatusBarProps {
  checks: SessionHygieneCheck[]
  evidenceState?: SessionHygieneEvidenceState
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
 * The verdict is always plain monospace text. Severity lives in the ink.
 * The tooltip names each finding and each check that was not assessed.
 */

/** Return the semantic ink for the assessed failure share. */
function verdictInk(failedShare: number, assessedCount: number): string {
  if (assessedCount === 0) return "var(--color-label-tertiary)"
  if (failedShare === 0) return "var(--color-system-green)"
  const pct = Math.round(failedShare * 100)
  return `color-mix(in oklch, var(--color-system-red-text) ${pct}%, var(--color-system-orange))`
}

export function SessionStatusBar({
  checks,
  evidenceState = "ready",
  cost,
  timestamp,
}: SessionStatusBarProps) {
  const failed = checks.filter((check) => check.status === "finding")
  const passed = checks.filter((check) => check.status === "clean")
  const notAssessed = checks.filter((check) => check.status === "notAssessed")
  const assessedCount = passed.length + failed.length
  const failedShare = assessedCount === 0 ? 0 : failed.length / assessedCount
  const allPassed = passed.length === checks.length && checks.length > 0
  const stateLabel = sessionHygieneStateLabel(evidenceState)
  const countText = `${passed.length}/${assessedCount} checks pass${
    notAssessed.length > 0 ? ` · ${notAssessed.length} not assessed` : ""
  }`
  const verdictLabel = stateLabel
    ? `${stateLabel} session hygiene checks`
    : allPassed
      ? "All checks pass"
      : failed.length > 0
        ? `${failed.length} of ${assessedCount} assessed checks failed${
            notAssessed.length > 0 ? `; ${notAssessed.length} not assessed` : ""
          }`
        : `${passed.length} of ${assessedCount} assessed checks pass; ${notAssessed.length} not assessed`
  const details = [...failed, ...notAssessed].map((check) => check.title).join(", ")
  const tooltip = details ? `${verdictLabel}: ${details}` : verdictLabel

  return (
    // The row matches the provider mark height in the adjacent column.
    <div className="flex h-[14px] w-full items-center gap-1.5 text-label-secondary">
      <Tooltip label={tooltip} delayMs={150}>
        <span
          aria-label={verdictLabel}
          className="font-mono type-footnote font-medium! tracking-tight! [word-spacing:-2px] leading-[13px] tabular-nums"
          style={{ color: verdictInk(failedShare, assessedCount) }}
        >
          {stateLabel ? `${stateLabel} checks` : countText}
        </span>
      </Tooltip>

      {timestamp && (
        <time
          dateTime={timestamp}
          aria-label={`Last activity ${relativeTime(timestamp)}`}
          // The host row reveals the reserved time label on hover.
          className="ml-auto shrink-0 type-callout tabular-nums text-label-tertiary opacity-0 transition-opacity duration-[var(--duration-fast)] group-hover:opacity-100"
        >
          {relativeTime(timestamp)}
        </time>
      )}

      {cost && (
        <SessionCostBadge
          {...cost}
          appearance={cost.isHighCost ? "pill" : "bare"}
          className={cn(!timestamp && "ml-auto")}
        />
      )}
    </div>
  )
}

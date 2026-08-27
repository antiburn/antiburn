import { Fragment } from "react"

import type { MockSessionHygieneCheck } from "../../lib/presentation/mockSessionHygiene"
import { Tooltip } from "../presentation/Tooltip"
import { SessionCostBadge, type SessionCostBadgeProps } from "./metrics/SessionCostBadge"

export interface SessionStatusBarProps {
  checks: MockSessionHygieneCheck[]
  /** Display values for the cost figure; omit when nothing priced the session. */
  cost?: SessionCostBadgeProps | null | undefined
}

/**
 * The verdict line shows the pass count and the session cost.
 *
 * The verdict uses plain monospace text. Its color moves from green to orange and red as failures increase.
 * The tooltip lists each check result.
 */

function verdictInk(failedShare: number): string {
  if (failedShare === 0) return "var(--color-system-green)"
  const pct = Math.round(failedShare * 100)
  return `color-mix(in oklch, var(--color-system-red-text) ${pct}%, var(--color-system-orange))`
}

function renderTooltip(checks: MockSessionHygieneCheck[]) {
  const failedChecks = checks.filter((check) => !check.passed)
  const passedChecks = checks.filter((check) => check.passed)
  return (
    <div className="grid grid-cols-[1fr_max-content] gap-x-2.5 gap-y-0 items-center font-mono [word-spacing:-2px]">
      {failedChecks.map((check) => (
        <Fragment key={check.id}>
          <span className="text-system-red">{check.title}</span>
          <span className="text-system-red text-lg">✘</span>
        </Fragment>
      ))}
      {failedChecks.length > 0 && <div className="col-span-full border-b border-separator" />}
      {passedChecks.map((check) => (
        <Fragment key={check.id}>
          <span className="text-system-green">{check.title}</span>
          <span className="text-system-green text-lg">✔</span>
        </Fragment>
      ))}
      <span className="col-span-full mt-2.5 text-label-secondary">
        Open the session for details
      </span>
    </div>
  )
}

export function SessionStatusBar({ checks, cost }: SessionStatusBarProps) {
  const failed = checks.filter((check) => !check.passed)
  const allPassed = failed.length === 0
  const passedCount = checks.length - failed.length
  const failedShare = checks.length === 0 ? 0 : failed.length / checks.length
  const verdictLabel = allPassed
    ? "All checks pass"
    : `${failed.length} of ${checks.length} checks failed`

  return (
    <div className="flex w-full items-center justify-between gap-x-1.5 text-label-secondary">
      <Tooltip label={renderTooltip(checks)} delayMs={150}>
        <span
          aria-label={verdictLabel}
          className="font-mono type-footnote font-medium! tracking-tight! [word-spacing:-2px] leading-[13px] tabular-nums"
          style={{ color: verdictInk(failedShare) }}
        >
          {passedCount}/{checks.length} burn checks
        </span>
      </Tooltip>

      {cost && <SessionCostBadge {...cost} appearance={cost.isHighCost ? "pill" : "bare"} />}
    </div>
  )
}

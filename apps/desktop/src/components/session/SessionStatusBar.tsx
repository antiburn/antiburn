// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
 * The line never paints a fill. Attention lives on the element that
 * earns it: a failing verdict sits in a solid brand pill, and a hot cost
 * sits in the same pill. The verdict is the text "N/M Pass". The tooltip
 * names each failed check. The host row places the provider mark in a
 * column of its own, left of this line.
 */
export function SessionStatusBar({ checks, cost, timestamp }: SessionStatusBarProps) {
  const failed = checks.filter((check) => !check.passed)
  const allPassed = failed.length === 0
  const passedCount = checks.length - failed.length
  const verdictLabel = allPassed
    ? "All checks pass"
    : `${failed.length} of ${checks.length} checks failed`
  const tooltip = allPassed
    ? verdictLabel
    : `${verdictLabel}: ${failed.map((check) => check.title).join(", ")}`

  return (
    <div className="flex h-[var(--control-height-regular)] w-full items-center gap-1.5 text-label-secondary">
      <Tooltip label={tooltip} delayMs={150}>
        <span
          // The verdict matches the cost figure's size, and the important
          // modifier beats the weight baked into .type-callout.
          aria-label={verdictLabel}
          className={cn(
            "type-callout font-medium! leading-[13px] tabular-nums",
            // The padding matches the cost pill, so the two share a height.
            !allPassed && "rounded-full bg-brand-tint px-1.5 py-px text-white",
          )}
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

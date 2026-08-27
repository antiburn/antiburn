import { Fragment } from "react"

import type { SessionHygieneEvidenceState } from "../../lib/insightsIpc"
import {
  sessionHygieneStateLabel,
  type SessionHygieneCheck,
} from "../../lib/presentation/sessionHygiene"
import { Tooltip } from "../presentation/Tooltip"
import { SessionCostBadge, type SessionCostBadgeProps } from "./metrics/SessionCostBadge"

export interface SessionStatusBarProps {
  checks: SessionHygieneCheck[]
  evidenceState?: SessionHygieneEvidenceState
  /** Display values for the cost figure; omit when nothing priced the session. */
  cost?: SessionCostBadgeProps | null | undefined
}

/**
 * The verdict line shows the pass count and the session cost.
 *
 * The verdict uses plain monospace text without badge chrome. Severity lives
 * in the ink. The ink moves from green through orange to red as findings rise.
 * The tooltip lists each check with its result.
 */

/**
 * Return the ink for the assessed finding share. A clean result is green.
 * Findings mix orange with red in proportion to their share.
 */
function verdictInk(failedShare: number, assessedCount: number): string {
  if (assessedCount === 0) return "var(--color-label-tertiary)"
  if (failedShare === 0) return "var(--color-system-green)"
  const pct = Math.round(failedShare * 100)
  return `color-mix(in oklch, var(--color-system-red-text) ${pct}%, var(--color-system-orange))`
}

const CHECK_MARK: Record<SessionHygieneCheck["status"], string> = {
  finding: "✘",
  clean: "✔",
  notAssessed: "–",
}

const INK_CLASS: Record<SessionHygieneCheck["ink"], string> = {
  "system-red-text": "text-system-red-text",
  "system-green": "text-system-green",
  "label-tertiary": "text-label-tertiary",
}

function renderTooltip(
  failed: SessionHygieneCheck[],
  passed: SessionHygieneCheck[],
  notAssessed: SessionHygieneCheck[],
) {
  const groups = [failed, passed, notAssessed].filter((group) => group.length > 0)
  return (
    <div className="grid grid-cols-[1fr_max-content] gap-x-2.5 gap-y-0 items-center font-mono [word-spacing:-2px]">
      {groups.map((group, index) => (
        <Fragment key={group[0]!.status}>
          {index > 0 && <div className="col-span-full border-b border-separator" />}
          {group.map((check) => (
            <Fragment key={check.id}>
              <span className={INK_CLASS[check.ink]}>{check.title}</span>
              <span className={`${INK_CLASS[check.ink]} text-lg`}>
                {CHECK_MARK[check.status]}
              </span>
            </Fragment>
          ))}
        </Fragment>
      ))}
      <span className="col-span-full mt-2.5 text-label-secondary">
        Open the session for details
      </span>
    </div>
  )
}

export function SessionStatusBar({
  checks,
  evidenceState = "ready",
  cost,
}: SessionStatusBarProps) {
  const failed = checks.filter((check) => check.status === "finding")
  const passed = checks.filter((check) => check.status === "clean")
  const notAssessed = checks.filter((check) => check.status === "notAssessed")
  const assessedCount = passed.length + failed.length
  const failedShare = assessedCount === 0 ? 0 : failed.length / assessedCount
  const allPassed = passed.length === checks.length && checks.length > 0
  const stateLabel = sessionHygieneStateLabel(evidenceState)
  const countText = `${passed.length}/${assessedCount} burn checks${
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
  const tooltip = stateLabel ? verdictLabel : renderTooltip(failed, passed, notAssessed)

  return (
    <div className="flex w-full items-center justify-between gap-x-1.5 text-label-secondary">
      <Tooltip label={tooltip} delayMs={150}>
        <span
          // The modifiers remove the wider sans spacing from type-footnote.
          // Monospace text already adds enough space between characters.
          // Negative word spacing keeps the count together.
          aria-label={verdictLabel}
          className="font-mono type-footnote font-medium! tracking-tight! [word-spacing:-2px] leading-[13px] tabular-nums"
          style={{ color: verdictInk(failedShare, assessedCount) }}
        >
          {stateLabel ? `${stateLabel} checks` : countText}
        </span>
      </Tooltip>

      {cost && <SessionCostBadge {...cost} appearance={cost.isHighCost ? "pill" : "bare"} />}
    </div>
  )
}

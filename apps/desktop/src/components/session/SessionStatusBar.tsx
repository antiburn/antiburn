import { Check, CircleDashed, X, type LucideIcon } from "lucide-react"
import { Fragment } from "react"

import type { SessionHygieneEvidenceState } from "../../lib/insightsIpc"
import {
  notAssessedReasonLabel,
  sessionHygieneStateIsTransient,
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

const CHECK_ICON: Record<SessionHygieneCheck["status"], LucideIcon> = {
  finding: X,
  clean: Check,
  // An open, broken outline reads as "not filled in". A dash reads as
  // punctuation next to the two solid marks.
  notAssessed: CircleDashed,
}

const CHECK_ICON_LABEL: Record<SessionHygieneCheck["status"], string> = {
  finding: "Finding",
  clean: "Pass",
  notAssessed: "Not assessed",
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
          {group.map((check) => {
            const Mark = CHECK_ICON[check.status]
            return (
              <Fragment key={check.id}>
                {/* A not-assessed check names itself only. The reason line below
                    carries the verdict, so the two do not repeat each other. */}
                <span className={INK_CLASS[check.ink]}>
                  {check.status === "notAssessed" ? check.name : check.title}
                </span>
                <Mark
                  size={14}
                  strokeWidth={2.5}
                  role="img"
                  aria-label={CHECK_ICON_LABEL[check.status]}
                  className={INK_CLASS[check.ink]}
                />
                {check.status === "notAssessed" && check.notAssessedReason && (
                  <span className="col-span-full type-caption text-label-tertiary">
                    {notAssessedReasonLabel(check.notAssessedReason)}
                  </span>
                )}
              </Fragment>
            )
          })}
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
  // The denominator is every check the session has, not only the assessed
  // ones. A denominator that moves with the assessed count contradicts the
  // not-assessed tail beside it.
  const failedShare = checks.length === 0 ? 0 : failed.length / checks.length
  const allPassed = passed.length === checks.length && checks.length > 0
  const stateLabel = sessionHygieneStateLabel(evidenceState)
  // A transient state ends on its own, so its label carries an ellipsis.
  const stateText = stateLabel
    ? `${stateLabel} checks${sessionHygieneStateIsTransient(evidenceState) ? "…" : ""}`
    : null
  const checkNoun = checks.length === 1 ? "burn check" : "burn checks"
  const notAssessedText = notAssessed.length > 0 ? ` · ${notAssessed.length} not assessed` : ""
  const countText =
    assessedCount === 0
      ? "Not assessed"
      : `${passed.length}/${checks.length} ${checkNoun}${notAssessedText}`
  const verdictLabel = stateLabel
    ? `${stateLabel} session hygiene checks`
    : allPassed
      ? "All checks pass"
      : assessedCount === 0
        ? "No checks assessed"
        : `${passed.length} of ${checks.length} ${checkNoun} pass${
            notAssessed.length > 0 ? `; ${notAssessed.length} not assessed` : ""
          }`
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
          {stateText ?? countText}
        </span>
      </Tooltip>

      {cost && <SessionCostBadge {...cost} appearance={cost.isHighCost ? "pill" : "bare"} />}
    </div>
  )
}

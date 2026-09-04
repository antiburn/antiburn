import { Check, Flame, X, type LucideIcon } from "lucide-react"
import { Fragment } from "react"

import type { SessionHygieneEvidenceState } from "../../lib/insightsIpc"
import {
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
  limitBadge?:
    | {
        label: string
        percent: number
        provider?: string
        windowId?: string
      }
    | undefined
  onLimitBadgeHover?: (badge: NonNullable<SessionStatusBarProps["limitBadge"]> | null) => void
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

function formatLimitPercent(percent: number): string {
  return `${Number(percent.toFixed(1))}%`
}

interface StatusMark {
  Icon: LucideIcon
  /** Box size in px. The tooltip surface sets 12px text. */
  size: number
  strokeWidth: number
  label: string
}

type AssessedHygieneCheck = SessionHygieneCheck & { status: "finding" | "clean" }

function isAssessed(check: SessionHygieneCheck): check is AssessedHygieneCheck {
  return check.status !== "notAssessed"
}

const STATUS_MARK: Record<AssessedHygieneCheck["status"], StatusMark> = {
  finding: { Icon: X, size: 12, strokeWidth: 2.5, label: "Finding" },
  clean: { Icon: Check, size: 12, strokeWidth: 2.5, label: "Passed" },
}

const INK_CLASS: Record<SessionHygieneCheck["ink"], string> = {
  "system-red-text": "text-system-red-text",
  "system-green": "text-system-green",
  "label-tertiary": "text-label-tertiary",
}

function renderTooltip(failed: AssessedHygieneCheck[], passed: AssessedHygieneCheck[]) {
  const groups = [failed, passed].filter((group) => group.length > 0)
  return (
    <div className="grid grid-cols-[1fr_max-content] gap-x-2.5 gap-y-0 items-center font-mono [word-spacing:-2px]">
      {groups.map((group, index) => (
        <Fragment key={group[0]!.status}>
          {index > 0 && <div className="col-span-full border-b border-separator" />}
          {group.map((check) => {
            const mark = STATUS_MARK[check.status]
            return (
              <Fragment key={check.id}>
                <span className={INK_CLASS[check.ink]}>{check.title}</span>
                <mark.Icon
                  size={mark.size}
                  strokeWidth={mark.strokeWidth}
                  role="img"
                  aria-label={mark.label}
                  className={`justify-self-center ${INK_CLASS[check.ink]}`}
                />
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
  limitBadge,
  onLimitBadgeHover,
}: SessionStatusBarProps) {
  const assessedChecks = checks.filter(isAssessed)
  const failed = assessedChecks.filter((check) => check.status === "finding")
  const passed = assessedChecks.filter((check) => check.status === "clean")
  const assessedCount = passed.length + failed.length
  const failedShare = assessedCount === 0 ? 0 : failed.length / assessedCount
  const allPassed = passed.length === assessedCount && assessedCount > 0
  const hasUnavailableChecks = assessedCount < checks.length
  const stateLabel = sessionHygieneStateLabel(evidenceState)
  // A transient state ends on its own, so its label carries an ellipsis.
  const stateText = stateLabel
    ? `${stateLabel} checks${sessionHygieneStateIsTransient(evidenceState) ? "…" : ""}`
    : null
  // Once at least one check is assessed, the verdict is worth more than the
  // state label — a stale or refreshing session still has a last result. Only
  // the never-assessed case keeps the plain state text in the count's place.
  const showStateText = stateLabel !== null && assessedCount === 0
  const checkNoun = assessedCount === 1 ? "burn check" : "burn checks"
  const countText = `${passed.length}/${assessedCount} ${
    allPassed && hasUnavailableChecks ? "assessed " : ""
  }${checkNoun}`
  // A transient state next to an assessed verdict still names itself, as a
  // prefix on the aria label and the tooltip text.
  const verdictPrefix = stateLabel && !showStateText ? `${stateLabel} — ` : ""
  const verdictLabel = showStateText
    ? `${stateLabel} session hygiene checks`
    : `${verdictPrefix}${
        allPassed
          ? hasUnavailableChecks
            ? "All assessed checks passed"
            : "All checks passed"
          : `${passed.length} of ${assessedCount} ${checkNoun} passed`
      }`
  const tooltip = showStateText ? verdictLabel : renderTooltip(failed, passed)
  const showVerdict = showStateText || assessedCount > 0
  const isHighLimitShare = (limitBadge?.percent ?? 0) >= 5

  return (
    <div className="flex w-full items-center justify-between gap-x-1.5 text-label-secondary">
      {showVerdict && (
        <Tooltip label={tooltip} delayMs={150}>
          <span
            // The modifiers remove the wider sans spacing from type-footnote.
            // Monospace text already adds enough space between characters.
            // Negative word spacing keeps the count together.
            aria-label={verdictLabel}
            className="font-mono type-footnote font-medium! tracking-tight! [word-spacing:-2px] leading-[13px] tabular-nums"
            style={{ color: verdictInk(failedShare, assessedCount) }}
          >
            {showStateText ? stateText : countText}
          </span>
        </Tooltip>
      )}

      {limitBadge ? (
        <Tooltip label={limitBadge.label} delayMs={150}>
          <span
            className={
              isHighLimitShare
                ? "flex shrink-0 items-center gap-0.5 rounded-full bg-brand-tint px-1.5 py-px font-mono type-footnote font-medium! leading-[13px] tracking-tight! text-white tabular-nums"
                : "font-mono type-footnote tabular-nums text-label-secondary"
            }
            data-session-limit-provider={limitBadge.provider}
            data-session-limit-window={limitBadge.windowId}
            data-session-limit-percent={limitBadge.percent.toFixed(4)}
            aria-label={
              isHighLimitShare
                ? `${limitBadge.label} This session uses 5% or more of your limit.`
                : limitBadge.label
            }
            tabIndex={0}
            onMouseEnter={() => onLimitBadgeHover?.(limitBadge)}
            onMouseLeave={() => onLimitBadgeHover?.(null)}
            onFocus={() => onLimitBadgeHover?.(limitBadge)}
            onBlur={() => onLimitBadgeHover?.(null)}
          >
            {isHighLimitShare && <Flame size={11} className="shrink-0" aria-hidden="true" />}
            {formatLimitPercent(limitBadge.percent)}
          </span>
        </Tooltip>
      ) : (
        cost && <SessionCostBadge {...cost} appearance={cost.isHighCost ? "pill" : "bare"} />
      )}
    </div>
  )
}

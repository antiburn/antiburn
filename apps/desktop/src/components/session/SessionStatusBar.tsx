import { Check, CircleDashed, Flame, X, type LucideIcon } from "lucide-react"
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
 *
 * The mix runs between the vivid fill tones, not the darkened text ones. The
 * text tones are tuned for contrast on a light surface, and a mostly-orange
 * mix of them reads as brown, which states nothing.
 */
function verdictInk(failedShare: number, assessedCount: number): string {
  if (assessedCount === 0) return "var(--color-label-tertiary)"
  if (failedShare === 0) return "var(--color-system-green)"
  const pct = Math.round(failedShare * 100)
  return `color-mix(in oklch, var(--color-system-red-tint) ${pct}%, var(--color-system-orange-tint))`
}

function formatLimitPercent(percent: number): string {
  if (percent > 0 && percent < 0.01) return "<0.01%"
  return `${Number(percent.toFixed(2))}%`
}

interface StatusMark {
  Icon: LucideIcon
  /** Box size in px. The tooltip surface sets 12px text. */
  size: number
  strokeWidth: number
  label: string
}

const STATUS_MARK: Record<SessionHygieneCheck["status"], StatusMark> = {
  finding: { Icon: X, size: 12, strokeWidth: 2.5, label: "Finding" },
  clean: { Icon: Check, size: 12, strokeWidth: 2.5, label: "Pass" },
  // An open, broken outline reads as "not filled in". A dash reads as
  // punctuation next to the two solid marks.
  //
  // The circle draws to the edge of its box, but a check or a cross does
  // not. So the circle needs a smaller box and a thinner stroke to carry
  // the same visual weight, and to keep its gaps legible.
  notAssessed: { Icon: CircleDashed, size: 10, strokeWidth: 2, label: "Not assessed" },
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
            const mark = STATUS_MARK[check.status]
            return (
              <Fragment key={check.id}>
                {/* A not-assessed check names itself only. The reason line below
                    carries the verdict, so the two do not repeat each other. */}
                <span className={INK_CLASS[check.ink]}>
                  {check.status === "notAssessed" ? check.name : check.title}
                </span>
                <mark.Icon
                  size={mark.size}
                  strokeWidth={mark.strokeWidth}
                  role="img"
                  aria-label={mark.label}
                  className={`justify-self-center ${INK_CLASS[check.ink]}`}
                />
                {/* The reason stays in the name column. A full-width line would
                    run under the mark column and break the right edge. */}
                {check.status === "notAssessed" && check.notAssessedReason && (
                  <span className="col-start-1 type-caption text-label-tertiary">
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
  limitBadge,
  onLimitBadgeHover,
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
  // Once at least one check is assessed, the verdict is worth more than the
  // state label — a stale or refreshing session still has a last result. Only
  // the never-assessed case keeps the plain state text in the count's place.
  const showStateText = stateLabel !== null && assessedCount === 0
  const checkNoun = checks.length === 1 ? "burn check" : "burn checks"
  const notAssessedText = notAssessed.length > 0 ? ` · ${notAssessed.length} not assessed` : ""
  const countText =
    assessedCount === 0
      ? "Not assessed"
      : `${passed.length}/${checks.length} ${checkNoun}${notAssessedText}`
  // A transient state next to an assessed verdict still names itself, as a
  // prefix on the aria label and the tooltip text.
  const verdictPrefix = stateLabel && !showStateText ? `${stateLabel} — ` : ""
  const verdictLabel = showStateText
    ? `${stateLabel} session hygiene checks`
    : `${verdictPrefix}${
        allPassed
          ? "All checks pass"
          : assessedCount === 0
            ? "No checks assessed"
            : `${passed.length} of ${checks.length} ${checkNoun} pass${
                notAssessed.length > 0 ? `; ${notAssessed.length} not assessed` : ""
              }`
      }`
  const tooltip = showStateText ? verdictLabel : renderTooltip(failed, passed, notAssessed)
  const isHighLimitShare = (limitBadge?.percent ?? 0) >= 5

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
          {showStateText ? stateText : countText}
        </span>
      </Tooltip>

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

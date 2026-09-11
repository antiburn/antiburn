import { Flame } from "lucide-react"

import type { SessionHygieneEvidenceState } from "../../lib/insightsIpc"
import { sessionBurnCheckPresentation } from "../../lib/presentation/burnChecks"
import type { SessionHygieneCheck } from "../../lib/presentation/sessionHygiene"
import { BurnCheckStatus } from "../burn-checks/BurnCheckStatus"
import { BURN_CHECK_MARKS, type BurnCheckMark } from "../burn-checks/burnCheckMarks"
import { Tooltip } from "../presentation/Tooltip"
import { SessionCostBadge, type SessionCostBadgeProps } from "./metrics/SessionCostBadge"

export interface SessionStatusBarProps {
  checks: SessionHygieneCheck[]
  evidenceState?: SessionHygieneEvidenceState
  /** Display values for the cost figure; omit when nothing priced the session. */
  cost?: SessionCostBadgeProps | null | undefined
  /**
   * A null percent shows the missing limit label: "unknown" when a live
   * window exists for the session's provider but did not attribute a share
   * to it, "no limit" when the provider reports no such window at all. An
   * omitted badge uses the cost.
   */
  limitBadge?:
    | {
        label: string
        percent: number | null
        provider?: string
        windowId?: string
        unknown?: boolean
      }
    | undefined
}

/**
 * Show the share as a figure and a percent sign.
 *
 * English style puts no space before the percent sign, so the two stay one
 * text run: the figure and the sign are sibling text nodes, and the value
 * copies as "17.2%". The empty element between them is a hair space. The
 * monospace percent sign fills its cell with ink, and without that space it
 * looks joined to the last digit.
 */
function LimitPercent({ percent }: { percent: number }) {
  return (
    <>
      {roundedLimitPercent(percent)}
      <span aria-hidden="true" className="inline-block w-px" />%
    </>
  )
}

function roundedLimitPercent(percent: number): number {
  return Number(percent.toFixed(1))
}

interface StatusMark extends BurnCheckMark {
  label: string
  headingClass: string
  textClass: string
}

const STATUS_MARK: Record<SessionHygieneCheck["status"], StatusMark> = {
  finding: {
    ...BURN_CHECK_MARKS.finding,
    label: "Failed",
    headingClass: "text-label-tertiary",
    textClass: "text-burn-check-failure-text",
  },
  clean: {
    ...BURN_CHECK_MARKS.clean,
    label: "Passed",
    headingClass: "text-label-tertiary",
    textClass: "text-label",
  },
  notAssessed: {
    ...BURN_CHECK_MARKS.notAssessed,
    label: "Not assessed",
    headingClass: "text-label-tertiary",
    textClass: "text-label-secondary",
  },
}

function tooltipCheckTitle(check: SessionHygieneCheck): string {
  return check.status === "notAssessed"
    ? check.title.replace(/ not assessed$/i, "")
    : check.title
}

function renderTooltip(checks: SessionHygieneCheck[]) {
  const groups = (["finding", "clean", "notAssessed"] as const)
    .map((status) => checks.filter((check) => check.status === status))
    .filter((group) => group.length > 0)
  return (
    <div className="flex min-w-[200px] flex-col" data-burn-check-tooltip="">
      <span className="type-callout font-semibold! text-label">Burn Checks</span>
      <div className="mt-2 flex flex-col gap-y-2.5">
        {groups.map((group) => {
          const mark = STATUS_MARK[group[0]!.status]
          return (
            <section key={group[0]!.status} aria-label={`${mark.label}, ${group.length}`}>
              <div
                className={`type-caption font-medium! tabular-nums ${mark.headingClass}`}
                data-burn-check-tooltip-group={group[0]!.status}
              >
                {mark.label} · {group.length}
              </div>
              <ul className="mt-1 flex flex-col gap-y-1">
                {group.map((check) => (
                  <li
                    key={check.id}
                    className="grid grid-cols-[14px_minmax(0,1fr)] items-start gap-x-1.5"
                  >
                    <mark.Icon
                      size={14}
                      strokeWidth={mark.strokeWidth}
                      aria-hidden="true"
                      className={`mt-px shrink-0 ${mark.iconClass}`}
                      data-burn-check-tooltip-mark={check.status}
                    />
                    <span className={`type-callout text-pretty ${mark.textClass}`}>
                      {tooltipCheckTitle(check)}
                    </span>
                  </li>
                ))}
              </ul>
            </section>
          )
        })}
      </div>
    </div>
  )
}

export function SessionStatusBar({
  checks,
  evidenceState = "ready",
  cost,
  limitBadge,
}: SessionStatusBarProps) {
  const presentation = sessionBurnCheckPresentation(checks, evidenceState)
  const hasCheckDetails = checks.length > 0
  const tooltip = hasCheckDetails ? renderTooltip(checks) : presentation.accessibleDescription
  const isHighLimitShare = roundedLimitPercent(limitBadge?.percent ?? 0) >= 5

  return (
    <div
      className="flex w-full min-w-0 items-center justify-between gap-x-2 text-label-secondary"
      data-session-status-bar=""
    >
      <Tooltip label={tooltip} delayMs={150}>
        <BurnCheckStatus presentation={presentation} omitUnassessed />
      </Tooltip>

      <div className="ml-auto shrink-0">
        {limitBadge && limitBadge.percent !== null ? (
          <Tooltip label={limitBadge.label} delayMs={150}>
            <span
              className={
                isHighLimitShare
                  ? // The pill keeps the tracking of type-footnote. Tighter
                    // tracking moves the wide percent sign into the last digit,
                    // because the monospace cell is already full.
                    "flex shrink-0 items-center gap-0.5 rounded-full bg-brand-tint px-1.5 py-px font-mono type-footnote font-medium! leading-[13px] text-white tabular-nums"
                  : // The same 13px line box as the pill, so the two states of
                    // the badge occupy one box.
                    "font-mono type-footnote leading-[13px] tabular-nums text-label-secondary"
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
            >
              {isHighLimitShare && <Flame size={11} className="shrink-0" aria-hidden="true" />}
              <LimitPercent percent={limitBadge.percent} />
            </span>
          </Tooltip>
        ) : limitBadge ? (
          <Tooltip label={limitBadge.label} delayMs={150}>
            <span
              className="font-mono type-footnote leading-[13px] text-label-secondary opacity-50"
              aria-label={limitBadge.label}
              tabIndex={0}
            >
              {limitBadge.unknown ? "unknown" : "no limit"}
            </span>
          </Tooltip>
        ) : (
          cost && <SessionCostBadge {...cost} appearance={cost.isHighCost ? "pill" : "bare"} />
        )}
      </div>
    </div>
  )
}

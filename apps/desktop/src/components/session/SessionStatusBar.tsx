import { Check, CircleMinus, Flame, X, type LucideIcon } from "lucide-react"
import { Fragment } from "react"

import type { SessionHygieneEvidenceState } from "../../lib/insightsIpc"
import { sessionBurnCheckPresentation } from "../../lib/presentation/burnChecks"
import type { SessionHygieneCheck } from "../../lib/presentation/sessionHygiene"
import { BurnCheckStatus } from "../burn-checks/BurnCheckStatus"
import { Tooltip } from "../presentation/Tooltip"
import { SessionCostBadge, type SessionCostBadgeProps } from "./metrics/SessionCostBadge"

export interface SessionStatusBarProps {
  checks: SessionHygieneCheck[]
  evidenceState?: SessionHygieneEvidenceState
  /** Display values for the cost figure; omit when nothing priced the session. */
  cost?: SessionCostBadgeProps | null | undefined
  /** A null percent shows the missing limit label. An omitted badge uses the cost. */
  limitBadge?:
    | {
        label: string
        percent: number | null
        provider?: string
        windowId?: string
      }
    | undefined
}

function formatLimitPercent(percent: number): string {
  return `${roundedLimitPercent(percent)}%`
}

function roundedLimitPercent(percent: number): number {
  return Number(percent.toFixed(1))
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
  clean: { Icon: Check, size: 12, strokeWidth: 2.5, label: "Passed" },
  notAssessed: { Icon: CircleMinus, size: 12, strokeWidth: 2, label: "Not assessed" },
}

const INK_CLASS: Record<SessionHygieneCheck["ink"], string> = {
  "system-red-text": "text-system-red-text",
  "system-green": "text-system-green",
  "label-tertiary": "text-label-tertiary",
}

function renderTooltip(checks: SessionHygieneCheck[]) {
  const groups = (["finding", "clean", "notAssessed"] as const)
    .map((status) => checks.filter((check) => check.status === status))
    .filter((group) => group.length > 0)
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
}: SessionStatusBarProps) {
  const presentation = sessionBurnCheckPresentation(checks, evidenceState)
  const hasCheckDetails = checks.length > 0
  const tooltip = hasCheckDetails ? renderTooltip(checks) : presentation.accessibleDescription
  const isHighLimitShare = roundedLimitPercent(limitBadge?.percent ?? 0) >= 5

  return (
    <div className="flex w-full items-center justify-between gap-x-1.5 text-label-secondary">
      <Tooltip label={tooltip} delayMs={150}>
        <BurnCheckStatus presentation={presentation} />
      </Tooltip>

      <div className="ml-auto">
        {limitBadge && limitBadge.percent !== null ? (
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
            >
              {isHighLimitShare && <Flame size={11} className="shrink-0" aria-hidden="true" />}
              {formatLimitPercent(limitBadge.percent)}
            </span>
          </Tooltip>
        ) : limitBadge ? (
          <Tooltip label={limitBadge.label} delayMs={150}>
            <span
              className="font-mono type-footnote text-label-secondary opacity-50"
              aria-label={limitBadge.label}
              tabIndex={0}
            >
              no limit
            </span>
          </Tooltip>
        ) : (
          cost && <SessionCostBadge {...cost} appearance={cost.isHighCost ? "pill" : "bare"} />
        )}
      </div>
    </div>
  )
}

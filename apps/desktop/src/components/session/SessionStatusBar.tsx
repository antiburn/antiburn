import type { SessionHygieneEvidenceState } from "../../lib/insightsIpc"
import { sessionBurnCheckPresentation } from "../../lib/presentation/burnChecks"
import type { SessionHygieneCheck } from "../../lib/presentation/sessionHygiene"
import { BurnCheckStatus } from "../burn-checks/BurnCheckStatus"
import { BURN_CHECK_MARKS, type BurnCheckMark } from "../burn-checks/burnCheckMarks"
import { Tooltip } from "../presentation/Tooltip"
import { SessionCostBadge, type SessionCostBadgeProps } from "./metrics/SessionCostBadge"
import { SessionLimitBadge, type SessionLimitBadgeInfo } from "./metrics/SessionLimitBadge"

export interface SessionStatusBarProps {
  checks: SessionHygieneCheck[]
  evidenceState?: SessionHygieneEvidenceState
  /** Display values for the cost figure; omit when nothing priced the session. */
  cost?: SessionCostBadgeProps | null | undefined
  /** Shared cohort cost-outlier state, also used when the cost figure is hidden. */
  isHighCost?: boolean
  /** The limit-share pill's values. An omitted badge uses the cost instead. */
  limitBadge?: SessionLimitBadgeInfo | undefined
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
  isHighCost: highCost,
  limitBadge,
}: SessionStatusBarProps) {
  const presentation = sessionBurnCheckPresentation(checks, evidenceState)
  const hasCheckDetails = checks.length > 0
  const tooltip = hasCheckDetails ? renderTooltip(checks) : presentation.accessibleDescription

  return (
    <div
      className="flex w-full min-w-0 items-center justify-between gap-x-2 text-label-secondary"
      data-session-status-bar=""
    >
      <Tooltip label={tooltip} delayMs={150}>
        <BurnCheckStatus presentation={presentation} />
      </Tooltip>

      <div className="ml-auto shrink-0">
        {limitBadge ? (
          <SessionLimitBadge
            limitBadge={limitBadge}
            isHighCost={highCost ?? cost?.isHighCost === true}
          />
        ) : (
          cost && <SessionCostBadge {...cost} appearance={cost.isHighCost ? "pill" : "bare"} />
        )}
      </div>
    </div>
  )
}

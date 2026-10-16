import { ArrowRight } from "lucide-react"
import { type CSSProperties } from "react"

import { BurnCheckFlame } from "../../../components/burn-checks/BurnCheckFlames"
import { CheckStatusText } from "../../../components/burn-checks/CheckStatusText"
import { checkResultStatus } from "../../../lib/presentation/checkRowStatus"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { cn } from "../../../lib/cn"
import type { CheckId } from "../../../lib/insightsIpc"
import { agentIconName, GENERIC_AGENT_ICON } from "../../../lib/presentation/agents"
import { formatTokenBurnPercent } from "../../../lib/presentation/checkReport"
import {
  BurnCheckCategoryIcon,
  burnCheckCategoryColor,
} from "../burn-checks/BurnCheckCategoryIcon"
import { type FixCategory, type FixStatus } from "./overviewProgressStore"
import { useOverviewProgress } from "./useOverviewProgress"

/** Failing checks first, highest estimated burn first. Ties keep report order. */
const STATUS_ORDER: Record<FixStatus, number> = {
  needsFix: 0,
  awaitingVerification: 1,
  checking: 2,
  error: 3,
  waiting: 3,
  notChecked: 4,
  passing: 5,
  snoozed: 6,
}

function byUrgency(a: FixCategory, b: FixCategory): number {
  return (
    STATUS_ORDER[a.status] - STATUS_ORDER[b.status] ||
    (b.estimatedBurnBasisPoints ?? -1) - (a.estimatedBurnBasisPoints ?? -1)
  )
}

export function OverviewFixes({
  onOpenCheck,
}: {
  onOpenCheck: (check: CheckId | undefined) => void
}) {
  const progress = useOverviewProgress()
  return (
    <section aria-label="Checks" className="flex flex-col gap-(--space-sm)">
      <div className="flex items-baseline justify-between">
        <h2 className="type-caption text-label-secondary">Checks</h2>
        <button
          type="button"
          onClick={() => onOpenCheck(undefined)}
          className="inline-flex items-center gap-1 type-caption text-label-secondary hover:text-label hover:underline hover:underline-offset-[3px]"
        >
          All checks
          <ArrowRight size={12} strokeWidth={2} aria-hidden="true" />
        </button>
      </div>
      <ul className="grid grid-cols-[repeat(auto-fill,minmax(14rem,1fr))] gap-(--space-sm)">
        {[...progress.categories].sort(byUrgency).map((category) => (
          <CheckCard key={category.id} category={category} onOpen={onOpenCheck} />
        ))}
      </ul>
    </section>
  )
}

/** One check as a card: its icon, name, result, burn estimate, and agents. */
function CheckCard({
  category,
  onOpen,
}: {
  category: FixCategory
  onOpen: (check: CheckId) => void
}) {
  const needsFix = category.status === "needsFix"
  const status = checkResultStatus(
    {
      ...category,
      lifecycle:
        category.lifecycle ??
        (category.status === "awaitingVerification"
          ? "awaitingVerification"
          : category.status === "passing"
            ? "passing"
            : null),
    },
    category.status === "snoozed",
  )
  const agents = [
    ...new Map(
      category.agents
        .map((agent) => (agent === "claude" ? "claude-code" : agent))
        .filter((agent) => agentIconName(agent) !== GENERIC_AGENT_ICON)
        .map((agent) => [agentIconName(agent), agent]),
    ).values(),
  ]
  const burn = category.estimatedBurnBasisPoints
  return (
    <li className="flex">
      <button
        type="button"
        onClick={() => onOpen(category.id)}
        data-status={category.status}
        aria-label={`${category.label}, ${status.text}`}
        // On hover, a faint tint of the icon's colour lies over the card fill.
        style={{ "--check-tint": burnCheckCategoryColor(category.id) } as CSSProperties}
        className={cn(
          "session-card relative flex w-full items-start gap-3 overflow-hidden rounded-(--radius-popover) bg-session-card p-3 text-start transition-[filter] duration-fast hover:bg-linear-to-b hover:from-(--check-tint)/10 hover:to-(--check-tint)/10 active:brightness-95",
        )}
      >
        {agents.length > 0 && (
          <span
            aria-hidden="true"
            className="session-vendor-watermark pointer-events-none absolute -right-1.5 -bottom-1.5 flex gap-1"
          >
            {agents.map((agent) => (
              <span
                key={agentIconName(agent)}
                className="inline-flex size-10 items-center justify-center"
              >
                {renderAgentIcon(agent, 40, undefined, "neutral")}
              </span>
            ))}
          </span>
        )}
        <BurnCheckCategoryIcon detector={category.id} bare />
        <span className="relative z-10 flex min-w-0 flex-col gap-0.5">
          <span className="type-body font-medium! text-label">{category.label}</span>
          <CheckStatusText status={status} />
          {needsFix && burn != null && (
            <span className="inline-flex items-center gap-1 type-footnote tabular-nums text-label-tertiary">
              <BurnCheckFlame basisPoints={burn} />
              {formatTokenBurnPercent(burn)} estimated burn
            </span>
          )}
        </span>
      </button>
    </li>
  )
}

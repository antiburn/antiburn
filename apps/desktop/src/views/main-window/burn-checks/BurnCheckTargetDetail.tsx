import type { BurnCheckTargetPayload } from "../../../lib/insightsIpc"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { formatApiEquivalentUsd } from "../../../lib/presentation/checks"
import { CHECK_UI } from "../../checks/checkUi"
import { BurnCheckTargetActions } from "./BurnCheckTargetActions"
import {
  ActionLimit,
  SampleSessions,
  targetTitle,
  watchStatus,
} from "./BurnCheckTargetPresentation"

/** The cache-read waste line for one target, summed across every currently
 * published session for it (`target.occurrenceCount`). Returns null unless
 * the estimate resolved to a priced dollar figure. */
export function targetCostLine(target: BurnCheckTargetPayload): string | null {
  const opportunity = target.display.estimatedOpportunity
  if (!opportunity || opportunity.unit !== "apiEquivalentUsd") return null
  const sessions = target.occurrenceCount
  return `${formatApiEquivalentUsd(opportunity.value)} in cache reads of this unused definition across ${sessions} session${sessions === 1 ? "" : "s"}, sub-agent requests included.`
}

export function BurnCheckTargetDetail({
  target,
  refresh,
}: {
  target: BurnCheckTargetPayload
  refresh: () => void
}) {
  const status = watchStatus(target)
  const guidance = CHECK_UI[target.finding.detector]
  const costLine = targetCostLine(target)
  return (
    <article className="min-w-0 rounded-control bg-surface-card/75 p-4">
      <div className="flex min-w-0 items-start gap-2">
        {renderAgentIcon(target.finding.agent, 16)}
        <h3 className="min-w-0 wrap-anywhere type-title-3 text-label">{targetTitle(target)}</h3>
      </div>
      <p className="mt-2 type-body text-label-secondary">{guidance.recommendation}</p>
      {costLine && (
        <p className="mt-1 type-callout tabular-nums text-label-secondary">{costLine}</p>
      )}
      <BurnCheckTargetActions target={target} refresh={refresh} />
      <ActionLimit target={target} />
      {status && (
        <p role="status" className="mt-2 type-callout text-label-secondary">
          {status}
        </p>
      )}
      <SampleSessions samples={target.samples} />
    </article>
  )
}

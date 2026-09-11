import type { BurnCheckTargetPayload } from "../../../lib/insightsIpc"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { CHECK_UI } from "../../checks/checkUi"
import { BurnCheckTargetActions } from "./BurnCheckTargetActions"
import {
  ActionLimit,
  SampleSessions,
  targetTitle,
  watchStatus,
} from "./BurnCheckTargetPresentation"

export function BurnCheckTargetDetail({
  target,
  refresh,
}: {
  target: BurnCheckTargetPayload
  refresh: () => void
}) {
  const status = watchStatus(target)
  const guidance = CHECK_UI[target.finding.detector]
  return (
    <article className="min-w-0 rounded-control border border-separator bg-surface-card p-4">
      <div className="flex min-w-0 items-start gap-2">
        {renderAgentIcon(target.finding.agent, 16)}
        <h3 className="min-w-0 wrap-anywhere type-title-3 text-label">{targetTitle(target)}</h3>
      </div>
      <p className="mt-2 type-body text-label-secondary">{guidance.recommendation}</p>
      <BurnCheckTargetActions target={target} refresh={refresh} />
      <ActionLimit target={target} />
      {status && (
        <p role="status" className="mt-2 type-callout text-label-secondary">
          {status}
        </p>
      )}
      <SampleSessions target={target} />
    </article>
  )
}

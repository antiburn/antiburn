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
    <article className="px-4 py-4">
      <div className="flex min-w-0 items-center gap-2">
        {renderAgentIcon(target.finding.agent, 16)}
        <h3 className="type-body font-semibold! text-label">{targetTitle(target)}</h3>
      </div>
      <p className="mt-1 type-callout text-label-secondary">{guidance.recommendation}</p>
      <BurnCheckTargetActions target={target} refresh={refresh} />
      <ActionLimit target={target} />
      {status && (
        <p role="status" className="mt-2 type-footnote text-label-secondary">
          {status}
        </p>
      )}
      <SampleSessions samples={target.samples} />
    </article>
  )
}

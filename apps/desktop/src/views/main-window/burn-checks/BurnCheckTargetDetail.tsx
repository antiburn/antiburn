import { Folder } from "lucide-react"
import { InfoPopover } from "../../../components/presentation/InfoPopover"
import type { BurnCheckDetectorId, BurnCheckTargetPayload } from "../../../lib/insightsIpc"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { formatApiEquivalentUsd } from "../../../lib/presentation/checks"
import { CHECK_UI } from "../../checks/checkUi"
import { RemindLaterAction } from "./RemindLaterAction"
import { BurnCheckTargetActions } from "./BurnCheckTargetActions"
import {
  ActionLimit,
  SampleSessions,
  scopeLabel,
  targetTitle,
  watchStatus,
} from "./BurnCheckTargetPresentation"

/** Formats priced cache-read waste with the most accurate available impact count. */
export function targetCostLine(target: BurnCheckTargetPayload): string | null {
  const opportunity = target.display.estimatedOpportunity
  if (!opportunity || opportunity.unit !== "apiEquivalentUsd") return null
  if (target.affectedSessionCount != null) {
    const sessions = target.affectedSessionCount
    return `${formatApiEquivalentUsd(opportunity.value)} in cache reads of this unused definition across ${sessions} session${sessions === 1 ? "" : "s"}, sub-agent requests included.`
  }
  const occurrences = target.occurrenceCount
  return `${formatApiEquivalentUsd(opportunity.value)} in cache reads of this unused definition across ${occurrences} occurrence${occurrences === 1 ? "" : "s"}, sub-agent requests included.`
}

export function BurnCheckTargetDetail({
  target,
  detector,
  refresh,
  reportRow = false,
}: {
  target: BurnCheckTargetPayload
  detector?: BurnCheckDetectorId
  refresh: () => void
  reportRow?: boolean
}) {
  const status = watchStatus(target)
  const guidance = CHECK_UI[target.finding.detector]
  const costLine = targetCostLine(target)
  return (
    <article
      className={
        reportRow
          ? "burn-check-resource min-w-0"
          : "min-w-0 rounded-control bg-surface-card/75 p-4"
      }
    >
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-4 gap-y-1">
        <div className="min-w-0 flex-1 basis-56">
          <h3 className="burn-check-resource-title type-title-3 text-label">
            <span className="burn-check-resource-icon">
              {renderAgentIcon(target.finding.agent, 16)}
            </span>
            <span className="min-w-0 wrap-anywhere">{targetTitle(target)}</span>
          </h3>
          <div className="burn-check-resource-metadata min-w-0">
            <div className="flex flex-wrap items-center type-callout text-label-tertiary">
              <span>
                {reportRow && target.display.scopeKind === "project"
                  ? "Project"
                  : scopeLabel(target.display.scopeKind)}
                {reportRow && target.projectName && (
                  <span className="text-label"> · {target.projectName}</span>
                )}
              </span>
              {reportRow && target.projectLocation && (
                <InfoPopover
                  label="Folder location"
                  icon={<Folder size={14} aria-hidden="true" />}
                >
                  {() => (
                    <>
                      <h4 className="type-headline text-label">Folder location</h4>
                      <p className="mt-2 wrap-anywhere font-mono type-footnote text-label-secondary">
                        {target.projectLocation}
                      </p>
                    </>
                  )}
                </InfoPopover>
              )}
            </div>
          </div>
        </div>
        {reportRow && (
          <div className="flex flex-wrap items-start justify-end gap-2">
            {detector && <RemindLaterAction detector={detector} />}
            <BurnCheckTargetActions target={target} refresh={refresh} compact embedded />
          </div>
        )}
      </div>
      <div className={reportRow ? "burn-check-resource-body" : undefined}>
        {reportRow ? (
          <p className="mt-1 type-callout tabular-nums text-label-secondary">
            {target.affectedSessionCount != null
              ? `${target.affectedSessionCount} ${target.affectedSessionCount === 1 ? "session" : "sessions"} affected`
              : "Affected-session count unavailable"}
          </p>
        ) : (
          <p className="mt-2 type-body text-pretty text-label-secondary">
            {guidance.recommendation}
          </p>
        )}
        {costLine && (
          <p className="mt-1 type-callout tabular-nums text-label-secondary">{costLine}</p>
        )}
        {!reportRow && <BurnCheckTargetActions target={target} refresh={refresh} />}
        <ActionLimit target={target} />
        {status && (
          <p role="status" className="mt-2 type-callout text-label-secondary">
            {status}
          </p>
        )}
        <SampleSessions
          samples={target.samples}
          {...(reportRow && target.affectedSessionCount != null
            ? { affectedSessionCount: target.affectedSessionCount }
            : {})}
          insetRows={reportRow}
        />
      </div>
    </article>
  )
}

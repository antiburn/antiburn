import { useState } from "react"

import { ProjectFolderActions } from "../../../components/session/ProjectFolderActions"
import { performProjectFolderAction } from "../../../lib/projectFolder"
import "../../../styles/session-detail.css"
import {
  getBurnCheckTargetEvidence,
  openBurnCheckSample,
  type BurnCheckTargetEvidencePayload,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { formatApiEquivalentUsd } from "../../../lib/presentation/checks"
import { CHECK_UI } from "../../checks/checkUi"
import { BurnCheckTargetActions } from "./BurnCheckTargetActions"
import {
  FailedSessions,
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
  refresh,
  reportRow = false,
}: {
  target: BurnCheckTargetPayload
  refresh: () => void
  reportRow?: boolean
}) {
  const [evidenceState, setEvidenceState] = useState<{
    actionId: string
    status: "loading" | "loaded" | "failed"
    evidence?: BurnCheckTargetEvidencePayload
  } | null>(null)
  const status = watchStatus(target)
  const guidance = CHECK_UI[target.finding.detector]
  const costLine = targetCostLine(target)
  const projectPath = target.projectPath
  const sample = target.samples[0]
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
        </div>
      </div>
      <div className="burn-check-resource-metadata min-w-0">
        <div className="flex items-center gap-1.5 type-callout text-label-tertiary">
          <span className="min-w-0 wrap-anywhere">
            {scopeLabel(target.display.scopeKind)}
            {target.configFile && ` (${target.configFile})`}
            {reportRow && target.projectName && (
              <span className="text-label"> · {target.projectName}</span>
            )}
          </span>
          {reportRow && projectPath && (
            <ProjectFolderActions
              key={projectPath}
              path={projectPath}
              onOpen={() => performProjectFolderAction(projectPath, "open")}
              onCopy={() => performProjectFolderAction(projectPath, "copy")}
            />
          )}
        </div>
      </div>
      <div className={reportRow ? "burn-check-resource-body" : undefined}>
        {reportRow && target.affectedSessionCount != null ? (
          <p className="mt-1 type-callout tabular-nums text-label-secondary">
            {`${target.affectedSessionCount} ${target.affectedSessionCount === 1 ? "session" : "sessions"} affected`}
          </p>
        ) : !reportRow ? (
          <div className="mt-2 space-y-1">
            {target.finding.certainty && (
              <p className="type-callout font-medium text-label">
                {target.finding.certainty === "likely" ? "Likely conflict" : "Possible conflict"}
              </p>
            )}
            <p className="type-body text-pretty text-label-secondary">
              {target.finding.certainty === "possible"
                ? "This may go against the instruction. Review the evidence; it does not prove the agent broke the rule."
                : guidance.recommendation}
            </p>
          </div>
        ) : null}
        {costLine && (
          <p className="mt-1 type-callout tabular-nums text-label-secondary">{costLine}</p>
        )}
        {!reportRow && <BurnCheckTargetActions target={target} refresh={refresh} />}
        {!reportRow && target.evidenceAvailable && (
          <section className="mt-3 border-t border-separator pt-3">
            <button
              type="button"
              className="burn-check-action type-callout"
              aria-expanded={evidenceState?.actionId === target.actionId}
              onClick={() => {
                if (evidenceState?.actionId === target.actionId) {
                  setEvidenceState(null)
                  return
                }
                setEvidenceState({ actionId: target.actionId, status: "loading" })
                void getBurnCheckTargetEvidence(target.actionId).then(
                  (evidence) =>
                    setEvidenceState({
                      actionId: target.actionId,
                      status: evidence ? "loaded" : "failed",
                      ...(evidence ? { evidence } : {}),
                    }),
                  () => setEvidenceState({ actionId: target.actionId, status: "failed" }),
                )
              }}
            >
              {evidenceState?.actionId === target.actionId ? "Hide details" : "View details"}
            </button>
            {evidenceState?.actionId === target.actionId && (
              <div className="mt-3 space-y-3">
                {evidenceState.status === "loading" && (
                  <p role="status" className="type-callout text-label-secondary">
                    Loading details…
                  </p>
                )}
                {evidenceState.status === "failed" && (
                  <p role="alert" className="type-callout text-label-secondary">
                    Details could not be loaded. Close and try again.
                  </p>
                )}
                {evidenceState.status === "loaded" &&
                  (evidenceState.evidence?.status === "unavailable" ? (
                    <p role="status" className="type-callout text-label-secondary">
                      This session or instruction file changed. These details are no longer available.
                    </p>
                  ) : (
                    <ol className="space-y-3">
                      {evidenceState.evidence?.items.map((item) => (
                        <li key={`${item.label}:${item.reference}`} className="space-y-1">
                          <p className="type-callout font-medium text-label">
                            {item.label === "observedAction"
                              ? "Action"
                              : item.label === "context"
                                ? "Context"
                                : `Instruction · ${item.sourceLabel}${item.startLine ? ` · lines ${item.startLine}-${item.endLine}` : ""}`}
                          </p>
                          <p className="type-callout text-label-secondary">{item.explanation}</p>
                          <p className="whitespace-pre-wrap break-words rounded-control bg-surface-card p-3 type-callout text-label">
                            {item.excerpt}
                          </p>
                          {item.observedAtMs != null && (
                            <p className="type-caption text-label-tertiary">
                              {new Date(item.observedAtMs).toLocaleString()}
                            </p>
                          )}
                          {item.limitation && (
                            <p className="type-callout text-label-secondary">{item.limitation}</p>
                          )}
                        </li>
                      ))}
                    </ol>
                  ))}
                {sample && (
                  <button
                    type="button"
                    className="burn-check-action type-callout"
                    onClick={() => void openBurnCheckSample(sample.navigationHandle)}
                  >
                    Open this session
                  </button>
                )}
              </div>
            )}
          </section>
        )}
        {status && (
          <p role="status" className="mt-2 type-callout text-label-secondary">
            {status}
          </p>
        )}
        <FailedSessions
          samples={target.samples}
          {...(target.finding.detector === "ignoredInstructions"
            ? { label: "Sessions to review" }
            : {})}
          {...(target.affectedSessionCount != null
            ? { total: target.affectedSessionCount }
            : {})}
        />
      </div>
    </article>
  )
}

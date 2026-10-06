import { useCallback, useRef, useState } from "react"

import { ProjectFolderActions } from "../../../components/session/ProjectFolderActions"
import { performProjectFolderAction } from "../../../lib/projectFolder"
import { noteInteraction } from "../../../lib/ipc"
import "../../../styles/session-detail.css"
import {
  getBurnCheckTargetEvidence,
  type BurnCheckTargetEvidencePayload,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { formatApiEquivalentUsd } from "../../../lib/presentation/checkReport"
import { CHECK_UI } from "../../checks/checkPresentation"
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

function EvidenceExcerpt({
  item,
  showMetadata = true,
}: {
  item: BurnCheckTargetEvidencePayload["items"][number]
  showMetadata?: boolean
}) {
  let text = item.excerpt
  if (item.label === "observedAction") {
    try {
      const action: unknown = JSON.parse(text)
      if (
        typeof action === "object" &&
        action !== null &&
        "command" in action &&
        typeof action.command === "string"
      ) {
        text = action.command
      }
    } catch {
      // Keep the original action when it is not a JSON object.
    }
  }
  const compact = text.replace(/\s+/g, " ").trim()
  return (
    <div className="space-y-1">
      {showMetadata && (
        <p className="type-caption text-label-tertiary">
          {item.sourceLabel}
          {item.explanation ? ` · ${item.explanation}` : ""}
        </p>
      )}
      <p className="break-words rounded-control bg-surface-card px-3 py-2 type-callout text-label">
        {compact}
      </p>
    </div>
  )
}

function orderedEvidence(items: BurnCheckTargetEvidencePayload["items"]) {
  const rank = { instruction: 0, observedAction: 1, context: 2 }
  return [...items].sort(
    (a, b) =>
      rank[a.label] - rank[b.label] ||
      (a.label === "context" && b.label === "context"
        ? (a.observedAtMs ?? 0) - (b.observedAtMs ?? 0)
        : 0),
  )
}

type EvidenceState = {
  findingId: string
  actionId: string
  status: "loading" | "loaded" | "failed"
  evidence?: BurnCheckTargetEvidencePayload
}

function IgnoredInstructionEvidence({
  target,
  state,
  sourcePath,
  retry,
}: {
  target: BurnCheckTargetPayload
  state: EvidenceState
  sourcePath: string | null
  retry: () => void
}) {
  const items =
    state.status === "loaded" && state.evidence?.status === "available"
      ? state.evidence.items
      : []
  const instruction = items.find((item) => item.label === "instruction")
  const action = items.find((item) => item.label === "observedAction")

  return (
    <div className="space-y-3">
      <div className="space-y-1">
        <p className="type-callout font-medium text-label">Instruction</p>
        {instruction ? (
          <>
            <p className="type-caption text-label-tertiary">
              {instruction.sourceLabel}
              {instruction.startLine != null &&
                ` · line${instruction.endLine !== instruction.startLine ? "s" : ""} ${instruction.startLine}${instruction.endLine !== instruction.startLine ? `–${instruction.endLine}` : ""}`}
            </p>
            <EvidenceExcerpt item={instruction} showMetadata={false} />
          </>
        ) : (
          <p
            role={state.status === "loading" ? "status" : undefined}
            className="type-callout text-label-secondary"
          >
            {state.status === "loading"
              ? "Loading the instruction…"
              : (target.display.instructionTitle ??
                sourcePath ??
                "Instruction cited by this check")}
          </p>
        )}
      </div>
      <div className="space-y-1">
        <p className="type-callout font-medium text-label">Where it was ignored</p>
        {action ? (
          <>
            <EvidenceExcerpt item={action} showMetadata={false} />
            {action.observedAtMs != null && (
              <time
                dateTime={new Date(action.observedAtMs).toISOString()}
                className="type-caption text-label-tertiary"
              >
                {new Date(action.observedAtMs).toLocaleString()}
              </time>
            )}
          </>
        ) : (
          <p
            role={state.status === "loading" ? "status" : undefined}
            className="type-callout text-label-secondary"
          >
            {state.status === "loading"
              ? "Loading the cited session action…"
              : target.finding.observation}
          </p>
        )}
      </div>
      {items.some((item) => item.limitation === outdatedInstructionsNote) && (
        <p className="type-caption text-label-tertiary">{outdatedInstructionsNote}</p>
      )}
      {state.status === "failed" && (
        <div>
          <p role="alert" className="type-callout text-label-secondary">
            Could not load the saved excerpts.
          </p>
          <button type="button" className="burn-check-action mt-2 type-callout" onClick={retry}>
            Retry
          </button>
        </div>
      )}
      {state.status === "loaded" && state.evidence?.status === "unavailable" && (
        <p role="status" className="type-callout text-label-tertiary">
          The exact instruction and action text was not saved with this finding.
        </p>
      )}
    </div>
  )
}

function instructionSourcePath(target: BurnCheckTargetPayload): string | null {
  const source = target.display.resourceIdentity
  if (!source) return null
  if (source.startsWith("home:")) return `~/${source.slice("home:".length)}`
  if (source.startsWith("project:")) {
    const relative = source.slice("project:".length).replace(/^\.\//, "")
    return target.projectPath ? `${target.projectPath}/${relative}` : `./${relative}`
  }
  return source
}

const outdatedInstructionsNote = "Note: this session may have run on outdated instructions."

export function BurnCheckTargetDetail({
  target,
  refresh,
  reportRow = false,
  openEvidence = false,
}: {
  target: BurnCheckTargetPayload
  refresh: () => void
  reportRow?: boolean
  openEvidence?: boolean
}) {
  const [evidenceState, setEvidenceState] = useState<EvidenceState | null>(null)
  const [showContext, setShowContext] = useState(false)
  const request = useRef(0)
  const currentTargetElement = useRef<HTMLElement | null>(null)
  const autoLoadedFinding = useRef<string | null>(null)
  const loadedFinding = useRef<string | null>(null)
  const loadEvidence = useCallback((actionId: string, findingId: string, detector: string) => {
    const token = ++request.current
    setEvidenceState({ findingId, actionId, status: "loading" })
    void getBurnCheckTargetEvidence(actionId).then(
      (evidence) => {
        if (
          token !== request.current ||
          currentTargetElement.current?.dataset.findingId !== findingId
        )
          return
        if (detector === "ignoredInstructions")
          noteInteraction({
            kind: "ignoredInstructionObserved",
            stage: "evidence",
            outcome: evidence
              ? evidence.status === "available"
                ? "available"
                : "unavailable"
              : "failed",
          })
        setEvidenceState({
          findingId,
          actionId,
          status: evidence ? "loaded" : "failed",
          ...(evidence ? { evidence } : {}),
        })
        if (evidence) loadedFinding.current = findingId
      },
      () => {
        if (
          token === request.current &&
          currentTargetElement.current?.dataset.findingId === findingId
        ) {
          if (detector === "ignoredInstructions")
            noteInteraction({
              kind: "ignoredInstructionObserved",
              stage: "evidence",
              outcome: "failed",
            })
          setEvidenceState({ findingId, actionId, status: "failed" })
        }
      },
    )
  }, [])
  const mounted = useCallback(
    (node: HTMLElement | null) => {
      if (!node) {
        request.current += 1
        if (autoLoadedFinding.current === currentTargetElement.current?.dataset.findingId) {
          autoLoadedFinding.current = null
        }
        currentTargetElement.current = null
        return
      }
      currentTargetElement.current = node
      const findingId = node.dataset.findingId ?? ""
      if (
        openEvidence &&
        target.evidenceAvailable &&
        autoLoadedFinding.current !== findingId &&
        loadedFinding.current !== findingId
      ) {
        autoLoadedFinding.current = findingId
        loadEvidence(
          node.dataset.evidenceActionId ?? "",
          findingId,
          node.dataset.detector ?? "",
        )
      }
    },
    [target.evidenceAvailable, openEvidence, loadEvidence],
  )
  const ignoredInstructions = target.finding.detector === "ignoredInstructions"
  const hasInstructionExcerpt =
    evidenceState?.status === "loaded" &&
    evidenceState.evidence?.status === "available" &&
    evidenceState.evidence.items.some((item) => item.label === "instruction")
  const status = watchStatus(target)
  const guidance = CHECK_UI[target.finding.detector]
  const costLine = targetCostLine(target)
  const projectPath = target.projectPath
  const sourcePath = ignoredInstructions ? instructionSourcePath(target) : target.configFile
  const hasEvidenceSelection = evidenceState?.findingId === target.findingId
  return (
    <article
      ref={mounted}
      data-evidence-action-id={target.actionId}
      data-finding-id={target.findingId}
      data-detector={target.finding.detector}
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
            {sourcePath &&
              (!ignoredInstructions || !hasInstructionExcerpt) &&
              ` (${sourcePath})`}
            {reportRow && target.projectName && (
              <span className="text-label"> · {target.projectName}</span>
            )}
          </span>
          {reportRow && projectPath && (
            <ProjectFolderActions
              key={projectPath}
              path={projectPath}
              onOpen={() =>
                performProjectFolderAction(projectPath, "open", {
                  kind: "burnCheck",
                  actionId: target.actionId,
                })
              }
              onCopy={() =>
                performProjectFolderAction(projectPath, "copy", {
                  kind: "burnCheck",
                  actionId: target.actionId,
                })
              }
            />
          )}
        </div>
      </div>
      <div className={reportRow ? "burn-check-resource-body" : undefined}>
        {!reportRow ? (
          <div className="mt-2 space-y-1">
            <p className="type-body text-pretty text-label-secondary">
              {target.finding.detector === "ignoredInstructions"
                ? "Instructions were ignored in these sessions."
                : guidance.recommendation}
            </p>
          </div>
        ) : null}
        {costLine && (
          <p className="mt-1 type-callout tabular-nums text-label-secondary">{costLine}</p>
        )}
        {!reportRow && <BurnCheckTargetActions target={target} refresh={refresh} />}
        {target.evidenceAvailable && (
          <section
            className={ignoredInstructions ? "mt-3" : "mt-3 border-t border-separator pt-3"}
            aria-label="Evidence"
          >
            {!openEvidence && (
              <button
                type="button"
                className="burn-check-action type-callout"
                aria-expanded={hasEvidenceSelection}
                onClick={() => {
                  if (hasEvidenceSelection) {
                    request.current += 1
                    setEvidenceState(null)
                    setShowContext(false)
                    return
                  }
                  loadEvidence(target.actionId, target.findingId, target.finding.detector)
                }}
              >
                {hasEvidenceSelection ? "Hide details" : "Show evidence"}
              </button>
            )}
            {hasEvidenceSelection && (
              <div className="mt-3 space-y-3">
                {ignoredInstructions ? (
                  <IgnoredInstructionEvidence
                    target={target}
                    state={evidenceState}
                    sourcePath={sourcePath ?? null}
                    retry={() =>
                      loadEvidence(target.actionId, target.findingId, target.finding.detector)
                    }
                  />
                ) : (
                  <>
                    {evidenceState.status === "loading" && (
                      <p role="status" className="type-callout text-label-secondary">
                        Loading evidence…
                      </p>
                    )}
                    {evidenceState.status === "failed" && (
                      <div>
                        <p role="alert" className="type-callout text-label-secondary">
                          Could not load evidence.
                        </p>
                        <button
                          type="button"
                          className="burn-check-action mt-2 type-callout"
                          onClick={() =>
                            loadEvidence(
                              target.actionId,
                              target.findingId,
                              target.finding.detector,
                            )
                          }
                        >
                          Retry
                        </button>
                      </div>
                    )}
                    {evidenceState.status === "loaded" &&
                      (evidenceState.evidence?.status === "unavailable" ? (
                        <p role="status" className="type-callout text-label-secondary">
                          The original evidence is no longer available.
                        </p>
                      ) : (
                        <div>
                          <ol className="space-y-3">
                            {orderedEvidence(evidenceState.evidence?.items ?? []).map(
                              (item) =>
                                (item.label !== "context" || showContext) && (
                                  <li
                                    key={`${item.label}:${item.reference}`}
                                    className="space-y-1"
                                  >
                                    <div className="flex min-w-0 items-baseline justify-between gap-3">
                                      <p className="type-callout font-medium text-label">
                                        {item.label === "observedAction"
                                          ? "Session action"
                                          : item.label === "context"
                                            ? "Context"
                                            : `Instruction · ${item.sourceLabel}${item.startLine ? ` · line${item.endLine !== item.startLine ? "s" : ""} ${item.startLine}${item.endLine !== item.startLine ? `–${item.endLine}` : ""}` : ""}`}
                                      </p>
                                      {item.observedAtMs != null && (
                                        <time
                                          dateTime={new Date(item.observedAtMs).toISOString()}
                                          className="shrink-0 type-caption text-label-tertiary"
                                        >
                                          {new Date(item.observedAtMs).toLocaleString()}
                                        </time>
                                      )}
                                    </div>
                                    {item.label === "instruction" &&
                                    item.excerpt === "Instruction text unavailable." ? (
                                      <p className="type-callout text-label-secondary">
                                        Unavailable or changed since this assessment.
                                      </p>
                                    ) : (
                                      <EvidenceExcerpt item={item} />
                                    )}
                                    {item.limitation &&
                                      item.excerpt !== "Instruction text unavailable." &&
                                      item.limitation !== outdatedInstructionsNote && (
                                        <p className="type-callout text-label-secondary">
                                          {item.limitation}
                                        </p>
                                      )}
                                  </li>
                                ),
                            )}
                          </ol>
                        </div>
                      ))}
                    {evidenceState.status === "loaded" &&
                      evidenceState.evidence?.status === "available" &&
                      evidenceState.evidence.items.some((item) => item.label === "context") && (
                        <button
                          type="button"
                          className="burn-check-action type-callout"
                          aria-expanded={showContext}
                          onClick={() => setShowContext((visible) => !visible)}
                        >
                          {showContext ? "Hide context" : "Show context"}
                        </button>
                      )}
                  </>
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
        <div className={reportRow && ignoredInstructions ? "mt-4" : undefined}>
          {reportRow && target.affectedSessionCount != null && (
            <p className="mb-1 type-callout tabular-nums text-label-secondary">
              {`${target.affectedSessionCount} ${target.affectedSessionCount === 1 ? "session" : "sessions"} affected`}
            </p>
          )}
          <FailedSessions
            samples={target.samples}
            {...(target.affectedSessionCount != null
              ? { total: target.affectedSessionCount }
              : {})}
          />
        </div>
      </div>
    </article>
  )
}

import { useCallback, useId, useRef, useState } from "react"
import { Info } from "lucide-react"

import { ProjectFolderActions } from "../../../components/session/ProjectFolderActions"
import { Tooltip } from "../../../components/presentation/Tooltip"
import { Skeleton } from "../../../components/ui/Skeleton"
import { performProjectFolderAction } from "../../../lib/projectFolder"
import { noteInteraction, smartCheckForDetector } from "../../../lib/ipc"
import "../../../styles/session-detail.css"
import {
  getBurnCheckTargetEvidence,
  type BurnCheckTargetEvidencePayload,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { formatApiEquivalentUsd } from "../../../lib/presentation/checkReport"
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
    return `Estimated ${formatApiEquivalentUsd(opportunity.value)} in cache reads across ${sessions} session${sessions === 1 ? "" : "s"}, including sub-agent requests.`
  }
  const occurrences = target.occurrenceCount
  return `Estimated ${formatApiEquivalentUsd(opportunity.value)} in cache reads across ${occurrences} occurrence${occurrences === 1 ? "" : "s"}, including sub-agent requests.`
}

function EvidenceExcerpt({
  item,
  showMetadata = true,
  showExplanation = true,
  citationPrefix,
  collapseLong = false,
}: {
  item: BurnCheckTargetEvidencePayload["items"][number]
  showMetadata?: boolean
  showExplanation?: boolean
  citationPrefix?: string
  collapseLong?: boolean
}) {
  const fullText = item.excerpt
  const display = item.label === "observedAction" ? actionDisplay(item) : null
  const shownText = display?.text ?? fullText
  const preview = shownText.length > 360 ? `${shownText.slice(0, 360).trimEnd()}…` : shownText
  const excerpt = (text: string) => (
    <pre
      id={
        citationPrefix ? `${citationPrefix}-${encodeURIComponent(item.reference)}` : undefined
      }
      tabIndex={citationPrefix ? -1 : undefined}
      className="whitespace-pre-wrap break-words rounded-control bg-surface-card px-3 py-2 type-callout text-label"
    >
      {text}
    </pre>
  )
  return (
    <div className="space-y-1">
      {showMetadata && (
        <p className="type-caption text-label-tertiary">
          {item.sourceLabel}
          {showExplanation && item.explanation ? ` · ${item.explanation}` : ""}
        </p>
      )}
      {collapseLong &&
      (display?.alwaysCollapse || ((display?.collapse ?? true) && shownText.length > 360)) ? (
        <>
          {excerpt(preview)}
          <details>
            <summary className="burn-check-action type-caption">
              {display?.alwaysCollapse ? "Show file contents" : "Show full details"}
            </summary>
            {excerpt(display?.detail ?? shownText)}
          </details>
        </>
      ) : (
        excerpt(shownText)
      )}
    </div>
  )
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value != null && typeof value === "object" && !Array.isArray(value)
}

function records(value: unknown): Record<string, unknown>[] {
  if (Array.isArray(value)) return value.flatMap(records)
  if (!isRecord(value)) return []
  return [value, ...Object.values(value).flatMap(records)]
}

function strings(value: unknown): string | null {
  if (typeof value === "string") return value
  if (Array.isArray(value) && value.every((part) => typeof part === "string")) {
    return value.join(" ")
  }
  return null
}

function actionDisplay(item: BurnCheckTargetEvidencePayload["items"][number]) {
  const source = item.sourceLabel.toLowerCase()
  let parsed: unknown
  try {
    parsed = JSON.parse(item.excerpt)
  } catch {
    const filePath = item.excerpt.match(/<path>([^<]+)<\/path>/)?.[1]
    if (source.includes("read")) {
      return {
        text: filePath ? `Contents of ${filePath}` : "Read result",
        detail: item.excerpt,
        collapse: false,
        alwaysCollapse: true,
      }
    }
    return { text: item.excerpt, collapse: true, alwaysCollapse: false }
  }
  const objects = records(parsed)
  const value = (...keys: string[]) => {
    for (const object of objects) {
      for (const key of keys) {
        const text = strings(object[key])
        if (text) return text
      }
    }
    return null
  }
  const isRead =
    source.includes("read") ||
    objects.some((object) =>
      [object.type, object.name, object.tool].some(
        (tool) => typeof tool === "string" && tool.toLowerCase() === "read",
      ),
    )
  const path = value("file_path", "filePath", "path", "paths")
  if (path && isRead) {
    const numeric = (key: string) =>
      objects.map((object) => object[key]).find((part) => typeof part === "number")
    const offset = numeric("offset") ?? numeric("startLine")
    const limit = numeric("limit")
    const range =
      typeof offset === "number"
        ? `, line ${offset}${typeof limit === "number" && limit > 1 ? `–${offset + limit - 1}` : ""}`
        : ""
    return { text: `Read: ${path}${range}`, collapse: false, alwaysCollapse: false }
  }
  const command = value("cmd", "command")
  if (command) {
    return {
      text: `Command: ${command}`,
      collapse: true,
      alwaysCollapse: false,
    }
  }
  if (path) {
    const numeric = (key: string) =>
      objects.map((object) => object[key]).find((part) => typeof part === "number")
    const offset = numeric("offset") ?? numeric("startLine")
    const limit = numeric("limit")
    const range =
      typeof offset === "number"
        ? `, line ${offset}${typeof limit === "number" && limit > 1 ? `–${offset + limit - 1}` : ""}`
        : ""
    const verb = source.includes("read")
      ? "Read"
      : source.includes("edit") || source.includes("write")
        ? "Changed"
        : "Path"
    return { text: `${verb}: ${path}${range}`, collapse: false, alwaysCollapse: false }
  }
  const query = value("query", "pattern")
  if (query) return { text: `Searched for: ${query}`, collapse: true, alwaysCollapse: false }
  return { text: item.excerpt, collapse: true, alwaysCollapse: false }
}

function orderedEvidence(items: BurnCheckTargetEvidencePayload["items"]) {
  const rank = { instruction: 0, observedAction: 1, context: 2 }
  return [...items].sort(
    (a, b) =>
      rank[a.label] - rank[b.label] ||
      (a.label !== "instruction" && b.label !== "instruction"
        ? (a.observedAtMs ?? 0) - (b.observedAtMs ?? 0)
        : 0),
  )
}

function evidenceHeading(
  detector: BurnCheckTargetPayload["finding"]["detector"],
  item: BurnCheckTargetEvidencePayload["items"][number],
) {
  if (item.label === "observedAction") {
    if (detector === "ignoredInstructions") return "What happened"
    if (detector === "overExploring") return "Files read"
    return "Work performed"
  }
  if (item.label === "context") return "Recorded task scope"
  const kind =
    detector === "scopeCreep"
      ? "Requested task"
      : detector === "skillOpportunities"
        ? "Relevant skill"
        : "Current instruction"
  const range =
    item.startLine == null
      ? ""
      : ` · line${item.endLine != null && item.endLine !== item.startLine ? "s" : ""} ${item.startLine}${item.endLine != null && item.endLine !== item.startLine ? `–${item.endLine}` : ""}`
  return `${kind} · ${item.sourceLabel}${range}`
}

export function EvidenceLimitsIcon({
  details,
  label = "About this evidence",
}: {
  details: string[]
  label?: string
}) {
  if (details.length === 0) return null
  return (
    <Tooltip
      label={
        <ul className="list-disc space-y-1 pl-4 type-callout font-normal!">
          {[...new Set(details)].map((detail) => (
            <li key={detail}>{detail}</li>
          ))}
        </ul>
      }
    >
      <span
        tabIndex={0}
        className="inline-flex shrink-0 text-label-tertiary"
        aria-label={label}
      >
        <Info size={14} aria-hidden="true" />
      </span>
    </Tooltip>
  )
}

export function SmartCheckEvidenceSkeleton() {
  return (
    <div role="status" aria-label="Loading evidence" className="space-y-3">
      <span className="sr-only">Loading evidence…</span>
      <Skeleton className="h-16 w-full" />
      <div className="space-y-2">
        <Skeleton className="h-3 w-24" />
        <Skeleton className="h-3 w-48 max-w-full" />
        <Skeleton className="h-16 w-full" />
      </div>
      <div className="space-y-2">
        <Skeleton className="h-3 w-24" />
        <Skeleton className="h-20 w-full" />
      </div>
    </div>
  )
}

type EvidenceState = {
  findingId: string
  actionId: string
  status: "loading" | "loaded" | "failed"
  evidence?: BurnCheckTargetEvidencePayload
  snapshot?: {
    target: BurnCheckTargetPayload
    evidence: BurnCheckTargetEvidencePayload
    occurrenceId?: string
  }
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
  const citationId = useId()
  const citationPrefix = `${citationId}-${encodeURIComponent(state.actionId)}`
  const items =
    state.status === "loaded" && state.evidence?.status === "available"
      ? state.evidence.items
      : []
  const instruction = items.find((item) => item.label === "instruction")
  const action = items.find((item) => item.label === "observedAction")
  return (
    <div className="space-y-3">
      <div className="space-y-1">
        <p className="type-callout font-medium! text-label">
          {instruction?.limitation?.includes("outdated instructions")
            ? "Current instruction"
            : "Instruction"}
        </p>
        {instruction ? (
          <>
            <p className="type-caption text-label-tertiary">
              {instruction.sourceLabel}
              {instruction.startLine != null &&
                ` · line${instruction.endLine != null && instruction.endLine !== instruction.startLine ? "s" : ""} ${instruction.startLine}${instruction.endLine != null && instruction.endLine !== instruction.startLine ? `–${instruction.endLine}` : ""}`}
            </p>
            <EvidenceExcerpt
              item={instruction}
              showMetadata={false}
              citationPrefix={citationPrefix}
            />
          </>
        ) : (
          <p className="type-callout text-label-secondary">
            {target.display.instructionTitle ?? sourcePath ?? "Instruction cited by this check"}
          </p>
        )}
      </div>
      <div className="space-y-1">
        <p className="type-callout font-medium text-label">What happened</p>
        {action ? (
          <>
            <EvidenceExcerpt
              item={action}
              showMetadata={false}
              showExplanation={false}
              citationPrefix={citationPrefix}
              collapseLong
            />
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
          <p className="type-callout text-label-secondary">
            {state.status === "loaded" && state.evidence?.status === "unavailable"
              ? "The cited session action is unavailable."
              : target.finding.observation}
          </p>
        )}
      </div>
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
          Saved excerpts aren’t available.
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
  const skillOpportunities = target.finding.detector === "skillOpportunities"
  const scopeCreep = target.finding.detector === "scopeCreep"
  const revisionEvidenceActionId =
    skillOpportunities ||
    scopeCreep ||
    target.finding.detector === "ignoredInstructions" ||
    target.finding.detector === "overExploring"
      ? target.actionId
      : null
  const citationId = useId()
  const [evidenceState, setEvidenceState] = useState<EvidenceState | null>(null)
  const request = useRef(0)
  const currentTargetElement = useRef<HTMLElement | null>(null)
  const autoLoadedFinding = useRef<string | null>(null)
  const loadedFinding = useRef<string | null>(null)
  const loadEvidence = useCallback((snapshotTarget: BurnCheckTargetPayload) => {
    const { actionId, findingId } = snapshotTarget
    const detector = snapshotTarget.finding.detector
    const token = ++request.current
    setEvidenceState((previous) => ({
      findingId,
      actionId,
      status: "loading",
      ...(previous?.findingId === findingId && previous.snapshot
        ? { snapshot: previous.snapshot }
        : {}),
    }))
    void getBurnCheckTargetEvidence(actionId).then(
      (evidence) => {
        if (
          token !== request.current ||
          currentTargetElement.current?.dataset.findingId !== findingId ||
          currentTargetElement.current?.dataset.evidenceActionId !== actionId
        )
          return
        const check = smartCheckForDetector(detector)
        if (check)
          noteInteraction({
            kind: "smartCheckObserved",
            check,
            observation: evidence
              ? evidence.status === "available"
                ? "evidence_available"
                : "evidence_unavailable"
              : "evidence_failed",
          })
        setEvidenceState((previous) => {
          const retained = previous?.findingId === findingId ? previous.snapshot : undefined
          const occurrence =
            evidence?.occurrences?.find((item) => item.findingId === retained?.occurrenceId) ??
            evidence?.occurrences?.[0]
          const selectedEvidence = evidence && occurrence ? occurrence : evidence
          return {
            findingId,
            actionId,
            status: selectedEvidence ? "loaded" : "failed",
            ...(selectedEvidence
              ? {
                  evidence: selectedEvidence,
                  snapshot: {
                    target: snapshotTarget,
                    evidence: selectedEvidence,
                    ...(occurrence ? { occurrenceId: occurrence.findingId } : {}),
                  },
                }
              : retained
                ? { snapshot: retained }
                : {}),
          }
        })
        if (evidence)
          loadedFinding.current = smartCheckForDetector(detector)
            ? `${findingId}:${actionId}`
            : findingId
      },
      () => {
        if (
          token === request.current &&
          currentTargetElement.current?.dataset.findingId === findingId &&
          currentTargetElement.current?.dataset.evidenceActionId === actionId
        ) {
          const check = smartCheckForDetector(detector)
          if (check)
            noteInteraction({
              kind: "smartCheckObserved",
              check,
              observation: "evidence_failed",
            })
          setEvidenceState((previous) => ({
            findingId,
            actionId,
            status: "failed",
            ...(previous?.findingId === findingId && previous.snapshot
              ? { snapshot: previous.snapshot }
              : {}),
          }))
        }
      },
    )
  }, [])
  const mounted = useCallback(
    (node: HTMLElement | null) => {
      if (!node) {
        currentTargetElement.current = null
        return
      }
      currentTargetElement.current = node
      const findingId = node.dataset.findingId ?? ""
      const actionId = revisionEvidenceActionId ?? node.dataset.evidenceActionId ?? ""
      const evidenceKey =
        revisionEvidenceActionId != null ? `${findingId}:${actionId}` : findingId
      if (
        (openEvidence || node.dataset.evidenceOpen === "true") &&
        target.evidenceAvailable &&
        autoLoadedFinding.current !== evidenceKey &&
        loadedFinding.current !== evidenceKey
      ) {
        autoLoadedFinding.current = evidenceKey
        loadEvidence(target)
      }
    },
    [target, revisionEvidenceActionId, openEvidence, loadEvidence],
  )
  const ignoredInstructions = target.finding.detector === "ignoredInstructions"
  const snapshot =
    evidenceState?.findingId === target.findingId ? evidenceState.snapshot : undefined
  const staleEvidence =
    snapshot != null &&
    revisionEvidenceActionId != null &&
    snapshot.target.actionId !== target.actionId
  const displayedEvidenceState = snapshot
    ? {
        findingId: snapshot.target.findingId,
        actionId: snapshot.target.actionId,
        status: "loaded" as const,
        evidence: snapshot.evidence,
      }
    : evidenceState
  const citationPrefix = `${citationId}-${encodeURIComponent(displayedEvidenceState?.actionId ?? target.actionId)}`
  const hasInstructionExcerpt =
    displayedEvidenceState?.findingId === target.findingId &&
    displayedEvidenceState?.status === "loaded" &&
    displayedEvidenceState.evidence?.status === "available" &&
    displayedEvidenceState.evidence.items.some((item) => item.label === "instruction")
  const status = watchStatus(target)
  const costLine = targetCostLine(target)
  const projectPath = target.projectPath
  const sourcePath = ignoredInstructions ? instructionSourcePath(target) : target.configFile
  const hasEvidenceSelection = displayedEvidenceState?.findingId === target.findingId
  const initialEvidenceLoading =
    target.evidenceAvailable &&
    !snapshot &&
    (hasEvidenceSelection ? displayedEvidenceState.status === "loading" : openEvidence)
  return (
    <article
      ref={mounted}
      data-evidence-action-id={target.actionId}
      data-evidence-open={hasEvidenceSelection}
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
            <span className="flex min-w-0 items-baseline gap-1.5">
              <span className="min-w-0 wrap-anywhere">{targetTitle(target)}</span>
            </span>
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
        {costLine && (
          <p className="mt-1 type-callout tabular-nums text-label-secondary">{costLine}</p>
        )}
        {!reportRow && <BurnCheckTargetActions target={target} refresh={refresh} />}
        {target.evidenceAvailable && (
          <section className="mt-3" aria-label="Evidence">
            {(!openEvidence || hasEvidenceSelection) && (
              <button
                type="button"
                className="burn-check-action type-callout"
                aria-expanded={hasEvidenceSelection}
                onClick={() => {
                  if (hasEvidenceSelection) {
                    request.current += 1
                    setEvidenceState(null)
                    return
                  }
                  loadEvidence(target)
                }}
              >
                {hasEvidenceSelection ? "Hide details" : "Show evidence"}
              </button>
            )}
            {initialEvidenceLoading && (
              <div className="mt-3 min-h-72" aria-busy="true">
                <SmartCheckEvidenceSkeleton />
              </div>
            )}
            {hasEvidenceSelection && !initialEvidenceLoading && (
              <div
                className="mt-3 space-y-3"
                aria-busy={staleEvidence && evidenceState?.status !== "failed"}
                data-snapshot-action-id={displayedEvidenceState.actionId}
              >
                {staleEvidence && evidenceState?.status !== "failed" && (
                  <p role="status" className="sr-only">
                    Showing previous evidence while the current snapshot loads.
                  </p>
                )}
                {(snapshot || !ignoredInstructions) && evidenceState?.status === "failed" && (
                  <div>
                    <p role="alert" className="type-callout text-label-secondary">
                      {snapshot
                        ? "Could not refresh evidence. Showing the previous snapshot."
                        : "Could not load evidence."}
                    </p>
                    <button
                      type="button"
                      className="burn-check-action mt-2 type-callout"
                      onClick={() => loadEvidence(target)}
                    >
                      Retry
                    </button>
                  </div>
                )}
                {ignoredInstructions ? (
                  <IgnoredInstructionEvidence
                    target={snapshot?.target ?? target}
                    state={displayedEvidenceState}
                    sourcePath={sourcePath ?? null}
                    retry={() => loadEvidence(target)}
                  />
                ) : (
                  <>
                    {displayedEvidenceState.status === "loaded" &&
                      (displayedEvidenceState.evidence?.status === "unavailable" ? (
                        <p role="status" className="type-callout text-label-secondary">
                          The original evidence is no longer available.
                        </p>
                      ) : (
                        <div>
                          <ol className="space-y-3">
                            {orderedEvidence(displayedEvidenceState.evidence?.items ?? []).map(
                              (item) =>
                                (item.label !== "context" ||
                                  target.finding.detector === "scopeCreep") && (
                                  <li
                                    key={`${item.label}:${item.reference}`}
                                    id={`${citationPrefix}-${encodeURIComponent(item.reference)}`}
                                    tabIndex={-1}
                                    className="space-y-1"
                                  >
                                    <div className="flex min-w-0 items-baseline justify-between gap-3">
                                      <p className="type-callout font-medium text-label">
                                        {evidenceHeading(target.finding.detector, item)}
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
                                      <EvidenceExcerpt
                                        item={item}
                                        showMetadata={item.label === "context"}
                                        showExplanation={false}
                                        collapseLong={
                                          item.label === "observedAction" ||
                                          item.label === "context"
                                        }
                                      />
                                    )}
                                  </li>
                                ),
                            )}
                          </ol>
                        </div>
                      ))}
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

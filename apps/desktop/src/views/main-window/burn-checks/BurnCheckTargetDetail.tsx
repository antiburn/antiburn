import { useCallback, useId, useRef, useState } from "react"
import { Info } from "lucide-react"

import { ProjectFolderActions } from "../../../components/session/ProjectFolderActions"
import { Tooltip } from "../../../components/presentation/Tooltip"
import { performProjectFolderAction } from "../../../lib/projectFolder"
import { noteInteraction, smartCheckForDetector } from "../../../lib/ipc"
import "../../../styles/session-detail.css"
import {
  getBurnCheckTargetEvidence,
  type BurnCheckTargetEvidencePayload,
  type BurnCheckTargetPayload,
  type BurnCheckReadExtentPayload,
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
    return `Estimated ${formatApiEquivalentUsd(opportunity.value)} in cache reads across ${sessions} session${sessions === 1 ? "" : "s"}, including helper requests.`
  }
  const occurrences = target.occurrenceCount
  return `Estimated ${formatApiEquivalentUsd(opportunity.value)} in cache reads across ${occurrences} occurrence${occurrences === 1 ? "" : "s"}, including helper requests.`
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
  const [expanded, setExpanded] = useState(false)
  const disclosureId = useId()
  const fullText = item.excerpt
  const display = item.label === "observedAction" ? actionDisplay(item) : null
  const shownText = display?.text ?? fullText
  const preview = shownText.length > 360 ? `${shownText.slice(0, 360).trimEnd()}…` : shownText
  const disclosureLabel = display?.alwaysCollapse
    ? "returned content"
    : display?.text.startsWith("Command:")
      ? "command"
      : display?.text.startsWith("Searched for:")
        ? "search query"
        : "full details"
  const excerpt = (text: string) => (
    <pre className="min-w-0 whitespace-pre-wrap wrap-anywhere rounded-control bg-surface-card px-3 py-2 type-callout text-label">
      {text}
    </pre>
  )
  return (
    <div
      className="min-w-0 space-y-1"
      id={
        citationPrefix ? `${citationPrefix}-${encodeURIComponent(item.reference)}` : undefined
      }
      tabIndex={citationPrefix ? -1 : undefined}
    >
      {showMetadata && (
        <p className="type-caption text-label-tertiary">
          {item.sourceLabel}
          {showExplanation && item.explanation ? ` · ${item.explanation}` : ""}
        </p>
      )}
      {collapseLong &&
      (display?.alwaysCollapse || ((display?.collapse ?? true) && shownText.length > 360)) ? (
        <>
          <button
            type="button"
            className="burn-check-action type-caption"
            aria-expanded={expanded}
            aria-controls={disclosureId}
            onClick={() => setExpanded(!expanded)}
          >
            {expanded ? `Hide ${disclosureLabel}` : `Show ${disclosureLabel}`}
          </button>
          <div id={disclosureId}>
            {expanded ? excerpt(display?.detail ?? shownText) : excerpt(preview)}
          </div>
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

function readExtent(extent: BurnCheckReadExtentPayload, kind: "Requested" | "Returned") {
  if (extent.unit === "unknown") return null
  const unit = extent.unit === "lines" ? "line" : "byte"
  const range =
    extent.offset == null
      ? null
      : `${unit} ${extent.offset}${extent.end_inclusive == null ? "" : `–${extent.end_inclusive}`}`
  const limit = extent.limit == null ? null : `limit ${extent.limit} ${extent.unit}`
  const details = [range, limit].filter(Boolean).join(" · ")
  return details ? `${kind}: ${details}` : null
}

function ReadResult({ item }: { item: BurnCheckTargetEvidencePayload["items"][number] }) {
  const [expanded, setExpanded] = useState(false)
  const contentId = useId()
  return (
    <div className="min-w-0 space-y-1">
      <button
        type="button"
        className="burn-check-action type-caption"
        aria-expanded={expanded}
        aria-controls={contentId}
        onClick={() => setExpanded(!expanded)}
      >
        {expanded ? "Hide returned content" : "Show returned content"}
      </button>
      {!expanded && (
        <pre className="min-w-0 whitespace-pre-wrap wrap-anywhere rounded-control bg-surface-card px-3 py-2 type-callout text-label">
          …
        </pre>
      )}
      <pre
        id={contentId}
        hidden={!expanded}
        className="min-w-0 whitespace-pre-wrap wrap-anywhere rounded-control bg-surface-card px-3 py-2 type-callout text-label"
      >
        {item.excerpt}
      </pre>
    </div>
  )
}

function ReadEvidence({
  evidence,
  citationPrefix,
  reason,
}: {
  evidence: BurnCheckTargetEvidencePayload
  citationPrefix: string
  reason?: BurnCheckTargetPayload["finding"]["overExploringReason"]
}) {
  const reads = evidence.comparison?.reads ?? []
  const readRecords = evidence.items.filter((item) => item.label === "observedAction")
  const assessedReads = reads
    .map((read, index) => ({ read, index }))
    .filter(({ index }) =>
      evidence.comparison?.explanation?.relationship === "laterReadRepeatsEarlier"
        ? index === reads.length - 1
        : true,
    )
  return (
    <div className="min-w-0 space-y-3">
      <section
        aria-label={
          reason === "excessive_file_breadth"
            ? "Assessed file set"
            : reads.length === 0
              ? "Recorded read evidence"
              : "Reads under review"
        }
        className="min-w-0 space-y-2"
      >
        <h4 className="type-callout font-medium! text-label">
          {reason === "excessive_file_breadth"
            ? "Files assessed as a group"
            : reads.length === 0
              ? "Read records used for this finding"
              : "Read under review"}
        </h4>
        {reads.length === 0 ? (
          <ol className="min-w-0 space-y-2">
            {readRecords.map((item) => (
              <li key={item.reference} className="min-w-0 space-y-1">
                {item.sourceLabel.toLowerCase().includes("result") ? (
                  <ReadResult item={item} />
                ) : (
                  <EvidenceExcerpt
                    item={item}
                    showMetadata
                    showExplanation={false}
                    citationPrefix={citationPrefix}
                    collapseLong
                  />
                )}
              </li>
            ))}
          </ol>
        ) : (
          <ol className="min-w-0 space-y-3">
            {assessedReads.map(({ read }) => {
              const request = evidence.items.find(
                (item) => item.reference === read.requestReference,
              )
              const result = evidence.items.find(
                (item) => item.reference === read.resultReference,
              )
              if (!request) return null
              const requested = readExtent(read.requestedExtent, "Requested")
              const returned = read.returnedExtent
                ? readExtent(read.returnedExtent, "Returned")
                : null
              return (
                <li
                  key={read.requestReference}
                  className="min-w-0 space-y-1"
                  id={`${citationPrefix}-${encodeURIComponent(read.requestReference)}`}
                  tabIndex={-1}
                >
                  <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                    <p className="min-w-0 wrap-anywhere type-callout text-label">
                      {read.paths.length ? read.paths.join(", ") : "Read request"}
                    </p>
                    {request.observedAtMs != null && (
                      <time
                        className="type-caption text-label-tertiary"
                        dateTime={new Date(request.observedAtMs).toISOString()}
                      >
                        {new Date(request.observedAtMs).toLocaleString()}
                      </time>
                    )}
                  </div>
                  {requested && (
                    <p className="type-caption text-label-secondary">{requested}</p>
                  )}
                  {returned && <p className="type-caption text-label-secondary">{returned}</p>}
                  {!result ? (
                    <p className="type-caption text-label-secondary">Read request</p>
                  ) : (
                    <div
                      id={`${citationPrefix}-${encodeURIComponent(result.reference)}`}
                      tabIndex={-1}
                    >
                      {read.resultStatus === "failed" && (
                        <p className="type-caption text-label-secondary">Read failed</p>
                      )}
                      <ReadResult item={result} />
                    </div>
                  )}
                </li>
              )
            })}
          </ol>
        )}
      </section>
    </div>
  )
}

function evidenceHeading(
  detector: BurnCheckTargetPayload["finding"]["detector"],
  item: BurnCheckTargetEvidencePayload["items"][number],
  observationKind?: "proposal" | "attempt" | "recorded",
) {
  if (item.label === "observedAction") {
    if (detector === "ignoredInstructions") return "What happened"
    if (detector === "overExploring") return "Files read"
    if (detector === "scopeCreep") {
      return observationKind === "proposal"
        ? "Proposed work"
        : observationKind === "attempt"
          ? "Attempted work"
          : "Recorded work"
    }
    return "Recorded work"
  }
  if (item.label === "context")
    return detector === "overExploring" ? "Requested task" : item.sourceLabel
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
}: {
  target: BurnCheckTargetPayload
  state: EvidenceState
  sourcePath: string | null
}) {
  const citationId = useId()
  if (state.status === "loaded" && state.evidence?.status === "unavailable") {
    return (
      <p role="status" className="type-callout text-label-tertiary">
        Source details could not be verified.
      </p>
    )
  }
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
            No action excerpt was returned for this finding.
          </p>
        )}
      </div>
      {orderedEvidence(items.filter((item) => item.label === "context")).map((item) => (
        <div key={item.reference} className="space-y-1">
          <p className="type-callout font-medium! text-label">{item.sourceLabel}</p>
          <EvidenceExcerpt
            item={item}
            showMetadata={false}
            showExplanation={false}
            citationPrefix={citationPrefix}
            collapseLong
          />
        </div>
      ))}
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

function evidenceDisclosure(detector: BurnCheckTargetPayload["finding"]["detector"]): string {
  switch (detector) {
    case "ignoredInstructions":
      return "instruction and action"
    case "overExploring":
      return "reads under review"
    case "scopeCreep":
      return "task and work"
    case "skillOpportunities":
      return "skill and related work"
    default:
      return "source details"
  }
}

export function BurnCheckTargetDetail({
  target,
  refresh,
  reportRow = false,
  openEvidence = false,
  evidenceActive = true,
}: {
  target: BurnCheckTargetPayload
  refresh: () => void
  reportRow?: boolean
  openEvidence?: boolean
  evidenceActive?: boolean
}) {
  const smartCheck = smartCheckForDetector(target.finding.detector)
  const skillOpportunities = target.finding.detector === "skillOpportunities"
  const scopeCreep = target.finding.detector === "scopeCreep"
  const revisionEvidenceActionId =
    skillOpportunities ||
    scopeCreep ||
    target.finding.detector === "ignoredInstructions" ||
    target.finding.detector === "overExploring"
      ? target.actionId
      : null
  const automaticEvidence = openEvidence
  const [evidenceOpen, setEvidenceOpen] = useState(openEvidence)
  const evidenceOpenRef = useRef(evidenceOpen)
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
        if (check && currentTargetElement.current?.dataset.evidenceActive === "true")
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
                    ...(occurrence
                      ? { occurrenceId: occurrence.findingId }
                      : selectedEvidence.status === "unavailable" && retained?.occurrenceId
                        ? { occurrenceId: retained.occurrenceId }
                        : {}),
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
          if (check && currentTargetElement.current?.dataset.evidenceActive === "true")
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
        evidenceActive &&
        (automaticEvidence || evidenceOpenRef.current) &&
        target.evidenceAvailable &&
        autoLoadedFinding.current !== evidenceKey &&
        loadedFinding.current !== evidenceKey
      ) {
        autoLoadedFinding.current = evidenceKey
        loadEvidence(target)
      }
    },
    [target, revisionEvidenceActionId, automaticEvidence, evidenceActive, loadEvidence],
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
    (hasEvidenceSelection ? displayedEvidenceState.status === "loading" : automaticEvidence)
  const selectedEvidence =
    displayedEvidenceState?.findingId === target.findingId
      ? displayedEvidenceState.evidence
      : undefined
  const explanation =
    selectedEvidence?.status === "available"
      ? selectedEvidence.comparison?.explanation
      : undefined
  const validExplanation =
    explanation?.version === 1 &&
    explanation.references.length >= 2 &&
    explanation.references.every((reference) =>
      selectedEvidence?.items.some(
        (item) => item.reference === reference && item.excerpt.trim() !== "",
      ),
    )
  return (
    <article
      ref={mounted}
      data-evidence-action-id={target.actionId}
      data-evidence-active={evidenceActive}
      data-evidence-open={evidenceOpen}
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
            {reportRow && smartCheck
              ? (target.projectName ?? "")
              : scopeLabel(target.display.scopeKind)}
            {sourcePath &&
              (!ignoredInstructions || !hasInstructionExcerpt) &&
              ` (${sourcePath})`}
            {reportRow && target.projectName && !(reportRow && smartCheck) && (
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
        {smartCheck && (
          <p className="mt-2 type-callout text-label-secondary">{target.finding.observation}</p>
        )}
        {costLine && (
          <p className="mt-1 type-callout tabular-nums text-label-secondary">{costLine}</p>
        )}
        {!reportRow && <BurnCheckTargetActions target={target} refresh={refresh} />}
        {(smartCheck != null || target.evidenceAvailable) && (
          <section className="mt-2" aria-label="Source details">
            <button
              type="button"
              className="burn-check-action type-caption"
              aria-expanded={evidenceOpen}
              onClick={() => {
                const next = !evidenceOpen
                evidenceOpenRef.current = next
                setEvidenceOpen(next)
                if (next && (!hasEvidenceSelection || staleEvidence)) loadEvidence(target)
              }}
            >
              {evidenceOpen
                ? `Hide ${smartCheck ? evidenceDisclosure(target.finding.detector) : "source details"}`
                : `Show ${smartCheck ? evidenceDisclosure(target.finding.detector) : "source details"}`}
            </button>
            {evidenceOpen && (
              <div
                className="mt-2 space-y-2"
                aria-busy={
                  initialEvidenceLoading ||
                  (staleEvidence && evidenceState?.status !== "failed")
                }
                data-snapshot-action-id={displayedEvidenceState?.actionId}
              >
                {staleEvidence && evidenceState?.status !== "failed" && (
                  <p role="status" className="type-caption text-label-tertiary">
                    Updating source details…
                  </p>
                )}
                {initialEvidenceLoading && (
                  <p role="status" className="type-caption text-label-tertiary">
                    Loading source details…
                  </p>
                )}
                {evidenceState?.status === "failed" && (
                  <div>
                    <p role="alert" className="type-callout text-label-secondary">
                      {snapshot
                        ? "Could not update source details. Earlier details remain available."
                        : "Could not load source details."}
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
                {validExplanation && (
                  <p className="type-callout text-label">{explanation.text}</p>
                )}
                {displayedEvidenceState?.status === "loaded" && ignoredInstructions ? (
                  <IgnoredInstructionEvidence
                    target={snapshot?.target ?? target}
                    state={displayedEvidenceState}
                    sourcePath={sourcePath ?? null}
                  />
                ) : displayedEvidenceState?.status === "loaded" &&
                  target.finding.detector === "overExploring" &&
                  selectedEvidence?.status === "available" ? (
                  <ReadEvidence
                    evidence={selectedEvidence}
                    citationPrefix={citationPrefix}
                    reason={target.finding.overExploringReason}
                  />
                ) : (
                  <>
                    {displayedEvidenceState?.status === "loaded" &&
                      (displayedEvidenceState.evidence?.status === "unavailable" ? (
                        <p role="status" className="type-callout text-label-secondary">
                          Source details could not be verified.
                        </p>
                      ) : (
                        <div>
                          <ol className="space-y-3">
                            {orderedEvidence(displayedEvidenceState.evidence?.items ?? []).map(
                              (item) =>
                                (item.label !== "context" ||
                                  revisionEvidenceActionId != null) && (
                                  <li
                                    key={`${item.label}:${item.reference}`}
                                    id={`${citationPrefix}-${encodeURIComponent(item.reference)}`}
                                    tabIndex={-1}
                                    className="space-y-1"
                                  >
                                    <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-3">
                                      <p className="type-callout font-medium text-label">
                                        {evidenceHeading(
                                          target.finding.detector,
                                          item,
                                          selectedEvidence?.comparison?.observationKind,
                                        )}
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
                                        This instruction has changed or is no longer in the
                                        source.
                                      </p>
                                    ) : (
                                      <EvidenceExcerpt
                                        item={item}
                                        showMetadata={false}
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
            active={evidenceActive}
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

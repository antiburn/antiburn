import { useCallback, useId, useRef, useState, type ReactNode } from "react"
import { Info } from "lucide-react"

import { ProjectFolderActions } from "../../../components/session/ProjectFolderActions"
import { Tooltip } from "../../../components/presentation/Tooltip"
import { performProjectFolderAction } from "../../../lib/projectFolder"
import { noteInteraction, smartCheckForDetector } from "../../../lib/ipc"
import "../../../styles/session-detail.css"
import {
  getBurnCheckTargetEvidence,
  type IgnoredInstructionDecisionCitationPayload,
  type BurnCheckTargetEvidencePayload,
  type BurnCheckTargetPayload,
} from "../../../lib/insightsIpc"
import { renderAgentIcon } from "../../../lib/agentIcon"
import { formatApiEquivalentUsd } from "../../../lib/presentation/checkReport"
import { CHECK_UI } from "../../checks/checkPresentation"
import { overExploringDetail } from "../../../lib/presentation/checkDefinitions"
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
  citationPrefix,
}: {
  item: BurnCheckTargetEvidencePayload["items"][number]
  showMetadata?: boolean
  citationPrefix?: string
}) {
  return (
    <div className="space-y-1">
      {showMetadata && (
        <p className="type-caption text-label-tertiary">
          {item.sourceLabel}
          {item.explanation ? ` · ${item.explanation}` : ""}
        </p>
      )}
      <pre
        id={
          citationPrefix ? `${citationPrefix}-${encodeURIComponent(item.reference)}` : undefined
        }
        tabIndex={citationPrefix ? -1 : undefined}
        className="whitespace-pre-wrap break-words rounded-control bg-surface-card px-3 py-2 type-callout text-label"
      >
        {item.excerpt}
      </pre>
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

function EvidenceInfo({ details }: { details: string[] }) {
  if (details.length === 0) return null
  return (
    <Tooltip
      label={
        <ul className="list-disc space-y-1 pl-4">
          {details.map((detail) => (
            <li key={detail}>{detail}</li>
          ))}
        </ul>
      }
      interactive
    >
      <button
        type="button"
        className="burn-check-action rounded-control p-1 text-label-tertiary"
        aria-label="About this evidence"
      >
        <Info size={14} aria-hidden="true" />
      </button>
    </Tooltip>
  )
}

function AssessmentDecision({
  contrast,
  details,
  advisory = false,
  children,
}: {
  contrast: string
  details: string[]
  advisory?: boolean
  children: ReactNode
}) {
  return (
    <section
      aria-label="Assessment decision"
      className="space-y-2 rounded-control bg-surface-card px-3 py-2"
    >
      <div className="flex items-center gap-1.5">
        <h4 className="type-callout font-medium text-label">
          {advisory ? "Why this was suggested" : "Why this was flagged"}
        </h4>
        <EvidenceInfo details={details} />
      </div>
      <p className="type-callout text-label-secondary">{contrast}</p>
      {children}
    </section>
  )
}

function DecisionProof({
  proof,
  items,
  citationPrefix,
  extraDetails = [],
}: {
  proof: NonNullable<BurnCheckTargetEvidencePayload["decisionProof"]>
  items: BurnCheckTargetEvidencePayload["items"]
  citationPrefix: string
  extraDetails?: string[]
}) {
  const claims: Record<IgnoredInstructionDecisionCitationPayload["claim"], string> = {
    rule_requirement: "Instruction requirement",
    anchored_action: "Cited action",
    prerequisite_contrast: "Prerequisite evidence",
    observed_context: "Observed context",
  }
  if (
    !proof.citations.some((citation) => citation.claim === "rule_requirement") ||
    !proof.citations.some((citation) => citation.claim === "anchored_action") ||
    (proof.prerequisite !== "not_required" &&
      !proof.citations.some((citation) => citation.claim === "prerequisite_contrast")) ||
    proof.citations.some(
      (citation) =>
        citation.source_ids.length === 0 ||
        citation.source_ids.some(
          (id) =>
            !items.some(
              (item) =>
                item.reference === id &&
                item.excerpt.length > 0 &&
                item.excerpt !== "Instruction text unavailable.",
            ),
        ),
    )
  )
    return null
  const details = [
    ...extraDetails,
    ...proof.coverage.limitations,
    proof.coverage.source_complete
      ? "The saved source includes every event used by this check."
      : "Some session events may be missing.",
    proof.coverage.selected_history_complete
      ? "Earlier events used by this check are saved."
      : "Some earlier events may be missing.",
    ...(proof.coverage.results_excluded
      ? []
      : ["This comparison includes selected recorded tool results."]),
    ...(proof.coverage.user_authority_excluded
      ? []
      : ["This comparison includes selected user-authority events."]),
    ...(proof.prerequisite === "earlier_request_absent" &&
    proof.coverage.read_request_inventory_complete
      ? [
          "The recorded read-request inventory is complete. A read request does not prove successful execution.",
        ]
      : []),
  ]
  return (
    <AssessmentDecision contrast={proof.contrast} details={details}>
      <ul className="space-y-1">
        {proof.citations.map((citation, index) => (
          <li key={`${citation.claim}:${index}`} className="type-caption text-label-tertiary">
            {claims[citation.claim]} · {citation.source_ids.length} source
            {citation.source_ids.length === 1 ? "" : "s"}
            <ul>
              {citation.source_ids.map((id) => (
                <li key={id}>
                  <a
                    href={`#${citationPrefix}-${encodeURIComponent(id)}`}
                    onClick={(event) => {
                      const excerpt = document.getElementById(event.currentTarget.hash.slice(1))
                      const disclosure = excerpt?.closest("details")
                      if (disclosure) disclosure.open = true
                      excerpt?.focus()
                    }}
                  >
                    {items.find((item) => item.reference === id)?.sourceLabel}
                  </a>
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ul>
    </AssessmentDecision>
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
  const citationPrefix = useId()
  const items =
    state.status === "loaded" && state.evidence?.status === "available"
      ? state.evidence.items
      : []
  const instruction = items.find((item) => item.label === "instruction")
  const action = items.find((item) => item.label === "observedAction")
  const decisionProof =
    state.status === "loaded" && state.evidence?.status === "available"
      ? state.evidence.decisionProof
      : undefined
  const itemLimitations = [
    ...new Set(
      items.map((item) => item.limitation).filter((value): value is string => Boolean(value)),
    ),
  ]

  return (
    <div className="space-y-3">
      {decisionProof ? (
        <DecisionProof
          proof={decisionProof}
          items={items}
          citationPrefix={citationPrefix}
          extraDetails={itemLimitations}
        />
      ) : (
        <EvidenceInfo details={itemLimitations} />
      )}
      <div className="space-y-1">
        <p className="type-callout font-medium text-label">Instruction</p>
        {instruction ? (
          <>
            <p className="type-caption text-label-tertiary">
              {instruction.sourceLabel}
              {instruction.startLine != null &&
                ` · line${instruction.endLine !== instruction.startLine ? "s" : ""} ${instruction.startLine}${instruction.endLine !== instruction.startLine ? `–${instruction.endLine}` : ""}`}
            </p>
            <EvidenceExcerpt item={instruction} citationPrefix={citationPrefix} />
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
        <p className="type-callout font-medium text-label">Session action</p>
        {action ? (
          <>
            <EvidenceExcerpt item={action} citationPrefix={citationPrefix} />
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
              : state.status === "loaded" && state.evidence?.status === "unavailable"
                ? "The cited session action is unavailable."
                : target.finding.observation}
          </p>
        )}
      </div>
      {items.some((item) => item.label === "context") && (
        <details className="space-y-3">
          <summary className="burn-check-action type-callout">Supporting events</summary>
          <p className="type-caption text-label-tertiary">
            Nearby events and counterevidence cited by this assessment. These are not additional
            violations.
          </p>
          <ol className="space-y-3">
            {orderedEvidence(items)
              .filter((item) => item.label === "context")
              .map((item) => (
                <li key={item.reference}>
                  {item.observedAtMs != null && (
                    <time
                      dateTime={new Date(item.observedAtMs).toISOString()}
                      className="type-caption text-label-tertiary"
                    >
                      {new Date(item.observedAtMs).toLocaleString()}
                    </time>
                  )}
                  <EvidenceExcerpt item={item} citationPrefix={citationPrefix} />
                </li>
              ))}
          </ol>
        </details>
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

const outdatedInstructionsNote = "Note: this session may have run on outdated instructions."

function AssessmentExplanation({
  target,
  evidence,
  citationPrefix,
  revealContext,
}: {
  target: BurnCheckTargetPayload
  evidence: BurnCheckTargetEvidencePayload
  citationPrefix: string
  revealContext: () => void
}) {
  const items = evidence.items
  const hasWork = items.some((item) => item.label === "observedAction")
  const hasScope = items.some((item) => item.label === "instruction")
  const hasContext = items.some((item) => item.label === "context")
  let contrast: string | null = null
  switch (target.finding.detector) {
    case "scopeCreep":
      if (hasScope && hasWork)
        contrast =
          "The latest recorded task scope does not approve this substantial optional work."
      break
    case "overExploring":
      if (hasContext && hasWork)
        contrast = overExploringDetail(target.finding.overExploringReason)
      break
    case "skillOpportunities":
      if (hasScope && hasWork)
        contrast =
          "This recorded work matches a skill in your current inventory. Use it for similar future work; current inventory does not prove past availability."
      break
  }
  const limits = [
    ...new Set(items.flatMap((item) => (item.limitation ? [item.limitation] : []))),
  ]
  if (evidence.status !== "available") return null
  if (!contrast) return <EvidenceInfo details={limits} />
  return (
    <AssessmentDecision
      contrast={contrast}
      details={limits}
      advisory={target.finding.detector === "skillOpportunities"}
    >
      <ul className="space-y-1">
        {items.map((item) => (
          <li
            key={`${item.label}:${item.reference}`}
            className="type-caption text-label-tertiary"
          >
            <a
              href={`#${citationPrefix}-${encodeURIComponent(item.reference)}`}
              onClick={revealContext}
            >
              {item.sourceLabel}
            </a>
          </li>
        ))}
      </ul>
    </AssessmentDecision>
  )
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
  const citationPrefix = useId()
  const [evidenceState, setEvidenceState] = useState<EvidenceState | null>(null)
  const [showContext, setShowContext] = useState(false)
  const request = useRef(0)
  const currentTargetElement = useRef<HTMLElement | null>(null)
  const autoLoadedFinding = useRef<string | null>(null)
  const loadedFinding = useRef<string | null>(null)
  const loadEvidence = useCallback(
    (actionId: string, findingId: string, detector: string) => {
      const token = ++request.current
      setEvidenceState({ findingId, actionId, status: "loading" })
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
          setEvidenceState({
            findingId,
            actionId,
            status: evidence ? "loaded" : "failed",
            ...(evidence ? { evidence } : {}),
          })
          if (evidence)
            loadedFinding.current =
              revisionEvidenceActionId != null ? `${findingId}:${actionId}` : findingId
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
            setEvidenceState({ findingId, actionId, status: "failed" })
          }
        },
      )
    },
    [revisionEvidenceActionId],
  )
  const mounted = useCallback(
    (node: HTMLElement | null) => {
      if (!node) {
        request.current += 1
        autoLoadedFinding.current = null
        currentTargetElement.current = null
        return
      }
      currentTargetElement.current = node
      const findingId = node.dataset.findingId ?? ""
      const actionId = revisionEvidenceActionId ?? node.dataset.evidenceActionId ?? ""
      const evidenceKey =
        revisionEvidenceActionId != null ? `${findingId}:${actionId}` : findingId
      if (
        openEvidence &&
        target.evidenceAvailable &&
        autoLoadedFinding.current !== evidenceKey &&
        loadedFinding.current !== evidenceKey
      ) {
        autoLoadedFinding.current = evidenceKey
        loadEvidence(actionId, findingId, node.dataset.detector ?? "")
      }
    },
    [target.evidenceAvailable, revisionEvidenceActionId, openEvidence, loadEvidence],
  )
  const ignoredInstructions = target.finding.detector === "ignoredInstructions"
  const hasInstructionExcerpt =
    evidenceState?.findingId === target.findingId &&
    evidenceState?.status === "loaded" &&
    evidenceState.evidence?.status === "available" &&
    evidenceState.evidence.items.some((item) => item.label === "instruction")
  const status = watchStatus(target)
  const guidance = CHECK_UI[target.finding.detector]
  const reasonDetail =
    target.finding.detector === "overExploring"
      ? overExploringDetail(target.finding.overExploringReason)
      : null
  const costLine = targetCostLine(target)
  const projectPath = target.projectPath
  const sourcePath = ignoredInstructions ? instructionSourcePath(target) : target.configFile
  const hasEvidenceSelection =
    evidenceState?.findingId === target.findingId &&
    (revisionEvidenceActionId == null || evidenceState.actionId === target.actionId)
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
        {!reportRow ||
        skillOpportunities ||
        scopeCreep ||
        target.finding.detector === "overExploring" ? (
          <div className="mt-2 space-y-1">
            <p className="type-body text-pretty text-label-secondary">
              {skillOpportunities
                ? "This work matches a skill in your current inventory."
                : target.finding.detector === "ignoredInstructions"
                  ? "Review the cited instruction and session action."
                  : guidance.recommendation}
            </p>
            {reasonDetail &&
              !(
                hasEvidenceSelection &&
                evidenceState.status === "loaded" &&
                evidenceState.evidence?.status === "unavailable"
              ) && <p className="type-callout text-label-secondary">{reasonDetail}</p>}
            {skillOpportunities && (
              <p className="type-callout text-label-secondary">
                Use this skill for similar future work. Current inventory does not prove it was
                available during this session.
              </p>
            )}
            {scopeCreep && (
              <p className="type-callout text-label-secondary">
                This assessment uses the latest recorded task scope and user approval evidence.
                Assistant proposals and tool permission do not establish approval. Later
                approval does not prove the work was approved before it occurred.
              </p>
            )}
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
                          {evidenceState.evidence && (
                            <AssessmentExplanation
                              target={target}
                              evidence={evidenceState.evidence}
                              citationPrefix={citationPrefix}
                              revealContext={() => setShowContext(true)}
                            />
                          )}
                          <ol className="space-y-3">
                            {orderedEvidence(evidenceState.evidence?.items ?? []).map(
                              (item) =>
                                (item.label !== "context" || showContext) && (
                                  <li
                                    key={`${item.label}:${item.reference}`}
                                    id={`${citationPrefix}-${encodeURIComponent(item.reference)}`}
                                    tabIndex={-1}
                                    className="space-y-1"
                                  >
                                    <div className="flex min-w-0 items-baseline justify-between gap-3">
                                      <p className="type-callout font-medium text-label">
                                        {item.label === "observedAction"
                                          ? skillOpportunities || scopeCreep
                                            ? "Recorded work"
                                            : "Session action"
                                          : item.label === "context"
                                            ? "Context"
                                            : `${scopeCreep ? "Latest recorded task scope" : skillOpportunities ? "Current skill" : "Instruction"} · ${item.sourceLabel}${item.startLine ? ` · line${item.endLine !== item.startLine ? "s" : ""} ${item.startLine}${item.endLine !== item.startLine ? `–${item.endLine}` : ""}` : ""}`}
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
                                    {!skillOpportunities &&
                                      item.limitation &&
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
                {ignoredInstructions && evidenceState.status === "loaded" && (
                  <section aria-label="Occurrences" className="space-y-3">
                    <p className="type-caption text-label-tertiary">
                      {`Showing ${Math.min(
                        (evidenceState.evidence?.occurrences?.length ?? 0) + 1,
                        target.occurrenceCount,
                      )} of ${target.occurrenceCount} finding${target.occurrenceCount === 1 ? "" : "s"}.`}
                    </p>
                    {evidenceState.evidence?.occurrences?.map((occurrence, index) => (
                      <details key={occurrence.findingId} className="space-y-3">
                        <summary className="burn-check-action type-callout">
                          Occurrence {index + 1}
                        </summary>
                        <IgnoredInstructionEvidence
                          target={target}
                          state={{ ...evidenceState, evidence: occurrence }}
                          sourcePath={sourcePath ?? null}
                          retry={() =>
                            loadEvidence(
                              target.actionId,
                              target.findingId,
                              target.finding.detector,
                            )
                          }
                        />
                      </details>
                    ))}
                  </section>
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

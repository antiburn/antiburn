import { useId, useState, type ReactNode } from "react"

import { pluralize } from "../../../lib/presentation/text"
import { ProjectFolderActions } from "../../../components/session/ProjectFolderActions"
import { performProjectFolderAction } from "../../../lib/projectFolder"
import "../../../styles/session-detail.css"
import {
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
    return `Estimated ${formatApiEquivalentUsd(opportunity.value)} in cache reads across ${sessions} ${pluralize(sessions, "session", "sessions")}, including helper requests.`
  }
  const occurrences = target.occurrenceCount
  return `Estimated ${formatApiEquivalentUsd(opportunity.value)} in cache reads across ${occurrences} ${pluralize(occurrences, "occurrence", "occurrences")}, including helper requests.`
}

export function EvidenceExcerpt({
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

export function CheckTargetHeading({
  agent,
  title,
  children,
}: {
  agent: string
  title: string
  children: ReactNode
}) {
  return (
    <>
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-x-4 gap-y-1">
        <div className="min-w-0 flex-1 basis-56">
          <h3 className="burn-check-resource-title type-title-3 text-label">
            <span className="burn-check-resource-icon">{renderAgentIcon(agent, 16)}</span>
            <span className="flex min-w-0 items-baseline gap-1.5">
              <span className="min-w-0 wrap-anywhere">{title}</span>
            </span>
          </h3>
        </div>
      </div>
      <div className="burn-check-resource-metadata min-w-0">
        <div className="flex items-center gap-1.5 type-callout text-label-tertiary">
          {children}
        </div>
      </div>
    </>
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

export function BurnCheckTargetDetail({
  target,
  refresh,
  reportRow = false,
  evidenceActive = true,
}: {
  target: BurnCheckTargetPayload
  refresh: () => void
  reportRow?: boolean
  evidenceActive?: boolean
}) {
  const status = watchStatus(target)
  const costLine = targetCostLine(target)
  const projectPath = target.projectPath
  const sourcePath = target.configFile
  return (
    <article
      data-finding-id={target.findingId}
      data-detector={target.finding.detector}
      className={
        reportRow
          ? "burn-check-resource min-w-0"
          : "min-w-0 rounded-control bg-surface-card/75 p-4"
      }
    >
      <CheckTargetHeading agent={target.finding.agent} title={targetTitle(target)}>
        <span className="min-w-0 wrap-anywhere">
          {scopeLabel(target.display.scopeKind)}
          {sourcePath && ` (${sourcePath})`}
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
      </CheckTargetHeading>
      <div className={reportRow ? "burn-check-resource-body" : undefined}>
        {costLine && (
          <p className="mt-1 type-callout tabular-nums text-label-secondary">{costLine}</p>
        )}
        {!reportRow && (
          <BurnCheckTargetActions target={target} refresh={refresh} active={evidenceActive} />
        )}
        {status && (
          <p role="status" className="mt-2 type-callout text-label-secondary">
            {status}
          </p>
        )}
        <div>
          {reportRow && target.affectedSessionCount != null && (
            <p className="mb-1 type-callout tabular-nums text-label-secondary">
              {`${target.affectedSessionCount} ${pluralize(target.affectedSessionCount, "session", "sessions")} affected`}
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

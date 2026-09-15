import type { SessionHygienePayload } from "../insightsIpc"
import type { SessionAnalysisPayload } from "../ipc"
import type { SessionSubject } from "../sessionSubject"
import type { BillableTokens } from "../types/session"
import { agentDisplayName } from "./agents"
import { modelRunNames } from "./models"
import {
  costBreakdownRows,
  costFigureLabel,
  formatCost,
  formatDuration,
} from "./sessionAnalysis"
import {
  notAssessedReasonLabel,
  sessionHygieneChecks,
  sessionHygieneDocumentation,
  sessionHygieneStateLabel,
} from "./sessionHygiene"

export interface SessionDiscussionInput {
  subject: SessionSubject
  payload: SessionAnalysisPayload | null
  hygiene: SessionHygienePayload
  loading: boolean
  refreshing: boolean
  error: boolean
}

/** Keep metadata on one Markdown line and prevent it from adding Markdown structure. */
function text(value: string): string {
  return value
    .replace(/\\/g, "\\\\")
    .replace(/[\r\n\t]/g, " ")
    .replace(/([`*_{}[\]()<>#|])/g, "\\$1")
}

function count(value: number | null | undefined): string {
  return value != null && Number.isFinite(value) ? value.toLocaleString() : "Unavailable"
}

function cost(value: number | null | undefined): string {
  return value != null && Number.isFinite(value) ? formatCost(value) : "Unavailable"
}

function duration(value: number | undefined): string {
  return value != null && Number.isFinite(value) ? formatDuration(value) : "Unavailable"
}

function tokens(
  label: string,
  value: { [K in keyof BillableTokens]?: number | undefined } | null | undefined,
): string[] {
  return [
    `- ${label}: input ${count(value?.inputTokens)}; output ${count(value?.outputTokens)}; cache read ${count(value?.cacheReadTokens)}; cache creation ${count(value?.cacheCreationTokens)}.`,
  ]
}

/** Build a discussion prompt from the selected session's loaded data, without reading its transcript. */
export function sessionDiscussionPrompt({
  subject,
  payload,
  hygiene,
  loading,
  refreshing,
  error,
}: SessionDiscussionInput): string {
  const data = payload?.analysisPending ? null : payload
  const metrics = data?.summary?.sessions.find(
    (session) => session.sessionId === subject.sessionId && session.agent === subject.agent,
  )
  const title =
    payload?.relations?.title?.trim() || payload?.title?.trim() || subject.title?.trim()
  const models = data ? modelRunNames(data.modelRuns) : []
  const scope = subject.subagent
    ? "Selected subagent transcript only"
    : "Selected session plus locally linked subagents (inclusive)"
  const activityScope = subject.subagent ? "Selected-transcript" : "Merged"
  const countScope = subject.subagent ? "Selected-transcript" : "Inclusive"
  const findings: string[] = []
  const otherChecks: string[] = []
  for (const check of sessionHygieneChecks(hygiene)) {
    const badge = hygiene.badges.find((item) => item.id === check.id)
    if (!badge) {
      otherChecks.push(`- Not assessed — ${check.name} (no result available).`)
    } else if (badge.status === "notAssessed") {
      const reason = badge.notAssessedReason
        ? notAssessedReasonLabel(badge.notAssessedReason)
        : "reason unavailable"
      otherChecks.push(`- Not assessed — ${check.name} (${reason}).`)
    } else if (badge.status === "clean") {
      otherChecks.push(`- Passed — ${check.name}.`)
    } else {
      const details = sessionHygieneDocumentation(check).findingDetails
      findings.push(
        "",
        `### ${check.name} — ${check.title}`,
        "",
        ...(details.length > 0
          ? details.map((detail) => `- Evidence: ${text(detail)}`)
          : ["- Evidence: Unavailable in the loaded payload."]),
        ...(badge.accounting
          ? [
              `- Repeated paid context accounting: ${badge.accounting === "cacheWrite" ? "cache writes" : "uncached input"}.`,
            ]
          : []),
      )
    }
  }

  return [
    "# antiburn session context",
    "",
    "Session metadata and analysis evidence. Transcript contents are not included.",
    "",
    "## Session",
    `- Title: ${title ? text(title) : "Unavailable"}`,
    `- ID: ${text(subject.sessionId)}`,
    `- Agent: ${text(agentDisplayName(subject.agent))} (${text(subject.agent)})`,
    `- Models / thinking modes: ${models.length ? models.map(text).join(", ") : data?.models.length ? data.models.map(text).join(", ") : "Unavailable"}`,
    `- Repository label: ${subject.repo ? text(subject.repo) : "Unavailable"}`,
    `- Origin: ${subject.wslDistro ? `WSL (${text(subject.wslDistro)})` : "Native"}`,
    ...(payload?.sourcePath
      ? ["- Source path (JSON string):", "```json", JSON.stringify(payload.sourcePath), "```"]
      : ["- Source path (JSON string): Unavailable"]),
    "",
    "## Scope and activity",
    `- Cost and billable-token scope: ${scope}.`,
    `- Event and input / output token scope: ${scope}.`,
    subject.subagent
      ? "- Activity times describe only the selected subagent transcript."
      : "- Activity times use the merged parent and locally linked subagent timeline, not a sum of individual active times.",
    "- Peak context, compactions, cache rehydrations, and provider cache misses describe only the selected transcript's context, not linked subagent contexts.",
    "- Fork relatives are not added to the totals; inherited history follows the source adapter's branch attribution.",
    `- Linked subagents reported: ${count(data?.orchestration?.subagentCount)}. Missing linkage or a zero count does not prove that no delegated work occurred.`,
    ...(subject.subagent
      ? [`- Orchestrator session ID: ${text(subject.subagent.parentSessionId)}`]
      : []),
    ...(data?.relations?.parent
      ? [
          `- Recorded fork parent ID: ${text(data.relations.parent.identity.sessionId)} (${data.relations.parent.available ? "available locally" : "unavailable locally"}).`,
        ]
      : []),
    "- Git branch name and complete delegation lineage: Unavailable in these payloads.",
    `- Active now: ${data ? (data.isActive ? "Yes" : "No") : "Unavailable"}`,
    `- ${activityScope} active time (idle gaps excluded): ${duration(metrics?.activeSecs)}`,
    `- ${activityScope} elapsed span (including idle gaps): ${duration(metrics?.durationSecs)}`,
    `- ${countScope} events: ${count(metrics?.eventCount)}`,
    `- ${countScope} input / output tokens: ${count(metrics?.tokensIn)} / ${count(metrics?.tokensOut)}`,
    `- Peak context tokens: ${count(metrics?.peakContextTokens)}`,
    `- Compactions / cache rehydrations / provider cache misses: ${count(metrics?.compactionCount)} / ${count(metrics?.cacheRehydrationCount)} / ${count(metrics?.cacheRoutingMissCount)}`,
    "",
    "## API-equivalent cost and billable tokens",
    `- ${costFigureLabel(data?.isActive ?? false)} (API-equivalent, not an invoice): ${cost(data?.cost?.totalUsd)}`,
    ...(data?.cost
      ? costBreakdownRows(data.cost).map((row) => `  - ${row.label}: ${cost(row.usd)}`)
      : []),
    ...(!subject.subagent && data?.orchestration
      ? [
          `- Selected transcript only, API-equivalent: ${cost(data.topLevelCost?.totalUsd)}; linked subagents only, API-equivalent: ${cost(data.subagentsCost?.totalUsd)}.`,
        ]
      : []),
    ...tokens(
      scope,
      subject.subagent
        ? metrics && {
            inputTokens: metrics.billableInputTokens,
            outputTokens: metrics.billableOutputTokens,
            cacheReadTokens: metrics.billableCacheReadTokens,
            cacheCreationTokens: metrics.billableCacheCreationTokens,
          }
        : data?.inclusiveTokens,
    ),
    ...(!subject.subagent && data?.subagentsTokens
      ? tokens("Linked subagents only", data.subagentsTokens)
      : []),
    "- Missing metrics and unpriced work are unknown, not zero. Cost coverage is limited to the loaded analysis and available prices.",
    "",
    "## Freshness and coverage",
    `- Analysis loading: ${loading}; refreshing: ${refreshing}; load error: ${error}.`,
    `- Analysis pending: ${payload ? String(payload.analysisPending) : "Unavailable"}; analysis stale: ${payload ? String(payload.analysisStale) : "Unavailable"}; analysis supported: ${payload ? String(payload.supportsAnalysis) : "Unavailable"}.`,
    `- Burn-check evidence: ${hygiene.evidenceState}${sessionHygieneStateLabel(hygiene.evidenceState) ? ` (${sessionHygieneStateLabel(hygiene.evidenceState)})` : ""}.`,
    "- Analysis and check evidence load independently; their exact revision and assessment time are unavailable.",
    ...(payload?.analysisPending
      ? ["- Pending analysis metrics are placeholders and are omitted."]
      : []),
    ...(hygiene.evidenceState === "pending" || hygiene.evidenceState === "processing"
      ? [
          "- Pending or processing evidence has no current completed assessment; not-assessed reasons may be placeholders.",
        ]
      : []),
    ...(payload?.analysisStale ||
    hygiene.evidenceState === "stale" ||
    hygiene.evidenceState === "activelyGrowing"
      ? ["- Stale or growing evidence can describe an earlier transcript."]
      : []),
    "- Passed applies only to the assessed check and stored scope; not assessed is not a pass.",
    "",
    "## Per-session burn-check evidence",
    "",
    "Loaded per-session badges, not global counts or a new assessment.",
    ...findings,
    ...(otherChecks.length ? ["", "### Other checks", "", ...otherChecks] : []),
    "",
    "## Question / requirement for analysis",
    "",
    "Answer the question or address the requirement below as directly as possible, using the session details where relevant.",
    "",
    "[Add your question or requirement here.]",
  ].join("\n")
}

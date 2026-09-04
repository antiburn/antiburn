import { invoke } from "@tauri-apps/api/core"
import { listen, type UnlistenFn } from "@tauri-apps/api/event"

import { hasShell } from "./ipc"
import type { LocalSessionIdentity } from "./types/session"

/* -------------------------------------------------------------------------
 * Local insights report — mirrors of the `Insights*Payload` types in
 * `src-tauri/src/dto.rs`. Counts, statuses, and structured reason
 * identifiers only; no transcript content anywhere in these shapes.
 * ---------------------------------------------------------------------- */

/** Coverage of the report window: every discovered session, partitioned by
 *  why it is or is not in the assessed cohort (FR-12). `discovered` is the
 *  denominator; every other count names one exclusive reason. */
export interface InsightsCoveragePayload {
  discovered: number
  unknownStart: number
  pending: number
  processing: number
  failed: number
  unsupported: number
  stale: number
  ready: number
  activelyGrowing: number
  awaitingProviderSupport: number
}

/** Why a category could not be assessed. Wording belongs to the pane. */
export type InsightsNotAssessedReason =
  | "noSessionsInWindow"
  | "capabilityMissing"
  | "incompleteEvidence"
  | "evidenceContractIncomplete"
  | "signalMissing"

/** One of the nine report categories, with its status and denominators. */
export interface InsightsCategoryPayload {
  /** Stable category identifier, e.g. `sessionsOverDepth`. */
  id: string
  eligible: number
  assessed: number
  status: "findings" | "clean" | "notAssessed"
  /** Sessions with at least one finding; null unless status is `findings`. */
  findingSessions: number | null
  /** Structured reason; null unless status is `notAssessed`. */
  notAssessedReason: InsightsNotAssessedReason | null
}

/** Bounded quota-pressure findings from transcript-attributable incidents. */
interface InsightsQuotaFindingsPayload {
  totalHits: number
  hardHits: number
  warnings: number
  affectedSessionCount: number
  hitsByLimitKind: { kind: string; hits: number }[]
  affectedModels: string[]
  affectedModelsTruncated: boolean
  firstObservedTsMs: number
  lastObservedTsMs: number
}

/** The quota-pressure section. `assessed` is false exactly when the
 *  transcripts carry no quota evidence — one condition, not a matrix. */
export interface InsightsQuotaPressurePayload {
  assessed: boolean
  findings: InsightsQuotaFindingsPayload | null
}

/** Bounded unknown record vocabulary from the local evidence cohort. */
export interface InsightsUnrecognizedRecordsPayload {
  types: string[]
  typesTruncated: boolean
  sessionsWithTypes: number
  inertSessions: number
  evidenceBearingSessions: number
  cappedSessions: number
  truncatedSessions: number
}

/** The thirty-day insights report for one environment scope. */
export interface InsightsReportPayload {
  environmentKey: string
  windowStartEpoch: number
  windowEndEpoch: number
  computedAtEpoch: number
  coverage: InsightsCoveragePayload
  /** Size of the assessed cohort — never a substitute for the coverage
   *  denominator, and presented separately from it. */
  assessedSessions: number
  categories: InsightsCategoryPayload[]
  quotaPressure: InsightsQuotaPressurePayload
  unrecognizedRecords: InsightsUnrecognizedRecordsPayload
  catalogRevision: number
}

export interface ChecksCategoryPayload {
  /** Stable category identifier, e.g. `sessionsOverDepth`. */
  id: string
  /** Applicable sessions with a confirmed finding. */
  finding: number
  /** Applicable sessions with complete evidence and no finding. */
  clean: number
  /** Sessions without enough evidence for a finding or clean result. */
  unavailable: number
  /** Estimated avoidable tokens divided by total used tokens, in basis points from 0 to 10000. */
  estimatedTokenBurnBasisPoints: number | null
}

/** The bounded subset of the local report needed by All checks. */
export interface ChecksReportPayload {
  /** False while this report snapshot still has queued or running evidence work. */
  evidenceSettled: boolean
  /** Estimated avoidable tokens divided by total used tokens, in basis points from 0 to 10000. */
  estimatedTokenBurnBasisPoints: number | null
  categories: ChecksCategoryPayload[]
}

/** Report calculation state plus the evidence backlog counts. */
export interface InsightsStatusPayload {
  calculating: boolean
  pending: number
  processing: number
}

export type SessionHygieneBadgeId =
  | "sessionOverdepth"
  | "modelOverthinking"
  | "overpoweredSubagents"
  | "obsoleteModel"
  | "fastModeOveruse"
  | "excessCacheRehydration"

export type SessionHygieneFindingEvidence =
  | {
      kind: "sessionOverdepth"
      maxRequestContextTokens: number
      depthCapTokens: number
    }
  | {
      kind: "modelOverthinking"
      tiers: Array<{
        tier: string
        mainLoopTurns: number
        delegatedTurns: number
      }>
    }
  | {
      kind: "overpoweredSubagents"
      mainModels: string[]
      delegatedModels: string[]
    }
  | {
      kind: "obsoleteModel"
      models: Array<{
        model: string
        replacement: string
      }>
    }
  | {
      kind: "fastModeOveruse"
      delegatedTurns: number
    }
  | {
      kind: "excessCacheRehydration"
      repeatedTokens: number
      paidTokens: number
      thresholdMultiple: number
    }

export interface SessionHygieneBadgePayload {
  id: SessionHygieneBadgeId
  status: "finding" | "clean" | "notAssessed"
  notAssessedReason: InsightsNotAssessedReason | null
  /** Which vendor billing mechanism backs an `excessCacheRehydration`
   *  verdict. Absent for every other badge and for old evidence with no
   *  `repeated_context` marker. */
  accounting?: "cacheWrite" | "uncachedInput"
  /** The stored facts that caused a finding. Absent for every other status. */
  findingEvidence?: SessionHygieneFindingEvidence
}

export type SessionHygieneEvidenceState =
  "pending" | "processing" | "ready" | "unsupported" | "failed" | "stale" | "activelyGrowing"

export interface SessionHygienePayload {
  badges: SessionHygieneBadgePayload[]
  evidenceState: SessionHygieneEvidenceState
}

/**
 * Aggregate hygiene numbers for the sessions in the activity window.
 * Mirrors Rust `HygieneSummaryPayload`.
 */
export interface HygieneSummary {
  /** Sessions in the window, after the disabled-agent display filter. */
  totalSessions: number
  /** Sessions whose analysis reached a terminal state. */
  settledSessions: number
  /** Sessions with current ready evidence, so the checks ran. */
  analyzedSessions: number
  /** Analyzed sessions with at least one finding. */
  failingSessions: number
  /** Badge id of the most frequent finding, when any session fails. */
  mostCommonFinding: SessionHygieneBadgeId | null
}

/**
 * The thirty-day insights report, computed on this machine.
 *
 * Opening the pane calls this; the shell deduplicates concurrent calls
 * into one reduction and also kicks a scan pass so a cold open does not
 * wait out a timer.
 */
export async function getInsightsReport(): Promise<InsightsReportPayload | null> {
  if (!hasShell()) return null
  return invoke<InsightsReportPayload>("get_insights_report")
}

/** The real local detector results and bounded session navigation targets. */
export async function getChecksReport(consumerId: string): Promise<ChecksReportPayload | null> {
  if (!hasShell()) return null
  return invoke<ChecksReportPayload>("get_checks_report", { consumerId })
}

/** Stop a Checks reduction that no visible popover still needs. */
export async function cancelChecksReport(consumerId: string): Promise<void> {
  if (!hasShell()) return
  await invoke("cancel_checks_report", { consumerId })
}

/** Run after the evidence worker publishes every item in its current queue. */
export async function onChecksReportChanged(handler: () => void): Promise<UnlistenFn> {
  return listen("checks:report-changed", handler)
}

/** Report calculation state and the pending/processing evidence backlog. */
export async function getInsightsStatus(): Promise<InsightsStatusPayload | null> {
  if (!hasShell()) return null
  return invoke<InsightsStatusPayload>("get_insights_status")
}

/** The aggregate hygiene numbers for the sessions in the activity window. */
export async function getHygieneSummary(): Promise<HygieneSummary | null> {
  if (!hasShell()) return null
  return invoke<HygieneSummary>("get_hygiene_summary")
}

/** The hygiene badges reduced from a bounded set of stored evidence rows. */
export async function getSessionHygiene(
  sessions: readonly LocalSessionIdentity[],
): Promise<SessionHygienePayload[] | null> {
  if (!hasShell()) return null
  return invoke<SessionHygienePayload[]>("get_session_hygiene", {
    sessions: sessions.map((session) => ({
      agent: session.agent,
      sessionId: session.sessionId,
      wslDistro: session.wslDistro ?? null,
    })),
  })
}

/**
 * Ask the report reduction in flight to stop at its next probe.
 *
 * The pane's session fires this when the pane closes. The reduction is
 * read-only, so cancelling never corrupts stored evidence.
 */
export async function cancelInsightsReport(): Promise<void> {
  if (!hasShell()) return
  await invoke("cancel_insights_report")
}

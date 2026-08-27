import { invoke } from "@tauri-apps/api/core"

import { hasShell } from "./ipc"

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
  catalogRevision: number
}

/** Report calculation state plus the evidence backlog counts. */
export interface InsightsStatusPayload {
  calculating: boolean
  pending: number
  processing: number
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

/** Report calculation state and the pending/processing evidence backlog. */
export async function getInsightsStatus(): Promise<InsightsStatusPayload | null> {
  if (!hasShell()) return null
  return invoke<InsightsStatusPayload>("get_insights_status")
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

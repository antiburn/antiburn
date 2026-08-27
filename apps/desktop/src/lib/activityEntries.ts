/**
 * Shaping the shell's activity payloads for the activity list.
 *
 * The shell sends values such as a cost estimate and a model list. The
 * *wording* around them ("Projected cost", the breakdown row labels) and the
 * cohort judgement ("this one is unusually expensive") are presentation, and
 * they already exist in `lib/presentation/sessionAnalysis`. This module is the
 * one place the two meet, kept pure so it can be tested without a shell.
 */

import type { SessionListEntry } from "../components/session/SessionList"
import type { ActivityEntryPayload } from "./ipc"
import { defaultAgentSurface, type AgentSurface } from "./presentation/agents"
import {
  costBreakdownRows,
  costFigureLabel,
  costOutlierThreshold,
} from "./presentation/sessionAnalysis"

/** Narrow the shell's surface string to the presentation layer's union. */
function surfaceOf(payload: ActivityEntryPayload): AgentSurface {
  if (payload.surface === "cli" || payload.surface === "ide_desktop") return payload.surface
  // `unknown` from the shell means "not classified", which is exactly what the
  // registry's slug-only fallback answers.
  return defaultAgentSurface(payload.agent)
}

/**
 * Shape one activity payload into a list entry.
 *
 * `highCostThreshold` is the cohort's outlier threshold, from
 * `costOutlierThreshold`. Pass `null` when there is no cohort to compare
 * against — the row's high-cost flag then reads as false, never as an error.
 */
export function toActivityEntry(
  payload: ActivityEntryPayload,
  highCostThreshold: number | null = null,
): SessionListEntry {
  return {
    agent: payload.agent,
    sessionId: payload.sessionId,
    repo: payload.repo,
    timestamp: payload.timestamp,
    isActive: payload.isActive,
    surface: surfaceOf(payload),
    wslDistro: payload.wslDistro,
    ...(payload.title ? { title: payload.title } : {}),
    hasForkParent: payload.hasForkParent,
    forkChildCount: payload.forkChildCount,
    modelRuns: payload.modelRuns,
    cost: payload.cost
      ? {
          totalUsd: payload.cost.totalUsd,
          figureLabel: costFigureLabel(payload.isActive),
          models: payload.models,
          isHighCost: highCostThreshold != null && payload.cost.totalUsd > highCostThreshold,
          breakdownRows: costBreakdownRows(payload.cost),
        }
      : null,
  }
}

/**
 * Shape one page of activity payloads into list entries.
 *
 * The high-cost flag is computed across the whole page rather than per row:
 * "unusually expensive" only means anything against a cohort, and the cohort is
 * the sessions the reader is looking at.
 */
export function toActivityEntries(
  payloads: readonly ActivityEntryPayload[],
): SessionListEntry[] {
  const threshold = costOutlierThreshold(
    payloads
      .map((payload) => payload.cost?.totalUsd)
      .filter((usd): usd is number => typeof usd === "number"),
  )

  return payloads.map((payload) => toActivityEntry(payload, threshold))
}

export function indexOfSession(
  entries: readonly SessionListEntry[],
  agent: string,
  sessionId: string,
  wslDistro?: string | null,
): number {
  return entries.findIndex(
    (entry) =>
      entry.agent === agent &&
      entry.sessionId === sessionId &&
      (entry.wslDistro ?? null) === (wslDistro ?? null),
  )
}

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Shaping the shell's activity payloads for the activity list.
 *
 * The shell sends values such as a cost estimate and a model list. The
 * *wording* around them ("Projected cost", the breakdown row labels) and the
 * cohort judgement ("this one is unusually expensive") are presentation, and
 * they already exist in `lib/presentation/sessionAnalytics`. This module is the
 * one place the two meet, kept pure so it can be tested without a shell.
 */

import type { SessionListEntry } from "../components/session/SessionList"
import type { ActivityEntryPayload } from "./ipc"
import { defaultAgentSurface, type AgentSurface } from "./presentation/agents"
import {
  costBreakdownRows,
  costFigureLabel,
  costOutlierThreshold,
} from "./presentation/sessionAnalytics"

/** Narrow the shell's surface string to the presentation layer's union. */
function surfaceOf(payload: ActivityEntryPayload): AgentSurface {
  if (payload.surface === "cli" || payload.surface === "ide_desktop") return payload.surface
  // `unknown` from the shell means "not classified", which is exactly what the
  // registry's slug-only fallback answers.
  return defaultAgentSurface(payload.agent)
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

  return payloads.map((payload) => ({
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
          isHighCost: threshold != null && payload.cost.totalUsd > threshold,
          breakdownRows: costBreakdownRows(payload.cost),
        }
      : null,
  }))
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

import type { CheckAvailability } from "../../../src/lib/checkAvailability"
import type { BurnCheckDetectorId, ChecksReportPayload } from "../../../src/lib/insightsIpc"
import { CHECK_DEFINITIONS } from "../../../src/lib/presentation/checkDefinitions"
import { emitFixtureEvent } from "./event"
import { hasSmartCheckFixture, smartCheckReport } from "./smartChecks"

const availability: CheckAvailability = {
  revision: 0,
  checks: (Object.keys(CHECK_DEFINITIONS) as BurnCheckDetectorId[]).map((id) => ({
    id,
    enabled: true,
  })),
  configured: false,
  savedKey: false,
  error: null,
  usage: {
    inputTokens: 0,
    outputTokens: 0,
    confirmedCalls: 0,
    cacheHits: 0,
    unknownOutcomes: 0,
    estimatedUsd: "$0.00",
    lastUsedAtEpoch: null,
  },
  historyDays: 0,
  backfill: {
    total: 0,
    waitingForData: 0,
    waitingForIdle: 0,
    ready: 0,
    queued: 0,
    running: 0,
    completed: 0,
    skipped: 0,
    failed: 0,
  },
}

export function fixtureCheckAvailability(): CheckAvailability {
  return structuredClone(
    hasSmartCheckFixture()
      ? { ...availability, configured: true, savedKey: true }
      : availability,
  )
}

export function setFixtureCheckEnabled(args?: Record<string, unknown>): CheckAvailability {
  const check = availability.checks.find(({ id }) => id === args?.detector)
  if (!check || typeof args?.enabled !== "boolean") throw new Error("Unknown fixture check")
  if (check.enabled !== args.enabled) {
    check.enabled = args.enabled
    availability.revision += 1
    emitFixtureEvent("checks:availability-changed", {
      status: "updated",
      snapshot: fixtureCheckAvailability(),
    })
  }
  return fixtureCheckAvailability()
}

export function fixtureChecksReport(): ChecksReportPayload {
  if (hasSmartCheckFixture()) return smartCheckReport()
  return {
    evidenceSettled: true,
    windowSessions: 128,
    pendingEvidence: 0,
    deferredEvidence: 0,
    estimatedTokenBurnBasisPoints: null,
    categories: availability.checks
      .filter(({ id, enabled }) => enabled && CHECK_DEFINITIONS[id].kind === "local")
      .map(({ id }) => ({
        id,
        finding: 0,
        clean: 128,
        unavailable: 0,
        estimatedTokenBurnBasisPoints: null,
      })),
  }
}

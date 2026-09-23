import type { SessionListEntry } from "../components/session/SessionList"
import { sessionBurnCheckPresentation } from "./presentation/burnChecks"
import { INITIAL_SESSION_HYGIENE, sessionHygieneChecks } from "./presentation/sessionHygiene"
import type { BurnCheckDetectorId } from "./insightsIpc"
import { visibleSessionHygieneChecks } from "./snoozedBurnChecks"
import { sessionHygieneFor, type SessionHygieneSnapshot } from "./useSessionHygiene"

import {
  FIXED_SESSION_FILTERS,
  type SessionFilter,
} from "./navigation/sessionFilterDefinitions"

/** A priced session counts as Material at or above this cost, in US dollars. */
export const MATERIAL_COST_FLOOR_USD = 1

export type SessionResultFilter = "all" | "failing" | "passing"
export type SessionSpendFilter = "all" | "notable" | "material"

export interface SessionFilters {
  agents: string[]
  result: SessionResultFilter
  spend: SessionSpendFilter
}

export function normalizeSessionFilters(filters: SessionFilters): SessionFilters {
  return { ...filters, agents: [...new Set(filters.agents)].sort() }
}

export function serializeSessionFilters(filters: SessionFilters): string {
  const { agents, result, spend } = normalizeSessionFilters(filters)
  return `v1:${JSON.stringify({ agents, result, spend })}`
}

export function parseSessionFilters(saved: string): SessionFilters {
  const all: SessionFilters = { agents: [], result: "all", spend: "all" }
  if (!saved.startsWith("v1:")) {
    const legacy = parseSessionFilterId(saved)
    switch (legacy.kind) {
      case "agent":
        return legacy.agent.trim() ? { ...all, agents: [legacy.agent] } : all
      case "failing":
      case "passing":
        return { ...all, result: legacy.kind }
      case "notable":
      case "material":
        return { ...all, spend: legacy.kind }
      case "all":
        return all
    }
  }
  try {
    const value: unknown = JSON.parse(saved.slice(3))
    if (typeof value !== "object" || value === null || Array.isArray(value)) return all
    if (!("agents" in value) || !("result" in value) || !("spend" in value)) return all
    const { agents, result, spend } = value
    if (
      !Array.isArray(agents) ||
      !agents.every(
        (agent): agent is string => typeof agent === "string" && agent.trim().length > 0,
      ) ||
      (result !== "all" && result !== "failing" && result !== "passing") ||
      (spend !== "all" && spend !== "notable" && spend !== "material")
    )
      return all
    return normalizeSessionFilters({ agents, result, spend })
  } catch {
    return all
  }
}

/** Parse a legacy filter. Preserve unknown agents and reset unknown kinds. */
function parseSessionFilterId(id: string): SessionFilter {
  const fixed = FIXED_SESSION_FILTERS.find((filter) => filter.id === id)
  if (fixed) return { kind: fixed.id }
  if (id.startsWith("agent:")) {
    const agent = id.slice("agent:".length)
    if (agent) return { kind: "agent", agent }
  }
  return { kind: "all" }
}

/** The failed/passed/unassessed hygiene counts backing an entry's row. */
function hygieneCountsFor(
  hygieneSnapshot: SessionHygieneSnapshot,
  entry: SessionListEntry,
  snoozed: ReadonlySet<BurnCheckDetectorId> = new Set(),
): { failed: number; passed: number; unassessed: number } {
  const payload = entry.sessionId
    ? sessionHygieneFor(hygieneSnapshot, {
        agent: entry.agent,
        sessionId: entry.sessionId,
        wslDistro: entry.wslDistro ?? null,
      })
    : INITIAL_SESSION_HYGIENE
  return sessionBurnCheckPresentation(
    visibleSessionHygieneChecks(sessionHygieneChecks(payload), snoozed),
    payload.evidenceState,
  ).counts
}

function resultMatches(
  entry: SessionListEntry,
  hygieneSnapshot: SessionHygieneSnapshot,
  snoozed: ReadonlySet<BurnCheckDetectorId>,
): Record<SessionResultFilter, boolean> {
  const counts = hygieneCountsFor(hygieneSnapshot, entry, snoozed)
  return {
    all: true,
    failing: counts.failed >= 1,
    passing: counts.failed === 0 && counts.passed >= 1,
  }
}

function spendMatches(entry: SessionListEntry): Record<SessionSpendFilter, boolean> {
  return {
    all: true,
    notable: entry.cost?.isHighCost === true,
    material: entry.cost != null && entry.cost.totalUsd >= MATERIAL_COST_FLOOR_USD,
  }
}

/** Preserve entry order and the full-cohort high-cost classification. */
export function filterSessionEntries(
  entries: readonly SessionListEntry[],
  hygieneSnapshot: SessionHygieneSnapshot,
  filters: SessionFilters,
  snoozed: ReadonlySet<BurnCheckDetectorId> = new Set(),
): SessionListEntry[] {
  const agents = new Set(filters.agents)
  return entries.filter(
    (entry) =>
      (agents.size === 0 || agents.has(entry.agent)) &&
      spendMatches(entry)[filters.spend] &&
      (filters.result === "all" ||
        resultMatches(entry, hygieneSnapshot, snoozed)[filters.result]),
  )
}

export interface SessionFilterCounts {
  all: number
  matching: number
  agentsAll: number
  agents: Record<string, number>
  result: Record<SessionResultFilter, number>
  spend: Record<SessionSpendFilter, number>
}

/** Count each candidate with the other facets unchanged. */
export function sessionFilterCounts(
  entries: readonly SessionListEntry[],
  hygieneSnapshot: SessionHygieneSnapshot,
  filters: SessionFilters,
  snoozed: ReadonlySet<BurnCheckDetectorId> = new Set(),
): SessionFilterCounts {
  const agents = new Set(filters.agents)
  const agentCounts = new Map(filters.agents.map((agent) => [agent, 0]))
  const counts: SessionFilterCounts = {
    all: entries.length,
    matching: 0,
    agentsAll: 0,
    agents: {},
    result: { all: 0, failing: 0, passing: 0 },
    spend: { all: 0, notable: 0, material: 0 },
  }

  for (const entry of entries) {
    const result = resultMatches(entry, hygieneSnapshot, snoozed)
    const spend = spendMatches(entry)
    const agentMatches = agents.size === 0 || agents.has(entry.agent)
    const otherFacetsMatch = result[filters.result] && spend[filters.spend]
    agentCounts.set(entry.agent, (agentCounts.get(entry.agent) ?? 0) + Number(otherFacetsMatch))
    if (otherFacetsMatch) {
      counts.agentsAll += 1
      if (agentMatches) counts.matching += 1
    }
    if (agentMatches && spend[filters.spend]) {
      counts.result.all += 1
      counts.result.failing += Number(result.failing)
      counts.result.passing += Number(result.passing)
    }
    if (agentMatches && result[filters.result]) {
      counts.spend.all += 1
      counts.spend.notable += Number(spend.notable)
      counts.spend.material += Number(spend.material)
    }
  }
  counts.agents = Object.fromEntries(agentCounts)
  return counts
}

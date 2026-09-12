/**
 * The Sessions sidebar filter vocabulary.
 *
 * A `SessionFilter` selects a subset of the loaded session list. Its id is
 * the persisted form: it survives a round trip through `AppSettings` and
 * doubles as a nav item id, so the format and parse helpers here are the
 * single place that shape stays in sync with itself.
 */

import type { SessionListEntry } from "../components/session/SessionList"
import { agentDisplayName } from "./presentation/agents"
import { sessionBurnCheckPresentation } from "./presentation/burnChecks"
import { INITIAL_SESSION_HYGIENE, sessionHygieneChecks } from "./presentation/sessionHygiene"
import { sessionHygieneFor, type SessionHygieneSnapshot } from "./useSessionHygiene"

/** One selectable filter over the loaded session list. */
export type SessionFilter =
  | { kind: "notable" }
  | { kind: "material" }
  | { kind: "agent"; agent: string }
  | { kind: "failing" }
  | { kind: "passing" }
  | { kind: "all" }

/** A priced session counts as Material at or above this cost, in US dollars. */
export const MATERIAL_COST_FLOOR_USD = 1

/** The persisted, nav-item id for one filter. Stable across releases. */
export function sessionFilterId(filter: SessionFilter): string {
  return filter.kind === "agent" ? `agent:${filter.agent}` : filter.kind
}

/**
 * Parse a persisted or nav-selected id back into a filter.
 *
 * An id this app never wrote — an older release, a hand-edited database, or
 * an agent slug that dropped out of the loaded list — parses to `all`. A
 * filter can always fall back to showing everything.
 */
export function parseSessionFilterId(id: string): SessionFilter {
  if (
    id === "notable" ||
    id === "material" ||
    id === "failing" ||
    id === "passing" ||
    id === "all"
  ) {
    return { kind: id }
  }
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
): { failed: number; passed: number; unassessed: number } {
  const payload = entry.sessionId
    ? sessionHygieneFor(hygieneSnapshot, {
        agent: entry.agent,
        sessionId: entry.sessionId,
        wslDistro: entry.wslDistro ?? null,
      })
    : INITIAL_SESSION_HYGIENE
  return sessionBurnCheckPresentation(sessionHygieneChecks(payload), payload.evidenceState)
    .counts
}

/** Whether one entry belongs to the given filter. */
export function matchesSessionFilter(
  entry: SessionListEntry,
  hygieneSnapshot: SessionHygieneSnapshot,
  filter: SessionFilter,
): boolean {
  switch (filter.kind) {
    case "notable":
      return entry.cost?.isHighCost === true
    case "material":
      return entry.cost != null && entry.cost.totalUsd >= MATERIAL_COST_FLOOR_USD
    case "agent":
      return entry.agent === filter.agent
    case "failing":
      return hygieneCountsFor(hygieneSnapshot, entry).failed >= 1
    case "passing": {
      const counts = hygieneCountsFor(hygieneSnapshot, entry)
      return counts.failed === 0 && counts.passed >= 1
    }
    case "all":
      return true
  }
}

/** The entries one filter selects, in the order they arrived. */
export function filterSessionEntries(
  entries: readonly SessionListEntry[],
  hygieneSnapshot: SessionHygieneSnapshot,
  filter: SessionFilter,
): SessionListEntry[] {
  return entries.filter((entry) => matchesSessionFilter(entry, hygieneSnapshot, filter))
}

/** A count for every fixed filter, plus one row per harness present. */
export interface SessionFilterCounts {
  notable: number
  material: number
  failing: number
  passing: number
  all: number
  /** Harnesses present in the loaded list, sorted by display name. */
  agents: Array<{ agent: string; displayName: string; count: number }>
}

/**
 * Fold the loaded list into a count per filter.
 *
 * Every fixed filter counts over the whole list in one pass. Agent counts key
 * on the raw slug, so two sessions from the same harness always fold into one
 * row regardless of how it renders.
 */
export function sessionFilterCounts(
  entries: readonly SessionListEntry[],
  hygieneSnapshot: SessionHygieneSnapshot,
): SessionFilterCounts {
  let notable = 0
  let material = 0
  let failing = 0
  let passing = 0
  const agentCounts = new Map<string, number>()

  for (const entry of entries) {
    if (matchesSessionFilter(entry, hygieneSnapshot, { kind: "notable" })) notable += 1
    if (matchesSessionFilter(entry, hygieneSnapshot, { kind: "material" })) material += 1
    const counts = hygieneCountsFor(hygieneSnapshot, entry)
    if (counts.failed >= 1) failing += 1
    else if (counts.passed >= 1) passing += 1
    agentCounts.set(entry.agent, (agentCounts.get(entry.agent) ?? 0) + 1)
  }

  const agents = [...agentCounts.entries()]
    .map(([agent, count]) => ({ agent, displayName: agentDisplayName(agent), count }))
    .sort((left, right) => left.displayName.localeCompare(right.displayName))

  return { notable, material, failing, passing, all: entries.length, agents }
}

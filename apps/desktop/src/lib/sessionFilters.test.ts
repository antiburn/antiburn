import { describe, expect, it } from "vitest"

import type { SessionListEntry } from "../components/session/SessionList"
import type { SessionHygienePayload } from "./insightsIpc"
import { localSessionKey } from "./presentation/localIdentity"
import {
  filterSessionEntries,
  sessionFilterCounts,
  MATERIAL_COST_FLOOR_USD,
  parseSessionFilters,
  serializeSessionFilters,
} from "./sessionFilters"
import type { SessionHygieneSnapshot } from "./useSessionHygiene"

function entry(over: Partial<SessionListEntry> = {}): SessionListEntry {
  return {
    agent: "claude-code",
    sessionId: "session-1",
    repo: "avery/widgets",
    timestamp: "2026-03-04T12:00:00.000Z",
    isActive: false,
    ...over,
  }
}

function hygienePayload(clean: number, finding: number): SessionHygienePayload {
  const ids = [
    "sessionOverdepth",
    "modelOverthinking",
    "overpoweredSubagents",
    "obsoleteModel",
    "fastModeOveruse",
    "excessCacheRehydration",
  ] as const
  const badges = ids.slice(0, clean + finding).map((id, index) => ({
    id,
    status: index < finding ? ("finding" as const) : ("clean" as const),
    notAssessedReason: null,
  }))
  return { badges, evidenceState: "ready", unusedResources: null }
}

/** A hygiene snapshot with one entry's payload keyed by its identity. */
function hygieneSnapshotFor(
  entryFor: SessionListEntry,
  payload: SessionHygienePayload,
): SessionHygieneSnapshot {
  return new Map([
    [localSessionKey(entryFor.agent, entryFor.sessionId ?? "", entryFor.wslDistro), payload],
  ])
}

const EMPTY_HYGIENE: SessionHygieneSnapshot = new Map()

describe("filter boundaries", () => {
  it.each([
    [null, false],
    [MATERIAL_COST_FLOOR_USD - 0.01, false],
    [MATERIAL_COST_FLOOR_USD, true],
  ])("applies the $1 floor to %s (%s)", (totalUsd, included) => {
    const session = entry({
      cost: totalUsd === null ? null : { totalUsd, figureLabel: "Estimated cost" },
    })
    expect(
      filterSessionEntries([session], EMPTY_HYGIENE, parseSessionFilters("material")),
    ).toEqual(included ? [session] : [])
  })

  it.each([
    [2, 0, "passing"],
    [1, 5, "failing"],
    [1, 0, "passing"],
    [0, 0, "neither"],
  ])("classifies %s clean and %s failed checks as %s", (clean, failed, result) => {
    const session = entry()
    const snapshot = hygieneSnapshotFor(session, hygienePayload(clean, failed))
    for (const filter of ["passing", "failing"]) {
      expect(filterSessionEntries([session], snapshot, parseSessionFilters(filter))).toEqual(
        filter === result ? [session] : [],
      )
    }
  })

  it("treats a session without a transcript id as unassessed", () => {
    const session = entry({ sessionId: undefined })
    for (const filter of ["passing", "failing"]) {
      expect(
        filterSessionEntries([session], EMPTY_HYGIENE, parseSessionFilters(filter)),
      ).toEqual([])
    }
  })
})

describe("sessionFilterCounts", () => {
  it("counts each facet and every present agent", () => {
    const notable = entry({
      sessionId: "notable",
      agent: "codex",
      // Below the Material floor: notable and material are independent flags.
      cost: { totalUsd: 0.5, figureLabel: "Estimated cost", isHighCost: true },
    })
    const material = entry({
      sessionId: "material",
      agent: "claude-code",
      cost: { totalUsd: MATERIAL_COST_FLOOR_USD, figureLabel: "Estimated cost" },
    })
    const unpriced = entry({ sessionId: "unpriced", agent: "cursor", cost: null })
    const failing = entry({ sessionId: "failing", agent: "codex" })
    const passing = entry({ sessionId: "passing", agent: "claude-code" })

    const entries = [notable, material, unpriced, failing, passing]
    const hygiene: SessionHygieneSnapshot = new Map([
      ...hygieneSnapshotFor(failing, hygienePayload(0, 1)),
      ...hygieneSnapshotFor(passing, hygienePayload(1, 0)),
    ])

    const counts = sessionFilterCounts(entries, hygiene, parseSessionFilters("all"))
    expect(counts.spend.notable).toBe(1)
    expect(counts.spend.material).toBe(1)
    expect(counts.result.failing).toBe(1)
    expect(counts.result.passing).toBe(1)
    expect(counts.all).toBe(5)
    expect(counts.agents).toEqual({ "claude-code": 2, codex: 2, cursor: 1 })
  })

  it("removes snoozed findings from both filtered rows and contextual counts", () => {
    const failing = entry({ sessionId: "failing" })
    const hygiene = new Map(hygieneSnapshotFor(failing, hygienePayload(0, 1)))
    const snoozed = new Set(["sessionsOverDepth"] as const)

    expect(
      filterSessionEntries([failing], hygiene, parseSessionFilters("failing"), snoozed),
    ).toEqual([])
    expect(
      sessionFilterCounts([failing], hygiene, parseSessionFilters("all"), snoozed),
    ).toMatchObject({
      result: { failing: 0, passing: 0 },
    })
  })

  it("counts nothing for an empty list", () => {
    const counts = sessionFilterCounts([], EMPTY_HYGIENE, parseSessionFilters("all"))
    expect(counts).toEqual({
      all: 0,
      matching: 0,
      agentsAll: 0,
      result: { all: 0, failing: 0, passing: 0 },
      spend: { all: 0, notable: 0, material: 0 },
      agents: {},
    })
  })
})

describe("saved contextual filters", () => {
  it.each([
    ["all", { agents: [], result: "all", spend: "all" }],
    ["notable", { agents: [], result: "all", spend: "notable" }],
    ["material", { agents: [], result: "all", spend: "material" }],
    ["failing", { agents: [], result: "failing", spend: "all" }],
    ["passing", { agents: [], result: "passing", spend: "all" }],
    ["agent:future-agent", { agents: ["future-agent"], result: "all", spend: "all" }],
  ])("migrates %s without changing its meaning", (saved, expected) => {
    expect(parseSessionFilters(saved)).toEqual(expected)
  })

  it.each([
    "",
    "bogus",
    "agent:",
    "agent:   ",
    "v2:{}",
    "v1:not json",
    "v1:null",
    "v1:[]",
    'v1:{"agents":[],"result":"unknown","spend":"all"}',
    'v1:{"agents":[],"result":"all","spend":"any"}',
    'v1:{"agents":[1],"result":"all","spend":"all"}',
    'v1:{"agents":[""],"result":"all","spend":"all"}',
    'v1:{"agents":["  "],"result":"all","spend":"all"}',
    'v1:{"agents":[],"result":"all"}',
  ])("falls back safely for %s", (saved) => {
    expect(parseSessionFilters(saved)).toEqual({ agents: [], result: "all", spend: "all" })
  })

  it("deduplicates and sorts opaque agent slugs for stable round trips", () => {
    const filters = {
      agents: ["future-agent", "codex", "codex"],
      result: "failing" as const,
      spend: "material" as const,
    }
    const saved = serializeSessionFilters(filters)
    expect(saved).toBe(
      'v1:{"agents":["codex","future-agent"],"result":"failing","spend":"material"}',
    )
    expect(serializeSessionFilters(parseSessionFilters(saved))).toBe(saved)
    expect(filters.agents).toEqual(["future-agent", "codex", "codex"])
  })
})

describe("composable facets and contextual counts", () => {
  const codex = entry({
    agent: "codex",
    sessionId: "a",
    cost: { totalUsd: 1, figureLabel: "Estimated cost", isHighCost: true },
  })
  const claude = entry({
    agent: "claude-code",
    sessionId: "b",
    cost: { totalUsd: 2, figureLabel: "Estimated cost" },
  })
  const cursor = entry({ agent: "cursor", sessionId: "c", cost: null })
  const sessions = [codex, claude, cursor]
  const hygiene = new Map([
    ...hygieneSnapshotFor(codex, hygienePayload(1, 1)),
    ...hygieneSnapshotFor(claude, hygienePayload(2, 0)),
  ])

  it("combines agents with OR and facets with AND", () => {
    const filters = {
      agents: ["codex", "claude-code"],
      result: "all" as const,
      spend: "all" as const,
    }
    expect(filterSessionEntries(sessions, hygiene, filters)).toEqual([codex, claude])
    expect(
      filterSessionEntries(sessions, hygiene, {
        ...filters,
        result: "failing",
        spend: "material",
      }),
    ).toEqual([codex])
    expect(
      filterSessionEntries(sessions, hygiene, { ...filters, agents: [], result: "all" }),
    ).toEqual(sessions)
    expect(
      filterSessionEntries(sessions, hygiene, {
        ...filters,
        agents: ["cursor"],
        spend: "material",
      }),
    ).toEqual([])
  })

  it("replaces just the option's facet and isolates each agent count", () => {
    const filters = {
      agents: ["codex", "future-agent"],
      result: "failing" as const,
      spend: "material" as const,
    }
    const counts = sessionFilterCounts(sessions, hygiene, filters)
    expect(counts).toEqual({
      all: 3,
      matching: 1,
      agentsAll: 1,
      agents: { codex: 1, "claude-code": 0, cursor: 0, "future-agent": 0 },
      result: { all: 1, failing: 1, passing: 0 },
      spend: { all: 1, material: 1, notable: 1 },
    })
    expect(
      sessionFilterCounts(sessions, hygiene, { ...filters, result: "all" }).agents,
    ).toEqual({ codex: 1, "claude-code": 1, cursor: 0, "future-agent": 0 })
  })

  it("uses the same snoozed checks for rows and counts", () => {
    const filters = { agents: [], result: "passing" as const, spend: "all" as const }
    const snoozed = new Set(["sessionsOverDepth"] as const)
    expect(filterSessionEntries(sessions, hygiene, filters, snoozed)).toEqual([codex, claude])
    const counts = sessionFilterCounts(sessions, hygiene, filters, snoozed)
    expect(counts.matching).toBe(2)
    expect(counts.result).toEqual({ all: 3, failing: 0, passing: 2 })
    expect(counts.agents).toEqual({ codex: 1, "claude-code": 1, cursor: 0 })
  })
})

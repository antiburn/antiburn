import { describe, expect, it } from "vitest"

import type { SessionListEntry } from "../components/session/SessionList"
import type { SessionHygienePayload } from "./insightsIpc"
import { localSessionKey } from "./presentation/localIdentity"
import {
  filterSessionEntries,
  matchesSessionFilter,
  parseSessionFilterId,
  sessionFilterCounts,
  sessionFilterId,
  MATERIAL_COST_FLOOR_USD,
  type SessionFilter,
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
  return { badges, evidenceState: "ready" }
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

describe("sessionFilterId / parseSessionFilterId", () => {
  it("round-trips every fixed filter through its stable id", () => {
    const filters: SessionFilter[] = [
      { kind: "notable" },
      { kind: "material" },
      { kind: "failing" },
      { kind: "passing" },
      { kind: "all" },
    ]
    for (const filter of filters) {
      expect(parseSessionFilterId(sessionFilterId(filter))).toEqual(filter)
    }
  })

  it("formats and parses an agent filter", () => {
    const filter: SessionFilter = { kind: "agent", agent: "claude-code" }
    expect(sessionFilterId(filter)).toBe("agent:claude-code")
    expect(parseSessionFilterId("agent:claude-code")).toEqual(filter)
  })

  it("falls back an unknown id to all", () => {
    expect(parseSessionFilterId("bogus")).toEqual({ kind: "all" })
    expect(parseSessionFilterId("")).toEqual({ kind: "all" })
  })

  it("falls back an agent id with no slug to all", () => {
    expect(parseSessionFilterId("agent:")).toEqual({ kind: "all" })
  })
})

describe("matchesSessionFilter — notable", () => {
  it("uses the existing high-cost flame flag, not a recomputed threshold", () => {
    const highCost = entry({
      cost: { totalUsd: 50, figureLabel: "Estimated cost", isHighCost: true },
    })
    const ordinary = entry({
      sessionId: "session-2",
      cost: { totalUsd: 50, figureLabel: "Estimated cost", isHighCost: false },
    })
    expect(matchesSessionFilter(highCost, EMPTY_HYGIENE, { kind: "notable" })).toBe(true)
    expect(matchesSessionFilter(ordinary, EMPTY_HYGIENE, { kind: "notable" })).toBe(false)
  })
})

describe("matchesSessionFilter — material", () => {
  it("excludes an unpriced session", () => {
    const unpriced = entry({ cost: null })
    expect(matchesSessionFilter(unpriced, EMPTY_HYGIENE, { kind: "material" })).toBe(false)
  })

  it("includes a session priced exactly at the floor", () => {
    const atFloor = entry({
      cost: { totalUsd: MATERIAL_COST_FLOOR_USD, figureLabel: "Estimated cost" },
    })
    expect(matchesSessionFilter(atFloor, EMPTY_HYGIENE, { kind: "material" })).toBe(true)
  })

  it("excludes a session priced just below the floor", () => {
    const belowFloor = entry({
      cost: { totalUsd: MATERIAL_COST_FLOOR_USD - 0.01, figureLabel: "Estimated cost" },
    })
    expect(matchesSessionFilter(belowFloor, EMPTY_HYGIENE, { kind: "material" })).toBe(false)
  })
})

describe("matchesSessionFilter — agent", () => {
  it("matches only the named agent", () => {
    const codex = entry({ agent: "codex" })
    expect(matchesSessionFilter(codex, EMPTY_HYGIENE, { kind: "agent", agent: "codex" })).toBe(
      true,
    )
    expect(
      matchesSessionFilter(codex, EMPTY_HYGIENE, { kind: "agent", agent: "claude-code" }),
    ).toBe(false)
  })
})

describe("matchesSessionFilter — failing / passing boundary", () => {
  it("counts 2/2 clean checks as passing, not failing", () => {
    const session = entry()
    const snapshot = hygieneSnapshotFor(session, hygienePayload(2, 0))
    expect(matchesSessionFilter(session, snapshot, { kind: "passing" })).toBe(true)
    expect(matchesSessionFilter(session, snapshot, { kind: "failing" })).toBe(false)
  })

  it("counts 5 findings against 1 clean check as failing, not passing", () => {
    const session = entry()
    const snapshot = hygieneSnapshotFor(session, hygienePayload(1, 5))
    expect(matchesSessionFilter(session, snapshot, { kind: "failing" })).toBe(true)
    expect(matchesSessionFilter(session, snapshot, { kind: "passing" })).toBe(false)
  })

  it("counts exactly one clean check as passing", () => {
    const session = entry()
    const snapshot = hygieneSnapshotFor(session, hygienePayload(1, 0))
    expect(matchesSessionFilter(session, snapshot, { kind: "passing" })).toBe(true)
    expect(matchesSessionFilter(session, snapshot, { kind: "failing" })).toBe(false)
  })

  it("counts a session with nothing assessed as neither", () => {
    const session = entry()
    const snapshot = hygieneSnapshotFor(session, hygienePayload(0, 0))
    expect(matchesSessionFilter(session, snapshot, { kind: "passing" })).toBe(false)
    expect(matchesSessionFilter(session, snapshot, { kind: "failing" })).toBe(false)
  })

  it("treats a session with no transcript id as unassessed", () => {
    const session = entry({ sessionId: undefined })
    expect(matchesSessionFilter(session, EMPTY_HYGIENE, { kind: "passing" })).toBe(false)
    expect(matchesSessionFilter(session, EMPTY_HYGIENE, { kind: "failing" })).toBe(false)
  })
})

describe("matchesSessionFilter — all", () => {
  it("matches every entry", () => {
    expect(matchesSessionFilter(entry(), EMPTY_HYGIENE, { kind: "all" })).toBe(true)
  })
})

describe("filterSessionEntries", () => {
  it("keeps only the entries the filter selects, in order", () => {
    const claude = entry({ sessionId: "a", agent: "claude-code" })
    const codex = entry({ sessionId: "b", agent: "codex" })
    const result = filterSessionEntries([claude, codex], EMPTY_HYGIENE, {
      kind: "agent",
      agent: "codex",
    })
    expect(result).toEqual([codex])
  })
})

describe("sessionFilterCounts", () => {
  it("counts every fixed filter and every present agent, sorted by display name", () => {
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

    const counts = sessionFilterCounts(entries, hygiene)
    expect(counts.notable).toBe(1)
    expect(counts.material).toBe(1)
    expect(counts.failing).toBe(1)
    expect(counts.passing).toBe(1)
    expect(counts.all).toBe(5)
    // Codex (2 sessions) and Cursor and Claude Code (1 each), by display name:
    // Claude Code, Codex, Cursor.
    expect(counts.agents).toEqual([
      { agent: "claude-code", displayName: "Claude Code", count: 2 },
      { agent: "codex", displayName: "Codex", count: 2 },
      { agent: "cursor", displayName: "Cursor", count: 1 },
    ])
  })

  it("gives an unrecognized agent slug a title-cased display name", () => {
    const counts = sessionFilterCounts([entry({ agent: "future-agent" })], EMPTY_HYGIENE)
    expect(counts.agents).toEqual([
      { agent: "future-agent", displayName: "Future Agent", count: 1 },
    ])
  })

  it("counts nothing for an empty list", () => {
    const counts = sessionFilterCounts([], EMPTY_HYGIENE)
    expect(counts).toEqual({
      notable: 0,
      material: 0,
      failing: 0,
      passing: 0,
      all: 0,
      agents: [],
    })
  })
})

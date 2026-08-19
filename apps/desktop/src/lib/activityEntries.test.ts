// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { describe, expect, it } from "vitest"

import { indexOfSession, toActivityEntries } from "./activityEntries"
import type { ActivityEntryPayload } from "./ipc"
import { HIGH_COST_MIN_SAMPLE } from "./presentation/sessionAnalytics"

function payload(over: Partial<ActivityEntryPayload> = {}): ActivityEntryPayload {
  return {
    agent: "claude-code",
    sessionId: "session-1",
    repo: "widgets",
    timestamp: "2026-08-01T10:00:00.000Z",
    isActive: false,
    surface: "cli",
    wslDistro: null,
    title: "Wire the tray popover",
    hasForkParent: false,
    forkChildCount: 0,
    subagentCount: 0,
    cost: null,
    models: [],
    activeSecs: null,
    durationSecs: null,
    ...over,
  }
}

function cost(totalUsd: number) {
  return {
    totalUsd,
    inputUsd: totalUsd / 4,
    outputUsd: totalUsd / 4,
    cacheReadUsd: totalUsd / 4,
    cacheWriteUsd: totalUsd / 4,
  }
}

describe("toActivityEntries", () => {
  it("carries the identity, repository, and title straight through", () => {
    const [entry] = toActivityEntries([payload()])
    expect(entry).toMatchObject({
      agent: "claude-code",
      sessionId: "session-1",
      repo: "widgets",
      title: "Wire the tray popover",
      surface: "cli",
    })
  })

  it("labels a live session as a projection and a finished one as an estimate", () => {
    const [live] = toActivityEntries([payload({ isActive: true, cost: cost(1) })])
    const [done] = toActivityEntries([payload({ isActive: false, cost: cost(1) })])
    expect(live?.cost?.figureLabel).toBe("Projected cost")
    expect(done?.cost?.figureLabel).toBe("Estimated cost")
  })

  it("omits the cost pill entirely when nothing could be priced", () => {
    const [entry] = toActivityEntries([payload({ cost: null })])
    expect(entry?.cost).toBeNull()
  })

  it("flags a cost outlier against the page, not against itself", () => {
    // Below the minimum sample the median is too noisy to judge, so even a wild
    // outlier is left unflagged.
    const small = toActivityEntries([payload({ cost: cost(0.1) }), payload({ cost: cost(90) })])
    expect(small.every((entry) => entry.cost?.isHighCost === false)).toBe(true)

    const cohort = Array.from({ length: HIGH_COST_MIN_SAMPLE }, (_, index) =>
      payload({ sessionId: `s${index}`, cost: cost(0.5) }),
    )
    const withOutlier = toActivityEntries([
      ...cohort,
      payload({ sessionId: "big", cost: cost(90) }),
    ])
    expect(withOutlier.at(-1)?.cost?.isHighCost).toBe(true)
    expect(withOutlier[0]?.cost?.isHighCost).toBe(false)
  })

  it("falls back to the agent registry when the shell could not classify a surface", () => {
    const [entry] = toActivityEntries([payload({ agent: "opencode", surface: "unknown" })])
    expect(entry?.surface).toBe("cli")
  })

  it("omits the time pill when the session has not been analyzed", () => {
    const [none] = toActivityEntries([payload({ activeSecs: null })])
    expect(none?.activeTime).toBeNull()

    const [some] = toActivityEntries([payload({ activeSecs: 600, durationSecs: 1200 })])
    expect(some?.activeTime).toEqual({ activeSecs: 600, durationSecs: 1200 })
  })
})

describe("indexOfSession", () => {
  it("finds a session by its full identity, environment included", () => {
    const entries = toActivityEntries([
      payload({ sessionId: "a" }),
      payload({ sessionId: "b", wslDistro: "Ubuntu" }),
    ])

    expect(indexOfSession(entries, "claude-code", "a", null)).toBe(0)
    expect(indexOfSession(entries, "claude-code", "b", "Ubuntu")).toBe(1)
    // The same id in a different environment is a different session.
    expect(indexOfSession(entries, "claude-code", "b", null)).toBe(-1)
    expect(indexOfSession(entries, "codex", "a", null)).toBe(-1)
  })
})

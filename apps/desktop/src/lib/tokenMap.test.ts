import { describe, expect, it } from "vitest"

import type { HudModeTokens, HudTokenMapPayload, HudTokenMapSession } from "./ipc"
import { DOT_VALUE_LADDER, deriveTokenMap, sessionRate } from "./tokenMap"

const zero: HudModeTokens = {
  looking: 0,
  running: 0,
  changing: 0,
  delegating: 0,
  thinking: 0,
  talking: 0,
  other: 0,
}

function session(
  id: string,
  modes: Partial<HudModeTokens>,
  extra: Partial<HudTokenMapSession> = {},
): HudTokenMapSession {
  const full = { ...zero, ...modes }
  const total = Object.values(full).reduce((sum, value) => sum + value, 0)
  return {
    agent: "claude-code",
    sessionId: id,
    title: null,
    lastTurnEpoch: null,
    // Five-minute window: rate is total / 5.
    tokensPerMin: total / 5,
    modes: full,
    subagents: [],
    ...extra,
  }
}

function payload(sessions: HudTokenMapSession[], nowEpoch = 1_000): HudTokenMapPayload {
  return { nowEpoch, windowSecs: 300, sessions }
}

describe("deriveTokenMap", () => {
  it("returns an empty square for no sessions", () => {
    const layout = deriveTokenMap(payload([]))
    expect(layout.dots).toEqual([])
    expect(layout.blobs).toEqual([])
    expect(layout.overflow).toBe(false)
  })

  it("picks the finest dot value at which everything fits", () => {
    // 6k tokens/min at 250 per dot = 24 dots, fits a 12x12 square.
    const layout = deriveTokenMap(payload([session("a", { looking: 30_000 })]))
    expect(layout.dotValue).toBe(250)
    expect(layout.dots).toHaveLength(24)
    expect(layout.dots.every((dot) => dot.mode === "looking")).toBe(true)
  })

  it("steps the ladder up when a burst would overflow", () => {
    // 200k tokens/min: 800 dots at 250, 100 dots at 2k, which fits 144 cells.
    const layout = deriveTokenMap(payload([session("a", { running: 1_000_000 })]))
    expect(layout.dotValue).toBe(2_000)
    expect(layout.dots).toHaveLength(100)
    expect(layout.blobs[0]).toMatchObject({ w: 10, h: 10 })
  })

  it("splits dots across modes with an exact sum and mode order", () => {
    const layout = deriveTokenMap(
      payload([session("a", { looking: 2_500, changing: 2_500, talking: 5_000 })]),
    )
    // 2k tokens/min at 250 = 8 dots: 2 looking, 2 changing, 4 talking.
    const modes = layout.dots.map((dot) => dot.mode)
    expect(modes).toEqual([
      "looking",
      "looking",
      "changing",
      "changing",
      "talking",
      "talking",
      "talking",
      "talking",
    ])
  })

  it("keeps one dim dot for a session that rounds to zero", () => {
    const busy = session("busy", { running: 1_000_000 })
    const quiet = session("quiet", { thinking: 100 })
    const layout = deriveTokenMap(payload([busy, quiet]))
    const quietDots = layout.dots.filter((dot) => dot.blob === 1)
    expect(quietDots).toHaveLength(1)
    expect(quietDots[0]).toMatchObject({ mode: "thinking", dim: true })
  })

  it("orders blobs busiest first and never overlaps them", () => {
    const layout = deriveTokenMap(
      payload([session("small", { talking: 5_000 }), session("big", { looking: 30_000 })]),
    )
    expect(layout.blobs.map((blob) => blob.sessionId)).toEqual(["big", "small"])
    const seen = new Set<string>()
    for (const dot of layout.dots) {
      const key = `${dot.x},${dot.y}`
      expect(seen.has(key)).toBe(false)
      seen.add(key)
      expect(dot.x).toBeLessThan(layout.cells)
      expect(dot.y).toBeLessThan(layout.cells)
    }
  })

  it("marks sub-agent dots small and counts them in the session rate", () => {
    const parent = session(
      "a",
      { delegating: 5_000 },
      {
        subagents: [
          { subagentId: "sub", tokensPerMin: 1_000, modes: { ...zero, changing: 5_000 } },
        ],
      },
    )
    expect(sessionRate(parent)).toBe(2_000)
    const layout = deriveTokenMap(payload([parent]))
    const small = layout.dots.filter((dot) => dot.small)
    expect(small).toHaveLength(4)
    expect(small.every((dot) => dot.mode === "changing")).toBe(true)
    expect(layout.dots.filter((dot) => !dot.small)).toHaveLength(4)
  })

  it("flags the newest turn live only when it is recent", () => {
    const recent = session("a", { looking: 5_000 }, { lastTurnEpoch: 990 })
    const stale = session("b", { looking: 5_000 }, { lastTurnEpoch: 500 })
    const layout = deriveTokenMap(payload([recent, stale], 1_000))
    const live = layout.dots.filter((dot) => dot.live)
    expect(live).toHaveLength(1)
    expect(live[0].blob).toBe(0)

    const none = deriveTokenMap(payload([stale], 1_000))
    expect(none.dots.some((dot) => dot.live)).toBe(false)
  })

  it("honours a minimum dot value", () => {
    const layout = deriveTokenMap(payload([session("a", { looking: 30_000 })]), {
      minDotValue: 1_000,
    })
    expect(layout.dotValue).toBe(1_000)
    expect(layout.dots).toHaveLength(6)
  })

  it("reports overflow when even the coarsest step cannot fit", () => {
    const top = DOT_VALUE_LADDER[DOT_VALUE_LADDER.length - 1]
    const sessions = Array.from({ length: 80 }, (_, index) =>
      session(`s${index}`, { looking: top * 5 * 4 }),
    )
    const layout = deriveTokenMap(payload(sessions), { cells: 6 })
    expect(layout.overflow).toBe(true)
    expect(layout.blobs.length).toBeLessThan(sessions.length)
    expect(layout.blobs.length).toBeGreaterThan(0)
  })
})

import { describe, expect, it } from "vitest"

import type { HudModeTokens, HudTokenMapPayload, HudTokenMapSession } from "./ipc"
import {
  DOT_VALUE_LADDER,
  deriveTokenMap,
  formatRate,
  frameColor,
  mapVisible,
  sessionRate,
} from "./tokenMap"

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
    // The default fixture wrote at `nowEpoch`, so it is live.
    lastTurnEpoch: 1_000,
    // Five-minute window: rate is total / 5.
    tokensPerMin: total / 5,
    modes: full,
    subagents: [],
    ...extra,
  }
}

function payload(sessions: HudTokenMapSession[], nowEpoch = 1_000): HudTokenMapPayload {
  // The spend rate is the LED's concern, not the layout's.
  return { nowEpoch, windowSecs: 300, sessions, spend: null }
}

describe("deriveTokenMap", () => {
  it("returns an empty square for no sessions", () => {
    const layout = deriveTokenMap(payload([]))
    expect(layout.dots).toEqual([])
    expect(layout.blobs).toEqual([])
    expect(layout.overflow).toBe(false)
  })

  it("picks the finest dot value at which everything fits", () => {
    // 6k tokens/min at 250 per dot = 24 dots, fits a 20x20 square.
    const layout = deriveTokenMap(payload([session("a", { looking: 30_000 })]))
    expect(layout.dotValue).toBe(250)
    expect(layout.dots).toHaveLength(24)
    expect(layout.dots.every((dot) => dot.mode === "looking")).toBe(true)
  })

  it("steps the ladder up when a burst would overflow", () => {
    // 200k tokens/min: 800 dots at 250, 400 dots at 500, which fills 400 cells.
    const layout = deriveTokenMap(payload([session("a", { running: 1_000_000 })]))
    expect(layout.dotValue).toBe(500)
    expect(layout.dots).toHaveLength(400)
    expect(layout.blobs[0]).toMatchObject({ w: 20, h: 20 })
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
    expect(layout.blobs.map((blob) => blob.topMode)).toEqual(["running", "thinking"])
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

  it("drops a session that stopped writing and pulses the newest turn", () => {
    const recent = session("a", { looking: 5_000 }, { lastTurnEpoch: 990 })
    const older = session("b", { looking: 5_000 }, { lastTurnEpoch: 920 })
    const stale = session("c", { looking: 5_000 }, { lastTurnEpoch: 500 })
    const unknown = session("d", { looking: 5_000 }, { lastTurnEpoch: null })
    const layout = deriveTokenMap(payload([recent, older, stale, unknown], 1_000))
    expect(layout.blobs.map((blob) => blob.sessionId)).toEqual(["a", "b"])
    const live = layout.dots.filter((dot) => dot.live)
    expect(live).toHaveLength(1)
    expect(live[0].blob).toBe(0)

    const none = deriveTokenMap(payload([stale], 1_000))
    expect(none.dots).toEqual([])
  })

  it("names the top mode of the newest live session and none when quiet", () => {
    const layout = deriveTokenMap(
      payload([
        session("old", { looking: 5_000 }, { lastTurnEpoch: 900 }),
        session("new", { changing: 2_000, talking: 500 }, { lastTurnEpoch: 990 }),
      ]),
    )
    expect(layout.liveMode).toBe("changing")
    expect(deriveTokenMap(null).liveMode).toBeNull()
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

  it("carries each session's mode split and sub-agents on its blob", () => {
    const sub = { subagentId: "sub-1", tokensPerMin: 100, modes: { ...zero, running: 500 } }
    const layout = deriveTokenMap(
      payload([session("a", { looking: 5_000 }, { subagents: [sub] })]),
    )
    expect(layout.blobs[0].modes.looking).toBe(5_000)
    expect(layout.blobs[0].subagents).toEqual([sub])
  })

  it("shows the map at two sessions, hides at one, and waits a poll after hiding", () => {
    // First sight of two sessions: on at once.
    expect(mapVisible(false, false, 2)).toBe(true)
    // One session: off at once, however it was.
    expect(mapVisible(true, true, 1)).toBe(false)
    expect(mapVisible(false, true, 0)).toBe(false)
    // The poll after a hide holds it off, the next one lets it back.
    expect(mapVisible(true, false, 2)).toBe(false)
    expect(mapVisible(false, false, 2)).toBe(true)
    // A shown map stays shown.
    expect(mapVisible(true, true, 3)).toBe(true)
  })

  it("formats rates for labels", () => {
    expect(formatRate(60)).toBe("60")
    expect(formatRate(999.6)).toBe("1000")
    expect(formatRate(1_000)).toBe("1k")
    expect(formatRate(4_250)).toBe("4.3k")
    expect(formatRate(12_400)).toBe("12k")
    expect(formatRate(1_300_000)).toBe("1.3M")
  })

  it("cycles the frame colours", () => {
    expect(frameColor(0)).toBe(frameColor(6))
    expect(frameColor(1)).not.toBe(frameColor(0))
  })
})

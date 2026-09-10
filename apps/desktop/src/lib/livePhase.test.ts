import { describe, expect, it } from "vitest"

import { LIVE_CYCLE_MS, livePhaseDelay, livePhaseStyle } from "./livePhase"

describe("livePhaseDelay", () => {
  it("gives the wall clock's position in the cycle as a negative delay", () => {
    expect(livePhaseDelay(10_000)).toBe("-2000ms")
    expect(livePhaseDelay(10_000 + LIVE_CYCLE_MS)).toBe("-2000ms")
  })

  it("gives one delay to every element at one instant", () => {
    const meter = livePhaseStyle("--led-sweep-delay", 1_234_567)
    const title = livePhaseStyle("--activity-row-shimmer-delay", 1_234_567)
    expect(meter["--led-sweep-delay" as keyof typeof meter]).toBe(
      title["--activity-row-shimmer-delay" as keyof typeof title],
    )
  })

  it("holds the phase for an element that mounts a cycle later", () => {
    const early = 1_000
    const late = early + LIVE_CYCLE_MS + 250
    const phase = (mount: number) =>
      (late - mount - Number.parseInt(livePhaseDelay(mount), 10)) % LIVE_CYCLE_MS
    expect(phase(early)).toBe(phase(late))
  })
})

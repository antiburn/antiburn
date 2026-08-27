import { describe, expect, it } from "vitest"

import { mockSessionHygiene } from "./mockSessionHygiene"

describe("mockSessionHygiene", () => {
  it("returns the six checks in display order", () => {
    expect(mockSessionHygiene("session-1").map((check) => check.id)).toEqual([
      "sessionOverdepth",
      "modelOverthinking",
      "overpoweredSubagents",
      "obsoleteModel",
      "fastModeOveruse",
      "excessCacheRehydration",
    ])
  })

  it("returns the same mock results for the same session", () => {
    expect(mockSessionHygiene("session-1")).toEqual(mockSessionHygiene("session-1"))
  })

  it("gives the prototype a mix of passing and failing sessions", () => {
    // The seed "session-7" fails one check at the current failure rate.
    const results = ["session-1", "session-2", "session-7"].flatMap(mockSessionHygiene)
    expect(results.some((check) => check.passed)).toBe(true)
    expect(results.some((check) => !check.passed)).toBe(true)
  })
})

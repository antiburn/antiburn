import { describe, expect, it } from "vitest"

import type { OverviewProgress } from "./overviewProgressStore"
import { withSnoozes } from "./useOverviewProgress"

describe("withSnoozes", () => {
  const progress = {
    failingCount: 2,
    categories: [
      {
        id: "unusedSkills",
        label: "",
        status: "needsFix",
        estimatedBurnBasisPoints: 300,
        finding: 0,
        clean: 0,
        agents: [],
      },
      {
        id: "unusedMcpServers",
        label: "",
        status: "needsFix",
        estimatedBurnBasisPoints: 10,
        finding: 0,
        clean: 0,
        agents: [],
      },
      {
        id: "modelOverthinking",
        label: "",
        status: "passing",
        estimatedBurnBasisPoints: null,
        finding: 0,
        clean: 0,
        agents: [],
      },
    ],
  } as unknown as OverviewProgress

  it("marks a snoozed check and removes it from the failing count", () => {
    const result = withSnoozes(progress, new Set(["unusedSkills"]))
    expect(result.categories.map((category) => category.status)).toEqual([
      "snoozed",
      "needsFix",
      "passing",
    ])
    expect(result.failingCount).toBe(1)
  })

  it("holds back failing checks until the snoozes are known", () => {
    const result = withSnoozes(progress, new Set(), false)
    expect(result.failingCount).toBe(0)
    expect(result.categories.some((category) => category.status === "needsFix")).toBe(false)
  })

  it("returns the same progress when nothing is snoozed", () => {
    expect(withSnoozes(progress, new Set())).toBe(progress)
  })
})

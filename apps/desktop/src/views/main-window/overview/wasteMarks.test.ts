import { describe, expect, it } from "vitest"

import type {
  BurnCheckTargetListPayload,
  ChecksCategoryPayload,
} from "../../../lib/insightsIpc"
import { pinnedDetectors, wasteMarks } from "./wasteMarks"

function check(id: ChecksCategoryPayload["id"], finding: number, clean: number) {
  return { id, finding, clean, unavailable: 0 } as ChecksCategoryPayload
}

const failures = [
  check("sessionsOverDepth", 4, 6),
  check("unusedMcpServers", 3, 1),
  check("unusedSkills", 1, 3),
  check("cacheChurn", 2, 8),
]

describe("wasteMarks", () => {
  it("pins session checks and keeps config checks for the flag", () => {
    expect(pinnedDetectors(failures)).toEqual(["sessionsOverDepth", "cacheChurn"])
  })

  it("turns samples into pins and config checks into shares, biggest first", () => {
    const targets = {
      sessionsOverDepth: {
        data: {
          targets: [],
          truncated: false,
          samples: [{ navigationHandle: "h1", title: "Fix login", observedAtMs: 5_000 }],
        } as unknown as BurnCheckTargetListPayload,
      },
      cacheChurn: { data: null },
    }
    const marks = wasteMarks(failures, targets)
    expect(marks.pins).toEqual([
      {
        detector: "sessionsOverDepth",
        label: expect.any(String),
        atEpoch: 5,
        title: "Fix login",
        navigationHandle: "h1",
      },
    ])
    expect(marks.config.map((item) => [item.detector, item.share])).toEqual([
      ["unusedMcpServers", 0.75],
      ["unusedSkills", 0.25],
    ])
  })
})

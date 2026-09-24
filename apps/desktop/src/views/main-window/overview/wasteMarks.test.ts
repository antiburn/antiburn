import { describe, expect, it } from "vitest"

import type {
  BurnCheckTargetListPayload,
  ChecksCategoryPayload,
} from "../../../lib/insightsIpc"
import { pinnedDetectors, wasteMarks } from "./wasteMarks"

function check(id: ChecksCategoryPayload["id"], finding: number, clean: number) {
  return {
    id,
    finding,
    clean,
    unavailable: 0,
    estimatedTokenBurnBasisPoints: id === "sessionsOverDepth" ? 1_250 : null,
  } as ChecksCategoryPayload
}

function target(resourceKind: string, currentValue: string, replacementValue: string) {
  return { display: { resourceKind, currentValue, replacementValue } }
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
          targets: [
            target("model", "claude-opus-4-5", "claude-sonnet-4-5"),
            target("session", "long", "short"),
            target("model", "claude-opus-4-5", "claude-sonnet-4-5"),
          ],
          truncated: false,
          samples: [
            {
              navigationHandle: "h1",
              title: "Fix login",
              observedAtMs: 5_000,
              repo: "web",
              agent: "claude",
              models: ["claude-opus-4-5", "claude-sonnet-4-5", "claude-opus-4-5"],
              cost: { totalUsd: 1.5 },
              hygiene: {
                badges: [
                  { id: "sessionOverdepth", status: "finding" },
                  { id: "excessCacheRehydration", status: "finding" },
                  { id: "modelOverthinking", status: "clean" },
                ],
              },
            },
          ],
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
        repo: "web",
        agent: "Claude",
        models: ["opus-4-5", "sonnet-4-5"],
        costUsd: 1.5,
        alsoFailed: ["Excess cache rehydration"],
      },
    ])
    expect(marks.checks?.find((item) => item.detector === "sessionsOverDepth")).toEqual({
      detector: "sessionsOverDepth",
      burnBasisPoints: 1_250,
      change: "opus-4-5 → sonnet-4-5",
    })
    expect(marks.checks?.find((item) => item.detector === "cacheChurn")?.change).toBeNull()
    expect(marks.config.map((item) => [item.detector, item.share])).toEqual([
      ["unusedMcpServers", 0.75],
      ["unusedSkills", 0.25],
    ])
  })
})

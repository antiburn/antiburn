import { describe, expect, it } from "vitest"

import type { SessionHygienePayload } from "../insightsIpc"
import { unusedContextRows, unusedContextTotalUsd } from "./unusedContext"

function hygiene(
  unusedResources: SessionHygienePayload["unusedResources"],
): SessionHygienePayload {
  return { badges: [], evidenceState: "ready", unusedResources }
}

describe("unusedContextRows", () => {
  it("returns an empty list when the payload carries no unused-resources evidence", () => {
    expect(unusedContextRows(hygiene(null))).toEqual([])
  })

  it("flattens all three kinds, ordered by cost descending, unpriced rows last", () => {
    const rows = unusedContextRows(
      hygiene({
        mcpServers: [{ name: "cheap-server", costUsd: 0.01 }],
        builtInTools: [
          { name: "expensive-tool", costUsd: 0.5 },
          { name: "unpriced-tool", costUsd: null },
        ],
        skills: [{ name: "mid-skill", costUsd: 0.2 }],
      }),
    )

    expect(rows.map((row) => row.name)).toEqual([
      "expensive-tool",
      "mid-skill",
      "cheap-server",
      "unpriced-tool",
    ])
    expect(rows[0]).toEqual({ name: "expensive-tool", kind: "Built-in tool", costUsd: 0.5 })
    expect(rows[2]).toEqual({ name: "cheap-server", kind: "MCP server", costUsd: 0.01 })
    expect(rows[3]).toEqual({ name: "unpriced-tool", kind: "Built-in tool", costUsd: null })
  })

  it("breaks a cost tie by name ascending", () => {
    const rows = unusedContextRows(
      hygiene({
        mcpServers: [{ name: "zeta", costUsd: 1 }],
        builtInTools: [{ name: "alpha", costUsd: 1 }],
        skills: [],
      }),
    )

    expect(rows.map((row) => row.name)).toEqual(["alpha", "zeta"])
  })

  it("puts every unpriced row last, ordering them by name", () => {
    const rows = unusedContextRows(
      hygiene({
        mcpServers: [{ name: "zeta", costUsd: null }],
        builtInTools: [{ name: "alpha", costUsd: null }],
        skills: [],
      }),
    )

    expect(rows.map((row) => row.name)).toEqual(["alpha", "zeta"])
  })
})

describe("unusedContextTotalUsd", () => {
  it("sums only the priced rows", () => {
    const rows = unusedContextRows(
      hygiene({
        mcpServers: [{ name: "a", costUsd: 0.1 }],
        builtInTools: [{ name: "b", costUsd: 0.2 }],
        skills: [{ name: "c", costUsd: null }],
      }),
    )

    expect(unusedContextTotalUsd(rows)).toBeCloseTo(0.3)
  })

  it("returns null when every row is unpriced", () => {
    const rows = unusedContextRows(
      hygiene({
        mcpServers: [{ name: "a", costUsd: null }],
        builtInTools: [],
        skills: [],
      }),
    )

    expect(unusedContextTotalUsd(rows)).toBeNull()
  })

  it("returns null for an empty row list", () => {
    expect(unusedContextTotalUsd([])).toBeNull()
  })
})

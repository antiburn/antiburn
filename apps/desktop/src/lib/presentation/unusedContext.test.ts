import { describe, expect, it } from "vitest"

import type { SessionHygienePayload } from "../insightsIpc"
import {
  unusedContextDisplayRows,
  unusedContextRows,
  unusedContextTotalUsd,
  type UnusedContextRow,
} from "./unusedContext"

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

describe("unusedContextDisplayRows", () => {
  it("rolls up two or more small rows of the same kind, across mixed kinds", () => {
    const rows: UnusedContextRow[] = [
      { name: "big-skill", kind: "Skill", costUsd: 1.2 },
      { name: "tiny-tool-a", kind: "Built-in tool", costUsd: 0.001 },
      { name: "tiny-tool-b", kind: "Built-in tool", costUsd: 0.002 },
      { name: "tiny-server-a", kind: "MCP server", costUsd: 0.0005 },
      { name: "tiny-server-b", kind: "MCP server", costUsd: 0.0015 },
    ]

    const display = unusedContextDisplayRows(rows)

    expect(display).toEqual([
      { type: "item", name: "big-skill", kind: "Skill", costUsd: 1.2 },
      {
        type: "rollup",
        kind: "Built-in tool",
        count: 2,
        names: ["tiny-tool-a", "tiny-tool-b"],
        costUsd: 0.003,
      },
      {
        type: "rollup",
        kind: "MCP server",
        count: 2,
        names: ["tiny-server-a", "tiny-server-b"],
        costUsd: 0.002,
      },
    ])
  })

  it("keeps a kind with exactly one small row as an item, not a rollup", () => {
    const rows: UnusedContextRow[] = [{ name: "solo-tiny", kind: "Skill", costUsd: 0.001 }]

    expect(unusedContextDisplayRows(rows)).toEqual([
      { type: "item", name: "solo-tiny", kind: "Skill", costUsd: 0.001 },
    ])
  })

  it("never rolls up unpriced rows, even several of the same kind", () => {
    const rows: UnusedContextRow[] = [
      { name: "unpriced-a", kind: "Skill", costUsd: null },
      { name: "unpriced-b", kind: "Skill", costUsd: null },
      { name: "unpriced-c", kind: "Skill", costUsd: null },
    ]

    expect(unusedContextDisplayRows(rows)).toEqual([
      { type: "item", name: "unpriced-a", kind: "Skill", costUsd: null },
      { type: "item", name: "unpriced-b", kind: "Skill", costUsd: null },
      { type: "item", name: "unpriced-c", kind: "Skill", costUsd: null },
    ])
  })

  it("orders item rows first in their incoming order, then rollups by summed cost descending", () => {
    const rows: UnusedContextRow[] = [
      // Incoming order already reflects unusedContextRows' cost-desc, unpriced-last sort.
      { name: "large", kind: "Skill", costUsd: 2 },
      { name: "unpriced", kind: "Skill", costUsd: null },
      { name: "small-tool-a", kind: "Built-in tool", costUsd: 0.0001 },
      { name: "small-tool-b", kind: "Built-in tool", costUsd: 0.0004 },
      { name: "small-server-a", kind: "MCP server", costUsd: 0.0035 },
      { name: "small-server-b", kind: "MCP server", costUsd: 0.0015 },
    ]

    const display = unusedContextDisplayRows(rows)

    // Items keep their incoming order.
    expect(display.slice(0, 2)).toEqual([
      { type: "item", name: "large", kind: "Skill", costUsd: 2 },
      { type: "item", name: "unpriced", kind: "Skill", costUsd: null },
    ])
    // The MCP server rollup (0.005) outweighs the built-in tool rollup (0.0003).
    expect(display.slice(2)).toEqual([
      {
        type: "rollup",
        kind: "MCP server",
        count: 2,
        names: ["small-server-a", "small-server-b"],
        costUsd: 0.005,
      },
      {
        type: "rollup",
        kind: "Built-in tool",
        count: 2,
        names: ["small-tool-a", "small-tool-b"],
        costUsd: 0.0005,
      },
    ])
  })
})

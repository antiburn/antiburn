import { describe, expect, it } from "vitest"
import { CHECK_DEFINITIONS, isCheckAvailable } from "./checkDefinitions"

describe("deterministic check definitions", () => {
  it("keeps the nine check identities, labels, and relative order", () => {
    expect(
      Object.entries(CHECK_DEFINITIONS)
        .filter(([, definition]) => definition.kind === "local")
        .map(([id, definition]) => [id, definition.label]),
    ).toEqual([
      ["sessionsOverDepth", "Session overdepth"],
      ["modelOverthinking", "Model overthinking"],
      ["overpoweredSubagents", "Overpowered subagents"],
      ["unusedMcpServers", "Unused MCP servers"],
      ["unusedBuiltInTools", "Unused built-in tools"],
      ["unusedSkills", "Unused skills"],
      ["oldModelUsage", "Old model usage"],
      ["overuseOfFastMode", "Fast mode overuse"],
      ["cacheChurn", "Excess cache rehydration"],
    ])
  })

  it("does not gate deterministic checks on a decision connection", () => {
    for (const [id, definition] of Object.entries(CHECK_DEFINITIONS)) {
      if (definition.kind === "local") {
        expect(isCheckAvailable(id as keyof typeof CHECK_DEFINITIONS, false)).toBe(true)
      }
    }
  })
})

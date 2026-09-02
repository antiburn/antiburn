import { cleanup, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { InitialContextBreakdown } from "../../../lib/types/session"
import { SkillsMcpChart } from "./SkillsMcpChart"

afterEach(() => {
  cleanup()
})

function breakdown(sources: InitialContextBreakdown["sources"]): InitialContextBreakdown {
  return { sources }
}

describe("SkillsMcpChart", () => {
  it("lists every skill and MCP row, sorted by tokens descending, with its name, origin, tokens, and status", () => {
    render(
      <SkillsMcpChart
        breakdown={breakdown([
          {
            source: "skill_instructions",
            sourceName: "research",
            tokenCount: 300,
            useCount: 2,
            origin: "project",
          },
          {
            source: "mcp_instructions",
            sourceName: "figma",
            tokenCount: 900,
            useCount: 0,
            origin: "plugin",
          },
        ])}
      />,
    )
    const names = screen.getAllByText(/^(figma|research)$/).map((el) => el.textContent)
    expect(names).toEqual(["figma", "research"])
    expect(screen.getByText("Plugin")).toBeTruthy()
    expect(screen.getByText("Project")).toBeTruthy()
    expect(screen.getByText("900")).toBeTruthy()
    expect(screen.getByText("300")).toBeTruthy()
    expect(screen.getByText("Unused")).toBeTruthy()
    expect(screen.getByText("Used ×2")).toBeTruthy()
  })

  it("labels a builtin_tool row 'Tool', with a Bundled origin and a Deferred status when unused", () => {
    render(
      <SkillsMcpChart
        breakdown={breakdown([
          {
            source: "skill_instructions",
            sourceName: "research",
            tokenCount: 300,
            useCount: 1,
            origin: "project",
          },
          {
            source: "builtin_tool",
            sourceName: "Bash",
            tokenCount: 20,
            useCount: 0,
            origin: "bundled",
            deferred: true,
          },
        ])}
      />,
    )
    const names = screen.getAllByText(/^(research|Bash)$/).map((el) => el.textContent)
    expect(names).toEqual(["research", "Bash"])
    expect(screen.getByText("Tool")).toBeTruthy()
    expect(screen.getByText("Bash")).toBeTruthy()
    expect(screen.getByText("Bundled")).toBeTruthy()
    expect(screen.getByText("Deferred")).toBeTruthy()
  })

  it("omits an unknown origin, or a missing one, from the detail line", () => {
    render(
      <SkillsMcpChart
        breakdown={breakdown([
          {
            source: "skill_instructions",
            sourceName: "research",
            tokenCount: 300,
            useCount: 1,
            origin: "unknown",
          },
          {
            source: "mcp_instructions",
            sourceName: "figma",
            tokenCount: 100,
            useCount: 0,
          },
        ])}
      />,
    )
    expect(screen.queryByText("—")).toBeNull()
    expect(screen.getAllByText(/^(Skill|MCP)$/)).toHaveLength(2)
  })

  it("shows a bare 'Used' when the session used a source exactly once", () => {
    render(
      <SkillsMcpChart
        breakdown={breakdown([
          {
            source: "mcp_instructions",
            sourceName: "figma",
            tokenCount: 100,
            useCount: 1,
          },
        ])}
      />,
    )
    expect(screen.getByText("Used")).toBeTruthy()
  })

  it("shows a footnote when no skill, MCP, or tool source loaded", () => {
    render(<SkillsMcpChart breakdown={breakdown([])} />)
    expect(screen.getByText("No skills, MCPs or tools loaded.")).toBeTruthy()
    expect(screen.queryByText("Used")).toBeNull()
  })

  function manyRows(count: number): InitialContextBreakdown["sources"] {
    return Array.from({ length: count }, (_, i) => ({
      source: "skill_instructions" as const,
      sourceName: `skill-${i}`,
      tokenCount: count - i,
      useCount: 0,
    }))
  }

  it("renders every row with no disclosure, however long the list is", () => {
    const total = 63
    render(<SkillsMcpChart breakdown={breakdown(manyRows(total))} />)
    expect(screen.getAllByText(/^(Skill|MCP|Tool)$/)).toHaveLength(total)
    expect(screen.queryByText(/^Show/)).toBeNull()
  })
})

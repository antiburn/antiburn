// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import type { InitialContextBreakdown } from "../../../lib/types/session"
import { SKILLS_MCP_COLLAPSED_ROWS, SkillsMcpChart } from "./SkillsMcpChart"
import { skillsMcpExpandedStore } from "./useSkillsMcpExpanded"

afterEach(() => {
  cleanup()
  skillsMcpExpandedStore.set(false)
})

function breakdown(sources: InitialContextBreakdown["sources"]): InitialContextBreakdown {
  return { trackingStatus: "trackedPartial", totalTokens: null, sources }
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
    const rows = screen.getAllByText(/^(Skill|MCP)$/)
    expect(rows.map((el) => el.nextSibling?.textContent)).toEqual(["figma", "research"])
    expect(screen.getByText("Plugin")).toBeTruthy()
    expect(screen.getByText("Project")).toBeTruthy()
    expect(screen.getByText("900")).toBeTruthy()
    expect(screen.getByText("300")).toBeTruthy()
    expect(screen.getByText("Unused")).toBeTruthy()
    expect(screen.getByText("Used ×2")).toBeTruthy()
  })

  it("renders an unknown origin, or a missing one, as an em dash", () => {
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
    expect(screen.getAllByText("—")).toHaveLength(2)
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

  it("shows a footnote when no skill or MCP source loaded", () => {
    render(
      <SkillsMcpChart
        breakdown={breakdown([
          { source: "system_instructions", sourceName: null, tokenCount: 900 },
        ])}
      />,
    )
    expect(screen.getByText("No skills or MCPs loaded.")).toBeTruthy()
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

  it("collapses to the first rows with a 'Show N more' toggle when there are more rows", () => {
    const total = SKILLS_MCP_COLLAPSED_ROWS + 17
    render(<SkillsMcpChart breakdown={breakdown(manyRows(total))} />)
    expect(screen.getAllByText(/^(Skill|MCP)$/)).toHaveLength(SKILLS_MCP_COLLAPSED_ROWS)
    expect(screen.getByText("Show 17 more")).toBeTruthy()
    expect(screen.queryByText("Show less")).toBeNull()
  })

  it("expands to every row and shows 'Show less' after clicking the toggle", () => {
    const total = SKILLS_MCP_COLLAPSED_ROWS + 17
    render(<SkillsMcpChart breakdown={breakdown(manyRows(total))} />)
    fireEvent.click(screen.getByText("Show 17 more"))
    expect(screen.getAllByText(/^(Skill|MCP)$/)).toHaveLength(total)
    expect(screen.getByText("Show less")).toBeTruthy()
    expect(screen.queryByText(/^Show \d+ more$/)).toBeNull()
  })

  it("renders no toggle when there are 5 or fewer rows", () => {
    render(<SkillsMcpChart breakdown={breakdown(manyRows(SKILLS_MCP_COLLAPSED_ROWS))} />)
    expect(screen.getAllByText(/^(Skill|MCP)$/)).toHaveLength(SKILLS_MCP_COLLAPSED_ROWS)
    expect(screen.queryByText(/^Show/)).toBeNull()
  })

  it("keeps the expanded state after the chart unmounts, for a chart mounted later", () => {
    const total = SKILLS_MCP_COLLAPSED_ROWS + 17
    const { unmount } = render(<SkillsMcpChart breakdown={breakdown(manyRows(total))} />)
    fireEvent.click(screen.getByText("Show 17 more"))
    expect(screen.getByText("Show less")).toBeTruthy()
    unmount()

    render(<SkillsMcpChart breakdown={breakdown(manyRows(total))} />)
    expect(screen.getAllByText(/^(Skill|MCP)$/)).toHaveLength(total)
    expect(screen.getByText("Show less")).toBeTruthy()
  })
})

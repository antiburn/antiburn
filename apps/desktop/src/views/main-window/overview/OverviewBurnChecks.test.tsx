import { fireEvent, render, screen, within } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import type { ChecksCategoryPayload, ChecksReportPayload } from "../../../lib/insightsIpc"
import { OverviewBurnChecks } from "./OverviewBurnChecks"

function category(
  id: ChecksCategoryPayload["id"],
  finding: number,
  clean: number,
  basisPoints: number | null = null,
): ChecksCategoryPayload {
  return { id, finding, clean, unavailable: 0, estimatedTokenBurnBasisPoints: basisPoints }
}

function report(
  categories: ChecksCategoryPayload[],
  overrides: Partial<ChecksReportPayload> = {},
): ChecksReportPayload {
  return {
    evidenceSettled: true,
    pendingEvidence: 0,
    estimatedTokenBurnBasisPoints: null,
    categories,
    ...overrides,
  }
}

describe("OverviewBurnChecks", () => {
  it("lists the two worst findings and opens the full section from every control", () => {
    const onOpen = vi.fn()
    render(
      <OverviewBurnChecks
        report={report(
          [
            category("unusedMcpServers", 12, 3, 250),
            category("cacheChurn", 4, 8, 900),
            category("oldModelUsage", 1, 10, null),
            category("unusedSkills", 0, 20),
          ],
          { estimatedTokenBurnBasisPoints: 40 },
        )}
        onOpen={onOpen}
      />,
    )
    const panel = screen.getByRole("region", { name: "Burn checks" })
    expect(within(panel).getByText("3 findings · 1 passed")).toBeVisible()
    expect(within(panel).getByText("Less than 1% estimated burn")).toBeVisible()
    // The gauge shows the burn share, floored to a trace, not the check count.
    const burnArc = panel.querySelector('[data-segment-id="burn"]')
    const restArc = panel.querySelector('[data-segment-id="rest"]')
    expect(burnArc).not.toBeNull()
    expect(Number(burnArc!.getAttribute("data-arc-angle"))).toBeLessThan(
      Number(restArc!.getAttribute("data-arc-angle")) / 50,
    )
    const rows = within(panel).getAllByRole("listitem")
    expect(rows).toHaveLength(2)
    expect(rows[0]).toHaveTextContent("Excess cache rehydration4 sessions")
    expect(rows[1]).toHaveTextContent("Unused MCP servers12 sessions")
    fireEvent.click(within(panel).getByRole("button", { name: /^Open Burn checks/ }))
    fireEvent.click(within(rows[0]!).getByRole("button"))
    expect(onOpen).toHaveBeenCalledTimes(2)
  })

  it("shows one positive line when every check passed", () => {
    render(
      <OverviewBurnChecks
        report={report([category("unusedSkills", 0, 20), category("cacheChurn", 0, 18)])}
        onOpen={vi.fn()}
      />,
    )
    const panel = screen.getByRole("region", { name: "Burn checks" })
    expect(within(panel).getByText("All 2 checks passed")).toBeVisible()
    expect(within(panel).getByText("Nothing to review right now.")).toBeVisible()
    expect(within(panel).queryByRole("list")).toBeNull()
  })

  it("never shows an unsettled report as zero findings", () => {
    render(
      <OverviewBurnChecks
        report={report([category("unusedSkills", 0, 0)], {
          evidenceSettled: false,
          pendingEvidence: 7,
        })}
        onOpen={vi.fn()}
      />,
    )
    const panel = screen.getByRole("region", { name: "Burn checks" })
    expect(within(panel).getByText("Assessing sessions")).toBeVisible()
    expect(within(panel).getByText("Results appear when the scan finishes.")).toBeVisible()
    expect(within(panel).queryByText(/0 findings|passed/)).toBeNull()
  })

  it("marks the panel busy and shows no result while the report loads", () => {
    render(<OverviewBurnChecks report={null} loading onOpen={vi.fn()} />)
    const panel = screen.getByRole("region", { name: "Burn checks" })
    expect(panel).toHaveAttribute("aria-busy", "true")
    expect(within(panel).queryByText(/finding|passed|Assessing/)).toBeNull()
  })
})

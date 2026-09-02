import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it } from "vitest"

import { efficiencyMetrics } from "../../../lib/presentation/sessionEfficiency"
import type { SessionEfficiency } from "../../../lib/types/session"
import { EfficiencyBreakdown } from "./EfficiencyBreakdown"

afterEach(cleanup)

function totals(over: Partial<SessionEfficiency> = {}): SessionEfficiency {
  return {
    totalUsd: 10,
    newWorkUsd: 3.4,
    carryUsd: 5.4,
    rewriteUsd: 1.2,
    growthTokens: 200_000,
    outputTokens: 50_000,
    pricedTurns: 12,
    unpricedTurns: 0,
    ...over,
  }
}

describe("EfficiencyBreakdown", () => {
  it("renders the headline and three spend rows with their values", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)
    expect(screen.getByText("$/MTok")).toBeTruthy()
    expect(screen.getByText("Real Work %")).toBeTruthy()
    expect(screen.getByText("Rewrite Waste %")).toBeTruthy()
    expect(screen.getByText("Carry %")).toBeTruthy()
    expect(screen.getByText("$40.00")).toBeTruthy()
    expect(screen.getByText("34%")).toBeTruthy()
    expect(screen.getByText("12%")).toBeTruthy()
    expect(screen.getByText("54%")).toBeTruthy()
    expect(screen.getAllByText("ok")).toHaveLength(4)
  })

  it("keeps the VU meter for the cost reading, which is not part of the composition", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)

    // The cost track splits into thirds — one zone per band — on the shared
    // meter palette: orange, then yellow, then red. The reading sits in the
    // middle third, so the fill has crossed into the yellow.
    const cost = screen.getByTestId("thermometer-costPerMTok")
    expect(cost.dataset.position).toBe("0.383")
    expect(cost.querySelector('[data-testid="segmented-meter-notch"]')).toBeTruthy()
    expect(cost.querySelectorAll(".bg-brand-tint").length).toBeGreaterThan(0)
    expect(cost.querySelectorAll(".bg-system-yellow-tint").length).toBeGreaterThan(0)
    expect(cost.querySelectorAll('[class~="bg-system-red-unlit/12"]').length).toBeGreaterThan(0)
  })

  it("draws the three shares as one composition track whose runs fill the width", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)

    const track = screen.getByTestId("efficiency-composition")
    const runs = Array.from(track.querySelectorAll<HTMLElement>("span"))
    expect(runs).toHaveLength(3)

    // The runs are the shares, in row order, and they account for the whole.
    const widths = runs.map((run) => Number(run.dataset.width))
    expect(widths).toEqual([0.34, 0.12, 0.54])
    expect(widths.reduce((sum, width) => sum + width, 0)).toBeCloseTo(1, 5)

    // Each slice keeps its own color so it stays recognisable between
    // sessions. The band word, not the run, carries the judgment.
    expect(runs[0]!.className).toContain("bg-share-work")
    expect(runs[1]!.className).toContain("bg-share-waste")
    expect(runs[2]!.className).toContain("bg-share-carry")

    // No share draws a meter of its own any more.
    expect(screen.queryByTestId("share-segment-realWorkShare")).toBeNull()
  })

  it("colours a good reading green and names a bad one by direction", () => {
    render(
      <EfficiencyBreakdown
        metrics={efficiencyMetrics(
          totals({ totalUsd: 25, newWorkUsd: 4, carryUsd: 14, rewriteUsd: 7 }),
          "claude-code",
        )}
      />,
    )
    const high = screen.getAllByText("high")
    expect(high).toHaveLength(2)
    expect(high[0]?.getAttribute("class")).toContain("text-system-red-text")
    expect(screen.getByText("low")).toBeTruthy()
  })

  it("marks a good reading in green", () => {
    render(
      <EfficiencyBreakdown
        metrics={efficiencyMetrics(
          totals({ totalUsd: 5, newWorkUsd: 4, carryUsd: 0.8, rewriteUsd: 0.2 }),
          "codex",
        )}
      />,
    )
    for (const good of screen.getAllByText("good")) {
      expect(good.getAttribute("class")).toContain("text-system-green")
    }
  })

  it("renders nothing when there is no spend", () => {
    const { container } = render(
      <EfficiencyBreakdown
        metrics={efficiencyMetrics(
          totals({ totalUsd: 0, newWorkUsd: 0, rewriteUsd: 0 }),
          "claude-code",
        )}
      />,
    )
    expect(container).toBeEmptyDOMElement()
  })

  it("explains a row in a tooltip, and paints nothing at rest", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "codex")} />)

    // At rest no guidance is in the document, so the block stays the height
    // of its readings.
    expect(screen.queryByText(/fresh input and output/)).toBeNull()

    const realWorkRow = screen.getByTestId("share-row-realWorkShare")
    fireEvent.focus(realWorkRow)
    expect(screen.getAllByText(/fresh input and output/).length).toBeGreaterThan(0)
    expect(
      screen.getAllByText("For Codex, aim for above 33%. Below 17% is too low.").length,
    ).toBeGreaterThan(0)

    fireEvent.blur(realWorkRow)
    expect(screen.queryByText(/fresh input and output/)).toBeNull()
  })

  it("gives the cost reading its own tooltip", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)

    const costRow = screen.getByTestId("cost-row")
    fireEvent.focus(costRow)
    expect(screen.getAllByText(/average cost for each million tokens/).length).toBeGreaterThan(
      0,
    )
  })
})

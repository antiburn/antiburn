import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
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
    // The scale names its middle band once; each share row names its own.
    expect(screen.getAllByText("ok")).toHaveLength(4)
  })

  it("draws the cost reading as a bullet graph that labels its own scale", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)

    // The bands sit at fixed thirds, so the scale never changes length. The
    // measure runs to this session's reading and the target marks the edge
    // of the good band.
    const cost = screen.getByTestId("thermometer-costPerMTok")
    expect(cost.dataset.position).toBe("0.383")
    expect(cost.querySelector('[data-testid="cost-band-good"]')).toBeTruthy()
    expect(cost.querySelector('[data-testid="cost-band-ok"]')).toBeTruthy()
    expect(cost.querySelector('[data-testid="cost-band-bad"]')).toBeTruthy()

    const measure = cost.querySelector<HTMLElement>('[data-testid="cost-measure"]')
    expect(Number.parseFloat(measure!.style.width)).toBeCloseTo(38.3, 1)
    const target = cost.querySelector<HTMLElement>('[data-testid="cost-target"]')
    expect(Number.parseFloat(target!.style.left)).toBeCloseTo(33.3, 1)

    // Each band names itself and its dollar range, and the current band is
    // the one in the label ink. The reading carries no tag of its own.
    const good = within(cost).getByTestId("cost-band-word-good")
    const ok = within(cost).getByTestId("cost-band-word-ok")
    expect(good).toHaveTextContent("under $33")
    expect(ok).toHaveTextContent("$33 – $80")
    expect(within(cost).getByTestId("cost-band-word-bad")).toHaveTextContent("over $80")
    expect(ok.dataset.current).toBe("true")
    expect(good.dataset.current).toBeUndefined()
    expect(screen.getByTestId("cost-row").querySelector(".rounded")).toBeNull()
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
    // sessions: label ink for real work, brand orange for waste, neutral
    // for carry. No slice takes a verdict colour.
    expect(runs[0]!.className).toContain("bg-label")
    expect(runs[1]!.className).toContain("bg-brand-tint")
    expect(runs[2]!.className).toContain("bg-share-carry")

    // No share draws a meter of its own any more.
    expect(screen.queryByTestId("share-segment-realWorkShare")).toBeNull()
  })

  it("names a bad reading by direction, in the same quiet ink as any other", () => {
    render(
      <EfficiencyBreakdown
        metrics={efficiencyMetrics(
          totals({ totalUsd: 25, newWorkUsd: 4, carryUsd: 14, rewriteUsd: 7 }),
          "claude-code",
        )}
      />,
    )
    const rewrite = within(screen.getByTestId("share-row-rewriteShare")).getByText("high")
    expect(rewrite.getAttribute("class")).toContain("text-label-tertiary")
    expect(rewrite.getAttribute("class")).not.toContain("share-waste")
    // The cost scale names its bad band by the same direction word.
    expect(screen.getByTestId("cost-band-word-bad")).toHaveTextContent("high")
    expect(screen.getByTestId("cost-band-word-bad").dataset.current).toBe("true")
    expect(within(screen.getByTestId("share-row-realWorkShare")).getByText("low")).toBeTruthy()
  })

  it("keeps a good reading in the same quiet ink", () => {
    render(
      <EfficiencyBreakdown
        metrics={efficiencyMetrics(
          totals({ totalUsd: 5, newWorkUsd: 4, carryUsd: 0.8, rewriteUsd: 0.2 }),
          "codex",
        )}
      />,
    )
    const good = within(screen.getByTestId("share-row-realWorkShare")).getByText("good")
    expect(good.getAttribute("class")).toContain("text-label-tertiary")
    expect(good.getAttribute("class")).not.toContain("share-work")
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

  it("prints the cost reading's guidance inline under its scale", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)

    const guidance = within(screen.getByTestId("cost-guidance"))
    expect(guidance.getByText(/average cost for each million tokens/)).toHaveClass(
      "text-label-secondary",
    )
    expect(
      guidance.getByText("For Claude, aim for below $33. Above $80 is too high."),
    ).toHaveClass("text-label-tertiary")
    expect(guidance.getByText(/Context tab shows/)).toBeInTheDocument()
    // The cost row is plain text now, not a tooltip trigger.
    expect(screen.getByTestId("cost-row")).not.toHaveAttribute("tabindex")
  })
})

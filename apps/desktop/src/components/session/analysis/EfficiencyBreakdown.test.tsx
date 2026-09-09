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
  it.each([
    { totalUsd: 5, figure: "$20.00", band: "good", ink: "text-label" },
    { totalUsd: 10, figure: "$40.00", band: "ok", ink: "text-label" },
    { totalUsd: 25, figure: "$100.00", band: "bad", ink: "text-brand" },
  ])("uses $ink for a $band efficiency hero", ({ totalUsd, figure, band, ink }) => {
    const metrics = efficiencyMetrics(totals({ totalUsd }), "claude-code")
    expect(metrics.costPerMTok?.band).toBe(band)
    render(<EfficiencyBreakdown metrics={metrics} section="cost" layout="wide" />)
    expect(screen.getByText(figure)).toHaveClass(ink)
    expect(screen.getByText(figure)).not.toHaveClass(
      ink === "text-label" ? "text-brand" : "text-label",
    )
  })

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

    // The bands sit at fixed thirds, so the scale never changes length, and
    // the measure runs to this session's reading.
    const cost = screen.getByTestId("thermometer-costPerMTok")
    expect(cost.dataset.position).toBe("0.383")
    expect(cost.querySelector('[data-testid="cost-band-good"]')).toBeTruthy()
    expect(cost.querySelector('[data-testid="cost-band-ok"]')).toBeTruthy()
    expect(cost.querySelector('[data-testid="cost-band-bad"]')).toBeTruthy()

    const measure = cost.querySelector<HTMLElement>('[data-testid="cost-measure"]')
    expect(Number.parseFloat(measure!.style.width)).toBeCloseTo(38.3, 1)
    // The band steps and the ranges mark every edge, so no target line draws.
    expect(cost.querySelector('[data-testid="cost-target"]')).toBeNull()

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

  it("drops the middle range in the wide pane, where the picker floats", () => {
    render(
      <EfficiencyBreakdown
        metrics={efficiencyMetrics(totals(), "claude-code")}
        layout="wide"
      />,
    )

    const cost = screen.getByTestId("thermometer-costPerMTok")
    const ok = within(cost).getByTestId("cost-band-word-ok")
    // The middle band keeps its word. Its range would print under the
    // floating section picker, and the outer ranges name both of its edges.
    expect(ok).toHaveTextContent("ok")
    expect(ok).not.toHaveTextContent("$33 – $80")
    expect(within(cost).getByTestId("cost-band-word-good")).toHaveTextContent("under $33")
    expect(within(cost).getByTestId("cost-band-word-bad")).toHaveTextContent("over $80")
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

  it("prints the cost reading's guidance inline under its scale as one paragraph", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)

    const guidance = screen.getByTestId("cost-guidance")
    // One quiet paragraph, so the sentences wrap at the pane's width instead
    // of each taking a line.
    expect(guidance.querySelectorAll("p")).toHaveLength(1)
    const paragraph = within(guidance).getByText(/average cost for each million tokens/)
    expect(paragraph).toHaveClass("text-label-tertiary")
    expect(paragraph).not.toHaveClass("font-medium")
    expect(paragraph).toHaveTextContent(
      "For Claude, aim for below $33. Above $80 is too high. Craft tight workflows",
    )
    expect(paragraph).toHaveTextContent(/Context tab shows/)
    expect(guidance).not.toHaveClass("max-w-prose")
    // The cost row is plain text now, not a tooltip trigger.
    expect(screen.getByTestId("cost-row")).not.toHaveAttribute("tabindex")
  })

  it("draws the wide bar above stacked legend rows", () => {
    render(
      <EfficiencyBreakdown
        metrics={efficiencyMetrics(totals(), "claude-code")}
        layout="wide"
      />,
    )

    const track = screen.getByTestId("efficiency-composition")
    expect(track.dataset.height).toBe("bar")
    expect(track).toHaveClass("h-6", "rounded-control")

    const legend = screen.getByTestId("composition-legend")
    expect(legend).toHaveClass("flex", "flex-col")
    // Each cell keeps its share, its name, its band word, and its tooltip.
    const realWork = screen.getByTestId("share-row-realWorkShare")
    expect(realWork).toHaveTextContent("34%")
    expect(realWork).toHaveTextContent("Real Work %")
    expect(realWork).toHaveAttribute("tabindex", "0")
    fireEvent.focus(realWork)
    expect(screen.getAllByText(/fresh input and output/).length).toBeGreaterThan(0)

    // The wide pane keeps the cost guidance in a tooltip on the hero figure,
    // so at rest the scale and its labels are the whole reading.
    expect(screen.queryByTestId("cost-guidance")).toBeNull()
    expect(screen.queryByText(/average cost for each million tokens/)).toBeNull()
    const hero = screen.getByTestId("cost-hero")
    expect(hero).toHaveTextContent("per million tokens")
    expect(hero).toHaveAttribute("tabindex", "0")
    fireEvent.focus(hero)
    expect(screen.getAllByText(/average cost for each million tokens/).length).toBeGreaterThan(
      0,
    )
    expect(
      screen.getAllByText("For Claude, aim for below $33. Above $80 is too high.").length,
    ).toBeGreaterThan(0)
    fireEvent.blur(hero)
    expect(screen.queryByText(/average cost for each million tokens/)).toBeNull()
  })

  it("keeps the popover's hairline track and stacked rows by default", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)
    const track = screen.getByTestId("efficiency-composition")
    expect(track.dataset.height).toBe("hairline")
    expect(track).toHaveClass("h-1", "rounded-full")
    expect(screen.getByTestId("composition-legend")).toHaveClass("flex-col")
  })
})

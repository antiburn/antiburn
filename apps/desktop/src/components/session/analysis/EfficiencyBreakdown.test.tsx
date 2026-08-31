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

  it("shows the cost bands and aligns the three spend components below them", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)
    const cost = screen.getByTestId("thermometer-costPerMTok")
    expect(cost.dataset.position).toBe("0.383")
    expect(cost.children).toHaveLength(4)
    expect(cost.children[0]?.getAttribute("class")).toContain("bg-system-green")
    expect(cost.children[1]?.getAttribute("class")).toContain("bg-separator")
    expect(cost.children[2]?.getAttribute("class")).toContain("bg-system-orange")

    const realWork = screen.getByTestId("share-segment-realWorkShare")
    expect(realWork.dataset).toMatchObject({
      start: "0.000",
      width: "0.340",
    })
    expect(realWork.children[0]?.getAttribute("class")).toContain("bg-system-blue")
    const rewrite = screen.getByTestId("share-segment-rewriteShare")
    expect(rewrite.dataset).toMatchObject({
      start: "0.340",
      width: "0.120",
    })
    expect(rewrite.children[1]?.getAttribute("class")).toContain("bg-system-indigo")
    const carry = screen.getByTestId("share-segment-carryShare")
    expect(carry.dataset).toMatchObject({
      start: "0.460",
      width: "0.540",
    })
    expect(carry.children[1]?.getAttribute("class")).toContain("bg-system-gold")
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
    expect(high[0]?.getAttribute("class")).toContain("text-system-orange")
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

  it("opens one metric explanation at a time", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "codex")} />)
    const cost = screen.getByRole("button", { name: "$/MTok details" })
    const realWork = screen.getByRole("button", { name: "Real Work % details" })
    const rewrite = screen.getByRole("button", { name: "Rewrite Waste % details" })
    const carry = screen.getByRole("button", { name: "Carry % details" })

    for (const row of [cost, realWork, rewrite, carry]) {
      expect(row.classList).toContain("cursor-pointer!")
      expect(row.getAttribute("aria-expanded")).toBe("false")
    }

    fireEvent.click(cost)
    expect(cost.getAttribute("aria-expanded")).toBe("true")
    expect(screen.getByText(/avg cost for each million tokens/)).toBeTruthy()

    fireEvent.click(realWork)
    expect(cost.getAttribute("aria-expanded")).toBe("false")
    expect(realWork.getAttribute("aria-expanded")).toBe("true")
    expect(screen.queryByText(/avg cost for each million tokens/)).toBeNull()
    expect(screen.getByText(/fresh input and output/)).toBeTruthy()
    expect(screen.getByText("For Codex, aim for above 33%. Below 17% is too low.")).toBeTruthy()

    fireEvent.click(realWork)
    expect(realWork.getAttribute("aria-expanded")).toBe("false")
  })
})

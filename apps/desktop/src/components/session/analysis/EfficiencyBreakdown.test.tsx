// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, render, screen } from "@testing-library/react"
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
  it("renders the three rows with their values and band words", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)
    expect(screen.getByText("$/MTok")).toBeTruthy()
    expect(screen.getByText("New Work %")).toBeTruthy()
    expect(screen.getByText("Rewrite %")).toBeTruthy()
    expect(screen.getByText("$40.00")).toBeTruthy()
    expect(screen.getByText("34%")).toBeTruthy()
    expect(screen.getByText("12%")).toBeTruthy()
    expect(screen.getAllByText("ok")).toHaveLength(3)
  })

  it("draws a thermometer per row with the mark where the session sits", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "claude-code")} />)
    // $40 per MTok sits inside the ok band, which runs from $33 to $80.
    const cost = screen.getByTestId("thermometer-costPerMTok")
    expect(cost.getAttribute("data-position")).toBe("0.383")
    expect(cost.children).toHaveLength(4)
    expect(cost.children[0]?.getAttribute("class")).toContain("bg-system-green")
    expect(cost.children[2]?.getAttribute("class")).toContain("bg-system-orange")
    // New work reads higher-is-better, so its good segment sits on the right.
    const newWork = screen.getByTestId("thermometer-newWorkShare")
    expect(newWork.children[0]?.getAttribute("class")).toContain("bg-system-orange")
    expect(newWork.children[2]?.getAttribute("class")).toContain("bg-system-green")
  })

  it("colours a good reading green and names a bad one by direction", () => {
    render(
      <EfficiencyBreakdown
        metrics={efficiencyMetrics(
          totals({ totalUsd: 25, newWorkUsd: 4, rewriteUsd: 7 }),
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
          totals({ totalUsd: 5, newWorkUsd: 4, rewriteUsd: 0.2 }),
          "codex",
        )}
      />,
    )
    for (const good of screen.getAllByText("good")) {
      expect(good.getAttribute("class")).toContain("text-system-green")
    }
  })

  it("shows a dash and no band when there is no spend", () => {
    render(
      <EfficiencyBreakdown
        metrics={efficiencyMetrics(
          totals({ totalUsd: 0, newWorkUsd: 0, rewriteUsd: 0 }),
          "claude-code",
        )}
      />,
    )
    expect(screen.getAllByText("—")).toHaveLength(3)
    expect(screen.queryByText("ok")).toBeNull()
    expect(screen.queryByTestId("thermometer-costPerMTok")).toBeNull()
  })

  it("gives each row its own info mark", () => {
    render(<EfficiencyBreakdown metrics={efficiencyMetrics(totals(), "codex")} />)
    expect(screen.getByLabelText("About $/MTok")).toBeTruthy()
    expect(screen.getByLabelText("About New Work %")).toBeTruthy()
    expect(screen.getByLabelText("About Rewrite %")).toBeTruthy()
  })
})

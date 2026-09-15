import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import type { ProviderUsageWindowPayload } from "../../../lib/providerUsageIpc"
import { OverviewSpendTotals } from "./OverviewSpendTotals"

function window(
  overrides: Partial<ProviderUsageWindowPayload> = {},
): ProviderUsageWindowPayload {
  return {
    tokensIn: 1_200_000,
    tokensOut: 300_000,
    cacheRead: 0,
    estimatedUsd: 42.8,
    costComplete: true,
    sessionCount: 4,
    ...overrides,
  }
}

describe("OverviewSpendTotals", () => {
  it("shows a spend figure with its token count and sessions for each span", () => {
    render(
      <OverviewSpendTotals
        totals={{
          today: window(),
          week: window({ estimatedUsd: 218.4, sessionCount: 1 }),
          monthToDate: window(),
          last30Days: window({ estimatedUsd: 864.2, costComplete: false }),
        }}
      />,
    )
    const today = screen.getByText("Today", { selector: "[aria-hidden]" }).closest("div")!
    expect(today).toHaveTextContent("$42.80")
    expect(today).toHaveTextContent("1.50M · 4 sessions")
    expect(today).not.toHaveTextContent("partial")
    const week = screen.getByText("7 days", { selector: "[aria-hidden]" }).closest("div")!
    expect(week).toHaveTextContent("$218")
    expect(week).toHaveTextContent("1 session")
    const month = screen.getByText("30 days", { selector: "[aria-hidden]" }).closest("div")!
    expect(month).toHaveTextContent("$864")
    expect(month).toHaveTextContent("partial")
  })

  it("leads with the token count when a span has no priced model", () => {
    render(
      <OverviewSpendTotals
        totals={{
          today: window({ estimatedUsd: null, costComplete: false }),
          week: window(),
          monthToDate: window(),
          last30Days: window(),
        }}
      />,
    )
    const today = screen.getByText("Today", { selector: "[aria-hidden]" }).closest("div")!
    expect(today).toHaveTextContent("1.50M")
    expect(today).not.toHaveTextContent("$")
    expect(today).toHaveTextContent("4 sessions · partial")
  })

  it("holds placeholders while loading", () => {
    render(<OverviewSpendTotals totals={null} loading />)
    expect(screen.getByRole("region", { name: "Estimated local spend" })).toHaveAttribute(
      "aria-busy",
      "true",
    )
    expect(screen.queryByText("$")).toBeNull()
  })
})

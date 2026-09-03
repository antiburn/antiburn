import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import type { ProviderUsageWindowsPayload } from "../../lib/ipc"
import { UsageSpendSummary } from "./UsageSpendSummary"

function totals(window: ProviderUsageWindowsPayload["today"]): ProviderUsageWindowsPayload {
  return { today: window, week: window, monthToDate: window, last30Days: window }
}

describe("UsageSpendSummary", () => {
  it("leads with the cost and marks one that some provider usage left unpriced", () => {
    render(
      <UsageSpendSummary
        totals={totals({
          tokensIn: 1_000,
          tokensOut: 200,
          cacheRead: 50,
          estimatedUsd: 1.25,
          costComplete: false,
          sessionCount: 2,
        })}
      />,
    )

    expect(screen.getAllByText("$1.25")).toHaveLength(3)
    expect(screen.getByText("1.25k tokens · est.")).toBeInTheDocument()
    expect(screen.getAllByText("1.25k")).toHaveLength(2)
    expect(screen.getByText("Today")).toBeInTheDocument()
    expect(screen.getByText("Last 7 days")).toBeInTheDocument()
    expect(screen.getByText("Last 30 days")).toBeInTheDocument()
    expect(screen.queryByRole("heading")).not.toBeInTheDocument()
  })

  it("shows the token count as the figure when nothing could be priced", () => {
    render(
      <UsageSpendSummary
        totals={totals({
          tokensIn: 1_000,
          tokensOut: 200,
          cacheRead: 50,
          estimatedUsd: null,
          costComplete: false,
          sessionCount: 2,
        })}
      />,
    )

    expect(screen.getAllByText("1.25k")).toHaveLength(3)
    expect(screen.getAllByText("tokens")).toHaveLength(3)
  })
})

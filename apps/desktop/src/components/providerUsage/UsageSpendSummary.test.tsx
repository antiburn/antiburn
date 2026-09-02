import { render, screen } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import type { ProviderUsageWindowsPayload } from "../../lib/ipc"
import { UsageSpendSummary } from "./UsageSpendSummary"

describe("UsageSpendSummary", () => {
  it("shows the known cost when some provider usage is unpriced", () => {
    const window = {
      tokensIn: 1_000,
      tokensOut: 200,
      cacheRead: 50,
      estimatedUsd: 1.25,
      costComplete: false,
      sessionCount: 2,
    }
    const totals: ProviderUsageWindowsPayload = {
      today: window,
      week: window,
      monthToDate: window,
      last30Days: window,
    }

    render(<UsageSpendSummary totals={totals} />)

    expect(screen.getAllByText("$1.25 · 1.3k")).toHaveLength(3)
  })
})

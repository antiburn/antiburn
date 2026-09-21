import { fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type {
  AllowanceUsageAccountPayload,
  AllowanceUsageSummaryPayload,
  ProviderUsageWindowsPayload,
} from "../../../lib/providerUsageIpc"
import { OverviewUsage } from "./OverviewUsage"

const TOTALS: ProviderUsageWindowsPayload = {
  today: window_(1.5),
  week: window_(12.25),
  last30Days: window_(48),
  monthToDate: window_(30),
}

function window_(estimatedUsd: number) {
  return {
    tokensIn: 1_000,
    tokensOut: 500,
    cacheRead: 0,
    estimatedUsd,
    costComplete: true,
    sessionCount: 3,
  }
}

function account(
  overrides: Partial<AllowanceUsageAccountPayload> = {},
): AllowanceUsageAccountPayload {
  return {
    provider: "anthropic",
    displayName: "Claude",
    accountKey: "account",
    utilization: {
      typicalPercent: 40,
      peakPercent: 62,
      averagePercent: 41.4,
      periodCount: 9,
      maxedPeriodCount: 0,
      windowKind: "weekly",
      firstPeriodAt: "2026-07-13T00:00:00Z",
      lastPeriodAt: "2026-09-14T00:00:00Z",
    },
    burst: {
      typicalPercent: 15,
      peakPercent: 100,
      averagePercent: 27.5,
      periodCount: 18,
      maxedPeriodCount: 1,
      windowKind: "rolling",
      firstPeriodAt: "2026-08-30T00:00:00Z",
      lastPeriodAt: "2026-09-14T00:00:00Z",
    },
    overage: {
      blockCount: 8,
      waitedSeconds: 39_960,
      blocksWithoutWait: 0,
      lastBlockAt: "2026-09-14T03:43:00Z",
    },
    days: [],
    previousDays: [],
    ...overrides,
  }
}

function summary(
  accounts: AllowanceUsageAccountPayload[] = [account()],
): AllowanceUsageSummaryPayload {
  return {
    accounts,
    utilizationSpanDays: 60,
    overageSpanDays: 30,
    generatedAt: "2026-09-15T00:00:00Z",
  }
}

function renderTotals(
  overrides: Partial<Parameters<typeof OverviewUsage>[0]> = {},
): (next: "cost" | "allowance") => void {
  const onMetricChange = vi.fn()
  render(
    <OverviewUsage
      metric="allowance"
      onMetricChange={onMetricChange}
      totals={TOTALS}
      days={[]}
      previousDays={[]}
      allowance={summary()}
      {...overrides}
    />,
  )
  return onMetricChange
}

afterEach(() => {
  vi.restoreAllMocks()
})

describe("OverviewUsage", () => {
  it("shows the spend figures on the cost branch and the meters on the allowance branch", () => {
    const { rerender } = render(
      <OverviewUsage
        metric="cost"
        onMetricChange={vi.fn()}
        totals={TOTALS}
        days={[]}
        previousDays={[]}
        allowance={summary()}
      />,
    )
    expect(screen.getByRole("region", { name: "Estimated local spend" })).toBeInTheDocument()
    expect(screen.queryByRole("region", { name: "Allowance" })).not.toBeInTheDocument()

    rerender(
      <OverviewUsage
        metric="allowance"
        onMetricChange={vi.fn()}
        totals={TOTALS}
        days={[]}
        previousDays={[]}
        allowance={summary()}
      />,
    )
    expect(screen.getByRole("region", { name: "Allowance" })).toBeInTheDocument()
    expect(
      screen.queryByRole("region", { name: "Estimated local spend" }),
    ).not.toBeInTheDocument()
  })

  it("hands the page the unit the reader picked", () => {
    const onMetricChange = renderTotals()
    fireEvent.click(screen.getByRole("radio", { name: "Cost" }))
    expect(onMetricChange).toHaveBeenCalledWith("cost")
  })

  it("states the average utilization and the limit hits beside it with their cause", () => {
    renderTotals()
    const cell = screen.getByRole("region", { name: "Allowance" })

    expect(within(cell).getByText("Average subscription utilization")).toBeInTheDocument()
    expect(within(cell).getByText("41%")).toBeInTheDocument()
    expect(within(cell).getByText("average subscription utilization")).toBeInTheDocument()
    expect(within(cell).getByText("11h")).toBeInTheDocument()
    expect(within(cell).getByText(/8 limit hits in 30 days/)).toBeInTheDocument()
    expect(within(cell).getByText("1 of 18 windows estimated at 100%")).toBeInTheDocument()
  })

  it("says how antiburn makes each hero figure", () => {
    // A hero figure has room for a name and none for a method. A reader who
    // doubts the number wants the method and the span it covers.
    renderTotals()
    const cell = screen.getByRole("region", { name: "Allowance" })

    const utilization = within(cell).getByText("41%").closest("[tabindex]")
    fireEvent.focus(utilization!)
    expect(screen.getByRole("tooltip")).toHaveTextContent(/across 9 weeks in the last 60 days/)
    expect(screen.getByRole("tooltip")).toHaveTextContent(/current week can be incomplete/)
    fireEvent.blur(utilization!)

    const limitHits = within(cell).getByText("11h").closest("[tabindex]")
    fireEvent.focus(limitHits!)
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      /waited for the limit to reset, across 8 limit hits in the last 30 days/,
    )
  })

  it("counts the limit hits when none states a reset", () => {
    // Codex refuses without naming a reset. The limit hits still happened,
    // so the figure falls back to counting them.
    renderTotals({
      allowance: summary([
        account({
          overage: {
            blockCount: 1,
            waitedSeconds: 0,
            blocksWithoutWait: 1,
            lastBlockAt: "2026-09-11T00:00:00Z",
          },
        }),
      ]),
    })
    const cell = screen.getByRole("region", { name: "Allowance" })

    expect(within(cell).getByText("1")).toBeInTheDocument()
    expect(within(cell).getByText(/limit hit in 30 days/)).toBeInTheDocument()
    expect(within(cell).getByText(/no stated reset/)).toBeInTheDocument()
  })

  it("shows no utilization figure for an account with no meter history", () => {
    // A gap is unknown, never zero. An account the provider never metered
    // gets no hero rather than a hero against a made-up allowance.
    renderTotals({ allowance: summary([account({ utilization: null })]) })
    const cell = screen.getByRole("region", { name: "Allowance" })

    expect(within(cell).queryByText(/subscription utilization/i)).not.toBeInTheDocument()
    expect(within(cell).getByText("11h")).toBeInTheDocument()
  })

  it("says the readings have not arrived rather than showing an empty meter", () => {
    renderTotals({ allowance: summary([]) })
    expect(screen.getByText(/no allowance history yet/)).toBeInTheDocument()
  })

  it("states a failed read rather than calling it an account with no history", () => {
    // The two states look the same on the page, so each one names its cause.
    renderTotals({ allowance: null, allowanceError: true })
    expect(screen.getByRole("alert")).toHaveTextContent(/cannot read the allowance figures/)
    expect(screen.queryByText(/no allowance history yet/)).not.toBeInTheDocument()
  })

  it("draws no chart after a failed allowance read", () => {
    // The totals state the failed read. A chart that says it has no readings
    // yet states a different thing about the same account.
    renderTotals({ allowance: null, allowanceError: true })
    expect(screen.queryByText(/no allowance history to chart yet/)).not.toBeInTheDocument()
    expect(screen.queryByRole("region", { name: "Allowance by day" })).not.toBeInTheDocument()
  })

  it("keeps the subscription figures when the cost read fails", () => {
    // The two units are separate reads. A failed cost read must not take the
    // unit the reader is looking at off the page.
    renderTotals({ totals: null, usageError: true })
    expect(screen.getByRole("region", { name: "Allowance" })).toBeInTheDocument()
    expect(screen.getByRole("radio", { name: "Cost" })).toBeInTheDocument()
    expect(screen.queryByText("Local usage is unavailable.")).not.toBeInTheDocument()
  })

  it("states the failed cost read on the cost branch, with a retry", () => {
    const onRetryUsage = vi.fn()
    renderTotals({ metric: "cost", totals: null, usageError: true, onRetryUsage })
    expect(screen.getByRole("alert")).toHaveTextContent("Local usage is unavailable.")
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    expect(onRetryUsage).toHaveBeenCalled()
  })
})

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
    plan: null,
    utilization: {
      utilizationPercent: 41.4,
      weeklyWindowCount: 4,
      shortWindowCount: 18,
      modelWindowCount: 2,
    },
    chart: { shortWindows: [], weeklyWindows: [], rolling: [] },
    ...overrides,
  }
}

function summary(
  accounts: AllowanceUsageAccountPayload[] = [account()],
): AllowanceUsageSummaryPayload {
  return {
    accounts,
    utilizationSpanDays: 28,
    rangeStartEpoch: 0,
    rangeEndEpoch: 30 * 86400,
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
      allowance={summary()}
      {...overrides}
    />,
  )
  return onMetricChange
}

afterEach(() => {
  vi.restoreAllMocks()
  localStorage.clear()
})

describe("OverviewUsage", () => {
  it("shows the spend figures on the cost branch and the meters on the allowance branch", () => {
    const { rerender } = render(
      <OverviewUsage
        metric="cost"
        onMetricChange={vi.fn()}
        totals={TOTALS}
        days={[]}
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

  it("shows each account's average subscription usage", () => {
    renderTotals()
    const cell = screen.getByRole("region", { name: "Allowance" })
    expect(within(cell).getByText("Claude")).toBeInTheDocument()
    expect(within(cell).getByText("41%")).toBeInTheDocument()
    expect(within(cell).getByText("Average subscription usage")).toBeInTheDocument()
  })

  it("explains the figure's window counts and time span", () => {
    renderTotals()
    const utilization = screen.getByText("41%").closest("[tabindex]")
    fireEvent.focus(utilization!)
    expect(screen.getByRole("tooltip")).toHaveTextContent(
      "last 28 days, covering 4 weekly windows, 18 5-hour windows and 2 specific model windows",
    )
  })

  it("selects and saves a provider account without changing the metric", () => {
    localStorage.clear()
    const onMetricChange = renderTotals({
      allowance: summary([
        account(),
        account({ provider: "openai", accountKey: "second", displayName: "Codex" }),
      ]),
    })
    fireEvent.click(screen.getByRole("radio", { name: "Codex" }))
    expect(screen.getByRole("radio", { name: "Codex" })).toHaveAttribute("aria-checked", "true")
    expect(screen.getByText(/Codex: not enough window history/)).toBeInTheDocument()
    expect(onMetricChange).not.toHaveBeenCalled()
    expect(JSON.parse(localStorage.getItem("antiburn.overview.view.v1")!)).toEqual({
      accountTabKey: "openai:second",
    })
  })

  it("shows no utilization figure for an account with no meter history", () => {
    // A gap is unknown, never zero. An account the provider never metered
    // gets no hero rather than a hero against a made-up allowance.
    renderTotals({ allowance: summary([account({ utilization: null })]) })
    const cell = screen.getByRole("region", { name: "Allowance" })

    expect(within(cell).queryByText(/subscription utilization/i)).not.toBeInTheDocument()
    expect(within(cell).getByText(/no allowance history yet/)).toBeInTheDocument()
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
    expect(screen.queryByRole("region", { name: "Allowance chart" })).not.toBeInTheDocument()
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

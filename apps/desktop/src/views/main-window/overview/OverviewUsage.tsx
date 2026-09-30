import { useState } from "react"

import type {
  AllowanceUsageAccountPayload,
  AllowanceUsageSummaryPayload,
  ProviderUsageDayPayload,
  ProviderUsageWindowsPayload,
} from "../../../lib/providerUsageIpc"
import { cn } from "../../../lib/cn"
import { SegmentedControl } from "../../../components/ui/SegmentedControl"
import { OverviewAllowanceChart } from "./OverviewAllowanceChart"
import { OverviewAllowanceTotals } from "./OverviewAllowanceTotals"
import { OverviewSpendChart } from "./OverviewSpendChart"
import { OverviewSpendTotals } from "./OverviewSpendTotals"
import {
  readOverviewViewPrefs,
  writeOverviewViewPrefs,
  type OverviewMetric,
} from "./overviewViewPrefs"

import "./overview.css"

export type { OverviewMetric }

const METRICS: ReadonlyArray<{ value: OverviewMetric; label: string }> = [
  { value: "cost", label: "Cost" },
  { value: "allowance", label: "Subscription" },
]

function accountTabKey(account: AllowanceUsageAccountPayload): string {
  return `${account.provider}:${account.accountKey}`
}

export function OverviewUsage({
  metric,
  onMetricChange,
  totals,
  days,
  allowance,
  allowanceLoading = false,
  allowanceError = false,
  usageError = false,
  onRetryUsage,
  loading = false,
}: {
  metric: OverviewMetric
  onMetricChange: (next: OverviewMetric) => void
  totals: ProviderUsageWindowsPayload | null
  days: ProviderUsageDayPayload[]
  allowance: AllowanceUsageSummaryPayload | null
  allowanceLoading?: boolean
  allowanceError?: boolean
  usageError?: boolean
  onRetryUsage?: () => void
  loading?: boolean
}) {
  const costFailed = usageError && !totals
  const allowanceFailed = allowanceError && !allowance

  const chartAccounts = allowance?.accounts ?? []
  const [selectedTabKey, setSelectedTabKey] = useState<string | null>(
    () => readOverviewViewPrefs().accountTabKey ?? null,
  )
  const selectedAccount =
    chartAccounts.find((account) => accountTabKey(account) === selectedTabKey) ??
    chartAccounts[0] ??
    null

  // Prototype: the chart starts closed on every load. A headline figure
  // opens it on that account; the same figure again closes it.
  const [chartOpen, setChartOpen] = useState(false)

  function selectTab(next: string): void {
    if (chartOpen && selectedAccount && accountTabKey(selectedAccount) === next) {
      setChartOpen(false)
      return
    }
    setChartOpen(true)
    setSelectedTabKey(next)
    writeOverviewViewPrefs({ accountTabKey: next })
  }

  return (
    <section
      aria-label="Usage"
      className="overview-usage flex min-h-0 flex-col gap-(--space-md)"
    >
      <SegmentedControl
        options={METRICS}
        value={metric}
        onChange={onMetricChange}
        ariaLabel="Usage unit"
        variant="text-tabs"
        size="large"
        className="self-end"
      />

      {metric === "cost" ? (
        costFailed ? (
          <div className="flex flex-1 items-center justify-center text-center">
            <div>
              <p role="alert" className="type-body text-label-secondary">
                Local usage is unavailable.
              </p>

              {onRetryUsage && (
                <button type="button" onClick={onRetryUsage} className="ui-push-button mt-3">
                  Retry
                </button>
              )}
            </div>
          </div>
        ) : (
          <>
            <OverviewSpendTotals totals={totals} loading={loading} />
            <OverviewSpendChart days={days} loading={loading} />
          </>
        )
      ) : (
        <>
          <OverviewAllowanceTotals
            accounts={chartAccounts}
            utilizationSpanDays={allowance?.utilizationSpanDays ?? 0}
            loading={allowanceLoading}
            error={allowanceError}
            selectedKey={chartOpen && selectedAccount ? accountTabKey(selectedAccount) : null}
            onSelect={selectTab}
          />

          {!allowanceFailed && (
            <div
              inert={!chartOpen}
              className={cn(
                "-mt-(--space-md) grid transition-[grid-template-rows,opacity] duration-medium",
                chartOpen ? "grid-rows-[1fr] opacity-100" : "grid-rows-[0fr] opacity-0",
              )}
            >
              <div className="min-h-0 overflow-hidden">
                <div className="pt-(--space-md)">
                  <OverviewAllowanceChart
                    account={selectedAccount}
                    rangeStartEpoch={allowance?.rangeStartEpoch ?? 0}
                    rangeEndEpoch={allowance?.rangeEndEpoch ?? 0}
                    loading={allowanceLoading}
                  />
                </div>
              </div>
            </div>
          )}
        </>
      )}
    </section>
  )
}

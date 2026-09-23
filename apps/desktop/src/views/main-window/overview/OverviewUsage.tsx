import { useState, type ReactNode } from "react"

import type {
  AllowanceUsageAccountPayload,
  AllowanceUsageSummaryPayload,
  ProviderUsageDayPayload,
  ProviderUsageWindowsPayload,
} from "../../../lib/providerUsageIpc"
import { SegmentedControl } from "../../../components/ui/SegmentedControl"
import { OverviewAllowanceChart } from "./OverviewAllowanceChart"
import { OverviewAllowanceRadial } from "./OverviewAllowanceRadial"
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
  showFigures = true,
  center,
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
  /** False hides the hero figures above the chart. */
  showFigures?: boolean
  /** The content in the middle of the allowance chart. */
  center?: ReactNode
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

  function selectTab(next: string): void {
    setSelectedTabKey(next)
    writeOverviewViewPrefs({ accountTabKey: next })
  }

  const chartControls = chartAccounts.length >= 2 && (
    <SegmentedControl
      options={chartAccounts.map((account) => ({
        value: accountTabKey(account),
        label: account.displayName,
      }))}
      value={selectedAccount ? accountTabKey(selectedAccount) : ""}
      onChange={selectTab}
      ariaLabel="Provider"
      variant="text-tabs"
      size="regular"
    />
  )

  return (
    <section
      aria-label="Usage"
      className="overview-usage flex min-h-0 flex-1 flex-col gap-(--space-lg)"
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
            {showFigures && <OverviewSpendTotals totals={totals} loading={loading} />}
            <OverviewSpendChart days={days} loading={loading} />
          </>
        )
      ) : (
        <>
          {showFigures && (
            <OverviewAllowanceTotals
              accounts={chartAccounts}
              utilizationSpanDays={allowance?.utilizationSpanDays ?? 0}
              loading={allowanceLoading}
              error={allowanceError}
            />
          )}

          {!allowanceFailed &&
            (selectedAccount ? (
              <OverviewAllowanceRadial
                account={selectedAccount}
                rangeEndEpoch={allowance?.rangeEndEpoch ?? 0}
                controls={chartControls}
                center={center}
              />
            ) : (
              <OverviewAllowanceChart
                account={selectedAccount}
                rangeStartEpoch={allowance?.rangeStartEpoch ?? 0}
                rangeEndEpoch={allowance?.rangeEndEpoch ?? 0}
                loading={allowanceLoading}
                controls={chartControls}
              />
            ))}
        </>
      )}
    </section>
  )
}

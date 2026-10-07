import { useState } from "react"

import type {
  AllowanceUsageAccountPayload,
  AllowanceUsageSummaryPayload,
  ProviderUsageDayPayload,
  ProviderUsageWindowsPayload,
} from "../../../lib/providerUsageIpc"
import { SegmentedControl } from "../../../components/ui/SegmentedControl"
import { OverviewAllowanceBackdrop } from "./OverviewAllowanceBackdrop"
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
  allowanceCollecting = false,
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
  allowanceCollecting?: boolean
  usageError?: boolean
  onRetryUsage?: () => void
  loading?: boolean
}) {
  const costFailed = usageError && !totals

  const chartAccounts = allowance?.accounts ?? []
  const [selectedTabKey, setSelectedTabKey] = useState<string | null>(
    () => readOverviewViewPrefs().accountTabKey ?? null,
  )
  const selectedAccount =
    chartAccounts.find((account) => accountTabKey(account) === selectedTabKey) ??
    chartAccounts[0] ??
    null

  // With more than one account, a headline figure picks the account that the
  // banner draws.
  const choosable = chartAccounts.length > 1

  function selectTab(next: string): void {
    setSelectedTabKey(next)
    writeOverviewViewPrefs({ accountTabKey: next })
  }

  return (
    <section
      aria-label="Usage"
      className="overview-usage relative isolate flex min-h-0 flex-1 flex-col gap-(--space-md)"
    >
      {metric === "allowance" && !allowanceLoading && (
        <OverviewAllowanceBackdrop
          account={selectedAccount}
          rangeStartEpoch={allowance?.rangeStartEpoch ?? 0}
          rangeEndEpoch={allowance?.rangeEndEpoch ?? 0}
        />
      )}
      {/* One header row: the figures on the left, the unit tabs on the right. */}
      <div className="flex items-start gap-(--space-lg)">
        <div className="@container min-w-0 flex-1">
          {metric === "cost" ? (
            !costFailed && <OverviewSpendTotals totals={totals} loading={loading} />
          ) : (
            <OverviewAllowanceTotals
              accounts={chartAccounts}
              utilizationSpanDays={allowance?.utilizationSpanDays ?? 0}
              loading={allowanceLoading}
              error={allowanceError}
              collecting={allowanceCollecting}
              selectedKey={choosable && selectedAccount ? accountTabKey(selectedAccount) : null}
              {...(choosable ? { onSelect: selectTab } : {})}
            />
          )}
        </div>
        <SegmentedControl
          options={METRICS}
          value={metric}
          onChange={onMetricChange}
          ariaLabel="Usage unit"
          variant="text-tabs"
          size="large"
          className="shrink-0"
        />
      </div>

      {metric === "cost" &&
        (costFailed ? (
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
          <OverviewSpendChart days={days} loading={loading} banner />
        ))}
    </section>
  )
}

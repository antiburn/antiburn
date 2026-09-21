import type {
  AllowanceUsageSummaryPayload,
  ProviderUsageDayPayload,
  ProviderUsageWindowsPayload,
} from "../../../lib/providerUsageIpc"
import { SegmentedControl } from "../../../components/ui/SegmentedControl"
import { OverviewAllowanceChart } from "./OverviewAllowanceChart"
import { OverviewAllowanceTotals } from "./OverviewAllowanceTotals"
import { OverviewSpendChart } from "./OverviewSpendChart"
import { OverviewSpendTotals } from "./OverviewSpendTotals"
import { allowanceAccounts } from "./overviewAllowance"

import "./overview.css"

/** Which unit the whole Overview page reads in. */
export type OverviewMetric = "cost" | "allowance"

const METRICS: ReadonlyArray<{ value: OverviewMetric; label: string }> = [
  { value: "cost", label: "Cost" },
  { value: "allowance", label: "Subscription" },
]

/**
 * The Overview's usage block, in one of two units.
 *
 * Cost states what the local sessions would cost at list price.
 * Subscription estimates how much of each plan the reader used from the
 * same quota periods as Limits, and counts provider refusals. A subscriber pays
 * one price whatever the token count, so the dollar figure answers a
 * question they do not have.
 *
 * The unit control sits at the top right, over the figures and the chart
 * together, because it changes both. It reads as the control of the block
 * under it, which is what it is.
 *
 * The figures come first and the chart follows them. The figures answer
 * "where do I stand" in one line, which is the first question. The chart
 * answers "how did I get here", which the reader asks second.
 */
export function OverviewUsage({
  metric,
  onMetricChange,
  totals,
  days,
  previousDays,
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
  previousDays: ProviderUsageDayPayload[]
  allowance: AllowanceUsageSummaryPayload | null
  allowanceLoading?: boolean
  allowanceError?: boolean
  usageError?: boolean
  onRetryUsage?: () => void
  loading?: boolean
}) {
  // Each unit reads its own figures, so a failed read belongs to the unit it
  // failed for. The other unit stays on the page.
  const costFailed = usageError && !totals
  const allowanceFailed = allowanceError && !allowance
  return (
    <section
      aria-label="Usage"
      className="overview-usage flex min-h-0 flex-1 flex-col gap-[var(--space-lg)]"
    >
      <SegmentedControl
        options={METRICS}
        value={metric}
        onChange={onMetricChange}
        ariaLabel="Usage unit"
        variant="text-tabs"
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
            <OverviewSpendChart days={days} previousDays={previousDays} loading={loading} />
          </>
        )
      ) : (
        <>
          <OverviewAllowanceTotals
            accounts={allowanceAccounts(allowance)}
            spanDays={allowance?.overageSpanDays ?? 0}
            utilizationSpanDays={allowance?.utilizationSpanDays ?? 0}
            loading={allowanceLoading}
            error={allowanceError}
          />
          {/* The totals above state the failed read. A chart that says it has
              no readings yet states a different thing, so it stays away. */}
          {!allowanceFailed && (
            <OverviewAllowanceChart
              accounts={allowanceAccounts(allowance)}
              loading={allowanceLoading}
            />
          )}
        </>
      )}
    </section>
  )
}

import type { ProviderUsageWindowsPayload } from "../../lib/ipc"
import { formatCompact, formatCost } from "../../lib/presentation/sessionAnalysis"
import { windowTokens } from "../../lib/presentation/providerUsage"

const EMPTY_USAGE_WINDOW = {
  tokensIn: 0,
  tokensOut: 0,
  cacheRead: 0,
  estimatedUsd: null,
  costComplete: true,
  sessionCount: 0,
}

export const EMPTY_USAGE_WINDOWS: ProviderUsageWindowsPayload = {
  today: { ...EMPTY_USAGE_WINDOW },
  week: { ...EMPTY_USAGE_WINDOW },
  monthToDate: { ...EMPTY_USAGE_WINDOW },
  last30Days: { ...EMPTY_USAGE_WINDOW },
}

function metric(window: ProviderUsageWindowsPayload["today"]): string {
  const tokens = formatCompact(windowTokens(window))
  return window.estimatedUsd == null ? tokens : `${formatCost(window.estimatedUsd)} · ${tokens}`
}

export function UsageSpendSummary({
  totals,
  compact = false,
}: {
  totals: ProviderUsageWindowsPayload
  compact?: boolean
}) {
  return (
    <section aria-label="Usage and spend" className={compact ? "px-3 py-2" : "px-1 py-2"}>
      <div className="flex items-baseline justify-between gap-3">
        <h2 className="type-caption font-medium tracking-wide uppercase text-label">
          Total usage
        </h2>
      </div>
      {/* Each cell holds its text off its own left edge. The inset makes the
          three readings sit nearer the middle of their columns. The text stays
          left-aligned, so the figures still line up down the column. */}
      <dl className="mt-2 grid grid-cols-3 gap-3">
        <div className="min-w-0 pl-2">
          <dt className="type-caption text-label-secondary">Today</dt>
          <dd className="truncate type-footnote tabular-nums text-label">
            {metric(totals.today)}
          </dd>
        </div>
        <div className="min-w-0 pl-2">
          <dt className="type-caption text-label-secondary">This week</dt>
          <dd className="truncate type-footnote tabular-nums text-label">
            {metric(totals.week)}
          </dd>
        </div>
        <div className="min-w-0 pl-2">
          <dt className="type-caption text-label-secondary">Last 30 days</dt>
          <dd className="truncate type-footnote tabular-nums text-label">
            {metric(totals.last30Days)}
          </dd>
        </div>
      </dl>
    </section>
  )
}

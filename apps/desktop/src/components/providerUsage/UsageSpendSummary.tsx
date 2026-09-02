import type { ProviderUsageWindowsPayload } from "../../lib/ipc"
import { formatCompact, formatCost } from "../../lib/presentation/sessionAnalysis"
import { windowTokens } from "../../lib/presentation/providerUsage"

export const EMPTY_USAGE_WINDOWS: ProviderUsageWindowsPayload = {
  today: { tokensIn: 0, tokensOut: 0, cacheRead: 0, estimatedUsd: null, sessionCount: 0 },
  week: { tokensIn: 0, tokensOut: 0, cacheRead: 0, estimatedUsd: null, sessionCount: 0 },
  monthToDate: { tokensIn: 0, tokensOut: 0, cacheRead: 0, estimatedUsd: null, sessionCount: 0 },
  last30Days: { tokensIn: 0, tokensOut: 0, cacheRead: 0, estimatedUsd: null, sessionCount: 0 },
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
      <dl className="mt-2 grid grid-cols-3 gap-3">
        <div className="min-w-0">
          <dt className="type-caption text-label-secondary">Today</dt>
          <dd className="truncate type-footnote tabular-nums text-label">
            {metric(totals.today)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="type-caption text-label-secondary">This week</dt>
          <dd className="truncate type-footnote tabular-nums text-label">
            {metric(totals.week)}
          </dd>
        </div>
        <div className="min-w-0">
          <dt className="type-caption text-label-secondary">Last 30 days</dt>
          <dd className="truncate type-footnote tabular-nums text-label">
            {metric(totals.last30Days)}
          </dd>
        </div>
      </dl>
    </section>
  )
}

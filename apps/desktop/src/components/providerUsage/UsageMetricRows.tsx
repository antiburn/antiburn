import { ArrowRight, Gauge, TrendingDown, TrendingUp, type LucideIcon } from "lucide-react"

import type { ProviderUsagePayload } from "../../lib/ipc"
import { paceTrend, usageMetricRows } from "../../lib/presentation/providerUsage"

/** The local spend trend, after the provider's limit pace. */
export function UsageMetricRows({ provider }: { provider: ProviderUsagePayload }) {
  const trend = paceTrend(provider)
  const trendIcon: LucideIcon =
    trend.kind === "picking-up"
      ? TrendingUp
      : trend.kind === "easing"
        ? TrendingDown
        : ArrowRight
  const icons: Record<string, LucideIcon> = { trend: trendIcon }

  return (
    <dl className="space-y-1.5">
      {usageMetricRows(provider)
        .filter((row) => row.key === "trend")
        .map((row) => {
          const Icon = icons[row.key] ?? Gauge
          return (
            <div key={row.key} className="flex items-baseline justify-between gap-3">
              <dt className="flex items-center gap-1.5 type-footnote text-label-secondary">
                <Icon size={12} strokeWidth={1.75} aria-hidden="true" className="shrink-0" />
                {row.label}
              </dt>
              <dd className="type-footnote tabular-nums text-label">{row.value}</dd>
            </div>
          )
        })}
    </dl>
  )
}

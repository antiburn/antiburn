import { cn } from "../../lib/cn"
import type { ProviderUsagePayload } from "../../lib/ipc"
import {
  providerWindow,
  sessionCountLabel,
  usageMetricLabel,
  usageWindowLabel,
  windowShareOfMonth,
  USAGE_WINDOWS,
} from "../../lib/presentation/providerUsage"

/**
 * The three windows with a bar under each.
 *
 * The bar is each window's share of the same provider's month — Today fits
 * inside Last 7 days fits inside This month, so the fills read as
 * accumulation over time. It is deliberately NOT a meter against an
 * allowance: that figure does not exist on this machine, so the bar's only
 * denominator is the reader's own month. The accessible name says whose
 * month, so a screen reader is told the same thing the geometry implies.
 */
export function UsageWindowRows({
  provider,
  className = "",
}: {
  provider: ProviderUsagePayload
  className?: string
}) {
  return (
    <dl className={cn("space-y-2", className)}>
      {USAGE_WINDOWS.map(({ value }) => {
        const window = providerWindow(provider, value)
        const share = windowShareOfMonth(provider, value)
        return (
          <div key={value}>
            <div className="flex items-baseline justify-between gap-3">
              <dt className="type-footnote text-label-secondary">{usageWindowLabel(value)}</dt>
              <dd className="flex items-baseline gap-2">
                <span className="type-caption text-label-tertiary">
                  {sessionCountLabel(window.sessionCount)}
                </span>
                <span className="type-footnote tabular-nums text-label">
                  {usageMetricLabel(window)}
                </span>
              </dd>
            </div>
            <div
              role="img"
              aria-label={`${usageWindowLabel(value)}: this window's share of the month of local spend shown below it`}
              className="mt-1 w-full overflow-hidden rounded-full"
              style={{ height: 3, backgroundColor: "var(--color-separator)" }}
            >
              <div
                className="h-full rounded-full"
                data-testid={`usage-share-${value}`}
                style={{
                  width: `${Math.round(share * 100)}%`,
                  // accent-fill, not accent: the live macOS system-accent
                  // token breaks as a background-color (see tokens.css); the
                  // -val fallback is the raw palette entry, always emitted.
                  backgroundColor: "var(--color-accent-fill, var(--color-accent-fill-val))",
                }}
              />
            </div>
          </div>
        )
      })}
    </dl>
  )
}

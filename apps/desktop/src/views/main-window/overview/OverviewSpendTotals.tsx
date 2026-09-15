import type {
  ProviderUsageWindowPayload,
  ProviderUsageWindowsPayload,
} from "../../../lib/providerUsageIpc"
import {
  formatSpendFigure,
  formatTokenFigure,
  sessionCountLabel,
  windowTokens,
} from "../../../lib/presentation/providerUsage"

import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { Skeleton } from "../../../components/ui/Skeleton"

import "./overview.css"

const SPANS: ReadonlyArray<{
  key: keyof ProviderUsageWindowsPayload
  label: string
  accessibleLabel: string
}> = [
  { key: "today", label: "Today", accessibleLabel: "Today" },
  { key: "week", label: "7 days", accessibleLabel: "Last 7 days" },
  { key: "last30Days", label: "30 days", accessibleLabel: "Last 30 days" },
]

/**
 * The Overview's headline: estimated local spend today, this week, and over
 * the trailing thirty days, each as a hero figure over its token count and
 * session count. The cells carry their own labels; there is no heading. An
 * unpriced window leads with its token count instead of a zero-dollar
 * figure, and a partly priced one says so.
 */
export function OverviewSpendTotals({
  totals,
  loading = false,
}: {
  totals: ProviderUsageWindowsPayload | null
  loading?: boolean
}) {
  return (
    <section aria-label="Estimated local spend" aria-busy={loading || undefined}>
      <dl className="overview-totals">
        {SPANS.map((span) => (
          <div key={span.key} className="overview-totals-cell min-w-0 border-separator">
            <dt className="type-callout text-label-secondary">
              <span aria-hidden="true">{span.label}</span>
              <span className="sr-only">{span.accessibleLabel}</span>
            </dt>
            {loading || !totals ? (
              <>
                <dd className="mt-[var(--space-xs)]">
                  <Skeleton className="h-8 w-28" />
                </dd>
                <dd className="mt-[var(--space-xs)]">
                  <Skeleton className="h-3 w-36 max-w-full" />
                </dd>
              </>
            ) : (
              <SpendCell window={totals[span.key]} />
            )}
          </div>
        ))}
      </dl>
    </section>
  )
}

function SpendCell({ window }: { window: ProviderUsageWindowPayload }) {
  const hasCost = window.estimatedUsd != null
  const tokens = windowTokens(window)
  return (
    <>
      <dd className="type-hero-figure mt-[var(--space-xs)] whitespace-nowrap font-mono text-measure">
        <SegmentFigure>
          {hasCost ? formatSpendFigure(window.estimatedUsd ?? 0) : formatTokenFigure(tokens)}
        </SegmentFigure>
      </dd>
      <dd className="type-caption mt-[var(--space-xs)] whitespace-nowrap text-label-tertiary">
        {hasCost && (
          <>
            <SegmentFigure>{formatTokenFigure(tokens)}</SegmentFigure>
            <span aria-hidden="true"> · </span>
          </>
        )}
        {sessionCountLabel(window.sessionCount)}
        {!window.costComplete && (
          <>
            <span aria-hidden="true"> · </span>
            <span title="Some models in this period have no known price.">partial</span>
          </>
        )}
      </dd>
    </>
  )
}

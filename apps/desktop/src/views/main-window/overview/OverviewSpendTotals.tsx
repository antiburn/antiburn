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

import { Tooltip } from "../../../components/presentation/Tooltip"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"
import { Skeleton } from "../../../components/ui/Skeleton"

import "./overview.css"

const SPANS: ReadonlyArray<{
  key: keyof ProviderUsageWindowsPayload
  label: string
  accessibleLabel: string
  /** The span in the reader's words, inside the tooltip sentence. */
  span: string
}> = [
  { key: "today", label: "Today", accessibleLabel: "Today", span: "since midnight" },
  { key: "week", label: "7 days", accessibleLabel: "Last 7 days", span: "in the last 7 days" },
  {
    key: "last30Days",
    label: "30 days",
    accessibleLabel: "Last 30 days",
    span: "in the last 30 days",
  },
]

/**
 * What the figure over one span is, and what it is not.
 *
 * The figure reads in dollars, which a reader takes for a bill. It is an
 * estimate antiburn makes from the reader's own files, so the tooltip says
 * where it comes from and what it cannot know.
 */
function spendTooltip(span: string): string {
  return (
    `The tokens your sessions used ${span}, priced at each model's list ` +
    `rate. antiburn counts them from your own session records. It is an ` +
    `estimate, not a bill: it knows nothing of your plan or your discounts.`
  )
}

/**
 * The Overview's headline: estimated local spend today, this week, and over
 * the trailing thirty days, each as a hero figure over its token count and
 * session count. The cells carry their own labels; there is no heading. An
 * unpriced window leads with its token count instead of a zero-dollar
 * figure, and a partly priced one says so.
 *
 * Each cell carries a tooltip. A dollar figure reads as a bill, so the
 * tooltip says the figure is antiburn's own estimate and names what it
 * cannot know.
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
          <Tooltip key={span.key} label={spendTooltip(span.span)}>
            <div className="overview-totals-cell min-w-0 border-separator" tabIndex={0}>
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
          </Tooltip>
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

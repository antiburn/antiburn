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

import { HeroFigures, type HeroFigureCell } from "../../../components/ui/HeroFigures"
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
  const cells: HeroFigureCell[] = SPANS.map((span) => ({
    key: span.key,
    label: (
      <>
        <span aria-hidden="true">{span.label}</span>
        <span className="sr-only">{span.accessibleLabel}</span>
      </>
    ),
    ...(loading || !totals
      ? {
          figure: <Skeleton className="h-8 w-28" />,
          caption: <Skeleton className="h-3 w-36 max-w-full" />,
        }
      : spendCell(totals[span.key])),
  }))
  return (
    <section aria-label="Estimated local spend" aria-busy={loading || undefined}>
      <HeroFigures cells={cells} />
    </section>
  )
}

function spendCell(
  window: ProviderUsageWindowPayload,
): Pick<HeroFigureCell, "figure" | "caption"> {
  const hasCost = window.estimatedUsd != null
  const tokens = windowTokens(window)
  return {
    figure: (
      <SegmentFigure>
        {hasCost ? formatSpendFigure(window.estimatedUsd ?? 0) : formatTokenFigure(tokens)}
      </SegmentFigure>
    ),
    caption: (
      <>
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
      </>
    ),
  }
}

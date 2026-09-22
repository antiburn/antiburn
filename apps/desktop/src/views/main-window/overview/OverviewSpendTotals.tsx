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

const SPANS: ReadonlyArray<{
  key: keyof ProviderUsageWindowsPayload
  label: string
  accessibleLabel: string
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

function spendTooltip(span: string): string {
  return `Estimated cost, at list price, of the tokens you used ${span}.`
}

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
    tooltip: spendTooltip(span.span),
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
        {sessionCountLabel(window.sessionCount)}
        {hasCost && (
          <>
            <span aria-hidden="true"> · </span>
            <SegmentFigure>{`${formatTokenFigure(tokens)} tokens`}</SegmentFigure>
          </>
        )}
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

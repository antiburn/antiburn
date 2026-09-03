import type { ProviderUsageWindowsPayload } from "../../lib/ipc"
import { cn } from "../../lib/cn"
import {
  formatSpendFigure,
  formatTokenFigure,
  windowTokens,
} from "../../lib/presentation/providerUsage"
import { SegmentFigure } from "../ui/SegmentFigure"

type UsageWindow = ProviderUsageWindowsPayload["today"]

const EMPTY_USAGE_WINDOW: UsageWindow = {
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

/** The large figure: the cost when the models could be priced, else the tokens. */
function figure(window: UsageWindow): string {
  if (window.estimatedUsd != null) {
    return formatSpendFigure(window.estimatedUsd)
  }
  return formatTokenFigure(windowTokens(window))
}

/**
 * The line under the figure: the token count behind a cost, or the unit alone
 * when the tokens are the figure. The Today column also carries the hedge.
 */
function caption(window: UsageWindow, hedge: boolean): string {
  if (window.estimatedUsd == null) return "tokens"
  const tokens = formatTokenFigure(windowTokens(window))
  return hedge ? `${tokens} tokens · est.` : tokens
}

/**
 * The spend summary at the top of the popover: three windows, each a label
 * over a figure over a caption. The card has no heading. The figures are the
 * first ink, in the mono face the session cost badges use for a price. A
 * full-width band in the header surface carries the summary, so it reads as
 * the popover's header and not as one of the session cards. The band ends at
 * the ring row.
 *
 * Every column has the same three fixed-height lines, so the rows align
 * across columns without a shared grid row.
 */
export function UsageSpendSummary({
  totals,
  compact = false,
}: {
  totals: ProviderUsageWindowsPayload
  compact?: boolean
}) {
  return (
    <section
      aria-label="Usage and spend"
      title="Estimated locally at API rates. Your provider bill may differ."
      className={cn("bg-surface-header", compact ? "px-4 pt-3 pb-2.5" : "px-3 pt-3 pb-2.5")}
    >
      <dl className="grid grid-cols-[1.35fr_1fr_1fr] gap-x-3">
        <SpendColumn label="Today" window={totals.today} primary />
        <SpendColumn label="Last 7 days" window={totals.week} />
        <SpendColumn label="Last 30 days" window={totals.last30Days} />
      </dl>
    </section>
  )
}

function SpendColumn({
  label,
  window,
  primary = false,
}: {
  label: string
  window: UsageWindow
  primary?: boolean
}) {
  return (
    <div className="min-w-0">
      <dt className="type-caption text-label-secondary">{label}</dt>
      {/* The important modifiers are necessary because the type-* classes
          are unlayered CSS. The 17 px line is tighter than the type scale,
          the way the status bar sets its own figure line. */}
      <dd
        className={cn(
          primary ? "type-title-3" : "type-body-large",
          "font-mono tracking-tight! leading-[17px] whitespace-nowrap text-label",
        )}
      >
        <SegmentFigure>{figure(window)}</SegmentFigure>
      </dd>
      <dd className="type-caption whitespace-nowrap text-label-tertiary">
        <SegmentFigure>{caption(window, primary)}</SegmentFigure>
      </dd>
    </div>
  )
}

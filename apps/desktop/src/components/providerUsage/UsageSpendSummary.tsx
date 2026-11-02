import type { ProviderUsageWindowsPayload } from "../../lib/ipc"
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

export function UsageSpendSummary({
  totals,
  hasSubscriptionPlan = false,
}: {
  totals: ProviderUsageWindowsPayload
  hasSubscriptionPlan?: boolean
}) {
  return (
    <section
      aria-label="Usage and spend"
      title={
        hasSubscriptionPlan
          ? "You're on subscription, so these are just estimated dollar values."
          : undefined
      }
      className="px-[var(--space-sm)] pt-[var(--space-md)]"
    >
      <dl className="grid grid-cols-3 gap-x-[var(--space-sm)] rounded-control bg-surface-card px-[var(--space-md)] py-[var(--space-sm)] shadow-stats-card">
        <SpendColumn
          label="Today"
          accessibleLabel="Today"
          window={totals.today}
          subscription={hasSubscriptionPlan}
        />
        <SpendColumn
          label="7 days"
          accessibleLabel="Last 7 days"
          window={totals.week}
          subscription={hasSubscriptionPlan}
        />
        <SpendColumn
          label="30 days"
          accessibleLabel="Last 30 days"
          window={totals.last30Days}
          subscription={hasSubscriptionPlan}
        />
      </dl>
    </section>
  )
}

function SpendColumn({
  label,
  accessibleLabel,
  window,
  subscription,
}: {
  label: string
  accessibleLabel: string
  window: UsageWindow
  subscription: boolean
}) {
  const tokens = formatTokenFigure(windowTokens(window))
  const spend = window.estimatedUsd == null ? null : formatSpendFigure(window.estimatedUsd)
  const leadWithTokens = subscription || spend == null
  return (
    <div className="min-w-0">
      <dt className="sr-only">{accessibleLabel}</dt>
      <dd className="type-title-3 font-semibold! whitespace-nowrap text-label">
        <SegmentFigure>{leadWithTokens ? tokens : spend}</SegmentFigure>
        {leadWithTokens && <span className="sr-only"> tokens</span>}
      </dd>
      <dd className="type-footnote whitespace-nowrap text-label-secondary">
        {spend != null && (
          <>
            <SegmentFigure>{leadWithTokens ? spend : tokens}</SegmentFigure>
            {leadWithTokens && <span className="sr-only"> estimated dollars</span>}
            {!leadWithTokens && <span className="sr-only"> tokens</span>}
            <span aria-hidden="true" className="mx-[var(--space-xs)] text-label-tertiary">
              ·
            </span>
          </>
        )}
        <span aria-hidden="true" className="text-label-tertiary">
          {label}
        </span>
      </dd>
    </div>
  )
}

import { useState, useSyncExternalStore } from "react"

import { cn } from "../../lib/cn"

import type { SessionListEntry } from "../../components/session/SessionList"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { type MainOverviewSession } from "./MainOverviewSession"
import { OverviewProviderLimits } from "./overview/OverviewProviderLimits"
import { OverviewRecentSessions } from "./overview/OverviewRecentSessions"
import { OverviewUsage, type OverviewMetric } from "./overview/OverviewUsage"
import { readOverviewViewPrefs, writeOverviewViewPrefs } from "./overview/overviewViewPrefs"

import "./overview/overview.css"

export function OverviewView({
  active,
  session,
  onOpenSessions,
  onSelectSession,
}: {
  active: boolean
  session: MainOverviewSession
  onOpenSessions: () => void
  onSelectSession: (entry: SessionListEntry) => void
}) {
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const [selectedMetric, setMetric] = useState<OverviewMetric | null>(() => {
    const saved = readOverviewViewPrefs().metric
    return saved === "cost" || saved === "allowance" ? saved : null
  })
  // What the last run found out about this reader's plans, so the page can
  // open on the right unit instead of guessing and correcting itself.
  const [rememberedPlan] = useState<boolean | undefined>(
    () => readOverviewViewPrefs().hadSubscriptionPlan,
  )
  // `allowance` and `liveUsage` are both null until the first read returns, and
  // a reader with no subscription looks exactly like one whose plans have not
  // been read yet. Only believe them once one of the two has reported.
  const planSettled = state.liveUsageSettled || state.allowance != null
  const observedPlan = planSettled
    ? (state.allowance?.accounts.some((account) => account.plan != null) ?? false) ||
      (state.liveUsage?.providers.some((provider) => provider.plan != null) ?? false)
    : undefined
  const hasSubscriptionPlan = observedPlan ?? rememberedPlan

  const metric = selectedMetric ?? (hasSubscriptionPlan === false ? "cost" : "allowance")
  // On a first run there is no choice and nothing remembered, so the unit above
  // is a guess. Hold the figures back rather than draw them under a tab that is
  // about to change.
  const metricSettled = selectedMetric != null || hasSubscriptionPlan != null
  const usage = state.usage
  const loading = (!usage && !state.usageError) || !metricSettled

  return (
    <div
      className={cn(
        "overview-layout min-h-0 min-w-0 flex-1 bg-surface-window",
        "grid grid-cols-[auto_minmax(0,1fr)_clamp(206px,21%,316px)_auto] grid-rows-[minmax(0,1fr)] gap-x-(--space-2xl) pt-(--space-2xl) mb-(--space-2xl)",
      )}
      data-overview-active={active ? "" : undefined}
    >
      <h1 className="sr-only">Overview</h1>

      <ScrollPane
        topEdgeFade
        className="col-2 overview-viewport"
        viewportClassName="[&>div]:flex! [&>div]:min-block-full"
      >
        <div
          role="region"
          aria-label={loading ? "Loading Overview" : "Overview"}
          aria-busy={loading || undefined}
          className="@container grow shrink-0 flex w-full flex-col gap-(--space-2xl)"
        >
          {loading && (
            <p role="status" className="sr-only">
              Loading Overview
            </p>
          )}

          <OverviewUsage
            metric={metric}
            onMetricChange={(next) => {
              setMetric(next)
              writeOverviewViewPrefs({ metric: next })
            }}
            totals={usage?.totals ?? null}
            days={usage?.days ?? []}
            allowance={state.allowance}
            allowanceLoading={state.allowanceLoading || !metricSettled}
            allowanceError={state.allowanceError}
            usageError={state.usageError}
            onRetryUsage={session.refresh}
            loading={loading}
          />

          <OverviewRecentSessions
            active={active && state.active}
            entries={state.recentSessions}
            loading={loading && !state.recentSessions}
            onSelect={onSelectSession}
            onOpenAll={onOpenSessions}
            metric={metric}
            liveUsage={state.liveUsage ?? undefined}
            sessionLimitAllocations={state.sessionLimitAllocations}
          />
        </div>
      </ScrollPane>

      <ScrollPane
        className={cn(
          "overview-provider-limits min-h-0 min-w-0",
          "rounded-(--radius-popover) shadow-[var(--shadow-raised),var(--shadow-stats-card)]",
          "bg-(--color-surface-window) bg-gradient-to-b from-(--color-surface-sidebar) to-(--color-surface-sidebar)",
        )}
        viewportTabIndex={0}
        viewportLabel="Provider limits card"
        topEdgeFade
      >
        <OverviewProviderLimits live={state.liveUsage} loading={!state.liveUsageSettled} />
      </ScrollPane>
    </div>
  )
}

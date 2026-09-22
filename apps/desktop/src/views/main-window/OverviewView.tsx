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
  const hasSubscriptionPlan =
    state.allowance?.accounts.some((account) => account.plan != null) ||
    state.liveUsage?.providers.some((provider) => provider.plan != null)
  const metric = selectedMetric ?? (hasSubscriptionPlan ? "allowance" : "cost")
  const usage = state.usage
  const loading = !usage && !state.usageError

  return (
    <div
      className={cn(
        "min-h-0 min-w-0 flex-1 bg-surface-window",
        "grid grid-cols-[auto_minmax(0,1fr)_clamp(206px,21%,316px)_auto] grid-rows-[auto_minmax(0,1fr)_auto] gap-x-(--space-2xl) mb-(--space-2xl)",
      )}
      data-overview-active={active ? "" : undefined}
    >
      <div
        className="col-span-full h-(--main-window-titlebar-height)"
        data-tauri-drag-region
        aria-hidden="true"
      />

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
            allowanceLoading={state.allowanceLoading}
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
          "min-h-0 min-w-0",
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

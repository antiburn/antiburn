import { useState, useSyncExternalStore } from "react"

import { cn } from "../../lib/cn"

import type { SessionListEntry } from "../../components/session/SessionList"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { useAppSettings } from "../settings/useAppSettings"
import { type MainOverviewSession } from "./MainOverviewSession"
import { OverviewFixes } from "./overview/OverviewFixes"
import { overviewProgress, subscribeOverviewProgress } from "./overview/overviewProgressStore"
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
  // The first-run scan reveals these cards as each real stage ends. Outside
  // first run (`mode` not `"firstRun"`) both cards show at once, same as the
  // rest of the page.
  const progress = useSyncExternalStore(
    subscribeOverviewProgress,
    overviewProgress,
    overviewProgress,
  )
  const isFirstRun = progress.mode === "firstRun"
  const readingDone = !isFirstRun || progress.read.done
  const analysisDone = !isFirstRun || progress.check.done
  const { settings } = useAppSettings()
  const [selectedMetric, setMetric] = useState<OverviewMetric | null>(() => {
    const saved = readOverviewViewPrefs().metric
    return saved === "cost" || saved === "allowance" ? saved : null
  })
  // What the last run found out about this reader's plans, so the page can
  // open on the right unit instead of guessing and correcting itself.
  const [rememberedPlan] = useState<boolean | undefined>(
    () => readOverviewViewPrefs().hadSubscriptionPlan,
  )
  // A detected plan settles the answer at once. A reader with no plan looks
  // exactly like one whose allowance and live-usage reads have not both
  // answered yet, so a negative answer only settles once both have.
  const observedPlan =
    (state.allowance?.accounts.some((account) => account.plan != null) ?? false) ||
    (state.liveUsage?.providers.some((provider) => provider.plan != null) ?? false)
  const allowanceSettled =
    !state.allowanceLoading && (state.allowance != null || state.allowanceError)
  const planSettled = observedPlan || (allowanceSettled && state.liveUsageSettled)
  const settledPlan = planSettled ? observedPlan : undefined
  const hasSubscriptionPlan = settledPlan ?? rememberedPlan

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

          <div
            inert={!analysisDone}
            className={cn(
              "rounded-(--radius-popover) shadow-[var(--shadow-raised),var(--shadow-stats-card)] bg-surface-sidebar p-(--space-lg) transition-[opacity,translate] duration-slow",
              !analysisDone && "translate-y-2 opacity-0",
            )}
          >
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
              allowanceCollecting={isFirstRun}
              usageError={state.usageError}
              onRetryUsage={session.refresh}
              loading={loading}
            />
          </div>

          <OverviewFixes />

          <div
            inert={!readingDone}
            className={cn(
              "rounded-(--radius-popover) shadow-[var(--shadow-raised),var(--shadow-stats-card)] bg-surface-sidebar p-(--space-lg) transition-[opacity,translate] duration-slow",
              !readingDone && "translate-y-2 opacity-0",
            )}
          >
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
        <OverviewProviderLimits
          live={state.liveUsage}
          loading={!state.liveUsageSettled}
          liveUsageStarted={settings.liveUsageStarted}
        />
      </ScrollPane>
    </div>
  )
}

import { useState, useSyncExternalStore } from "react"

import { cn } from "../../lib/cn"
import type { BurnCheckDetectorId } from "../../lib/insightsIpc"

import type { SessionListEntry } from "../../components/session/SessionList"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { useAppSettings } from "../settings/useAppSettings"
import { FirstRunTakeover } from "./overview/FirstRunTakeover"
import { type MainOverviewSession } from "./MainOverviewSession"
import { OverviewFixes } from "./overview/OverviewFixes"
import {
  overviewProgress,
  RECENT_SESSIONS_TRANSITION_NAME,
  stepDocked,
  USAGE_TRANSITION_NAME,
  subscribeOverviewProgress,
} from "./overview/overviewProgressStore"
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
  onOpenChecks,
}: {
  active: boolean
  session: MainOverviewSession
  onOpenSessions: () => void
  onSelectSession: (entry: SessionListEntry) => void
  onOpenChecks: (check: BurnCheckDetectorId | undefined) => void
}) {
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const progress = useSyncExternalStore(
    subscribeOverviewProgress,
    overviewProgress,
    overviewProgress,
  )
  // Until the first-run check answers, it is not known whether the takeover
  // or the steady Overview owns the main column, so neither shows. Otherwise a
  // first run shows the steady Overview for a moment first.
  const modeDecided = progress.mode !== "pending"
  const isFirstRun = progress.mode === "firstRun"
  // The takeover owns the main column until the fixes step is done, in
  // place of the usage card, the checks list and Recent sessions.
  const showTakeover = isFirstRun && progress.flow !== "done"
  const { settings } = useAppSettings()
  // The right-hand pane only exists once live usage is both enabled and
  // started; turning either off in Settings takes it away, and the grid
  // drops its column so the remaining columns keep their width.
  const showProviderLimits = settings.liveUsageEnabled && settings.liveUsageStarted
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

  const usageCard = (
    <div
      style={{ viewTransitionName: USAGE_TRANSITION_NAME }}
      // The card takes the free height of the page, so its banner chart grows
      // to fill the window.
      className="flex flex-1 flex-col p-(--space-lg)"
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
        allowanceCollecting={
          isFirstRun &&
          progress.flow !== "done" &&
          settings.liveUsageEnabled &&
          settings.liveUsageStarted
        }
        usageError={state.usageError}
        onRetryUsage={session.refresh}
        loading={loading}
      />
    </div>
  )

  const recentSessions = (
    <div
      style={{ viewTransitionName: RECENT_SESSIONS_TRANSITION_NAME }}
      // No card: the rows carry their own fill. The list sits at the
      // bottom of the page and free space collects above it.
      className="mt-auto"
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
        showChecks={!showTakeover || stepDocked(progress.flow, "checks")}
        showOpenAll={!showTakeover}
      />
    </div>
  )

  return (
    <div
      className={cn(
        "overview-layout min-h-0 min-w-0 flex-1 bg-surface-window",
        "grid grid-rows-[minmax(0,1fr)] gap-x-(--space-2xl) pt-(--space-2xl) mb-(--space-2xl)",
        showProviderLimits
          ? "grid-cols-[auto_minmax(0,1fr)_clamp(206px,21%,316px)_auto]"
          : "grid-cols-[auto_minmax(0,1fr)_auto]",
      )}
      data-overview-active={active ? "" : undefined}
    >
      <h1 className="sr-only">Overview</h1>

      <ScrollPane
        topEdgeFade
        className="col-2 overview-viewport"
        viewportClassName="[&>div]:flex! [&>div]:min-block-full"
      >
        {showTakeover ? (
          // The usage card and Recent sessions fade in around the takeover
          // after the Checks step. The session subscription keeps data current.
          <div className="flex grow flex-col gap-(--space-2xl)">
            {stepDocked(progress.flow, "checks") && usageCard}
            <FirstRunTakeover
              onOpenChecks={onOpenChecks}
              // Between the usage card and Recent sessions, the step sits
              // in a card like the provider limits card, and fills the free
              // height. The config checks that replace it have no card.
              className={cn(
                stepDocked(progress.flow, "checks") &&
                  "rounded-(--radius-popover) bg-(--color-surface-window) bg-gradient-to-b from-(--color-surface-sidebar) to-(--color-surface-sidebar) shadow-stats-card",
              )}
            />
            {stepDocked(progress.flow, "checks") && recentSessions}
          </div>
        ) : modeDecided ? (
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

            {usageCard}

            <OverviewFixes onOpenCheck={onOpenChecks} />

            {recentSessions}
          </div>
        ) : null}
      </ScrollPane>

      {showProviderLimits && (
        <ScrollPane
          className={cn(
            "overview-provider-limits min-h-0 min-w-0",
            "rounded-(--radius-popover) shadow-stats-card",
            "bg-(--color-surface-window) bg-gradient-to-b from-(--color-surface-sidebar) to-(--color-surface-sidebar)",
          )}
          viewportTabIndex={0}
          viewportLabel="Provider limits card"
          topEdgeFade
        >
          <OverviewProviderLimits live={state.liveUsage} loading={!state.liveUsageSettled} />
        </ScrollPane>
      )}
    </div>
  )
}

import { useState, useSyncExternalStore } from "react"

import { cn } from "../../lib/cn"

import type { SessionListEntry } from "../../components/session/SessionList"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { checksPresentation } from "../../lib/presentation/checks"
import { snoozedDetectorIds, useSnoozedBurnChecks } from "../../lib/snoozedBurnChecks"
import { type BurnChecksSession } from "./BurnChecksSession"
import { type MainOverviewSession } from "./MainOverviewSession"
import { EnhanceActionBar } from "./overview/EnhanceActionBar"
import { EnhanceWizard, type EnhanceStep } from "./overview/EnhanceWizard"
import { OverviewProviderLimits } from "./overview/OverviewProviderLimits"
import { OverviewRecentSessions } from "./overview/OverviewRecentSessions"
import { OverviewUsage, type OverviewMetric } from "./overview/OverviewUsage"
import { readOverviewViewPrefs, writeOverviewViewPrefs } from "./overview/overviewViewPrefs"

import "./overview/overview.css"

export function OverviewView({
  active,
  session,
  checks,
  navigationRevision,
  onOpenSessions,
  onSelectSession,
}: {
  active: boolean
  session: MainOverviewSession
  checks: BurnChecksSession
  navigationRevision: number
  onOpenSessions: () => void
  onSelectSession: (entry: SessionListEntry) => void
}) {
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const checksState = useSyncExternalStore(
    active ? checks.subscribe : checks.subscribeInactive,
    checks.getSnapshot,
    checks.getSnapshot,
  )
  const snoozes = useSnoozedBurnChecks()
  const failingChecks =
    checksState.report && snoozes.status === "ready"
      ? checksPresentation(checksState.report, false, snoozedDetectorIds(snoozes.records))
          .failures.length
      : null

  const [enhanceOpen, setEnhanceOpen] = useState(false)
  const [enhanceStep, setEnhanceStep] = useState<EnhanceStep>(
    () => readOverviewViewPrefs().enhanceStep ?? 1,
  )
  // Any sidebar or history move, including a click on Overview itself,
  // closes the wizard. The sidebar is the way back.
  const [seenRevision, setSeenRevision] = useState(navigationRevision)
  if (seenRevision !== navigationRevision) {
    setSeenRevision(navigationRevision)
    setEnhanceOpen(false)
  }
  function changeEnhanceStep(step: EnhanceStep) {
    setEnhanceStep(step)
    writeOverviewViewPrefs({ enhanceStep: step })
  }

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
        "grid grid-cols-[auto_minmax(0,1fr)_clamp(206px,21%,316px)_auto] grid-rows-[minmax(0,1fr)_auto] gap-x-(--space-2xl) gap-y-(--space-lg) pt-(--space-2xl) mb-(--space-2xl)",
      )}
      data-overview-active={active ? "" : undefined}
    >
      <h1 className="sr-only">Overview</h1>

      {enhanceOpen ? (
        <div className="col-[2/4] row-span-2 flex min-h-0 min-w-0">
          <EnhanceWizard
            step={enhanceStep}
            onStepChange={changeEnhanceStep}
            onFinish={() => setEnhanceOpen(false)}
          />
        </div>
      ) : (
        <>
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

          <div className="col-[2/4]">
            <EnhanceActionBar
              failingChecks={failingChecks}
              onOpen={() => setEnhanceOpen(true)}
            />
          </div>
        </>
      )}
    </div>
  )
}

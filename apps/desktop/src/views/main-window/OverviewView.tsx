import { useCallback, useState, useSyncExternalStore } from "react"

import { cn } from "../../lib/cn"
import { openBurnCheckSample, type BurnCheckDetectorId } from "../../lib/insightsIpc"

import type { SessionListEntry } from "../../components/session/SessionList"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { checksPresentation } from "../../lib/presentation/checks"
import { snoozedDetectorIds, useSnoozedBurnChecks } from "../../lib/snoozedBurnChecks"
import { type BurnChecksSession } from "./BurnChecksSession"
import { type MainOverviewSession } from "./MainOverviewSession"
import { EnhanceOrb } from "./overview/EnhanceBanner"
import { EnhanceWizard, type EnhanceStep } from "./overview/EnhanceWizard"
// import { OverviewConfigChecks } from "./overview/OverviewConfigChecks"
import { OverviewProviderLimits } from "./overview/OverviewProviderLimits"
// import { OverviewRecentSessions } from "./overview/OverviewRecentSessions"
import { OverviewUsage, type OverviewMetric } from "./overview/OverviewUsage"
import { SHOW_WEEK_FLOWER } from "./overview/weekChart"
import { enhanceButtonState } from "./overview/enhanceState"
import { pinnedDetectors, wasteMarks } from "./overview/wasteMarks"
import {
  readOverviewViewPrefs,
  writeOverviewViewPrefs,
  type OverviewViewPrefs,
} from "./overview/overviewViewPrefs"

import "./overview/overview.css"

// The wizard handlers read the clock through this function. The handlers run
// only on events, not during render.
const now = () => Date.now()

export function OverviewView({
  active,
  session,
  checks,
  navigationRevision,
  onOpenSessions: _onOpenSessions,
  onSelectSession: _onSelectSession,
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
  const checkGroups =
    checksState.report && snoozes.status === "ready"
      ? checksPresentation(checksState.report, false, snoozedDetectorIds(snoozes.records))
      : null
  const failing = checkGroups?.failures.map((check) => check.id) ?? null
  const waste = checkGroups
    ? {
        ...wasteMarks(checkGroups.failures, checksState.targets),
        onOpen: (pin: { navigationHandle: string }) =>
          void openBurnCheckSample(pin.navigationHandle),
      }
    : undefined
  const pinned = pinnedDetectors(checkGroups?.failures ?? [])

  const [enhanceOpen, setEnhanceOpen] = useState(false)
  const [enhancePrefs, setEnhancePrefs] = useState(readOverviewViewPrefs)
  const enhanceStep: EnhanceStep = enhancePrefs.enhanceStep ?? 1
  const buttonState = enhanceButtonState(
    failing,
    checkGroups?.awaiting?.length ?? 0,
    enhancePrefs,
  )
  function saveEnhancePrefs(partial: OverviewViewPrefs) {
    writeOverviewViewPrefs(partial)
    setEnhancePrefs((prefs) => ({ ...prefs, ...partial }))
  }
  // Any sidebar or history move, including a click on Overview itself,
  // closes the wizard. The sidebar is the way back.
  const [seenRevision, setSeenRevision] = useState(navigationRevision)
  if (seenRevision !== navigationRevision) {
    setSeenRevision(navigationRevision)
    setEnhanceOpen(false)
  }
  function openEnhance() {
    // A resume keeps the saved step. Pending fixes open on Watch. Every
    // other state starts a new run on the first step.
    const step: EnhanceStep =
      buttonState.kind === "resume" ? enhanceStep : buttonState.kind === "watching" ? 4 : 1
    saveEnhancePrefs({ enhanceStartedAt: now(), enhanceStep: step })
    setEnhanceOpen(true)
  }
  function finishEnhance() {
    saveEnhancePrefs({
      enhanceCompletedAt: now(),
      enhanceSeenFailing: failing ?? [],
      enhanceStep: 1,
    })
    setEnhanceOpen(false)
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
        "grid grid-cols-[auto_minmax(0,1fr)_clamp(206px,21%,316px)_auto] grid-rows-[minmax(0,1fr)] gap-x-(--space-2xl) gap-y-(--space-lg) pt-(--space-2xl) mb-(--space-2xl)",
      )}
      data-overview-active={active ? "" : undefined}
    >
      <h1 className="sr-only">Overview</h1>

      {enhanceOpen ? (
        <div className="col-[2/4] flex min-h-0 min-w-0">
          <EnhanceWizard
            step={enhanceStep}
            checks={checks}
            checksState={checksState}
            onStepChange={(step) => saveEnhancePrefs({ enhanceStep: step })}
            onFinish={finishEnhance}
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
                showFigures={false}
                center={
                  <EnhanceOrb
                    state={buttonState}
                    onOpen={openEnhance}
                    shape={SHOW_WEEK_FLOWER ? "round" : "bar"}
                  />
                }
                waste={waste}
                loading={loading}
              />
              {pinned.map((detector) => (
                <PinTargets key={detector} detector={detector} checks={checks} />
              ))}

              {/* The config checks and the Optimise card are off for now, while the
                  round Optimise button in the chart is tried alone.
                            <div className="relative flex min-h-64 flex-col">
                {checkGroups && (
                  <div inert>
                    <OverviewConfigChecks
                      failures={checkGroups.failures}
                      passing={checkGroups.wins}
                    />
                  </div>
                )}
                <div className="enhance-scrim absolute inset-0 flex flex-col rounded-(--radius-popover) p-(--space-md)">
                  <EnhanceBanner
                    failingLabels={
                      checkGroups?.failures.map((check) => CHECK_LABELS[check.id]) ?? null
                    }
                  />
                </div>
              </div>
              */}

              {/* Recent sessions is off for now, while the Optimise card is tried alone.
              <OverviewRecentSessions
                active={active && state.active}
                entries={state.recentSessions}
                loading={loading && !state.recentSessions}
                onSelect={_onSelectSession}
                onOpenAll={_onOpenSessions}
                metric={metric}
                liveUsage={state.liveUsage ?? undefined}
                sessionLimitAllocations={state.sessionLimitAllocations}
              />
              */}
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
        </>
      )}
    </div>
  )
}

/** Loads the sample sessions of one pinned check while the Overview chart
 *  shows, so its pins can go on the chart. */
function PinTargets({
  detector,
  checks,
}: {
  detector: BurnCheckDetectorId
  checks: BurnChecksSession
}) {
  const trackTargets = useCallback(
    (node: HTMLElement | null) => checks.setTargetsVisible(detector, node !== null, false),
    [checks, detector],
  )
  return <span ref={trackTargets} hidden />
}

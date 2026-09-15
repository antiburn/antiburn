import { useSyncExternalStore } from "react"

import { isMacOS } from "../../lib/platform"

import type { SessionListEntry } from "../../components/session/SessionList"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { type MainOverviewSession } from "./MainOverviewSession"
import { OverviewBurnChecks } from "./overview/OverviewBurnChecks"
import { OverviewProviderLimits } from "./overview/OverviewProviderLimits"
import { OverviewRecentSessions } from "./overview/OverviewRecentSessions"
import { OverviewSpendChart } from "./overview/OverviewSpendChart"
import { OverviewSpendTotals } from "./overview/OverviewSpendTotals"

import "./overview/overview.css"

/**
 * The main window's landing section: local spend totals, then one card
 * with Burn checks over recent sessions beside the provider limits card,
 * and the daily spend chart along the bottom, where it takes any height
 * the window has to spare. The Burn checks and Sessions panels are
 * summaries; their controls leave for the full sections.
 */
export function OverviewView({
  active,
  session,
  onOpenBurnChecks,
  onOpenSessions,
  onSelectSession,
}: {
  active: boolean
  session: MainOverviewSession
  onOpenBurnChecks: () => void
  onOpenSessions: () => void
  onSelectSession: (entry: SessionListEntry) => void
}) {
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const usage = state.usage
  const loading = !usage && !state.usageError
  return (
    <div
      className="flex min-h-0 min-w-0 flex-1 flex-col bg-surface-window"
      data-overview-active={active ? "" : undefined}
    >
      {isMacOS() && (
        <div
          className="h-[var(--main-window-titlebar-height)] shrink-0"
          data-tauri-drag-region
          aria-hidden="true"
        />
      )}
      <h1 className="sr-only">Overview</h1>
      {!usage && state.usageError ? (
        <div className="flex flex-1 items-center justify-center text-center">
          <div>
            <p role="alert" className="type-body text-label-secondary">
              Local usage is unavailable.
            </p>
            <button type="button" onClick={session.refresh} className="ui-push-button mt-3">
              Retry
            </button>
          </div>
        </div>
      ) : (
        <ScrollPane className="min-h-0" topEdgeFade>
          <div
            role="region"
            aria-label={loading ? "Loading Overview" : "Overview"}
            aria-busy={loading || undefined}
            className="overview-page flex w-full flex-col gap-[var(--space-xl)] px-8 py-6"
          >
            {loading && (
              <p role="status" className="sr-only">
                Loading Overview.
              </p>
            )}
            <OverviewSpendTotals totals={usage?.totals ?? null} loading={loading} />
            <div className="overview-panels">
              <div className="overview-stack p-[var(--space-lg)]">
                <OverviewBurnChecks
                  report={state.report}
                  loading={loading && !state.report}
                  onOpen={onOpenBurnChecks}
                />
                <OverviewRecentSessions
                  entries={state.recentSessions}
                  loading={loading && !state.recentSessions}
                  onSelect={onSelectSession}
                  onOpenAll={onOpenSessions}
                />
              </div>
              <OverviewProviderLimits
                live={state.liveUsage}
                loading={loading && !state.liveUsage}
              />
            </div>
            <OverviewSpendChart
              days={usage?.days ?? []}
              previousDays={usage?.previousDays ?? []}
              loading={loading}
            />
          </div>
        </ScrollPane>
      )}
    </div>
  )
}

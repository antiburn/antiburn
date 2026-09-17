import { useState, useSyncExternalStore } from "react"

import { isMacOS } from "../../lib/platform"

import type { SessionListEntry } from "../../components/session/SessionList"
import { ScrollPane } from "../../components/ui/ScrollPane"
import { type MainOverviewSession } from "./MainOverviewSession"
import { OverviewRecentSessions } from "./overview/OverviewRecentSessions"
import { OverviewUsage, type OverviewMetric } from "./overview/OverviewUsage"

import "./overview/overview.css"

/**
 * The main window's landing section: the usage block, which is the unit
 * control over the daily chart and the totals it summarizes, and then the
 * recent sessions card. The chart takes any height the window has to spare.
 * The Sessions panel is a summary; its control leaves for the full section.
 *
 * The page holds the unit. The chart and the totals both read it, so the
 * page can never show dollars in one place and allowance in another.
 *
 * The live provider limits are not here. They sit in the main window's
 * sidebar, where every section shows them.
 */
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
  // The page opens on the subscription, because the plan is the limit a
  // reader meets. A dollar estimate is the second question.
  const [metric, setMetric] = useState<OverviewMetric>("allowance")
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
          <OverviewUsage
            metric={metric}
            onMetricChange={setMetric}
            totals={usage?.totals ?? null}
            days={usage?.days ?? []}
            previousDays={usage?.previousDays ?? []}
            allowance={state.allowance}
            allowanceLoading={state.allowanceLoading}
            allowanceError={state.allowanceError}
            usageError={state.usageError}
            onRetryUsage={session.refresh}
            loading={loading}
          />
          <div className="overview-stack p-[var(--space-lg)]">
            <OverviewRecentSessions
              entries={state.recentSessions}
              loading={loading && !state.recentSessions}
              onSelect={onSelectSession}
              onOpenAll={onOpenSessions}
            />
          </div>
        </div>
      </ScrollPane>
    </div>
  )
}

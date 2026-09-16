import { useSyncExternalStore } from "react"

import { isMacOS } from "../../lib/platform"

import { ScrollPane } from "../../components/ui/ScrollPane"
import { Skeleton } from "../../components/ui/Skeleton"
import { type BurnChecksSession } from "./BurnChecksSession"
import { BurnChecksHeader } from "./burn-checks/BurnChecksHeader"
import { BurnChecksReport } from "./burn-checks/BurnChecksReport"

export function BurnChecksView({
  active,
  session,
}: {
  active: boolean
  session: BurnChecksSession
}) {
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const report = state.report
  if (!report)
    return (
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col bg-surface-window">
        {state.error && isMacOS() && (
          <div
            className="main-window-empty-titlebar"
            data-tauri-drag-region
            aria-hidden="true"
          />
        )}
        <h1 className="sr-only">Burn checks</h1>
        {state.error ? (
          <div className="flex flex-1 items-center justify-center text-center">
            <div>
              <p role="alert" className="type-body text-label-secondary">
                Burn checks are unavailable.
              </p>
              <button type="button" onClick={session.refresh} className="ui-push-button mt-3">
                Retry
              </button>
            </div>
          </div>
        ) : (
          <div
            role="region"
            aria-label="Loading Burn checks"
            aria-busy="true"
            className="burn-checks-page w-full"
          >
            <p role="status" className="sr-only">
              Loading Burn checks.
            </p>
            <div className="burn-checks-report">
              <div className="burn-checks-layout">
                <section
                  className="main-window-collection burn-checks-collection"
                  aria-label="Loading burn check collection"
                >
                  <BurnChecksHeader />
                  <ScrollPane
                    className="min-h-0"
                    viewportClassName="burn-checks-collection-scroll"
                  >
                    <div className="burn-checks-collection-content">
                      <Skeleton data-skeleton="group-label" className="h-6 w-28" />
                      <div className="burn-checks-group-body mt-3">
                        {["w-40", "w-32", "w-44"].map((width) => (
                          <div
                            key={width}
                            data-skeleton="check-row"
                            className="grid min-h-[70px] grid-cols-[32px_minmax(0,1fr)] items-center gap-x-3 rounded-[var(--radius-popover)] bg-session-card px-3 py-3"
                          >
                            <Skeleton
                              data-skeleton-slot="icon"
                              className="h-8 w-8 rounded-full justify-self-center"
                            />
                            <span className="flex min-w-0 flex-col gap-1.5">
                              <Skeleton data-skeleton-slot="title" className={`h-3 ${width}`} />
                              <Skeleton data-skeleton-slot="summary" className="h-3 w-24" />
                              <Skeleton data-skeleton-slot="metric" className="h-3 w-20" />
                            </span>
                          </div>
                        ))}
                      </div>
                    </div>
                  </ScrollPane>
                </section>
                <section
                  className="main-window-detail burn-checks-detail-pane"
                  aria-label="Loading burn check details"
                >
                  <ScrollPane className="min-h-0" viewportClassName="burn-checks-detail-scroll">
                    <div className="burn-checks-detail-content" data-skeleton="detail">
                      <Skeleton className="h-6 w-48 max-w-full" />
                      <Skeleton className="mt-2 h-4 w-32 max-w-full" />
                      <div className="mt-4 border-t border-separator pt-4">
                        <Skeleton className="h-4 w-80 max-w-full" />
                        <Skeleton className="mt-3 h-7 w-28" />
                      </div>
                    </div>
                  </ScrollPane>
                </section>
              </div>
            </div>
          </div>
        )}
      </div>
    )
  return (
    <div className="burn-checks-page w-full bg-surface-window">
      <h1 className="sr-only">Burn checks</h1>
      {state.error && (
        <p role="status" className="px-6 pt-2 type-callout text-system-red-text">
          Could not refresh Burn checks. Previous results remain visible.{" "}
          <button className="underline" onClick={session.refresh}>
            Retry
          </button>
        </p>
      )}
      <BurnChecksReport report={report} session={session} state={state} />
    </div>
  )
}

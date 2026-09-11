import { useSyncExternalStore } from "react"

import { ScrollPane } from "../../components/ui/ScrollPane"
import { Skeleton } from "../../components/ui/Skeleton"
import { type BurnChecksSession } from "./BurnChecksSession"
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
      <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-surface-window">
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
          <ScrollPane className="min-h-0" topEdgeFade>
            <div
              role="region"
              aria-label="Loading Burn checks"
              aria-busy="true"
              className="w-full px-8 py-6"
            >
              <p role="status" className="sr-only">
                Loading Burn checks.
              </p>
              <section
                data-skeleton="hero"
                className="grid grid-cols-[88px_minmax(0,1fr)] items-center gap-[var(--space-2xl)] py-[calc(var(--space-lg)*2)]"
              >
                <div className="contents">
                  <Skeleton
                    data-skeleton-slot="icon"
                    className="h-[88px] w-[88px] rounded-full"
                  />
                  <div className="flex min-h-[88px] min-w-0 flex-col justify-between">
                    <Skeleton className="h-4 w-24" />
                    <Skeleton data-skeleton-slot="result" className="h-9 w-40" />
                    <Skeleton data-skeleton-slot="summary" className="h-4 w-72 max-w-full" />
                    <Skeleton className="h-4 w-24" />
                  </div>
                </div>
              </section>
              <div className="mt-8 px-1">
                <Skeleton data-skeleton="group-label" className="h-6 w-28" />
              </div>
              <div className="mt-3 overflow-hidden rounded-control border border-separator/40 bg-surface-card/50">
                {["w-40", "w-32", "w-44"].map((width) => (
                  <div
                    key={width}
                    data-skeleton="check-row"
                    className="grid grid-cols-[28px_minmax(0,1fr)_max-content_14px] items-center gap-x-3 border-b border-separator px-4 py-3 last:border-b-0"
                  >
                    <Skeleton
                      data-skeleton-slot="icon"
                      className="h-4 w-4 justify-self-center"
                    />
                    <span className="min-w-0 space-y-1.5">
                      <Skeleton data-skeleton-slot="title" className={`h-3 ${width}`} />
                      <Skeleton data-skeleton-slot="summary" className="h-3 w-24" />
                    </span>
                    <Skeleton data-skeleton-slot="metric" className="h-3 w-20" />
                    <Skeleton data-skeleton-slot="disclosure" className="h-3.5 w-3.5" />
                  </div>
                ))}
              </div>
            </div>
          </ScrollPane>
        )}
      </div>
    )
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col bg-surface-window">
      <ScrollPane className="min-h-0" topEdgeFade>
        <div className="w-full px-8 py-6">
          <h1 className="sr-only">Burn checks</h1>
          {state.error && (
            <p role="status" className="mb-3 type-callout text-system-red-text">
              Could not refresh Burn checks. Previous results remain visible.{" "}
              <button className="underline" onClick={session.refresh}>
                Retry
              </button>
            </p>
          )}
          <BurnChecksReport report={report} session={session} state={state} />
        </div>
      </ScrollPane>
    </div>
  )
}

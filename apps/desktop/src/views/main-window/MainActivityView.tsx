import { lazy, Suspense, useRef, useState, useSyncExternalStore } from "react"
import { CalendarDays } from "lucide-react"

import { SessionList } from "../../components/session/SessionList"
import { PushButton } from "../../components/ui/PushButton"
import { openSettingsWindow } from "../../lib/ipc"
import type { SessionQuotaOpenTarget } from "../../components/session/SessionQuotaSection"
import { renderAgentIcon } from "../../lib/agentIcon"
import { filterSessionEntries, sessionFilterCounts } from "../../lib/sessionFilters"
import { agentDisplayName } from "../../lib/presentation/agents"
import { costOutlierThreshold } from "../../lib/presentation/sessionAnalysis"
import { sessionKey, type SessionSubject } from "../../lib/sessionSubject"
import { snoozedDetectorIds, useSnoozedBurnChecks } from "../../lib/snoozedBurnChecks"
import type { SessionHygieneSnapshot } from "../../lib/useSessionHygiene"
import { SessionEmptyDetail } from "./SessionEmptyDetail"
import { SessionFiltersHeader } from "./SessionFiltersHeader"
import { CollectionDetailPane, type CollectionItem } from "./CollectionDetailPane"
import {
  type MainActivitySession,
  orderedActivityEntries,
  subjectForEntry,
} from "./MainActivitySession"

const SessionPane = lazy(() =>
  import("../popover/SessionPane").then((module) => ({ default: module.SessionPane })),
)

interface SessionItem extends CollectionItem {
  subject: SessionSubject
}
function itemForSubject(subject: SessionSubject): SessionItem {
  return {
    id: sessionKey(subject),
    label: subject.title || `Session ${subject.sessionId.slice(0, 7)}`,
    subject,
  }
}

export function MainActivityView({
  active,
  session,
  hygieneBySession,
  onOpenQuota,
}: {
  active: boolean
  session: MainActivitySession
  /** Fetched once above this view, pinned to the full unfiltered list. */
  hygieneBySession: SessionHygieneSnapshot
  /** Open one quota window on the Quota screen. Omitted where there is no
   *  Quota screen to open. */
  onOpenQuota?: (target: SessionQuotaOpenTarget) => void
}) {
  const [rangeError, setRangeError] = useState(false)
  const filterButtonRef = useRef<HTMLButtonElement | null>(null)
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const snoozes = useSnoozedBurnChecks()
  const snoozedDetectors = snoozedDetectorIds(snoozes.records)
  const calendarEntries = orderedActivityEntries(state)
  const highCostThresholdUsd = costOutlierThreshold(
    calendarEntries.flatMap((entry) => (entry.cost ? [entry.cost.totalUsd] : [])),
  )
  const eligibleEntries = calendarEntries.map((entry) => {
    if (!entry.cost) return entry
    const isHighCost =
      highCostThresholdUsd != null && entry.cost.totalUsd > highCostThresholdUsd
    return entry.cost.isHighCost === isHighCost
      ? entry
      : { ...entry, cost: { ...entry.cost, isHighCost } }
  })
  const filteredEntries =
    snoozes.status === "ready"
      ? filterSessionEntries(eligibleEntries, hygieneBySession, state.filters, snoozedDetectors)
      : []
  const counts = sessionFilterCounts(
    eligibleEntries,
    hygieneBySession,
    state.filters,
    snoozedDetectors,
  )
  const agents = Object.keys(counts.agents).sort((left, right) =>
    agentDisplayName(left).localeCompare(agentDisplayName(right)),
  )
  const ordered = filteredEntries.filter((entry) => entry.sessionId)
  const items = ordered.map((entry) => itemForSubject(subjectForEntry(entry)))
  const selected = state.subject ? itemForSubject(state.subject) : null
  const index = selected ? items.findIndex((item) => item.id === selected.id) : -1
  const previous = index > 0 ? ordered[index - 1] : undefined
  const next = index >= 0 ? ordered[index + 1] : undefined
  const filterEmpty = eligibleEntries.length > 0 && filteredEntries.length === 0
  const days = state.settings.activityWindowDays

  async function changeTimeRange() {
    setRangeError(false)
    try {
      await openSettingsWindow("general", "recentDays")
    } catch {
      setRangeError(true)
    }
  }

  function clearFilters() {
    session.clearFilters()
    filterButtonRef.current?.focus()
  }

  return (
    <CollectionDetailPane<SessionItem>
      title="Sessions"
      items={items}
      selection={selected}
      externalDetailRevealRevision={state.detailRevealRevision}
      onSelectionChange={(item) =>
        session.selectEntry({
          ...item.subject,
          timestamp: item.subject.timestamp ?? "",
          repo: item.subject.repo ?? "",
          isActive: false,
        })
      }
      emptyMessage="No sessions yet"
      detailEmptyMessage="Select a session to view its details."
      detailEmptyContent={<SessionEmptyDetail />}
      detailOwnsViewport
      renderCollection={({ openDetail, compact }) => (
        <div className="session-filter-collection flex min-h-0 min-w-0 flex-1 flex-col">
          {state.entries !== null && snoozes.status === "ready" && (
            <SessionFiltersHeader
              filters={state.filters}
              counts={counts}
              agents={agents}
              onToggleAgent={session.toggleAgent}
              onResetAgents={session.resetAgents}
              onResultChange={session.setResultFilter}
              onSpendChange={session.setSpendFilter}
              onClear={session.clearFilters}
              highCostThresholdUsd={highCostThresholdUsd ?? undefined}
              days={days}
              onChangeTimeRange={() => void changeTimeRange()}
              triggerRef={filterButtonRef}
            />
          )}
          {rangeError && (
            <p role="status" className="px-4 py-2 type-callout text-label-secondary">
              Could not open time-range settings. Try again.
            </p>
          )}
          {state.listError && (
            <div role="status" className="px-4 py-2 type-callout text-label-secondary">
              Could not load sessions.{" "}
              <button className="text-label underline" onClick={session.refreshList}>
                Retry
              </button>
            </div>
          )}
          {state.settingsError && (
            <p role="status" className="px-4 py-2 type-callout text-label-secondary">
              Could not save the session preferences. Try again.
            </p>
          )}
          {snoozes.status === "error" ? (
            <p role="status" className="px-4 py-2 type-body text-label-secondary">
              Sessions are unavailable.
            </p>
          ) : state.entries === null || snoozes.status === "loading" ? (
            <p role="status" className="px-4 py-2 type-body text-label-secondary">
              {state.listError ? "Sessions are unavailable." : "Loading sessions…"}
            </p>
          ) : (
            <SessionList
              entries={filteredEntries}
              toolbarTopPadding="space-sm"
              hideEmptyToolbar
              {...(filterEmpty
                ? {
                    emptyTitle: "No matching sessions",
                    emptyDescription: "",
                    emptyIcon: null,
                    emptyActions: <PushButton onClick={clearFilters}>Clear filters</PushButton>,
                  }
                : {
                    emptyDescription:
                      "Try a wider time range. New sessions appear here as they are discovered on this machine.",
                    emptyIcon: <CalendarDays size={20} strokeWidth={1.75} aria-hidden="true" />,
                    emptyActions: (
                      <PushButton onClick={() => void changeTimeRange()}>
                        Change time range
                      </PushButton>
                    ),
                  })}
              draggableHeader={false}
              days={state.settings.activityWindowDays}
              selectedKey={selected?.id ?? null}
              onSelect={session.selectEntry}
              openOnClick={compact}
              onOpenDetail={(entry) => {
                if (entry.sessionId) openDetail(itemForSubject(subjectForEntry(entry)))
              }}
              active={state.active}
              renderAgentIcon={renderAgentIcon}
              now={new Date(state.now)}
              badgeMetric={state.settings.sessionBadgeMetric}
              onBadgeMetricChange={(metric) => void session.setBadgeMetric(metric)}
              liveUsage={state.liveUsage}
              sessionLimitAllocations={state.allocations}
              hygieneBySession={hygieneBySession}
              snoozedDetectors={snoozedDetectors}
            />
          )}
        </div>
      )}
      renderDetail={(item) => (
        <Suspense
          fallback={
            <p role="status" className="px-4 py-2 type-body text-label-secondary">
              Loading session…
            </p>
          }
        >
          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            {state.analysis?.error && (
              <div role="status" className="px-4 py-2 type-callout text-label-secondary">
                Could not refresh this session.{" "}
                <button className="text-label underline" onClick={session.refreshAnalysis}>
                  Retry
                </button>
              </div>
            )}
            <SessionPane
              embedded
              active={state.active}
              subject={item.subject}
              payload={state.analysis?.key === item.id ? state.analysis.payload : null}
              loading={state.loading}
              refreshing={state.refreshing}
              error={state.analysis?.error ?? false}
              {...(previous ? { onPrev: () => session.selectEntry(previous) } : {})}
              {...(next ? { onNext: () => session.selectEntry(next) } : {})}
              onOpenSession={session.openRelated}
              sessionQuota={state.sessionQuota}
              {...(onOpenQuota ? { onOpenQuota } : {})}
              onDeleted={session.deleted}
            />
          </div>
        </Suspense>
      )}
    />
  )
}

import { lazy, Suspense, useSyncExternalStore } from "react"

import { SessionList } from "../../components/session/SessionList"
import { renderAgentIcon } from "../../lib/agentIcon"
import { filterSessionEntries } from "../../lib/sessionFilters"
import { sessionKey, type SessionSubject } from "../../lib/sessionSubject"
import { isMacOS } from "../../lib/platform"
import type { SessionHygieneSnapshot } from "../../lib/useSessionHygiene"
import { SessionEmptyDetail } from "./SessionEmptyDetail"
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
}: {
  active: boolean
  session: MainActivitySession
  /** Fetched once above this view, pinned to the full unfiltered list. */
  hygieneBySession: SessionHygieneSnapshot
}) {
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const filteredEntries = filterSessionEntries(
    state.entries ?? [],
    hygieneBySession,
    state.filter,
  )
  const ordered = orderedActivityEntries({ ...state, entries: filteredEntries }).filter(
    (entry) => entry.sessionId,
  )
  const items = ordered
    .filter((entry) => entry.sessionId)
    .map((entry) => itemForSubject(subjectForEntry(entry)))
  const selected = state.subject ? itemForSubject(state.subject) : null
  const index = selected ? items.findIndex((item) => item.id === selected.id) : -1
  const previous = index > 0 ? ordered[index - 1] : undefined
  const next = index >= 0 ? ordered[index + 1] : undefined
  // A non-empty list can filter down to nothing. The day-window empty copy
  // would be misleading here, so this state gets its own short message.
  const filterEmpty = (state.entries?.length ?? 0) > 0 && filteredEntries.length === 0

  return (
    <CollectionDetailPane<SessionItem>
      title="Sessions"
      items={items}
      selection={selected}
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
      renderCollection={({ openDetail }) => (
        <div className="flex min-h-0 min-w-0 flex-1 flex-col">
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
              Could not save the badge preference. Try again.
            </p>
          )}
          {state.entries === null ? (
            <p role="status" className="px-4 py-2 type-body text-label-secondary">
              {state.listError ? "Sessions are unavailable." : "Loading sessions…"}
            </p>
          ) : (
            <SessionList
              entries={filteredEntries}
              {...(filterEmpty
                ? {
                    emptyTitle: "No sessions match this filter.",
                    emptyDescription: "Choose a different filter to see more sessions.",
                  }
                : {})}
              draggableHeader={isMacOS()}
              days={state.settings.activityWindowDays}
              selectedKey={selected?.id ?? null}
              onSelect={session.selectEntry}
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
              {...(state.history.length ? { onBack: session.goBack } : {})}
              {...(previous ? { onPrev: () => session.selectEntry(previous) } : {})}
              {...(next ? { onNext: () => session.selectEntry(next) } : {})}
              onOpenSession={session.openRelated}
              onDeleted={session.deleted}
            />
          </div>
        </Suspense>
      )}
    />
  )
}

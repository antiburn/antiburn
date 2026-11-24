import { lazy, Suspense, useRef, useState, useSyncExternalStore } from "react"
import { CalendarDays } from "lucide-react"

import { SessionList, type SessionListEntry } from "../../components/session/SessionList"
import { PushButton } from "../../components/ui/PushButton"
import type { SessionQuotaOpenTarget } from "../../components/session/SessionQuotaSection"
import { renderAgentIcon } from "../../lib/agentIcon"
import {
  filterSessionEntries,
  serializeSessionFilters,
  sessionFilterCounts,
} from "../../lib/sessionFilters"
import type { LocalSessionIdentity } from "../../lib/types/session"
import { agentDisplayName } from "../../lib/presentation/agents"
import type { SessionHygienePayload } from "../../lib/insightsIpc"
import { costOutlierThreshold } from "../../lib/presentation/sessionAnalysis"
import { sessionKey, type SessionSubject } from "../../lib/sessionSubject"
import {
  getSnoozedBurnChecksSnapshot,
  snoozedDetectorIds,
  useSnoozedBurnChecks,
} from "../../lib/snoozedBurnChecks"
import {
  sessionHygieneIdentities,
  type SessionHygieneResource,
} from "../../lib/useSessionHygiene"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { SessionEmptyDetail } from "./SessionEmptyDetail"
import { SessionFiltersHeader } from "./SessionFiltersHeader"
import { CollectionDetailPane, type CollectionItem } from "./CollectionDetailPane"
import {
  type MainActivitySession,
  type MainActivitySnapshot,
  orderedActivityEntries,
  subjectForEntry,
} from "./MainActivitySession"

const SessionPane = lazy(() =>
  import("../popover/SessionPane").then((module) => ({ default: module.SessionPane })),
)
const appendErrorMessage = "Could not load more sessions. Press Right Arrow to retry."
const linkedSessionSearchLimit = 3

interface SessionItem extends CollectionItem {
  subject: SessionSubject
}
interface AdjacentScope {
  subject: SessionSubject
  key: string
}
function itemForSubject(subject: SessionSubject): SessionItem {
  return {
    id: sessionKey(subject),
    label: subject.title || `Session ${subject.sessionId.slice(0, 7)}`,
    subject,
  }
}

function activityEntriesWithSpend(state: MainActivitySnapshot) {
  const calendarEntries = orderedActivityEntries(state)
  const highCostThresholdUsd = costOutlierThreshold(
    calendarEntries.flatMap((entry) => (entry.cost ? [entry.cost.totalUsd] : [])),
  )
  const entries = calendarEntries.map((entry) => {
    if (!entry.cost) return entry
    const isHighCost =
      highCostThresholdUsd != null && entry.cost.totalUsd > highCostThresholdUsd
    return entry.cost.isHighCost === isHighCost
      ? entry
      : { ...entry, cost: { ...entry.cost, isHighCost } }
  })
  return { entries, highCostThresholdUsd }
}

function hygieneKey(identity: LocalSessionIdentity | SessionListEntry): string {
  return localSessionKey(
    identity.agent,
    identity.sessionId ?? "",
    identity.wslDistro,
    identity.remoteHostId,
  )
}

function hasUnresolvedCheckEvidence(state: SessionHygienePayload["evidenceState"] | undefined) {
  return state === "pending" || state === "processing" || state === "stale"
}

function adjacentScopeKey(state: MainActivitySnapshot) {
  return JSON.stringify([
    serializeSessionFilters(state.filters),
    state.settings.activityWindowDays,
    state.settings.disabledAgents,
  ])
}

export function MainActivityView({
  active,
  session,
  sessionHygiene,
  onOpenQuota,
  onOpenMemory,
  onOpenRangeSettings,
}: {
  active: boolean
  session: MainActivitySession
  /** Fetched once above this view, pinned to the full unfiltered list. */
  sessionHygiene: SessionHygieneResource
  /** Open one quota window on the Quota screen. Omitted where there is no
   *  Quota screen to open. */
  onOpenQuota?: (target: SessionQuotaOpenTarget) => void
  onOpenMemory?: (target: { slug: string; path: string }) => void
  /** Open Settings → Sessions on the "Show the last" row. Omitted where
   *  the host cannot open Settings. */
  onOpenRangeSettings?: () => void
}) {
  const filterButtonRef = useRef<HTMLButtonElement | null>(null)
  const advancing = useRef<AbortController | null>(null)
  const [adjacentStatus, setAdjacentStatus] = useState<
    (AdjacentScope & { message: string; untilFiltersReady?: true }) | null
  >(null)
  const [checkRetry, setCheckRetry] = useState<
    (AdjacentScope & { append?: Promise<void>; loadMore?: true; direction?: "previous" }) | null
  >(null)
  const state = useSyncExternalStore(
    active ? session.subscribe : session.subscribeInactive,
    session.getSnapshot,
    session.getSnapshot,
  )
  const snoozes = useSnoozedBurnChecks()
  const snoozedDetectors = snoozedDetectorIds(snoozes.records)
  const hygieneBySession = sessionHygiene.snapshot
  const { entries: eligibleEntries, highCostThresholdUsd } = activityEntriesWithSpend(state)
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
  const agents = [...new Set([...state.availableAgents, ...Object.keys(counts.agents)])].sort(
    (left, right) => agentDisplayName(left).localeCompare(agentDisplayName(right)),
  )
  const resultEvidenceStates =
    state.filters.result === "all"
      ? []
      : sessionHygieneIdentities(
          filterSessionEntries(eligibleEntries, hygieneBySession, {
            ...state.filters,
            result: "all",
          }),
        ).map((identity) => hygieneBySession.get(hygieneKey(identity))?.evidenceState)
  const awaitingResultRead = resultEvidenceStates.includes(undefined)
  const awaitingResultFilter =
    awaitingResultRead || resultEvidenceStates.some(hasUnresolvedCheckEvidence)
  const ordered = filteredEntries.filter((entry) => entry.sessionId)
  const items = ordered.map((entry) => itemForSubject(subjectForEntry(entry)))
  const selected = state.subject ? itemForSubject(state.subject) : null
  const loadedIndex = selected
    ? eligibleEntries.findIndex((entry) => sessionKey(subjectForEntry(entry)) === selected.id)
    : -1
  const index = selected ? items.findIndex((item) => item.id === selected.id) : -1
  const previous = index > 0 ? ordered[index - 1] : undefined
  const next = index >= 0 ? ordered[index + 1] : undefined
  const checkResults = state.filters.result !== "all"
  const canLocateSelection =
    selected !== null &&
    !state.subject?.subagent &&
    state.hasMoreSessions &&
    loadedIndex < 0 &&
    !state.entries?.some((entry) => sessionKey(subjectForEntry(entry)) === selected.id)
  const scopeKey = adjacentScopeKey(state)
  const retryChecks = checkRetry?.subject === state.subject && checkRetry.key === scopeKey
  const retryPreviousChecks = retryChecks && checkRetry.direction === "previous"
  const detailStatus =
    adjacentStatus?.subject === state.subject &&
    adjacentStatus.key === scopeKey &&
    (!adjacentStatus.untilFiltersReady || snoozes.status !== "ready")
      ? adjacentStatus.message
      : retryChecks
        ? retryPreviousChecks
          ? "Press Left Arrow to resume checking previous sessions."
          : "Press Right Arrow to resume checking older sessions."
        : canLocateSelection
          ? "Press Left or Right Arrow to find this session in the list."
          : null
  const filterEmpty = eligibleEntries.length > 0 && filteredEntries.length === 0
  const days = state.settings.activityWindowDays

  function clearFilters() {
    session.clearFilters()
    filterButtonRef.current?.focus()
  }

  function cancelAdjacentNavigation() {
    advancing.current?.abort()
    advancing.current = null
    setAdjacentStatus(null)
  }

  async function navigateAdjacent(direction: "previous" | "next", loadMore = false) {
    const origin = session.getSnapshot()
    const subject = origin.subject
    if (advancing.current || !subject || subject.subagent || !origin.active) return
    const request = new AbortController()
    const scope = { subject, key: adjacentScopeKey(origin) }
    const backward = direction === "previous"
    const retryArrow = backward ? "Left Arrow" : "Right Arrow"
    const subjectId = sessionKey(subject)
    const mustLocate = !origin.entries?.some(
      (entry) => sessionKey(subjectForEntry(entry)) === subjectId,
    )
    const forwardRetry =
      !backward && !loadMore && checkRetry?.subject === subject && checkRetry.key === scope.key
        ? checkRetry
        : null
    const resumedAppend = forwardRetry?.append
    const current = () => {
      const snapshot = session.getSnapshot()
      return !request.signal.aborted &&
        snapshot.active &&
        snapshot.subject === subject &&
        adjacentScopeKey(snapshot) === scope.key
        ? snapshot
        : null
    }
    const adjacentEntries = (snapshot: MainActivitySnapshot) => {
      const { entries } = activityEntriesWithSpend(snapshot)
      const index = entries.findIndex(
        (entry) => sessionKey(subjectForEntry(entry)) === subjectId,
      )
      if (index < 0) return []
      const candidates = backward ? entries.slice(0, index).reverse() : entries.slice(index + 1)
      return candidates.filter((entry) => entry.sessionId)
    }
    const readSnoozes = () => {
      const snapshot = getSnoozedBurnChecksSnapshot()
      if (snapshot.status === "ready") return snoozedDetectorIds(snapshot.records)
      setAdjacentStatus({
        ...scope,
        message:
          snapshot.status === "loading"
            ? "Loading session filters…"
            : "Sessions are unavailable.",
        untilFiltersReady: true,
      })
      return null
    }

    if (!readSnoozes()) return
    advancing.current = request
    // Stop waiting for this selection as soon as it changes. A shared list
    // append can still complete for the new selection or the Sessions list.
    const stopWatching = session.subscribeInactive(() => {
      if (current()) return
      if (advancing.current === request) {
        advancing.current = null
        setAdjacentStatus(null)
      }
      request.abort()
    })
    request.signal.addEventListener("abort", stopWatching, { once: true })
    try {
      if (mustLocate) {
        setAdjacentStatus({ ...scope, message: "Loading more sessions…" })
        let latest = origin
        for (
          let reads = 0;
          latest.hasMoreSessions && reads < linkedSessionSearchLimit;
          reads++
        ) {
          await session.loadMoreSessions()
          const snapshot = current()
          if (!snapshot) return
          latest = snapshot
          if (latest.listError || latest.loadMoreSessionsError) {
            setAdjacentStatus({
              ...scope,
              message: "Could not load more sessions. Press Left or Right Arrow to retry.",
            })
            return
          }
          const { entries } = activityEntriesWithSpend(latest)
          const position = entries.findIndex(
            (entry) => sessionKey(subjectForEntry(entry)) === subjectId,
          )
          if (position < 0) continue
          if (backward) break
          // If the target is on the tail, continue through the next batch.
          if (position < entries.length - 1 || !latest.hasMoreSessions) break
        }
        if (
          !latest.entries?.some((entry) => sessionKey(subjectForEntry(entry)) === subjectId)
        ) {
          setAdjacentStatus({
            ...scope,
            message: latest.hasMoreSessions
              ? "Press Left or Right Arrow to keep looking for this session."
              : "No more sessions to load.",
          })
          return
        }
      }
      const retryFailedAppend =
        resumedAppend &&
        !origin.loadingMoreSessions &&
        (origin.loadMoreSessionsError || origin.listError)
      let append = retryFailedAppend ? undefined : resumedAppend
      // Use at most one history batch for this key press.
      let canAppend = loadMore || resumedAppend !== undefined || forwardRetry?.loadMore === true
      const checked = new Map<
        string,
        { badge: SessionHygienePayload | undefined; count: number }
      >()
      const failedReads = new Set<string>()
      while (true) {
        const latest = current()
        if (!latest) return
        const hygiene = sessionHygiene.getSnapshot()
        const currentSnoozes = readSnoozes()
        if (!currentSnoozes) return
        const candidates = filterSessionEntries(
          adjacentEntries(latest),
          hygiene,
          { ...latest.filters, result: "all" },
          currentSnoozes,
        )
        const match =
          latest.filters.result === "all"
            ? candidates[0]
            : filterSessionEntries(candidates, hygiene, latest.filters, currentSnoozes).find(
                (entry) =>
                  !hasUnresolvedCheckEvidence(hygiene.get(hygieneKey(entry))?.evidenceState),
              )
        const beforeMatch = match ? candidates.slice(0, candidates.indexOf(match)) : candidates
        const missing =
          latest.filters.result === "all"
            ? []
            : sessionHygieneIdentities(beforeMatch).filter((identity) => {
                const key = hygieneKey(identity)
                const badge = hygiene.get(key)
                return (
                  !badge ||
                  hasUnresolvedCheckEvidence(badge.evidenceState) ||
                  (badge.evidenceState === "failed" && checked.get(key)?.badge !== badge)
                )
              })
        if (!missing.length) {
          if (!match && canAppend) {
            canAppend = false
            setAdjacentStatus({ ...scope, message: "Loading more sessions…" })
            append ??= session.loadMoreSessions()
            // Keep the append available if the reader closes compact detail.
            if (latest.filters.result !== "all") setCheckRetry({ ...scope, append })
            await append
            continue
          }
          setCheckRetry((retry) => (backward && retry?.append ? retry : null))
          if (match) {
            setAdjacentStatus(null)
            session.selectEntry(match)
          } else {
            setAdjacentStatus({
              ...scope,
              message: backward
                ? "No previous matching sessions."
                : latest.loadMoreSessionsError || latest.listError
                  ? appendErrorMessage
                  : !latest.hasMoreSessions
                    ? "No more matching sessions."
                    : "No matches in this batch. Press Right Arrow to keep looking.",
            })
          }
          return
        }
        if (
          missing.some((identity) => {
            const key = hygieneKey(identity)
            return failedReads.has(key) || (checked.get(key)?.count ?? 0) >= 2
          })
        ) {
          const appendFailed = latest.loadMoreSessionsError || latest.listError
          if (appendFailed && !backward) setCheckRetry(null)
          setAdjacentStatus({
            ...scope,
            message:
              appendFailed && !backward
                ? appendErrorMessage
                : `Could not load checks for more sessions. Press ${retryArrow} to retry.`,
          })
          return
        }
        // A check read can return evidence that is still pending.
        if (
          missing.some((identity) => {
            const key = hygieneKey(identity)
            return (
              checked.has(key) && hasUnresolvedCheckEvidence(hygiene.get(key)?.evidenceState)
            )
          })
        ) {
          setAdjacentStatus({
            ...scope,
            message: `Check results are still pending. Press ${retryArrow} to retry.`,
          })
          return
        }
        setAdjacentStatus({ ...scope, message: "Loading checks for more sessions…" })
        const appendRetry = append ? { append } : { loadMore: true as const }
        setCheckRetry(
          backward
            ? { ...scope, direction: "previous" }
            : { ...scope, ...(canAppend ? appendRetry : {}) },
        )
        const resolved = await sessionHygiene.resolve(missing, request.signal, () => {
          const snapshot = current()
          return snapshot ? sessionHygieneIdentities(snapshot.entries ?? []) : null
        })
        if (!current()) return
        const resolvedHygiene = sessionHygiene.getSnapshot()
        for (const identity of missing) {
          const key = hygieneKey(identity)
          if (resolved)
            checked.set(key, {
              badge: resolvedHygiene.get(key),
              count: (checked.get(key)?.count ?? 0) + 1,
            })
          else failedReads.add(key)
        }
      }
    } finally {
      request.signal.removeEventListener("abort", stopWatching)
      if (!request.signal.aborted) stopWatching()
      if (advancing.current === request) {
        advancing.current = null
        if (!current()) setAdjacentStatus(null)
      }
    }
  }

  return (
    <CollectionDetailPane<SessionItem>
      title="Sessions"
      items={items}
      selection={selected}
      {...(canLocateSelection ? { outsideSelectionMessage: null } : {})}
      externalDetailRevealRevision={state.detailRevealRevision}
      onDetailDismiss={cancelAdjacentNavigation}
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
      renderCollection={({ openDetail, compact, visible }) => (
        <div className="session-filter-collection flex min-h-0 min-w-0 flex-1 flex-col">
          {state.entries !== null && snoozes.status === "ready" && (
            <SessionFiltersHeader
              filters={state.filters}
              counts={counts}
              sessionTotal={state.sessionTotal}
              hasMoreSessions={state.hasMoreSessions}
              awaitingResultFilter={awaitingResultFilter}
              agents={agents}
              onToggleAgent={session.toggleAgent}
              onResetAgents={session.resetAgents}
              onResultChange={session.setResultFilter}
              onSpendChange={session.setSpendFilter}
              onClear={session.clearFilters}
              highCostThresholdUsd={highCostThresholdUsd ?? undefined}
              days={days}
              onChangeTimeRange={() => onOpenRangeSettings?.()}
              triggerRef={filterButtonRef}
            />
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
              {...(state.hasMoreSessions
                ? {
                    continuation: {
                      loading: state.loadingMoreSessions,
                      autoLoad: !awaitingResultRead,
                      filterKey: serializeSessionFilters(state.filters),
                      failed: state.loadMoreSessionsError,
                      onLoadMore: session.loadMoreSessions,
                    },
                  }
                : {})}
              toolbarTopPadding="space-sm"
              hideEmptyToolbar
              {...(filterEmpty
                ? {
                    emptyTitle: awaitingResultFilter
                      ? "No matches yet"
                      : "No matching sessions",
                    emptyDescription: awaitingResultFilter
                      ? "Some sessions still need check results."
                      : "",
                    emptyIcon: null,
                    emptyActions: <PushButton onClick={clearFilters}>Clear filters</PushButton>,
                  }
                : {
                    emptyDescription:
                      "Try a wider time range. New sessions appear here as they are discovered on this machine.",
                    emptyIcon: <CalendarDays size={20} strokeWidth={1.75} aria-hidden="true" />,
                    emptyActions: (
                      <PushButton onClick={() => onOpenRangeSettings?.()}>
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
              active={state.active && visible}
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
            {detailStatus && (
              <p role="status" className="px-4 py-2 type-callout text-label-secondary">
                {detailStatus}
              </p>
            )}
            <SessionPane
              embedded
              active={state.active}
              subject={item.subject}
              payload={state.analysis?.key === item.id ? state.analysis.payload : null}
              loading={state.loading}
              refreshing={state.refreshing}
              error={state.analysis?.error ?? false}
              {...(retryPreviousChecks ||
              (!previous && canLocateSelection) ||
              (checkResults && loadedIndex > 0)
                ? { onPrev: () => void navigateAdjacent("previous") }
                : previous
                  ? { onPrev: () => session.selectEntry(previous) }
                  : {})}
              {...(retryChecks && !retryPreviousChecks
                ? { onNext: () => void navigateAdjacent("next") }
                : next && !checkResults
                  ? { onNext: () => session.selectEntry(next) }
                  : canLocateSelection
                    ? { onNext: () => void navigateAdjacent("next") }
                    : next || (loadedIndex >= 0 && state.hasMoreSessions)
                      ? { onNext: () => void navigateAdjacent("next", !next) }
                      : checkResults &&
                          loadedIndex >= 0 &&
                          loadedIndex < eligibleEntries.length - 1
                        ? { onNext: () => void navigateAdjacent("next") }
                        : {})}
              onOpenSession={session.openRelated}
              sessionQuota={state.sessionQuota}
              {...(onOpenQuota ? { onOpenQuota } : {})}
              sessionMemories={state.sessionMemories}
              {...(onOpenMemory ? { onOpenMemory } : {})}
              onDeleted={session.deleted}
            />
          </div>
        </Suspense>
      )}
    />
  )
}

import type { SessionListEntry } from "../../components/session/SessionList"
import { indexOfSession, toActivityEntries, toActivityEntry } from "../../lib/activityEntries"
import { applyTheme } from "../../lib/appearance"
import type { AttentionKind } from "../../lib/attention"
import {
  DEFAULT_SETTINGS,
  EMPTY_LIVE_USAGE,
  EMPTY_PROVIDER_USAGE,
  EMPTY_SESSION_LIMIT_ALLOCATIONS,
  appInfo,
  getLiveUsage,
  getProviderUsage,
  getSessionLimitAllocations,
  getSessionAnalysis,
  getSettings,
  getStorageHealth,
  getSubagentAnalysis,
  HEALTHY_STORAGE,
  hidePopover,
  listRecentSessions,
  listRepositories,
  onPopoverHidden,
  onPopoverShown,
  onLiveUsageChanged,
  onScanEvent,
  onSessionEntryChanged,
  onSessionsInvalidated,
  onSettingsChanged,
  onStorageHealth,
  openSettingsWindow,
  popoverContentReady,
  refreshLiveUsage,
  scanNow,
  setPopoverHeight,
  setSettings,
  type ActivityEntryPayload,
  type AppSettings,
  type LiveUsageSummaryPayload,
  type ProviderUsageSummaryPayload,
  type SessionLimitAllocationSummaryPayload,
  type SessionAnalysisPayload,
  type StorageHealthPayload,
} from "../../lib/ipc"
import {
  cancelChecksReport,
  getChecksReport,
  onChecksReportChanged,
  type ChecksReportPayload,
} from "../../lib/insightsIpc"
import {
  popoverHeightFor,
  prefersReducedMotion,
  type PopoverSurface,
} from "../../lib/popoverHeight"
import { hidePopoverPeek } from "../../lib/popoverPeekIpc"
import { localSessionKey } from "../../lib/presentation/localIdentity"
import { costOutlierThreshold } from "../../lib/presentation/sessionAnalysis"
import {
  isCurrentWindowVisible,
  isFloatingHudEnabled,
  isOverlayWindowVisible,
  openOverlayWindow,
} from "../../lib/overlayWindow"
import { isMacOS } from "../../lib/platform"
import type { LocalRepositoryItem, LocalRepositoryStatus } from "../../lib/types/repository"
import type { SessionSubject } from "./SessionPane"

/**
 * The imperative boundary between the popover window and the shell.
 *
 * React reads immutable snapshots through `useSyncExternalStore`; IPC calls,
 * event subscriptions, and the window's own keyboard handling stay here,
 * where they belong to the external systems that created them rather than to
 * a component lifecycle. See `views/onboarding/OnboardingSession.ts` for the
 * same shape applied to the first-run window.
 */

/** One session's loaded analysis, tagged with the subject it belongs to. */
type PopoverAnalysisState = {
  key: string
  payload: SessionAnalysisPayload | null
  error: boolean
} | null

export interface PopoverSnapshot {
  appVersion: string | null
  debugBuild: boolean
  settings: AppSettings | null
  entries: SessionListEntry[] | null
  repositories: LocalRepositoryItem[]
  /** Provider usage, or null while the first snapshot is in flight. */
  usage: ProviderUsageSummaryPayload | null
  liveUsage: LiveUsageSummaryPayload
  sessionLimitAllocations: SessionLimitAllocationSummaryPayload
  /** Whether a `refreshUsage` call is in flight, for the limits section's spinner. */
  usageRefreshing: boolean
  /** Whether the full Usage view is showing over the activity list. */
  showUsage: boolean
  /** The real local report rendered by the Activity summary and anchored preview. */
  checksReport: ChecksReportPayload | null
  /** True when the latest Checks report request fails. */
  checksUnavailable: boolean
  /** The surface whose native resize has completed and React can render. */
  presentedSurface: PopoverSurface
  /** The session retained while a request to present another surface is in flight. */
  presentedSession: SessionSubject | null
  storage: StorageHealthPayload
  /** Banners the reader has waved away this run. */
  dismissed: readonly AttentionKind[]
  /** Navigation stack. Empty means the activity list is showing. */
  stack: SessionSubject[]
  /**
   * The most recently settled (or failed) analysis load, tagged with its
   * subject's key.
   *
   * One field rather than three, and it carries its own key: "still loading"
   * is then *derived* by the reader (the key does not match the subject on
   * top of the stack) instead of being a flag this class has to flip on the
   * way in — which is what keeps opening a session from cascading renders.
   */
  analysis: PopoverAnalysisState
  /**
   * Whether a re-load of the open session's analysis is in flight while an
   * earlier result is still on screen. Drives the detail header's spinner.
   */
  analysisRefreshing: boolean
  /**
   * A timestamp bumped every `NOW_TICK_MS` while a detail pane is open.
   *
   * The value itself has no reader: its purpose is to change the snapshot
   * reference so the header's relative-time text ("last just now") re-renders
   * on its own clock, not the arrival of new data.
   */
  now: number
}

/**
 * Identity key for a subject's analysis load. Stable across re-navigation.
 *
 * Scoped by environment as well as agent and id: the same session id can
 * exist natively and inside a WSL distribution, and without the environment
 * in the key a subject moving between the two would keep showing the other
 * environment's stale (or loading) analysis.
 *
 * A sub-agent id is only unique within its launching session, so its key
 * carries the parent's local identity too.
 */
export function sessionKey(subject: SessionSubject): string {
  return subject.subagent
    ? JSON.stringify([
        "subagent",
        localSessionKey(subject.agent, subject.subagent.parentSessionId, subject.wslDistro),
        subject.subagent.subagentId,
      ])
    : localSessionKey(subject.agent, subject.sessionId, subject.wslDistro)
}

/** Load one subject's analysis. Sub-agents come from their own command. */
async function loadAnalysis(subject: SessionSubject): Promise<SessionAnalysisPayload | null> {
  if (subject.subagent) {
    return getSubagentAnalysis(
      subject.agent,
      subject.subagent.parentSessionId,
      subject.subagent.subagentId,
      subject.wslDistro,
    )
  }
  return getSessionAnalysis(subject.agent, subject.sessionId, subject.wslDistro)
}

/**
 * How often the store forces a snapshot change while a detail pane is open,
 * so the header's relative-time text ("last just now") stays current even
 * when nothing else about the session has changed.
 */
const NOW_TICK_MS = 30_000

/**
 * How long the list can go without a full refetch before `listenScanEvent`
 * forces one, even though the pass reported `listChanged: false`. The
 * backstop for a signal this session missed or got wrong; matches the
 * backend scheduler's own tick, so reconciliation never lags a full cycle
 * behind the pass that produced it.
 */
const LIST_RECONCILE_MS = 60_000

/**
 * Floor shared by `scan:finished` and `sessions:entry-changed` usage
 * refreshes that report no list change. A re-described pass or a patched row
 * is not, by itself, a reason to recompute 30-day usage totals and resolve
 * both live provider accounts (F1, R6): an active session's row updates
 * every few seconds, and a usage refresh on every one of those would cost as
 * much as the list rebuild it was meant to avoid. `listChanged` still forces
 * an immediate refresh, since that means a session was discovered or
 * removed.
 */
const USAGE_REFRESH_MIN_MS = 30_000

/**
 * How often usage is polled while the popover is visible, independent of any
 * scan (R6). The backend's `POPOVER_LIVE_USAGE_MAX_AGE` (50 s) is tuned to
 * sit just under this, so an open popover's own polling is what keeps a
 * live reading current.
 */
const USAGE_VISIBLE_POLL_MS = 60_000

/**
 * Order list rows the way the backend does: newest activity first, and a
 * revived session's own id breaking a tie. `listenSessionEntryChanged`
 * re-sorts with this after patching one row in place, so a session whose
 * activity just moved reliably lands where a full re-list would put it.
 */
function compareByRecency(a: SessionListEntry, b: SessionListEntry): number {
  if (a.timestamp !== b.timestamp) return a.timestamp > b.timestamp ? -1 : 1
  const aId = a.sessionId ?? ""
  const bId = b.sessionId ?? ""
  return aId > bId ? -1 : aId < bId ? 1 : 0
}

/** Narrow the shell's status string to the repository list's union. */
function repositoryStatus(status: string): LocalRepositoryStatus {
  switch (status) {
    case "accessible":
    case "permission_denied":
    case "not_cloned":
    case "disabled":
      return status
    default:
      return "accessible"
  }
}

export class PopoverSession {
  private listeners = new Set<() => void>()
  private started = false
  private generation = 0
  private analysisToken = 0
  private checksToken = 0
  private checksConsumerId: string | null = null
  private checksRefresh: Promise<void> | null = null
  private checksRefreshQueued = false
  private resizeToken = 0
  /**
   * How many `refreshUsage` calls are currently in flight.
   *
   * A counter rather than a boolean: the popover-shown signal and a
   * scan-finished event can each trigger a refresh close together, and the
   * first one to settle must not clear the spinner out from under the one
   * still running. The snapshot's `usageRefreshing` is `count > 0`.
   */
  private usageRefreshCount = 0
  /** How many `refreshAnalysis` calls are in flight; see `usageRefreshCount`. */
  private analysisRefreshCount = 0
  /**
   * Set when a matching `sessions:entry-changed` event lands while a
   * `refreshAnalysis` call is already in flight. Coalesced to one extra run,
   * not one per event: the in-flight call's own result is already stale by
   * the time it lands, so every event that arrives before it finishes is
   * asking for the same thing.
   */
  private pendingAnalysisRefresh = false
  private liveUsageRevision = 0
  private sessionLimitAllocationRequested = 0
  private sessionLimitAllocationRefresh: Promise<void> | null = null
  private sessionLimitAllocationExpiryTimer: ReturnType<typeof setTimeout> | null = null
  private initialContentReady = false
  private contentReadyReportedGeneration: number | null = null
  private contentReadyReportInFlightGeneration: number | null = null
  private contentReadyRetryGeneration: number | null = null

  /** Set while a coalesced `refreshEntries` call is in flight; see `pendingAnalysisRefresh`. */
  private entriesRefreshInFlight = false
  private entriesRefreshQueued = false
  /**
   * When `listenScanEvent` last refetched the full list. Read against
   * `LIST_RECONCILE_MS` so a pass that never sets `listChanged` still gets
   * reconciled eventually.
   */
  private lastListReconcileAt = 0

  /**
   * When a usage refresh last ran from `listenScanEvent` or
   * `listenSessionEntryChanged`. Read against `USAGE_REFRESH_MIN_MS` (F1, R6)
   * so a quiet stream of events with no list change refreshes usage on a
   * shared floor, not on every event.
   */
  private lastUsageRefreshAt = 0

  /**
   * Whether the popover is currently on screen. R6: gates the
   * `sessions:entry-changed` usage refresh and whether the visible-only poll
   * is running.
   *
   * Defaults `true` rather than `false`: the session can start after the
   * shell's first `popover:shown` already fired, before this class's own
   * listener was registered to hear it, and a wrongly-`false` default would
   * then never start the poll until the popover cycled hidden and shown again.
   */
  private visible = true

  private nowTickTimer: ReturnType<typeof setInterval> | null = null
  /** The visible-only usage poll (R6); see `startUsagePolling`/`stopUsagePolling`. */
  private usagePollTimer: ReturnType<typeof setInterval> | null = null

  private stopSettingsListening: (() => void) | null = null
  private stopSessionsInvalidatedListening: (() => void) | null = null
  private stopSessionEntryChangedListening: (() => void) | null = null
  private stopChecksReportChangedListening: (() => void) | null = null
  private stopStorageHealthListening: (() => void) | null = null
  private stopScanListening: (() => void) | null = null
  private stopPopoverShownListening: (() => void) | null = null
  private stopPopoverHiddenListening: (() => void) | null = null
  private stopLiveUsageListening: (() => void) | null = null

  private snapshot: PopoverSnapshot = {
    appVersion: null,
    debugBuild: false,
    settings: null,
    entries: null,
    repositories: [],
    usage: null,
    liveUsage: EMPTY_LIVE_USAGE,
    sessionLimitAllocations: EMPTY_SESSION_LIMIT_ALLOCATIONS,
    usageRefreshing: false,
    showUsage: false,
    checksReport: null,
    checksUnavailable: false,
    presentedSurface: "activity",
    presentedSession: null,
    storage: HEALTHY_STORAGE,
    dismissed: [],
    stack: [],
    analysis: null,
    analysisRefreshing: false,
    now: Date.now(),
  }

  getSnapshot = (): PopoverSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  /* -----------------------------------------------------------------------
   * Navigation
   * -------------------------------------------------------------------- */

  openSession = (subject: SessionSubject): void => {
    this.update({ stack: [...this.snapshot.stack, subject] })
    this.syncHeight()
    this.openAnalysis(subject)
  }

  goBack = (): void => {
    this.update({ stack: this.snapshot.stack.slice(0, -1) })
    this.syncHeight()
    const top = this.snapshot.stack.at(-1)
    if (top) {
      this.openAnalysis(top)
    } else {
      this.syncDetailTimers()
    }
  }

  /** Replace the top of the stack, for the newer/older traversal. */
  replaceTop = (subject: SessionSubject): void => {
    this.update({ stack: [...this.snapshot.stack.slice(0, -1), subject] })
    this.syncHeight()
    this.openAnalysis(subject)
  }

  /** The shown session's local records were deleted: go back and re-list. */
  sessionDeleted = (): void => {
    this.goBack()
    void this.refreshEntries(this.windowDays()).catch(() => {})
  }

  setShowUsage = (show: boolean): void => {
    this.update({ showUsage: show })
    this.syncHeight()
  }

  dismissBanner = (id: AttentionKind): void => {
    if (this.snapshot.dismissed.includes(id)) return
    this.update({ dismissed: [...this.snapshot.dismissed, id] })
  }

  /**
   * Open or close the popover's usage-limits section, and persist the choice
   * so it never changes again on its own.
   *
   * Optimistic, the same way the settings window writes: the toggle must not
   * lag behind the pointer, and the stored answer replaces this one a moment
   * later — silently, since a boolean the store would reject does not exist.
   */
  setOverviewLimitsExpanded = (expanded: boolean): void => {
    const current = this.snapshot.settings ?? DEFAULT_SETTINGS
    if (current.overviewLimitsExpanded === expanded) return
    const next = { ...current, overviewLimitsExpanded: expanded }
    this.update({ settings: next })
    void setSettings(next)
      .then((saved) => this.update({ settings: saved }))
      .catch(() => {})
  }

  setSessionBadgeMetric = (metric: AppSettings["sessionBadgeMetric"]): void => {
    const current = this.snapshot.settings ?? DEFAULT_SETTINGS
    if (current.sessionBadgeMetric === metric) return
    const next = { ...current, sessionBadgeMetric: metric }
    this.update({ settings: next })
    void setSettings(next)
      .then((saved) => this.update({ settings: saved }))
      .catch(() => {})
  }

  /** Run a discovery pass. The source-access banner's only action. */
  rescan = async (): Promise<void> => {
    await scanNow().catch(() => null)
  }

  /* -----------------------------------------------------------------------
   * Lifecycle
   * -------------------------------------------------------------------- */

  private start(): void {
    this.started = true
    const generation = ++this.generation
    this.checksConsumerId = crypto.randomUUID()
    this.initialContentReady = false

    void this.loadInitial(generation)
    void this.listenSettings(generation)
    void this.listenSessionsInvalidated(generation)
    void this.listenSessionEntryChanged(generation)
    void this.startChecks(generation)
    void this.listenStorageHealth(generation)
    void this.listenScanEvent(generation)
    void this.listenPopoverShown(generation)
    void this.listenPopoverHidden(generation)
    void this.listenLiveUsage(generation)

    // ⌘, opens Settings — the platform's standard preferences shortcut, which
    // an accessory app with no application menu has to own itself. Bound
    // alongside Escape on `window`, deliberately: it is the last object in an
    // event's propagation path, so every surface listening on `document` has
    // already had its chance to claim the key first.
    window.addEventListener("keydown", this.onWindowKeyDown)

    // A stack carried over from a previous start (the window never really
    // unmounts, but the listener count can still hit zero and come back)
    // still needs its relative-time ticker running again.
    this.syncDetailTimers()

    // R6: the session starts visible (see `visible`'s doc comment), so its
    // usage poll starts immediately rather than waiting for a `popover:shown`
    // that may already have fired.
    this.startUsagePolling()
  }

  private stop(): void {
    this.started = false
    this.generation += 1
    this.stopSettingsListening?.()
    this.stopSettingsListening = null
    this.stopSessionsInvalidatedListening?.()
    this.stopSessionsInvalidatedListening = null
    this.stopSessionEntryChangedListening?.()
    this.stopSessionEntryChangedListening = null
    this.stopChecksReportChangedListening?.()
    this.stopChecksReportChangedListening = null
    this.stopStorageHealthListening?.()
    this.stopStorageHealthListening = null
    this.stopScanListening?.()
    this.stopScanListening = null
    this.stopPopoverShownListening?.()
    this.stopPopoverShownListening = null
    this.stopPopoverHiddenListening?.()
    this.stopPopoverHiddenListening = null
    this.stopLiveUsageListening?.()
    this.stopLiveUsageListening = null
    this.stopNowTicking()
    this.stopUsagePolling()
    this.stopSessionLimitAllocationExpiryTimer()
    this.checksRefreshQueued = false
    const checksConsumerId = this.checksConsumerId
    this.checksConsumerId = null
    if (checksConsumerId) void cancelChecksReport(checksConsumerId)
    window.removeEventListener("keydown", this.onWindowKeyDown)
  }

  // First load: read independent shell state together, then list sessions for
  // the stored time window. The cached limits do not wait for either read.
  private loadInitial = async (generation: number): Promise<void> => {
    const usage = this.loadCachedUsage()
    const [stored, health, info] = await Promise.all([
      getSettings().catch(() => DEFAULT_SETTINGS),
      getStorageHealth().catch(() => HEALTHY_STORAGE),
      appInfo().catch(() => null),
    ])
    if (generation !== this.generation) return
    applyTheme(stored.theme)
    this.update({
      appVersion: info?.appVersion ?? null,
      debugBuild: info?.debugBuild ?? false,
      settings: stored,
      storage: health,
    })
    // The repository list is read on first paint rather than waiting for a
    // scan to finish, because the source-access banner needs it — a blocked
    // repository is exactly the case where no scan will ever complete to
    // deliver the news. Not awaited: it is a store read that nothing below
    // depends on, and the activity list is what a reader opened the popover
    // for.
    void this.refreshRepositoryList()
    await Promise.all([
      this.refreshEntries(stored.activityWindowDays).catch(() => this.update({ entries: [] })),
      usage,
    ])
    if (generation !== this.generation) return
    this.initialContentReady = true
    this.reportContentReady()
    void this.refreshUsage()
  }

  // Settings are written in the settings window but rendered here: the theme,
  // the day window, and the pause state all change what this window shows.
  // The shell broadcasts every write, and the popover restyles and re-queries
  // as needed instead of waiting for its next mount (which never comes — the
  // window lives for the whole run).
  private listenSettings = async (generation: number): Promise<void> => {
    const unlisten = await onSettingsChanged((settings) => {
      if (generation !== this.generation) return
      const previousDays = this.windowDays()
      const previousDisabled = (this.snapshot.settings?.disabledAgents ?? []).join(",")
      applyTheme(settings.theme)
      this.update({ settings })
      if (
        settings.activityWindowDays !== previousDays ||
        settings.disabledAgents.join(",") !== previousDisabled
      ) {
        void this.refreshEntries(settings.activityWindowDays).catch(() => {})
      }
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopSettingsListening = unlisten
  }

  // Sessions can leave the index without a scan — a repository opt-out purges
  // its rows on the spot — and the list must not keep showing them.
  private listenSessionsInvalidated = async (generation: number): Promise<void> => {
    const unlisten = await onSessionsInvalidated(() => {
      if (generation !== this.generation) return
      void this.refreshEntries(this.windowDays()).catch(() => {})
      void this.refreshUsage()
      void this.refreshRepositoryList()
      void this.refreshChecks()
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopSessionsInvalidatedListening = unlisten
  }

  // Opening a session computes its analysis and the shell caches it, but a
  // cache write alone does not change what a scan already put in the list.
  // The shell pushes the one changed row here so the pills stay current
  // without a full re-query, and — since this is also the scan pass's own
  // per-session signal, not only the worker's — this also refreshes an open
  // detail pane's analysis when the changed session is the one on screen.
  //
  // R6: while the popover is visible, this is also a usage-refresh signal,
  // on the same `USAGE_REFRESH_MIN_MS` floor `listenScanEvent` uses — an
  // active session's row updates faster than a full pass re-describes it, so
  // waiting for `scan:finished` alone would leave usage stale in between.
  // Hidden, this does nothing for usage: the visible-only poll is what keeps
  // a hidden popover's next open cheap instead.
  private listenSessionEntryChanged = async (generation: number): Promise<void> => {
    const unlisten = await onSessionEntryChanged((entry) => {
      if (generation !== this.generation) return
      this.patchOrRefetchEntry(entry)
      this.refreshOpenAnalysisIfMatching(entry)
      void this.refreshSessionLimitAllocations()
      if (this.visible && Date.now() - this.lastUsageRefreshAt >= USAGE_REFRESH_MIN_MS) {
        this.lastUsageRefreshAt = Date.now()
        void this.refreshUsage()
      }
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopSessionEntryChangedListening = unlisten
  }

  private listenChecksReportChanged = async (generation: number): Promise<void> => {
    const unlisten = await onChecksReportChanged(() => {
      if (generation !== this.generation) return
      void this.refreshChecks()
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopChecksReportChangedListening = unlisten
  }

  private startChecks = async (generation: number): Promise<void> => {
    await this.listenChecksReportChanged(generation)
    if (generation !== this.generation) return
    await this.refreshChecks()
  }

  /**
   * Patch the one row `entry` describes in place, re-sorted the way the
   * backend orders the list so a revived session moves back to the top. A
   * session not already on screen — one that just re-entered the window, or
   * one the popover has never listed — triggers a coalesced full refetch
   * instead of being dropped.
   */
  private patchOrRefetchEntry(entry: ActivityEntryPayload): void {
    const entries = this.snapshot.entries
    if (!entries) return
    const index = indexOfSession(entries, entry.agent, entry.sessionId, entry.wslDistro)
    if (index === -1) {
      this.requestEntriesRefresh()
      return
    }
    // The cohort for the high-cost flag is the list on screen, with the
    // replaced row's own cost swapped in — the same set `toActivityEntries`
    // would see on a full re-list.
    const threshold = costOutlierThreshold(
      entries
        .map((row, i) => (i === index ? entry.cost?.totalUsd : row.cost?.totalUsd))
        .filter((usd): usd is number => typeof usd === "number"),
    )
    const next = [...entries]
    next[index] = toActivityEntry(entry, threshold)
    next.sort(compareByRecency)
    this.update({ entries: next })
  }

  /**
   * Refresh `refreshEntries` at most once at a time: a burst of
   * `sessions:entry-changed` events for sessions outside the current list
   * (a watcher-driven scan describing several new sessions in one pass, say)
   * must not start a refetch per event.
   */
  private requestEntriesRefresh = (): void => {
    if (this.entriesRefreshInFlight) {
      this.entriesRefreshQueued = true
      return
    }
    this.entriesRefreshInFlight = true
    void this.refreshEntries(this.windowDays())
      .catch(() => {})
      .finally(() => {
        this.entriesRefreshInFlight = false
        if (this.entriesRefreshQueued) {
          this.entriesRefreshQueued = false
          this.requestEntriesRefresh()
        }
      })
  }

  /**
   * Refresh the open detail pane's analysis when `entry` describes the
   * subject on top of the stack — its own session, or for a sub-agent
   * subject its parent's session, in the same environment.
   */
  private refreshOpenAnalysisIfMatching(entry: ActivityEntryPayload): void {
    const subject = this.snapshot.stack.at(-1)
    if (!subject) return
    const sessionId = subject.subagent?.parentSessionId ?? subject.sessionId
    if (
      entry.agent !== subject.agent ||
      entry.sessionId !== sessionId ||
      (entry.wslDistro ?? null) !== (subject.wslDistro ?? null)
    ) {
      return
    }
    this.requestAnalysisRefresh()
  }

  /**
   * Refresh the open analysis, coalesced: a matching event that lands while
   * a refresh is already in flight schedules exactly one more, run after the
   * in-flight one settles, rather than a second overlapping load.
   */
  private requestAnalysisRefresh = (): void => {
    if (this.analysisRefreshCount > 0) {
      this.pendingAnalysisRefresh = true
      return
    }
    void this.refreshAnalysis()
  }

  // Storage health changes rarely and matters immediately, so it is pushed
  // rather than polled. Only changes are emitted, so this is not a per-tick
  // event.
  private listenStorageHealth = async (generation: number): Promise<void> => {
    const unlisten = await onStorageHealth((status) => {
      if (generation !== this.generation) return
      this.update({ storage: status })
      // A failure that recovers should not leave its banner dismissed, or the
      // next failure would arrive silently.
      if (!status.failing) {
        this.update({ dismissed: this.snapshot.dismissed.filter((id) => id !== "storage") })
      }
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopStorageHealthListening = unlisten
  }

  // The scan is the only thing that changes what is on screen behind the
  // reader's back, so that is what the list listens for rather than polling.
  // Only `finished` matters here: no surface in this window draws a pass in
  // progress, so the intermediate phases have nothing to say. A full refetch
  // of entries and repositories only runs when the pass says the list needs
  // one, or the reconcile interval has elapsed — `sessions:entry-changed`
  // already keeps individual rows current in between. Usage follows its own
  // floor (R5): `listChanged` forces an immediate refresh, and otherwise a
  // pass only counts when it re-described at least one session
  // (`reDescribed > 0`) — an idle pass, the common case now that the watcher
  // does the real freshness work, refreshes nothing. `sessions:entry-changed`
  // shares this same floor while the popover is visible (R6), which is what
  // replaces the usage refresh a row patch never used to trigger — see
  // `listenSessionEntryChanged`.
  private listenScanEvent = async (generation: number): Promise<void> => {
    const unlisten = await onScanEvent((status, phase) => {
      if (generation !== this.generation) return
      if (phase !== "finished") return
      const now = Date.now()
      if (status.listChanged || now - this.lastListReconcileAt >= LIST_RECONCILE_MS) {
        this.lastListReconcileAt = now
        void this.refreshEntries(this.windowDays()).catch(() => {})
        void this.refreshRepositoryList()
      }
      if (
        status.listChanged ||
        (status.reDescribed > 0 && now - this.lastUsageRefreshAt >= USAGE_REFRESH_MIN_MS)
      ) {
        this.lastUsageRefreshAt = now
        void this.refreshUsage()
      }
      void this.refreshChecks()
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopScanListening = unlisten
  }

  // The shell's own signal that the popover just reached the screen — no
  // longer paired with a scan kick (R1: opening the popover does not ask for
  // one). The open session's analysis can grow while the popover is hidden,
  // and nothing else asks for it again until the reader navigates, so it
  // still gets its own refresh here.
  //
  // Entries are also refetched here, even though the scan scheduler now
  // ticks unconditionally and `listenScanEvent` above is the primary path:
  // this is a cheap defence against a `scan:finished` event missed while the
  // popover was hidden, so a reader never sees a stale list for a whole tick.
  private listenPopoverShown = async (generation: number): Promise<void> => {
    const unlisten = await onPopoverShown(() => {
      if (generation !== this.generation) return
      this.visible = true
      this.startUsagePolling()
      if (this.initialContentReady) this.reportContentReady(true)
      void this.restoreFloatingHud(generation)
      void this.refreshEntries(this.windowDays()).catch(() => {})
      void this.refreshUsage()
      void this.refreshAnalysis()
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopPopoverShownListening = unlisten
    void this.restoreFloatingHud(generation)
  }

  // R6: the close-side counterpart. Usage freshness while visible does not
  // ride on a scan, so it needs its own signal for when to stop polling too.
  private listenPopoverHidden = async (generation: number): Promise<void> => {
    const unlisten = await onPopoverHidden(() => {
      if (generation !== this.generation) return
      this.visible = false
      this.stopUsagePolling()
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopPopoverHiddenListening = unlisten
  }

  private reportContentReady(retryAfterPendingFailure = false): void {
    const rendererGeneration = window.__ANTIBURN_WINDOW_GENERATION__
    if (typeof rendererGeneration !== "number" || !Number.isSafeInteger(rendererGeneration))
      return
    if (this.contentReadyReportedGeneration === rendererGeneration) return
    if (this.contentReadyReportInFlightGeneration === rendererGeneration) {
      if (retryAfterPendingFailure) this.contentReadyRetryGeneration = rendererGeneration
      return
    }
    this.contentReadyReportInFlightGeneration = rendererGeneration
    void popoverContentReady(rendererGeneration)
      .then(() => {
        this.contentReadyReportedGeneration = rendererGeneration
      })
      .catch(() => undefined)
      .finally(() => {
        if (this.contentReadyReportInFlightGeneration === rendererGeneration) {
          this.contentReadyReportInFlightGeneration = null
        }
        const retry =
          this.contentReadyRetryGeneration === rendererGeneration &&
          this.contentReadyReportedGeneration !== rendererGeneration
        if (this.contentReadyRetryGeneration === rendererGeneration) {
          this.contentReadyRetryGeneration = null
        }
        if (retry) this.reportContentReady()
      })
  }

  private restoreFloatingHud = async (generation: number): Promise<void> => {
    if (!isMacOS() || !isFloatingHudEnabled()) return
    const visible = await isCurrentWindowVisible()
    if (generation !== this.generation || !visible) return
    const overlayVisible = await isOverlayWindowVisible()
    if (generation !== this.generation || overlayVisible) return
    await openOverlayWindow().catch(() => {})
  }

  private listenLiveUsage = async (generation: number): Promise<void> => {
    const unlisten = await onLiveUsageChanged((liveUsage) => {
      if (generation !== this.generation) return
      this.liveUsageRevision += 1
      this.update({ liveUsage })
      void this.refreshSessionLimitAllocations()
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopLiveUsageListening = unlisten
  }

  private onWindowKeyDown = (event: KeyboardEvent): void => {
    if (event.key === "Escape") {
      // A surface with something nearer to close — an open provider panel —
      // claims the key by calling `preventDefault`. Anything left over
      // dismisses the popover, which is the keyboard's only way out of a tray
      // window.
      if (event.defaultPrevented) return
      void hidePopover().catch(() => {})
      return
    }
    if ((event.metaKey || event.ctrlKey) && event.key === ",") {
      event.preventDefault()
      void openSettingsWindow()
    }
  }

  /* -----------------------------------------------------------------------
   * Refreshers
   * -------------------------------------------------------------------- */

  private refreshEntries = async (days: number): Promise<void> => {
    const payloads = await listRecentSessions(days)
    this.update({ entries: toActivityEntries(payloads) })
  }

  private loadCachedUsage = async (): Promise<void> => {
    const liveUsageRevision = this.liveUsageRevision
    void this.refreshSessionLimitAllocations()
    const [usage, liveUsage] = await Promise.all([
      getProviderUsage().catch(() => EMPTY_PROVIDER_USAGE),
      getLiveUsage().catch(() => EMPTY_LIVE_USAGE),
    ])
    this.update({ usage })
    if (liveUsageRevision === this.liveUsageRevision) {
      this.update({ liveUsage })
    }
  }

  private refreshUsage = async (): Promise<void> => {
    // Counted rather than flagged directly, and flushed to the snapshot as
    // `count > 0`: the popover-shown signal and a scan-finished event can
    // each start a refresh close together, and the first call to settle must
    // not clear the spinner while a second one is still in flight.
    this.usageRefreshCount += 1
    this.update({ usageRefreshing: true })
    try {
      // Publish each half when it settles. Local spend does not wait for the
      // provider refresh, and the cached limit remains visible meanwhile.
      await Promise.all([
        getProviderUsage()
          .then((usage) => this.update({ usage }))
          .catch(() => undefined),
        refreshLiveUsage()
          .then((liveUsage) => {
            this.liveUsageRevision += 1
            this.update({ liveUsage })
          })
          .catch(() => undefined),
      ])
      await this.refreshSessionLimitAllocations()
      // Usage arriving can flip the derived surface to 'usage' on its own —
      // the reader may have already asked to see it before there was
      // anything to show — so the height request has to follow this update
      // too, not only the click-driven ones.
      this.syncHeight()
    } finally {
      this.usageRefreshCount -= 1
      this.update({ usageRefreshing: this.usageRefreshCount > 0 })
    }
  }

  private refreshSessionLimitAllocations = (): Promise<void> => {
    this.sessionLimitAllocationRequested += 1
    if (!this.sessionLimitAllocationRefresh) {
      this.sessionLimitAllocationRefresh = this.runSessionLimitAllocationRefreshes().finally(
        () => {
          this.sessionLimitAllocationRefresh = null
        },
      )
    }
    return this.sessionLimitAllocationRefresh
  }

  private runSessionLimitAllocationRefreshes = async (): Promise<void> => {
    let completed = 0
    while (completed < this.sessionLimitAllocationRequested) {
      const target = this.sessionLimitAllocationRequested
      const generation = this.generation
      const sessionLimitAllocations = await getSessionLimitAllocations().catch(() => null)
      if (sessionLimitAllocations && generation === this.generation) {
        this.update({ sessionLimitAllocations })
        this.scheduleSessionLimitAllocationExpiry()
      }
      completed = target
    }
  }

  private scheduleSessionLimitAllocationExpiry(): void {
    this.stopSessionLimitAllocationExpiryTimer()
    const now = Date.now()
    let nextReset = Number.POSITIVE_INFINITY
    for (const allocation of this.snapshot.sessionLimitAllocations.allocations) {
      const reset = Date.parse(allocation.resetsAt)
      if (reset > now && reset < nextReset) nextReset = reset
    }
    if (!Number.isFinite(nextReset)) return
    this.sessionLimitAllocationExpiryTimer = setTimeout(
      () => {
        this.sessionLimitAllocationExpiryTimer = null
        this.update({ now: Date.now() })
        this.scheduleSessionLimitAllocationExpiry()
      },
      Math.min(nextReset - now + 1, 2_147_483_647),
    )
  }

  private stopSessionLimitAllocationExpiryTimer(): void {
    if (this.sessionLimitAllocationExpiryTimer === null) return
    clearTimeout(this.sessionLimitAllocationExpiryTimer)
    this.sessionLimitAllocationExpiryTimer = null
  }

  private refreshRepositoryList = async (): Promise<void> => {
    const payloads = await listRepositories().catch(() => [])
    this.update({
      repositories: payloads.map((payload) => ({
        ...payload,
        status: repositoryStatus(payload.status),
      })),
    })
  }

  /** Load a subject's analysis and make sure the relative-time ticker is running — a detail pane is now open. */
  private openAnalysis(subject: SessionSubject): void {
    void this.loadAnalysisFor(subject)
    this.syncDetailTimers()
  }

  /** Start or stop the relative-time ticker to match whether a detail pane is open. */
  private syncDetailTimers(): void {
    if (this.snapshot.stack.length > 0) {
      this.startNowTicking()
    } else {
      this.stopNowTicking()
    }
  }

  private startNowTicking(): void {
    if (this.nowTickTimer !== null) return
    this.nowTickTimer = setInterval(() => this.update({ now: Date.now() }), NOW_TICK_MS)
  }

  private stopNowTicking(): void {
    if (this.nowTickTimer === null) return
    clearInterval(this.nowTickTimer)
    this.nowTickTimer = null
  }

  /**
   * R6: usage freshness while the popover is visible, independent of a scan.
   * Each tick stamps `lastUsageRefreshAt` the same way the scan- and
   * entry-changed-triggered refreshes do, so they all share one floor rather
   * than a poll immediately re-triggering one of the others.
   */
  private startUsagePolling(): void {
    if (this.usagePollTimer !== null) return
    this.usagePollTimer = setInterval(() => {
      this.lastUsageRefreshAt = Date.now()
      void this.refreshUsage()
    }, USAGE_VISIBLE_POLL_MS)
  }

  private stopUsagePolling(): void {
    if (this.usagePollTimer === null) return
    clearInterval(this.usagePollTimer)
    this.usagePollTimer = null
  }

  private loadAnalysisFor = (subject: SessionSubject): Promise<void> => {
    const key = sessionKey(subject)
    const generation = this.generation
    const token = ++this.analysisToken
    return loadAnalysis(subject)
      .then((payload) => {
        if (generation !== this.generation || token !== this.analysisToken) return
        this.update({ analysis: { key, payload, error: false } })
      })
      .catch(() => {
        if (generation !== this.generation || token !== this.analysisToken) return
        this.update({ analysis: { key, payload: null, error: true } })
      })
  }

  /**
   * Re-load the analysis for the session on top of the stack. The settled
   * result stays on screen until the new one lands: `loading` is derived from
   * a key mismatch, and the key does not change here, so the reader sees the
   * header spinner rather than the skeleton.
   *
   * A matching `sessions:entry-changed` event that lands while this is
   * already running is not dropped: `requestAnalysisRefresh` queues exactly
   * one more call, picked up here once this one settles.
   */
  private refreshAnalysis = async (): Promise<void> => {
    const top = this.snapshot.stack.at(-1)
    if (!top) return
    this.analysisRefreshCount += 1
    this.update({ analysisRefreshing: true })
    try {
      await this.loadAnalysisFor(top)
    } finally {
      this.analysisRefreshCount -= 1
      this.update({ analysisRefreshing: this.analysisRefreshCount > 0 })
      if (this.analysisRefreshCount === 0 && this.pendingAnalysisRefresh) {
        this.pendingAnalysisRefresh = false
        void this.refreshAnalysis()
      }
    }
  }

  private refreshChecks = (): Promise<void> => {
    if (this.checksRefresh) {
      this.checksRefreshQueued = true
      return this.checksRefresh
    }
    this.checksRefresh = this.loadChecks().finally(() => {
      this.checksRefresh = null
      if (this.checksRefreshQueued) {
        this.checksRefreshQueued = false
        void this.refreshChecks()
      }
    })
    return this.checksRefresh
  }

  private loadChecks = async (): Promise<void> => {
    const generation = this.generation
    const token = ++this.checksToken
    const consumerId = this.checksConsumerId
    if (!consumerId) return
    try {
      const checksReport = await getChecksReport(consumerId)
      if (generation !== this.generation || token !== this.checksToken) return
      if (!checksReport) {
        this.update({ checksUnavailable: true })
        return
      }
      const replacesVisibleReport = this.snapshot.checksReport != null
      this.update({ checksReport, checksUnavailable: false })
      if (replacesVisibleReport) void hidePopoverPeek().catch(() => undefined)
    } catch {
      if (generation === this.generation && token === this.checksToken) {
        this.update({ checksUnavailable: true })
      }
      return
    }
  }

  /* -----------------------------------------------------------------------
   * Derived
   * -------------------------------------------------------------------- */

  private windowDays(): number {
    return this.snapshot.settings?.activityWindowDays ?? DEFAULT_SETTINGS.activityWindowDays
  }

  private surface(): PopoverSurface {
    const { showUsage, usage, stack } = this.snapshot
    return showUsage && usage ? "usage" : stack.length > 0 ? "session" : "activity"
  }

  // Reduced motion is a webview preference, so the decision is made here and
  // the shell simply honours it.
  private syncHeight(): void {
    const surface = this.surface()
    const presentedSession = surface === "session" ? (this.snapshot.stack.at(-1) ?? null) : null
    const targetHeight = popoverHeightFor(surface)
    const token = ++this.resizeToken

    // A taller or equal-height surface fits immediately. A shorter surface
    // waits so its larger replacement stays mounted during the contraction.
    if (targetHeight >= popoverHeightFor(this.snapshot.presentedSurface)) {
      this.update({ presentedSurface: surface, presentedSession })
    }

    void setPopoverHeight(targetHeight, !prefersReducedMotion())
      .then(() => {
        if (token !== this.resizeToken || surface !== this.surface()) return
        // A failed shell resize must not leave old navigation active forever.
        // The shell has already stopped a failed or superseded animation.
        this.update({ presentedSurface: surface, presentedSession })
      })
      .catch(() => {
        if (token !== this.resizeToken || surface !== this.surface()) return
        this.update({ presentedSurface: surface, presentedSession })
      })
  }

  private update(change: Partial<PopoverSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }
}

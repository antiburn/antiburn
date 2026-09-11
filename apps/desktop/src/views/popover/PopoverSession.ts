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
  getLiveSessions,
  getLiveUsage,
  getProviderUsage,
  getSessionLimitAllocations,
  getSettings,
  getStorageHealth,
  HEALTHY_STORAGE,
  hidePopover,
  listRecentSessions,
  listRepositories,
  onPopoverHidden,
  onPopoverShown,
  onLiveUsageChanged,
  onScanEvent,
  onSessionEntryChanged,
  onSessionLifecycle,
  onSessionsInvalidated,
  onSettingsChanged,
  onStorageHealth,
  openSettingsWindow,
  popoverContentReady,
  refreshLiveUsage,
  scanNow,
  setSettings,
  type ActivityEntryPayload,
  type AppSettings,
  type LiveUsageSummaryPayload,
  type ProviderUsageSummaryPayload,
  type SessionLimitAllocationSummaryPayload,
  type StorageHealthPayload,
} from "../../lib/ipc"
import {
  cancelChecksReport,
  getChecksReport,
  onChecksReportChanged,
  type ChecksReportPayload,
} from "../../lib/insightsIpc"
import { costOutlierThreshold } from "../../lib/presentation/sessionAnalysis"
import {
  applyLifecycleEvent,
  IDLE_LIVENESS,
  isLive,
  livenessExpiry,
  livenessFromSnapshot,
  liveModels,
  liveProviders,
  type Liveness,
} from "../../lib/sessionLiveness"
import { liveDisplayableProviders, liveWindows } from "../../lib/presentation/liveUsage"
import {
  isCurrentWindowVisible,
  isFloatingHudEnabled,
  isOverlayWindowVisible,
  openOverlayWindow,
} from "../../lib/overlayWindow"
import { isMacOS } from "../../lib/platform"
import { SurfaceExposureTracker, liveUsageObservations } from "../../lib/surfaceExposure"
import type { LocalRepositoryItem, LocalRepositoryStatus } from "../../lib/types/repository"

/**
 * The imperative boundary between the popover window and the shell.
 *
 * React reads immutable snapshots through `useSyncExternalStore`; IPC calls,
 * event subscriptions, and the window's own keyboard handling stay here,
 * where they belong to the external systems that created them rather than to
 * a component lifecycle. See `views/onboarding/OnboardingSession.ts` for the
 * same shape applied to the first-run window.
 */

export interface PopoverSnapshot {
  appVersion: string | null
  debugBuild: boolean
  settings: AppSettings | null
  entries: SessionListEntry[] | null
  /** True when the initial activity-list read failed. */
  entriesUnavailable: boolean
  repositories: LocalRepositoryItem[]
  /** Provider usage, or null while the first snapshot is in flight. */
  usage: ProviderUsageSummaryPayload | null
  liveUsage: LiveUsageSummaryPayload
  /** Whether a session is live, from the shell's lifecycle bus. */
  sessionLive: boolean
  /** The providers a live session draws on, sorted. Their meters blink. */
  liveProviders: readonly string[]
  /** The models a live session runs, sorted. A model-scoped meter reads this. */
  liveModels: readonly string[]
  sessionLimitAllocations: SessionLimitAllocationSummaryPayload
  /** Whether a `refreshUsage` call is in flight, for the limits section's spinner. */
  usageRefreshing: boolean
  /** The real local report rendered by the Activity summary and anchored preview. */
  checksReport: ChecksReportPayload | null
  /** True when the latest Checks report request fails. */
  checksUnavailable: boolean
  storage: StorageHealthPayload
  /** Banners the reader has waved away this run. */
  dismissed: readonly AttentionKind[]
  /**
   * A timestamp bumped every `NOW_TICK_MS` while the popover is visible.
   *
   * The activity list uses it for day boundaries and future-time checks.
   */
  now: number
}

/**
 * How often the store updates relative activity times while the popover is visible.
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

/** The longest delay a browser timer accepts, in milliseconds. */
const MAX_TIMEOUT_MS = 2_147_483_647

/** Minimum spacing between cached session-allocation reads. */
const SESSION_LIMIT_ALLOCATION_REFRESH_MIN_MS = 30_000

function liveUsageActive(settings: AppSettings | null): boolean {
  return Boolean(settings?.liveUsageEnabled && settings.onboardingCompleted)
}

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
  private analyticsVisibilityRevision = 0
  private analyticsVisible = false
  private readonly exposure = new SurfaceExposureTracker()
  private checksToken = 0
  private checksConsumerId: string | null = null
  private checksRefresh: Promise<void> | null = null
  private checksRefreshQueued = false
  /**
   * How many `refreshUsage` calls are currently in flight.
   *
   * A counter rather than a boolean: the popover-shown signal and a
   * scan-finished event can each trigger a refresh close together, and the
   * first one to settle must not clear the spinner out from under the one
   * still running. The snapshot's `usageRefreshing` is `count > 0`.
   */
  private usageRefreshCount = 0
  private liveUsageRevision = 0
  private sessionLimitAllocationRefresh: Promise<void> | null = null
  private sessionLimitAllocationRefreshPending = false
  private sessionLimitAllocationRefreshTimer: ReturnType<typeof setTimeout> | null = null
  private lastSessionLimitAllocationRefreshAt = 0
  private sessionLimitAllocationResultRevision = 0
  private initialContentReady = false
  private contentReadyReportedGeneration: number | null = null
  private contentReadyReportInFlightGeneration: number | null = null
  private contentReadyRetryGeneration: number | null = null

  /** Set while a coalesced `refreshEntries` call is in flight. */
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
  private stopSessionLifecycleListening: (() => void) | null = null
  private liveness: Liveness = IDLE_LIVENESS
  private livenessRevision = 0
  private livenessExpiry: number | null = null

  private snapshot: PopoverSnapshot = {
    appVersion: null,
    debugBuild: false,
    settings: null,
    entries: null,
    entriesUnavailable: false,
    repositories: [],
    usage: null,
    liveUsage: EMPTY_LIVE_USAGE,
    sessionLive: false,
    liveProviders: [],
    liveModels: [],
    sessionLimitAllocations: EMPTY_SESSION_LIMIT_ALLOCATIONS,
    usageRefreshing: false,
    checksReport: null,
    checksUnavailable: false,
    storage: HEALTHY_STORAGE,
    dismissed: [],
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
    void this.startPopoverVisibility(generation)
    void this.listenLiveUsage(generation)
    void this.listenSessionLifecycle(generation)

    // ⌘, opens Settings — the platform's standard preferences shortcut, which
    // an accessory app with no application menu has to own itself. Bound
    // alongside Escape on `window`, deliberately: it is the last object in an
    // event's propagation path, so every surface listening on `document` has
    // already had its chance to claim the key first.
    window.addEventListener("keydown", this.onWindowKeyDown)

    // A visible activity list needs its clock running.
    this.syncNowTicking()

    // R6: the session starts visible (see `visible`'s doc comment), so its
    // usage poll starts immediately rather than waiting for a `popover:shown`
    // that may already have fired.
    this.startUsagePolling()
  }

  private stop(): void {
    this.started = false
    this.generation += 1
    this.analyticsVisibilityRevision += 1
    this.analyticsVisible = false
    this.exposure.suspend()
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
    this.stopSessionLifecycleListening?.()
    this.stopSessionLifecycleListening = null
    this.clearLivenessExpiry()
    this.liveness = IDLE_LIVENESS
    this.stopNowTicking()
    this.stopUsagePolling()
    this.cancelSessionLimitAllocationRefresh()
    this.checksRefreshQueued = false
    const checksConsumerId = this.checksConsumerId
    this.checksConsumerId = null
    if (checksConsumerId) void cancelChecksReport(checksConsumerId)
    window.removeEventListener("keydown", this.onWindowKeyDown)
  }

  // First load: read independent shell state together, then list sessions for
  // the stored time window. The cached limits do not wait for either read.
  private loadInitial = async (generation: number): Promise<void> => {
    const usage = this.loadCachedUsage(generation)
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
    await Promise.all([this.loadInitialEntries(stored.activityWindowDays, generation), usage])
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
      const wasLiveUsageActive = liveUsageActive(this.snapshot.settings)
      applyTheme(settings.theme)
      this.update({ settings })
      if (!liveUsageActive(settings)) {
        this.cancelSessionLimitAllocationRefresh()
      } else if (!wasLiveUsageActive && this.visible) {
        this.requestSessionLimitAllocationRefresh(true)
      }
      if (
        settings.activityWindowDays !== previousDays ||
        settings.disabledAgents.join(",") !== previousDisabled
      ) {
        this.sessionLimitAllocationResultRevision += 1
        void this.refreshEntries(settings.activityWindowDays).catch(() => {})
        this.requestSessionLimitAllocationRefresh(false, true)
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
      this.requestSessionLimitAllocationRefresh(false, true)
      this.refreshLiveness(generation)
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopSessionsInvalidatedListening = unlisten
  }

  // The shell pushes one changed row so the activity pills stay current.
  // This avoids a full list query for each analysis cache write.
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
      this.requestSessionLimitAllocationRefresh(false, true)
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
  // one).
  //
  // Entries are also refetched here, even though the scan scheduler now
  // ticks unconditionally and `listenScanEvent` above is the primary path:
  // this is a cheap defence against a `scan:finished` event missed while the
  // popover was hidden, so a reader never sees a stale list for a whole tick.
  private listenPopoverShown = async (generation: number): Promise<void> => {
    const unlisten = await onPopoverShown(() => {
      if (generation !== this.generation) return
      this.analyticsVisibilityRevision += 1
      this.analyticsVisible = true
      this.visible = true
      this.update({ now: Date.now() })
      this.syncNowTicking()
      this.startUsagePolling()
      this.requestSessionLimitAllocationRefresh(true, true)
      if (this.initialContentReady) this.reportContentReady(true)
      void this.restoreFloatingHud(generation)
      void this.refreshEntries(this.windowDays()).catch(() => {})
      void this.refreshUsage()
      this.refreshLiveness(generation)
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
      this.analyticsVisibilityRevision += 1
      this.analyticsVisible = false
      this.exposure.conceal()
      this.visible = false
      this.sessionLimitAllocationResultRevision += 1
      this.syncNowTicking()
      this.stopUsagePolling()
      this.cancelSessionLimitAllocationRefresh()
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopPopoverHiddenListening = unlisten
  }

  // The usage meter sweeps from the same bus the HUD reads. A keyed session
  // turns on at `started` or `activity` and off at `quiet` or `idle`. The
  // local clock closes the same 30 s window for a snapshot, a missed event,
  // and keyless activity, a write the store has not indexed yet. The snapshot
  // on start, on show, and on invalidation puts the set right after a missed
  // event.
  private listenSessionLifecycle = async (generation: number): Promise<void> => {
    this.refreshLiveness(generation)
    const unlisten = await onSessionLifecycle((event) => {
      if (generation !== this.generation) return
      // A snapshot still in flight predates this event and must not replace it.
      this.livenessRevision += 1
      this.applyLiveness(applyLifecycleEvent(this.liveness, event), generation)
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopSessionLifecycleListening = unlisten
  }

  private refreshLiveness(generation: number): void {
    const revision = ++this.livenessRevision
    void getLiveSessions()
      .then((sessions) => {
        if (generation !== this.generation || revision !== this.livenessRevision) return
        this.applyLiveness(livenessFromSnapshot(sessions, this.liveness), generation)
      })
      .catch(() => {})
  }

  private applyLiveness(next: Liveness, generation: number): void {
    this.liveness = next
    this.clearLivenessExpiry()
    const now = Date.now()
    const sessionLive = isLive(next, now)
    const providers = liveProviders(next, now)
    const models = liveModels(next, now)
    if (
      sessionLive !== this.snapshot.sessionLive ||
      !sameList(providers, this.snapshot.liveProviders) ||
      !sameList(models, this.snapshot.liveModels)
    ) {
      this.update({ sessionLive, liveProviders: providers, liveModels: models })
    }
    const expiresAt = livenessExpiry(next, now)
    if (expiresAt == null) return
    this.livenessExpiry = window.setTimeout(
      () => {
        this.livenessExpiry = null
        if (generation === this.generation) this.applyLiveness(this.liveness, generation)
      },
      Math.min(expiresAt - now + 1, MAX_TIMEOUT_MS),
    )
  }

  private clearLivenessExpiry(): void {
    if (this.livenessExpiry == null) return
    window.clearTimeout(this.livenessExpiry)
    this.livenessExpiry = null
  }

  private startPopoverVisibility = async (generation: number): Promise<void> => {
    await Promise.allSettled([
      this.listenPopoverShown(generation),
      this.listenPopoverHidden(generation),
    ])
    if (generation === this.generation) await this.bootstrapAnalyticsVisibility(generation)
  }

  private bootstrapAnalyticsVisibility = async (generation: number): Promise<void> => {
    const revision = this.analyticsVisibilityRevision
    const visible = await isCurrentWindowVisible()
    if (generation !== this.generation || revision !== this.analyticsVisibilityRevision) return
    if (!visible) {
      this.exposure.conceal()
      return
    }
    this.analyticsVisible = true
    this.syncAnalyticsExposure()
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
    await openOverlayWindow("automatic").catch(() => {})
  }

  private listenLiveUsage = async (generation: number): Promise<void> => {
    const unlisten = await onLiveUsageChanged((liveUsage) => {
      if (generation !== this.generation) return
      this.liveUsageRevision += 1
      this.update({ liveUsage })
      this.requestSessionLimitAllocationRefresh()
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

  private loadInitialEntries = async (days: number, generation: number): Promise<void> => {
    try {
      await this.refreshEntries(days, generation)
    } catch {
      if (generation === this.generation && this.snapshot.entries === null) {
        this.update({ entries: [] })
      }
    }
  }

  private refreshEntries = async (
    days: number,
    generation = this.generation,
  ): Promise<void> => {
    try {
      const payloads = await listRecentSessions(days)
      if (generation !== this.generation) return
      this.update({ entries: toActivityEntries(payloads), entriesUnavailable: false })
    } catch (error) {
      if (
        generation === this.generation &&
        (this.snapshot.entries === null || this.snapshot.entries.length === 0)
      ) {
        this.update({ entriesUnavailable: true })
      }
      throw error
    }
  }

  private loadCachedUsage = async (generation: number): Promise<void> => {
    const liveUsageRevision = this.liveUsageRevision
    const [usage, liveUsage] = await Promise.all([
      getProviderUsage().catch(() => EMPTY_PROVIDER_USAGE),
      getLiveUsage().catch(() => EMPTY_LIVE_USAGE),
    ])
    if (generation !== this.generation) return
    if (liveUsageRevision === this.liveUsageRevision) {
      this.update({ usage, liveUsage })
    } else {
      this.update({ usage })
    }
    this.requestSessionLimitAllocationRefresh(true, true)
  }

  private refreshUsage = async (generation = this.generation): Promise<void> => {
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
          .then((usage) => {
            if (generation === this.generation) this.update({ usage })
          })
          .catch(() => undefined),
        refreshLiveUsage()
          .then((liveUsage) => {
            if (generation !== this.generation) return
            this.liveUsageRevision += 1
            this.update({ liveUsage })
          })
          .catch(() => undefined),
      ])
    } finally {
      this.usageRefreshCount -= 1
      if (generation === this.generation) {
        this.update({ usageRefreshing: this.usageRefreshCount > 0 })
      }
    }
  }

  private requestSessionLimitAllocationRefresh(
    immediate = false,
    includeDisabledCache = false,
  ): void {
    if (
      !this.started ||
      !this.visible ||
      (!includeDisabledCache && !liveUsageActive(this.snapshot.settings))
    )
      return
    this.sessionLimitAllocationRefreshPending = true
    if (immediate) this.lastSessionLimitAllocationRefreshAt = 0
    if (immediate && this.sessionLimitAllocationRefreshTimer !== null) {
      clearTimeout(this.sessionLimitAllocationRefreshTimer)
      this.sessionLimitAllocationRefreshTimer = null
    }
    if (this.sessionLimitAllocationRefresh || this.sessionLimitAllocationRefreshTimer !== null)
      return
    this.scheduleSessionLimitAllocationRefresh(immediate)
  }

  private scheduleSessionLimitAllocationRefresh(immediate = false): void {
    const delay = immediate
      ? 0
      : Math.max(
          0,
          this.lastSessionLimitAllocationRefreshAt +
            SESSION_LIMIT_ALLOCATION_REFRESH_MIN_MS -
            Date.now(),
        )
    if (delay > 0) {
      this.sessionLimitAllocationRefreshTimer = setTimeout(() => {
        this.sessionLimitAllocationRefreshTimer = null
        this.runSessionLimitAllocationRefresh()
      }, delay)
      return
    }
    this.runSessionLimitAllocationRefresh()
  }

  private runSessionLimitAllocationRefresh = (): void => {
    if (!this.sessionLimitAllocationRefreshPending || !this.visible) return
    this.sessionLimitAllocationRefreshPending = false
    this.lastSessionLimitAllocationRefreshAt = Date.now()
    const generation = this.generation
    const resultRevision = this.sessionLimitAllocationResultRevision
    this.sessionLimitAllocationRefresh = getSessionLimitAllocations()
      .then((sessionLimitAllocations) => {
        if (
          generation === this.generation &&
          this.visible &&
          resultRevision === this.sessionLimitAllocationResultRevision
        ) {
          this.update({ sessionLimitAllocations })
        }
      })
      .catch(() => undefined)
      .finally(() => {
        this.sessionLimitAllocationRefresh = null
        if (this.sessionLimitAllocationRefreshPending) {
          this.scheduleSessionLimitAllocationRefresh()
        }
      })
  }

  private cancelSessionLimitAllocationRefresh(): void {
    if (this.sessionLimitAllocationRefreshTimer !== null) {
      clearTimeout(this.sessionLimitAllocationRefreshTimer)
      this.sessionLimitAllocationRefreshTimer = null
    }
    this.sessionLimitAllocationRefreshPending = false
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

  /** Run the clock while the popover is visible. */
  private syncNowTicking(): void {
    if (this.visible) {
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
      this.update({ checksReport, checksUnavailable: false })
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

  private update(change: Partial<PopoverSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    this.syncAnalyticsExposure()
    for (const listener of this.listeners) listener()
  }

  private syncAnalyticsExposure(): void {
    if (!this.analyticsVisible) return
    const generation = this.exposure.expose({ surface: "activity", origin: "user" })
    const totals = this.snapshot.usage?.totals
    const hasLocalUsage =
      totals !== undefined &&
      [totals.today, totals.week, totals.monthToDate, totals.last30Days].some(
        (window) => window.sessionCount > 0,
      )
    const hasLiveReading = liveDisplayableProviders(this.snapshot.liveUsage).some(
      (provider) => liveWindows(provider).length > 0,
    )
    const hasLiveError = liveUsageObservations(this.snapshot.liveUsage).some(
      ({ state }) => state !== "fresh" && state !== "stale",
    )
    const hasData = (this.snapshot.entries?.length ?? 0) > 0 || hasLocalUsage || hasLiveReading
    if (hasData) {
      this.exposure.observe("ready", generation)
    } else if (this.snapshot.entriesUnavailable || hasLiveError) {
      this.exposure.observe("error", generation)
    } else if (this.snapshot.entries !== null && this.snapshot.usage !== null) {
      this.exposure.observe("empty", generation)
    }
    this.exposure.observeLiveUsage(this.snapshot.liveUsage, undefined, generation)
  }
}

function sameList(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((item, index) => item === right[index])
}

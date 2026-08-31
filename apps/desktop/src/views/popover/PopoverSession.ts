import type { SessionListEntry } from "../../components/session/SessionList"
import { indexOfSession, toActivityEntries, toActivityEntry } from "../../lib/activityEntries"
import { applyTheme } from "../../lib/appearance"
import type { AttentionKind } from "../../lib/attention"
import {
  DEFAULT_SETTINGS,
  EMPTY_LIVE_USAGE,
  EMPTY_PROVIDER_USAGE,
  appInfo,
  getLiveUsage,
  getProviderUsage,
  getSessionAnalysis,
  getSessionAnalysisFingerprint,
  getSettings,
  getStorageHealth,
  getSubagentAnalysis,
  HEALTHY_STORAGE,
  hidePopover,
  listRecentSessions,
  listRepositories,
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
  type AppSettings,
  type LiveUsageSummaryPayload,
  type ProviderUsageSummaryPayload,
  type SessionAnalysisPayload,
  type StorageHealthPayload,
} from "../../lib/ipc"
import {
  popoverHeightFor,
  prefersReducedMotion,
  type PopoverSurface,
} from "../../lib/popoverHeight"
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

/** Whether the settled analysis for `key` has nothing usable. */
function isUnavailableAnalysis(state: PopoverAnalysisState, key: string): boolean {
  if (state === null || state.key !== key) return false
  if (state.error) return true
  return (
    state.payload !== null &&
    state.payload.supportsAnalysis &&
    state.payload.summary === null &&
    state.payload.cost === null
  )
}

/**
 * Whether the settled analysis for `key` reports the drilldown as still
 * pending: the worker has not published a row set for this session yet, so
 * the shell served a placeholder payload rather than a real read.
 */
function isAnalysisPending(state: PopoverAnalysisState, key: string): boolean {
  return (
    state !== null &&
    state.key === key &&
    state.payload !== null &&
    state.payload.analysisPending
  )
}

/**
 * Whether the settled analysis for `key` reports itself as stale: it comes
 * from a published fence a fresher pass is already queued or running
 * behind, or whose transcript has since moved on. The data on screen is
 * real, so this does not gate what renders — it only keeps the poll running
 * so the fresh pass swaps in once the worker publishes it.
 */
function isAnalysisStale(state: PopoverAnalysisState, key: string): boolean {
  return (
    state !== null && state.key === key && state.payload !== null && state.payload.analysisStale
  )
}

export interface PopoverSnapshot {
  appVersion: string | null
  debugBuild: boolean
  settings: AppSettings | null
  entries: SessionListEntry[] | null
  repositories: LocalRepositoryItem[]
  /** Provider usage, or null while the first snapshot is in flight. */
  usage: ProviderUsageSummaryPayload | null
  liveUsage: LiveUsageSummaryPayload
  /** Whether a `refreshUsage` call is in flight, for the limits section's spinner. */
  usageRefreshing: boolean
  /** Whether the full Usage view is showing over the activity list. */
  showUsage: boolean
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
 * Fetch one subject's transcript fingerprint, for the live-detail poll.
 *
 * The parent fingerprint covers the parent transcript and every sub-agent
 * transcript, so a sub-agent subject fingerprints its parent session.
 */
async function loadAnalysisFingerprint(subject: SessionSubject): Promise<string> {
  const sessionId = subject.subagent?.parentSessionId ?? subject.sessionId
  return getSessionAnalysisFingerprint(subject.agent, sessionId, subject.wslDistro)
}

/** How often the open detail pane checks its transcript for new activity. */
export const ANALYSIS_POLL_MS = 10_000

/** How many times a subject can re-read an unavailable analysis. */
const MAX_UNAVAILABLE_ANALYSIS_RETRIES = 3

/**
 * How often the store forces a snapshot change while a detail pane is open,
 * so the header's relative-time text ("last just now") stays current even
 * when nothing else about the session has changed.
 */
const NOW_TICK_MS = 30_000

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
  private liveUsageRevision = 0
  private initialContentReady = false
  private contentReadyReportedGeneration: number | null = null
  private contentReadyReportInFlightGeneration: number | null = null
  private contentReadyRetryGeneration: number | null = null

  /**
   * The key of the subject the poll's fingerprint belongs to, and the last
   * fingerprint fetched for it. `null` fingerprint means no baseline has
   * landed yet, so a poll tick must not treat it as a change.
   */
  private analysisFingerprintKey: string | null = null
  private analysisFingerprint: string | null = null
  private analysisRetryKey: string | null = null
  private analysisRetryCount = 0
  /** Set while a poll tick's fetch is in flight, so ticks never overlap. */
  private analysisPollInFlight = false
  private analysisPollTimer: ReturnType<typeof setInterval> | null = null
  private nowTickTimer: ReturnType<typeof setInterval> | null = null

  private stopSettingsListening: (() => void) | null = null
  private stopSessionsInvalidatedListening: (() => void) | null = null
  private stopSessionEntryChangedListening: (() => void) | null = null
  private stopStorageHealthListening: (() => void) | null = null
  private stopScanListening: (() => void) | null = null
  private stopPopoverShownListening: (() => void) | null = null
  private stopLiveUsageListening: (() => void) | null = null

  private snapshot: PopoverSnapshot = {
    appVersion: null,
    debugBuild: false,
    settings: null,
    entries: null,
    repositories: [],
    usage: null,
    liveUsage: EMPTY_LIVE_USAGE,
    usageRefreshing: false,
    showUsage: false,
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
      this.analysisFingerprintKey = null
      this.analysisFingerprint = null
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
    this.initialContentReady = false

    void this.loadInitial(generation)
    void this.listenSettings(generation)
    void this.listenSessionsInvalidated(generation)
    void this.listenSessionEntryChanged(generation)
    void this.listenStorageHealth(generation)
    void this.listenScanEvent(generation)
    void this.listenPopoverShown(generation)
    void this.listenLiveUsage(generation)

    // ⌘, opens Settings — the platform's standard preferences shortcut, which
    // an accessory app with no application menu has to own itself. Bound
    // alongside Escape on `window`, deliberately: it is the last object in an
    // event's propagation path, so every surface listening on `document` has
    // already had its chance to claim the key first.
    window.addEventListener("keydown", this.onWindowKeyDown)

    // A stack carried over from a previous start (the window never really
    // unmounts, but the listener count can still hit zero and come back)
    // needs a fresh fingerprint baseline before the poll resumes on it.
    const top = this.snapshot.stack.at(-1)
    if (top) {
      const key = sessionKey(top)
      this.analysisFingerprintKey = key
      this.analysisFingerprint = null
      this.seedAnalysisFingerprint(top, generation, key)
    }
    this.syncDetailTimers()
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
    this.stopStorageHealthListening?.()
    this.stopStorageHealthListening = null
    this.stopScanListening?.()
    this.stopScanListening = null
    this.stopPopoverShownListening?.()
    this.stopPopoverShownListening = null
    this.stopLiveUsageListening?.()
    this.stopLiveUsageListening = null
    this.stopAnalysisPolling()
    this.stopNowTicking()
    this.analysisFingerprintKey = null
    this.analysisFingerprint = null
    this.analysisRetryKey = null
    this.analysisRetryCount = 0
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
  // without a full re-query.
  private listenSessionEntryChanged = async (generation: number): Promise<void> => {
    const unlisten = await onSessionEntryChanged((entry) => {
      if (generation !== this.generation) return
      const entries = this.snapshot.entries
      if (!entries) return
      const index = indexOfSession(entries, entry.agent, entry.sessionId, entry.wslDistro)
      // A session outside the current window is the next scan's business, so
      // an entry with no match here is not inserted.
      if (index === -1) return
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
      this.update({ entries: next })
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopSessionEntryChangedListening = unlisten
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
  // progress, so the intermediate phases have nothing to say.
  private listenScanEvent = async (generation: number): Promise<void> => {
    const unlisten = await onScanEvent((_status, phase) => {
      if (generation !== this.generation) return
      if (phase !== "finished") return
      void this.refreshEntries(this.windowDays()).catch(() => {})
      void this.refreshUsage()
      void this.refreshRepositoryList()
    })
    if (generation !== this.generation) {
      unlisten()
      return
    }
    this.stopScanListening = unlisten
  }

  // The shell's own signal that the popover just reached the screen —
  // separate from the scan events above on purpose. `note_shown` also kicks
  // a disk scan, but that kick is silently skipped while discovery is
  // paused or onboarding is unfinished, and even a scan that does run can
  // take a while to finish. Neither has any bearing on a provider's own
  // stated limits, so usage gets its own refresh here rather than waiting on
  // — or being silenced by — the scan pipeline. Entries and the repository
  // list are already covered by `listenScanEvent` above. The open session's
  // analysis is not: its transcript can grow while the popover is hidden,
  // and nothing else asks for it again until the reader navigates.
  private listenPopoverShown = async (generation: number): Promise<void> => {
    const unlisten = await onPopoverShown(() => {
      if (generation !== this.generation) return
      if (this.initialContentReady) this.reportContentReady(true)
      void this.restoreFloatingHud(generation)
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

  private refreshRepositoryList = async (): Promise<void> => {
    const payloads = await listRepositories().catch(() => [])
    this.update({
      repositories: payloads.map((payload) => ({
        ...payload,
        status: repositoryStatus(payload.status),
      })),
    })
  }

  /**
   * Load a subject's analysis, then seed the live poll's fingerprint baseline
   * from the freshly-loaded transcript, and make sure the poll and the
   * relative-time ticker are running — a detail pane is now open.
   *
   * The baseline is fetched only after `loadAnalysisFor` settles, not next to
   * it: that way the poll's first tick compares against a transcript state at
   * least as new as what is already on screen, and never fires a refresh on
   * data the reader has not even seen yet.
   */
  private openAnalysis(subject: SessionSubject): void {
    const generation = this.generation
    const key = sessionKey(subject)
    this.analysisFingerprintKey = key
    this.analysisFingerprint = null
    this.analysisRetryKey = key
    this.analysisRetryCount = 0
    void this.loadAnalysisFor(subject).then(() => {
      if (generation !== this.generation || key !== this.analysisFingerprintKey) return
      this.seedAnalysisFingerprint(subject, generation, key)
    })
    this.syncDetailTimers()
  }

  private seedAnalysisFingerprint(
    subject: SessionSubject,
    generation: number,
    key: string,
  ): void {
    void loadAnalysisFingerprint(subject)
      .then((fingerprint) => {
        if (generation !== this.generation || key !== this.analysisFingerprintKey) return
        this.analysisFingerprint = fingerprint
      })
      .catch(() => {})
  }

  /**
   * One poll tick: fetch the open subject's fingerprint and, if it moved
   * since the last tick (or the seed), re-load the analysis. Overlapping
   * ticks are dropped rather than queued — a slow fetch is left to finish on
   * its own, and the next interval simply tries again.
   *
   * A pending analysis (the worker has not published rows for this session
   * yet) or a stale one (rows are published, but a fresher pass is queued or
   * running behind them) re-loads on every tick, unbounded, whether or not
   * the fingerprint moved: there is nothing newer to compare against until
   * the worker's next pass lands, and that pass is what this poll is
   * waiting for. That check runs before, and short-circuits, the bounded
   * unavailable-analysis retry below, which answers a different question —
   * a published pass that turned up nothing to show.
   */
  private tickAnalysisPoll = (): void => {
    if (this.analysisPollInFlight) return
    const subject = this.snapshot.stack.at(-1)
    if (!subject) return
    const generation = this.generation
    const key = sessionKey(subject)
    this.analysisPollInFlight = true
    void loadAnalysisFingerprint(subject)
      .then((fingerprint) => {
        if (generation !== this.generation || key !== this.analysisFingerprintKey) return
        const previous = this.analysisFingerprint
        this.analysisFingerprint = fingerprint
        if (previous !== null && fingerprint !== previous) {
          void this.refreshAnalysis()
          return
        }
        if (
          isAnalysisPending(this.snapshot.analysis, key) ||
          isAnalysisStale(this.snapshot.analysis, key)
        ) {
          void this.refreshAnalysis()
          return
        }
        if (subject.subagent) return
        if (key !== this.analysisRetryKey) {
          this.analysisRetryKey = key
          this.analysisRetryCount = 0
        }
        if (
          isUnavailableAnalysis(this.snapshot.analysis, key) &&
          this.analysisRetryCount < MAX_UNAVAILABLE_ANALYSIS_RETRIES
        ) {
          this.analysisRetryCount += 1
          void this.refreshAnalysis()
        }
      })
      .catch(() => {})
      .finally(() => {
        this.analysisPollInFlight = false
      })
  }

  /** Start or stop the poll and the relative-time ticker to match whether a detail pane is open. */
  private syncDetailTimers(): void {
    if (this.snapshot.stack.length > 0) {
      this.startAnalysisPolling()
      this.startNowTicking()
    } else {
      this.stopAnalysisPolling()
      this.stopNowTicking()
    }
  }

  private startAnalysisPolling(): void {
    if (this.analysisPollTimer !== null) return
    this.analysisPollTimer = setInterval(this.tickAnalysisPoll, ANALYSIS_POLL_MS)
  }

  private stopAnalysisPolling(): void {
    if (this.analysisPollTimer === null) return
    clearInterval(this.analysisPollTimer)
    this.analysisPollTimer = null
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

  private loadAnalysisFor = (subject: SessionSubject): Promise<void> => {
    const key = sessionKey(subject)
    const generation = this.generation
    const token = ++this.analysisToken
    return loadAnalysis(subject)
      .then((payload) => {
        if (generation !== this.generation || token !== this.analysisToken) return
        this.update({ analysis: { key, payload, error: false } })
        if (!isUnavailableAnalysis(this.snapshot.analysis, key)) {
          this.analysisRetryKey = key
          this.analysisRetryCount = 0
        }
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
    void setPopoverHeight(popoverHeightFor(this.surface()), !prefersReducedMotion()).catch(
      () => {},
    )
  }

  private update(change: Partial<PopoverSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }
}

import type { SessionListEntry } from "../../components/session/SessionList"
import {
  getAllowanceUsage,
  getLiveUsage,
  getMainWindowVisible,
  getProviderUsage,
  getSessionLimitAllocations,
  mainWindowContentReady,
  onLiveUsageChanged,
  onMainWindowVisibilityChanged,
  onSessionIndexChanged,
  onSessionUpdated,
  type SessionIndexChangedPayload,
  type SessionUpdatedPayload,
} from "../../lib/ipc"
import { insightsBacklogStore } from "../../lib/insightsBacklogStore"
import type {
  AllowanceUsageSummaryPayload,
  LiveUsageSummaryPayload,
  ProviderUsageSummaryPayload,
  SessionLimitAllocationSummaryPayload,
} from "../../lib/providerUsageIpc"
import { scanStatusStore } from "../../lib/scanStatusStore"
import { readOverviewViewPrefs, writeOverviewViewPrefs } from "./overview/overviewViewPrefs"

export interface MainOverviewAdapter {
  getUsage(): Promise<ProviderUsageSummaryPayload>
  getAllowanceUsage(): Promise<AllowanceUsageSummaryPayload>
  getLiveUsage(): Promise<LiveUsageSummaryPayload>
  getSessionLimitAllocations(): Promise<SessionLimitAllocationSummaryPayload>
  getVisible(): Promise<boolean>
  onVisible(handler: (visible: boolean) => void): Promise<() => void>
  onLiveUsageChanged(handler: (usage: LiveUsageSummaryPayload) => void): Promise<() => void>
  onSessionIndexChanged(
    handler: (change: SessionIndexChangedPayload) => void,
  ): Promise<() => void>
  onSessionUpdated(handler: (update: SessionUpdatedPayload) => void): Promise<() => void>
}

export interface MainOverviewSessionListSource {
  getSnapshot(): { entries: SessionListEntry[] | null }
  subscribeList(listener: () => void): () => void
}

/** Whether a scan pass is in flight, and a way to hear when that changes.
 *  Narrowed to the one field this page acts on, so a test can stand it up
 *  without the rest of a `ScanStatus`. */
export interface MainOverviewScanSource {
  getSnapshot(): { running: boolean }
  subscribe(listener: () => void): () => void
}

/** Whether the insights worker pool has a backlog to drain, and a way to
 *  hear when that changes. Narrowed to the one field this page acts on, like
 *  `MainOverviewScanSource`. */
export interface MainOverviewBacklogSource {
  getSnapshot(): { active: boolean }
  subscribe(listener: () => void): () => void
}

/**
 * How long an event-driven read waits for the burst around it to stop.
 *
 * The session bus publishes one event per indexed session. A scan pass
 * therefore lands dozens of them in a few seconds, and each one used to run
 * the page's four reads again. The dirty flags already coalesce events that
 * arrive while a read is in flight; this covers the ones either side of it.
 */
const OVERVIEW_REFRESH_DEBOUNCE_MS = 300

/**
 * The longest the page holds its reads waiting for a scan pass to finish.
 *
 * The hold below is an optimisation, not a correctness requirement. A pass
 * that stalls, or a status that never reports its end, must cost the reader a
 * stale page for a few seconds rather than forever.
 */
const OVERVIEW_SCAN_HOLD_CAP_MS = 15_000

/**
 * How long the page throttles its reads while the insights worker pool has a
 * backlog to drain.
 *
 * One read every 5 s still reads as progress to a reader watching the page.
 * The worker's per-session events land a few hundred milliseconds apart while
 * a backlog drains. That gap is longer than `OVERVIEW_REFRESH_DEBOUNCE_MS`, so
 * the debounce fires between events and the page reads after almost every one.
 */
const OVERVIEW_BACKLOG_THROTTLE_MS = 5_000

/** Which of the page's reads an event asked for. */
type OverviewReadKind = "totals" | "allowance" | "allocations"

export interface MainOverviewSessionOptions {
  scanSource?: MainOverviewScanSource
  backlogSource?: MainOverviewBacklogSource
  debounceMs?: number
  backlogThrottleMs?: number
  scanHoldCapMs?: number
  /** Told whenever a settled read finds a new answer to whether this reader
   *  has a subscription plan. The production default remembers it for the
   *  Overview's next run. */
  rememberPlan?: (hadPlan: boolean) => void
}

const productionScanSource: MainOverviewScanSource = {
  // The store snapshot is null until its first load resolves. Treat that gap
  // as a running scan: the hold cap bounds the cost of a wrong guess, and the
  // first read of an activation runs regardless.
  getSnapshot: () => ({ running: scanStatusStore.getSnapshot()?.running ?? true }),
  subscribe: (listener) => scanStatusStore.subscribe(listener),
}

const productionBacklogSource: MainOverviewBacklogSource = {
  // The store snapshot is null until its first load resolves. Unlike the
  // scan hold, the throttle is an optimisation, not a correctness
  // requirement: treating that gap as "not active" costs a wrong guess a few
  // extra reads only, so this defaults to false rather than to true.
  getSnapshot: () => ({ active: insightsBacklogStore.getSnapshot()?.active ?? false }),
  subscribe: (listener) => insightsBacklogStore.subscribe(listener),
}

function rememberPlanInPrefs(hadPlan: boolean): void {
  writeOverviewViewPrefs({ hadSubscriptionPlan: hadPlan })
}

const productionAdapter: MainOverviewAdapter = {
  getUsage: () => getProviderUsage(),
  getAllowanceUsage: () => getAllowanceUsage(),
  getLiveUsage: () => getLiveUsage(),
  getSessionLimitAllocations: () => getSessionLimitAllocations(),
  getVisible: () => getMainWindowVisible(),
  onVisible: (handler) => onMainWindowVisibilityChanged(handler),
  onLiveUsageChanged: (handler) => onLiveUsageChanged(handler),
  onSessionIndexChanged: (handler) => onSessionIndexChanged(handler),
  onSessionUpdated: (handler) => onSessionUpdated(handler),
}

/**
 * Whether one row update can move the Overview's usage totals.
 *
 * A title-only change re-reads the recent rows alone. The `usage`, `checks`,
 * and `limits` facets are reserved by the bus; the page already honours them.
 */
export function overviewUpdateTouchesTotals(update: SessionUpdatedPayload): boolean {
  const facets = update.facets
  return Boolean(
    facets.metadata || facets.analysis || facets.usage || facets.checks || facets.limits,
  )
}

/** The most recent sessions the Overview page shows. */
export const OVERVIEW_RECENT_SESSION_COUNT = 3

export interface MainOverviewSnapshot {
  active: boolean
  /** The local usage summary, or null before the first successful read. */
  usage: ProviderUsageSummaryPayload | null
  /** True when the newest local usage read failed and nothing replaced it. */
  usageError: boolean
  /** Null before the first successful allowance read. */
  allowance: AllowanceUsageSummaryPayload | null
  /** True while an allowance read is in flight and nothing is on the page. */
  allowanceLoading: boolean
  /** True when the newest allowance read failed and nothing replaced it. */
  allowanceError: boolean
  /** The provider limit snapshot, or null before the first successful read. */
  liveUsage: LiveUsageSummaryPayload | null
  /** True once the first live-usage read answers, success or failure, or a
   *  push arrives. Stays true afterwards: `liveUsage` is retained across
   *  deactivation, so this flag must not revert to a loading state. */
  liveUsageSettled: boolean
  /** The newest local sessions, or null before the first successful read. */
  recentSessions: SessionListEntry[] | null
  /** Each recent session's estimated limit share, or null before the first
   *  successful read. A failed read keeps the last value. */
  sessionLimitAllocations: SessionLimitAllocationSummaryPayload | null
  /** True while the first local usage read is in flight. */
  loading: boolean
  /** True while a later local usage read is in flight. */
  refreshing: boolean
}

function overviewRecentEntries(entries: readonly SessionListEntry[]): SessionListEntry[] {
  return [...entries]
    .sort((left, right) => right.timestamp.localeCompare(left.timestamp))
    .slice(0, OVERVIEW_RECENT_SESSION_COUNT)
}

// The shared list supplies recent sessions. This store reads usage only while Overview is visible.
export class MainOverviewSession {
  private readonly adapter: MainOverviewAdapter
  private snapshot: MainOverviewSnapshot = {
    active: false,
    usage: null,
    usageError: false,
    allowance: null,
    allowanceLoading: false,
    allowanceError: false,
    liveUsage: null,
    liveUsageSettled: false,
    recentSessions: null,
    sessionLimitAllocations: null,
    loading: false,
    refreshing: false,
  }
  private readonly listeners = new Set<() => void>()
  private readonly activeListeners = new Set<() => void>()
  private readonly stops: Array<() => void> = []
  private generation = 0
  private workVersion = 0
  private refreshVersion = 0
  private allowanceTask: Promise<void> | null = null
  private allowanceDirty = false
  private sessionLimitAllocationsTask: Promise<void> | null = null
  private sessionLimitAllocationsDirty = false
  /** Counts every live-usage push and read start. A read applies its result
   *  only while it still holds the newest version, so a push always wins
   *  over a read still in flight. */
  private liveUsageVersion = 0
  private visible = false
  private initialized = false
  private refreshTask: Promise<void> | null = null
  private refreshDirty = false
  private readonly sessionList: MainOverviewSessionListSource
  /** Whether the local usage and allowance reads have each settled once,
   *  success or error. `main_window_content_ready` fires once both have,
   *  timing when the Overview's first paint had something to show. */
  private usageSettled = false
  private allowanceSettled = false
  private contentReadyReported = false
  private readonly scanSource: MainOverviewScanSource
  private readonly backlogSource: MainOverviewBacklogSource
  private readonly debounceMs: number
  private readonly backlogThrottleMs: number
  private readonly scanHoldCapMs: number
  /** The reads events have asked for and the debounce has not run yet. */
  private readonly pendingReads = new Set<OverviewReadKind>()
  private debounceTimer: ReturnType<typeof setTimeout> | null = null
  /** Armed while the backlog is active and reads are pending. A single
   *  trailing timer: later events while it is armed do not reset it, so a
   *  steady stream of them still flushes once per `backlogThrottleMs`. */
  private throttleTimer: ReturnType<typeof setTimeout> | null = null
  private scanHoldTimer: ReturnType<typeof setTimeout> | null = null
  /** True once a scan pass has finished, or the cap has fired, since the
   *  page became active. The hold applies to the first pass only: scoped
   *  passes run all day from watcher bursts, and holding on every one of
   *  them would starve the ordinary updates the page exists to show. */
  private scanSettled = false
  private readonly rememberPlan: (hadPlan: boolean) => void
  /** What the last settled read found out about this reader's plans, so the
   *  page can open on the right unit next run instead of guessing. Starts
   *  from the previous run's memory, so a read that repeats that answer does
   *  not write it again. */
  private rememberedPlan: boolean | undefined

  constructor(
    sessionList: MainOverviewSessionListSource,
    adapter: MainOverviewAdapter = productionAdapter,
    options: MainOverviewSessionOptions = {},
  ) {
    this.sessionList = sessionList
    this.adapter = adapter
    this.scanSource = options.scanSource ?? productionScanSource
    this.backlogSource = options.backlogSource ?? productionBacklogSource
    this.debounceMs = options.debounceMs ?? OVERVIEW_REFRESH_DEBOUNCE_MS
    this.backlogThrottleMs = options.backlogThrottleMs ?? OVERVIEW_BACKLOG_THROTTLE_MS
    this.scanHoldCapMs = options.scanHoldCapMs ?? OVERVIEW_SCAN_HOLD_CAP_MS
    this.rememberPlan = options.rememberPlan ?? rememberPlanInPrefs
    this.rememberedPlan = readOverviewViewPrefs().hadSubscriptionPlan
  }

  getSnapshot = (): MainOverviewSnapshot => this.snapshot
  subscribe = (listener: () => void): (() => void) => this.attach(listener, true)
  subscribeInactive = (listener: () => void): (() => void) => this.attach(listener, false)

  private update(patch: Partial<MainOverviewSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    for (const listener of this.listeners) listener()
  }

  /**
   * Remember whether this reader turned out to have a subscription plan, so
   * the Overview's next run can open on the right unit instead of guessing
   * and correcting itself in front of the reader.
   *
   * Reads the snapshot just updated, so it must run after the `update` call
   * for the read it reports on. A detected plan is certain on its own and
   * writes at once. A reader with no plan looks exactly like one whose
   * allowance and live-usage reads have not both answered yet, so a negative
   * answer only writes once both have settled. Writes only when the answer
   * differs from what the session last remembered, so a read that repeats
   * the same answer costs nothing.
   */
  private notePlanObservation(): void {
    const hadPlan =
      (this.snapshot.allowance?.accounts.some((account) => account.plan != null) ?? false) ||
      (this.snapshot.liveUsage?.providers.some((provider) => provider.plan != null) ?? false)
    if (!hadPlan && !(this.allowanceSettled && this.snapshot.liveUsageSettled)) return
    if (hadPlan === this.rememberedPlan) return
    this.rememberedPlan = hadPlan
    this.rememberPlan(hadPlan)
  }

  /** Reports once both reads have settled. Takes the renderer generation
   *  the same way `PopoverSession.reportContentReady` does. */
  private reportContentReadyOnceSettled(): void {
    if (this.contentReadyReported || !this.usageSettled || !this.allowanceSettled) return
    const generation = window.__ANTIBURN_WINDOW_GENERATION__
    if (typeof generation !== "number" || !Number.isSafeInteger(generation)) return
    this.contentReadyReported = true
    void mainWindowContentReady(generation).catch(() => undefined)
  }

  private attach(listener: () => void, active: boolean): () => void {
    this.listeners.add(listener)
    if (active) this.activeListeners.add(listener)
    if (this.listeners.size === 1) void this.start()
    this.syncActive()
    return () => {
      this.listeners.delete(listener)
      this.activeListeners.delete(listener)
      this.syncActive()
      if (this.listeners.size === 0) this.dispose()
    }
  }

  private async listen(generation: number, pending: Promise<() => void>): Promise<void> {
    const stop = await pending.catch(() => null)
    if (!stop) return
    if (generation !== this.generation) stop()
    else this.stops.push(stop)
  }

  private async start(): Promise<void> {
    const generation = ++this.generation
    let visibilityRevision = 0
    const whenCurrent = (work: () => void) => () => {
      if (generation === this.generation) work()
    }
    await Promise.all([
      this.listen(
        generation,
        this.adapter.onVisible((visible) => {
          if (generation !== this.generation) return
          visibilityRevision += 1
          this.visible = visible
          this.syncActive()
        }),
      ),
      this.listen(
        generation,
        Promise.resolve(
          this.sessionList.subscribeList(() => {
            if (generation !== this.generation || !this.snapshot.active) return
            this.refreshRecentSessions()
          }),
        ),
      ),
      this.listen(
        generation,
        Promise.resolve(
          this.scanSource.subscribe(() => {
            if (generation !== this.generation) return
            this.onScanStateChanged()
          }),
        ),
      ),
      this.listen(
        generation,
        Promise.resolve(
          this.backlogSource.subscribe(() => {
            if (generation !== this.generation) return
            this.onBacklogStateChanged()
          }),
        ),
      ),
      this.listen(
        generation,
        this.adapter.onLiveUsageChanged((liveUsage) => {
          if (generation !== this.generation || !this.snapshot.active) return
          // The push carries the newest figures. A read still in flight
          // carries older ones, so it must not land after this.
          this.liveUsageVersion += 1
          // Published straight away: the push is the figures, not a hint to
          // go and read them. Only the two reads it prompts are deferred.
          this.update({ liveUsage, liveUsageSettled: true })
          this.notePlanObservation()
          this.scheduleRead("allowance")
          this.scheduleRead("allocations")
        }),
      ),
      this.listen(
        generation,
        this.adapter.onSessionIndexChanged(whenCurrent(() => this.scheduleRead("totals"))),
      ),
      this.listen(
        generation,
        this.adapter.onSessionUpdated((update) => {
          if (generation !== this.generation) return
          // The recent rows come from the shared list already in memory, so
          // they cost nothing to redo and stay immediate.
          if (overviewUpdateTouchesTotals(update)) this.scheduleRead("totals")
          else this.refreshRecentSessions()
        }),
      ),
    ])
    const revision = visibilityRevision
    const visible = await this.adapter.getVisible().catch(() => false)
    if (generation !== this.generation) return
    if (revision === visibilityRevision) this.visible = visible
    this.initialized = true
    this.syncActive()
  }

  private syncActive(): void {
    const active = this.initialized && this.visible && this.activeListeners.size > 0
    if (active === this.snapshot.active) return
    this.workVersion += 1
    this.update({ active, loading: active && !this.snapshot.usage, refreshing: false })
    this.clearScheduledReads()
    if (!active) return
    // The first read of an activation is the one the reader is waiting for.
    // It runs now; only the churn behind it is deferred.
    // An activation that finds no pass running has nothing to hold for, so a
    // later pass is a later pass.
    this.scanSettled = !this.scanSource.getSnapshot().running
    this.refresh()
  }

  /**
   * Queue one of the page's reads, to run once the events around it stop.
   *
   * Held entirely while the first scan pass of this activation is still
   * running: a pass publishes its sessions as it goes, so reading before it
   * finishes means reading a figure that is about to change again.
   */
  private scheduleRead(kind: OverviewReadKind): void {
    // Inactive, the reads are dirty flags and no IO. Let them through, so a
    // resume still finds the page marked stale.
    if (!this.snapshot.active) {
      this.runRead(kind)
      return
    }
    this.pendingReads.add(kind)
    if (this.holdingForScan()) {
      this.armScanHoldCap()
      return
    }
    this.armDeferredFlush()
  }

  private holdingForScan(): boolean {
    if (this.scanSettled) return false
    return this.scanSource.getSnapshot().running
  }

  private armDebounce(): void {
    if (this.debounceTimer) return
    this.debounceTimer = setTimeout(() => {
      this.debounceTimer = null
      this.flushReads()
    }, this.debounceMs)
  }

  /** A single trailing timer: while it is armed, later events do not reset
   *  it, so a steady stream of them still flushes once per interval instead
   *  of never. */
  private armThrottle(): void {
    if (this.throttleTimer) return
    this.throttleTimer = setTimeout(() => {
      this.throttleTimer = null
      this.flushReads()
    }, this.backlogThrottleMs)
  }

  private armScanHoldCap(): void {
    if (this.scanHoldTimer) return
    this.scanHoldTimer = setTimeout(() => {
      this.scanHoldTimer = null
      this.releaseScanHold()
    }, this.scanHoldCapMs)
  }

  private onScanStateChanged(): void {
    if (this.scanSettled || this.scanSource.getSnapshot().running) return
    this.releaseScanHold()
  }

  /** The insights backlog draining is not a hold: it only changes which
   *  timer pending reads wait on. Once it drains, any reads still pending
   *  switch from the 5 s throttle to the ordinary debounce, so they land
   *  soon after rather than waiting out a throttle interval that no longer
   *  applies. */
  private onBacklogStateChanged(): void {
    if (this.backlogSource.getSnapshot().active) return
    if (this.throttleTimer) {
      clearTimeout(this.throttleTimer)
      this.throttleTimer = null
    }
    if (this.pendingReads.size > 0) this.armDebounce()
  }

  /** Stop holding for the rest of this activation, and run what queued up. */
  private releaseScanHold(): void {
    if (this.scanSettled) return
    this.scanSettled = true
    if (this.scanHoldTimer) {
      clearTimeout(this.scanHoldTimer)
      this.scanHoldTimer = null
    }
    if (this.pendingReads.size > 0) this.armDeferredFlush()
  }

  /** Which timer picks up reads that just became free to schedule: the
   *  throttle while the insights backlog is still active, the ordinary
   *  debounce otherwise. */
  private armDeferredFlush(): void {
    if (this.backlogSource.getSnapshot().active) this.armThrottle()
    else this.armDebounce()
  }

  private flushReads(): void {
    // A scan can start after the debounce armed but before it fired. Hold
    // the reads back in that case too, so a read never lands mid-pass.
    if (this.snapshot.active && this.holdingForScan()) {
      this.armScanHoldCap()
      return
    }
    const kinds = [...this.pendingReads]
    this.pendingReads.clear()
    if (!this.snapshot.active) return
    // A totals read already refreshes the allowance and the allocations, so
    // the other two would be the same work again.
    if (kinds.includes("totals")) this.refresh()
    else for (const kind of kinds) this.runRead(kind)
  }

  private runRead(kind: OverviewReadKind): void {
    if (kind === "totals") this.refresh()
    else if (kind === "allowance") this.refreshAllowance()
    else this.refreshSessionLimitAllocations()
  }

  private clearScheduledReads(): void {
    this.pendingReads.clear()
    if (this.debounceTimer) {
      clearTimeout(this.debounceTimer)
      this.debounceTimer = null
    }
    if (this.throttleTimer) {
      clearTimeout(this.throttleTimer)
      this.throttleTimer = null
    }
    if (this.scanHoldTimer) {
      clearTimeout(this.scanHoldTimer)
      this.scanHoldTimer = null
    }
  }

  refresh = (): void => {
    this.refreshVersion += 1
    this.refreshDirty = true
    if (!this.snapshot.active || this.refreshTask) return
    this.refreshTask = this.runRefresh().finally(() => {
      this.refreshTask = null
      if (this.refreshDirty && this.snapshot.active) this.refresh()
    })
  }

  private async runRefresh(): Promise<void> {
    while (this.refreshDirty && this.snapshot.active) {
      this.refreshDirty = false
      const work = this.workVersion
      const version = this.refreshVersion
      this.update({ loading: !this.snapshot.usage, refreshing: !!this.snapshot.usage })
      this.refreshAllowance()
      this.refreshRecentSessions()
      this.refreshSessionLimitAllocations()
      void this.loadLiveUsage(work, version, ++this.liveUsageVersion)
      try {
        const usage = await this.adapter.getUsage()
        if (work !== this.workVersion || version !== this.refreshVersion) continue
        this.usageSettled = true
        this.reportContentReadyOnceSettled()
        this.update({ usage, loading: false, refreshing: false, usageError: false })
      } catch {
        if (work === this.workVersion && version === this.refreshVersion) {
          this.usageSettled = true
          this.reportContentReadyOnceSettled()
          this.update({ loading: false, refreshing: false, usageError: true })
        }
      }
    }
  }

  /** Read the live provider limits. Only the newest read, by work, refresh,
   *  and live-usage version, is allowed to land: a push or a later refresh
   *  must not lose to a read still in flight. */
  private async loadLiveUsage(
    work: number,
    version: number,
    liveVersion: number,
  ): Promise<void> {
    const current = () =>
      work === this.workVersion &&
      version === this.refreshVersion &&
      liveVersion === this.liveUsageVersion &&
      this.snapshot.active
    try {
      const liveUsage = await this.adapter.getLiveUsage()
      if (current()) {
        this.update({ liveUsage, liveUsageSettled: true })
        this.notePlanObservation()
      }
    } catch {
      // The limits panel shows its own empty state. A failed read must not
      // hide the local totals, so liveUsage stays as it was.
      if (current()) {
        this.update({ liveUsageSettled: true })
        this.notePlanObservation()
      }
    }
  }

  /** Keep one quota read in flight while changes queue a single later read. */
  refreshAllowance = (): void => {
    this.allowanceDirty = true
    if (!this.snapshot.active || this.allowanceTask) return
    this.allowanceTask = this.runAllowanceRefresh().finally(() => {
      this.allowanceTask = null
      if (this.allowanceDirty && this.snapshot.active) this.refreshAllowance()
    })
  }

  private async runAllowanceRefresh(): Promise<void> {
    while (this.allowanceDirty && this.snapshot.active) {
      this.allowanceDirty = false
      const work = this.workVersion
      this.update({ allowanceLoading: !this.snapshot.allowance })
      try {
        const allowance = await this.adapter.getAllowanceUsage()
        if (work === this.workVersion && !this.allowanceDirty) {
          this.allowanceSettled = true
          this.reportContentReadyOnceSettled()
          this.update({ allowance, allowanceLoading: false, allowanceError: false })
          this.notePlanObservation()
        }
      } catch {
        // A failed read must not hide the cost totals beside the allowance.
        if (work === this.workVersion && !this.allowanceDirty) {
          this.allowanceSettled = true
          this.reportContentReadyOnceSettled()
          this.update({ allowanceLoading: false, allowanceError: true })
          this.notePlanObservation()
        }
      }
    }
  }

  /** Keep one allocations read in flight while changes queue a single later
   *  read. A failed read keeps the last value, so it never disturbs the rest
   *  of the page. */
  refreshSessionLimitAllocations = (): void => {
    this.sessionLimitAllocationsDirty = true
    if (!this.snapshot.active || this.sessionLimitAllocationsTask) return
    this.sessionLimitAllocationsTask = this.runSessionLimitAllocationsRefresh().finally(() => {
      this.sessionLimitAllocationsTask = null
      if (this.sessionLimitAllocationsDirty && this.snapshot.active) {
        this.refreshSessionLimitAllocations()
      }
    })
  }

  private async runSessionLimitAllocationsRefresh(): Promise<void> {
    while (this.sessionLimitAllocationsDirty && this.snapshot.active) {
      this.sessionLimitAllocationsDirty = false
      const work = this.workVersion
      try {
        const sessionLimitAllocations = await this.adapter.getSessionLimitAllocations()
        if (work === this.workVersion && !this.sessionLimitAllocationsDirty) {
          this.update({ sessionLimitAllocations })
        }
      } catch {
        // Keep the last value; a failed read must not disturb the rest of
        // the page.
      }
    }
  }

  /** Refresh the newest sessions from the shared main-window list. */
  refreshRecentSessions = (): void => {
    if (!this.snapshot.active) return
    const entries = this.sessionList.getSnapshot().entries
    if (entries) {
      const rows = overviewRecentEntries(entries)
      this.update({ recentSessions: rows })
    }
  }

  dispose = (): void => {
    this.generation += 1
    this.workVersion += 1
    this.initialized = false
    this.visible = false
    this.allowanceDirty = false
    this.sessionLimitAllocationsDirty = false
    this.clearScheduledReads()
    for (const stop of this.stops.splice(0)) stop()
    this.update({ active: false, loading: false, refreshing: false, allowanceLoading: false })
  }
}

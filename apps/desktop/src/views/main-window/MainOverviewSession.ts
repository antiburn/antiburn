import type { SessionListEntry } from "../../components/session/SessionList"
import {
  cancelChecksReport,
  getChecksReport,
  onChecksReportChanged,
  type ChecksReportPayload,
} from "../../lib/insightsIpc"
import {
  getLiveUsage,
  getMainWindowVisible,
  getProviderUsage,
  onLiveUsageChanged,
  onMainWindowVisibilityChanged,
  onSessionIndexChanged,
  onSessionUpdated,
  type SessionIndexChangedPayload,
  type SessionUpdatedPayload,
} from "../../lib/ipc"
import type {
  LiveUsageSummaryPayload,
  ProviderUsageSummaryPayload,
} from "../../lib/providerUsageIpc"

export interface MainOverviewAdapter {
  getUsage(): Promise<ProviderUsageSummaryPayload>
  getLiveUsage(): Promise<LiveUsageSummaryPayload>
  getChecksReport(consumerId: string): Promise<ChecksReportPayload | null>
  cancelChecksReport(consumerId: string): Promise<void>
  getVisible(): Promise<boolean>
  onVisible(handler: (visible: boolean) => void): Promise<() => void>
  onLiveUsageChanged(handler: (usage: LiveUsageSummaryPayload) => void): Promise<() => void>
  onChecksReportChanged(handler: () => void): Promise<() => void>
  onSessionIndexChanged(
    handler: (change: SessionIndexChangedPayload) => void,
  ): Promise<() => void>
  onSessionUpdated(handler: (update: SessionUpdatedPayload) => void): Promise<() => void>
}

export interface MainOverviewSessionListSource {
  getSnapshot(): { entries: SessionListEntry[] | null }
  subscribeList(listener: () => void): () => void
}

const productionAdapter: MainOverviewAdapter = {
  getUsage: () => getProviderUsage(),
  getLiveUsage: () => getLiveUsage(),
  getChecksReport: (consumerId) => getChecksReport(consumerId),
  cancelChecksReport: (consumerId) => cancelChecksReport(consumerId),
  getVisible: () => getMainWindowVisible(),
  onVisible: (handler) => onMainWindowVisibilityChanged(handler),
  onLiveUsageChanged: (handler) => onLiveUsageChanged(handler),
  onChecksReportChanged: (handler) => onChecksReportChanged(handler),
  onSessionIndexChanged: (handler) => onSessionIndexChanged(handler),
  onSessionUpdated: (handler) => onSessionUpdated(handler),
}

/**
 * Whether one row update can move the Overview's spend totals or report.
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

/** How many recent sessions the Overview page shows. */
export const OVERVIEW_RECENT_SESSION_COUNT = 3

let nextConsumer = 0

export interface MainOverviewSnapshot {
  active: boolean
  /** The local usage summary, or null before the first successful read. */
  usage: ProviderUsageSummaryPayload | null
  /** True when the newest local usage read failed and nothing replaced it. */
  usageError: boolean
  /** The provider limit snapshot, or null before the first successful read. */
  liveUsage: LiveUsageSummaryPayload | null
  /** The Burn checks report, or null before the first successful read. */
  report: ChecksReportPayload | null
  /** The newest local sessions, or null before the first successful read. */
  recentSessions: SessionListEntry[] | null
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

/**
 * Own the Overview section's reads: local provider usage, the live provider
 * limits, and the Burn checks report. The main window supplies the newest
 * sessions through the shared list source, so this store does not start a
 * second session-list read. It refreshes the page while the section is
 * visible and on every `session:index-changed` event or row update that can
 * move its totals.
 *
 * The checks report is read under a consumer id of its own. The backend
 * keeps that report warm until the store cancels the consumer, which it does
 * whenever the section goes inactive.
 */
export class MainOverviewSession {
  private readonly adapter: MainOverviewAdapter
  private snapshot: MainOverviewSnapshot = {
    active: false,
    usage: null,
    usageError: false,
    liveUsage: null,
    report: null,
    recentSessions: null,
    loading: false,
    refreshing: false,
  }
  private readonly listeners = new Set<() => void>()
  private readonly activeListeners = new Set<() => void>()
  private readonly stops: Array<() => void> = []
  private generation = 0
  private workVersion = 0
  private refreshVersion = 0
  private reportVersion = 0
  private visible = false
  private initialized = false
  private refreshTask: Promise<void> | null = null
  private refreshDirty = false
  private consumerId: string | null = null
  private readonly sessionList: MainOverviewSessionListSource

  constructor(
    sessionList: MainOverviewSessionListSource,
    adapter: MainOverviewAdapter = productionAdapter,
  ) {
    this.sessionList = sessionList
    this.adapter = adapter
  }

  getSnapshot = (): MainOverviewSnapshot => this.snapshot
  subscribe = (listener: () => void): (() => void) => this.attach(listener, true)
  subscribeInactive = (listener: () => void): (() => void) => this.attach(listener, false)

  private update(patch: Partial<MainOverviewSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    for (const listener of this.listeners) listener()
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
        this.adapter.onLiveUsageChanged((liveUsage) => {
          if (generation === this.generation && this.snapshot.active) this.update({ liveUsage })
        }),
      ),
      this.listen(
        generation,
        this.adapter.onChecksReportChanged(whenCurrent(this.refreshReport)),
      ),
      this.listen(generation, this.adapter.onSessionIndexChanged(whenCurrent(this.refresh))),
      this.listen(
        generation,
        this.adapter.onSessionUpdated((update) => {
          if (generation !== this.generation) return
          if (overviewUpdateTouchesTotals(update)) this.refresh()
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
    if (!active) {
      this.releaseConsumer()
      return
    }
    this.consumerId = `main-home-${++nextConsumer}`
    this.refresh()
  }

  private releaseConsumer(): void {
    const consumerId = this.consumerId
    this.consumerId = null
    if (consumerId) void this.adapter.cancelChecksReport(consumerId).catch(() => undefined)
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
      void this.loadLiveUsage(work, version)
      this.refreshReport()
      this.refreshRecentSessions()
      try {
        const usage = await this.adapter.getUsage()
        if (work !== this.workVersion || version !== this.refreshVersion) continue
        this.update({ usage, loading: false, refreshing: false, usageError: false })
      } catch {
        if (work === this.workVersion && version === this.refreshVersion) {
          this.update({ loading: false, refreshing: false, usageError: true })
        }
      }
    }
  }

  private async loadLiveUsage(work: number, version: number): Promise<void> {
    try {
      const liveUsage = await this.adapter.getLiveUsage()
      if (
        work === this.workVersion &&
        version === this.refreshVersion &&
        this.snapshot.active
      ) {
        this.update({ liveUsage })
      }
    } catch {
      // The limits panel shows its own empty state. A failed read must not
      // hide the local totals.
    }
  }

  /** Re-read the checks report alone. The newest read wins. */
  refreshReport = (): void => {
    const consumerId = this.consumerId
    if (!this.snapshot.active || !consumerId) return
    void this.loadReport(consumerId, this.workVersion, ++this.reportVersion)
  }

  private async loadReport(consumerId: string, work: number, version: number): Promise<void> {
    try {
      const report = await this.adapter.getChecksReport(consumerId)
      if (report && work === this.workVersion && version === this.reportVersion) {
        this.update({ report })
      }
    } catch {
      // The checks panel keeps its last report. A failed read must not hide
      // the rest of the page.
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
    this.refreshTask = null
    this.releaseConsumer()
    for (const stop of this.stops.splice(0)) stop()
    this.update({ active: false, loading: false, refreshing: false })
  }
}

import type { SessionListEntry } from "../../components/session/SessionList"
import { toActivityEntries } from "../../lib/activityEntries"
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
  listRecentSessions,
  onLiveUsageChanged,
  onMainWindowVisibilityChanged,
  onScanEvent,
  onSessionEntryChanged,
  onSessionsInvalidated,
  type ActivityEntryPayload,
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
  listRecentSessions(): Promise<ActivityEntryPayload[]>
  getVisible(): Promise<boolean>
  onVisible(handler: (visible: boolean) => void): Promise<() => void>
  onLiveUsageChanged(handler: (usage: LiveUsageSummaryPayload) => void): Promise<() => void>
  onChecksReportChanged(handler: () => void): Promise<() => void>
  onSessionsInvalidated(handler: () => void): Promise<() => void>
  onSessionEntryChanged(handler: () => void): Promise<() => void>
  onScanFinished(handler: () => void): Promise<() => void>
}

const productionAdapter: MainOverviewAdapter = {
  getUsage: () => getProviderUsage(),
  getLiveUsage: () => getLiveUsage(),
  getChecksReport: (consumerId) => getChecksReport(consumerId),
  cancelChecksReport: (consumerId) => cancelChecksReport(consumerId),
  listRecentSessions: () => listRecentSessions(),
  getVisible: () => getMainWindowVisible(),
  onVisible: (handler) => onMainWindowVisibilityChanged(handler),
  onLiveUsageChanged: (handler) => onLiveUsageChanged(handler),
  onChecksReportChanged: (handler) => onChecksReportChanged(handler),
  onSessionsInvalidated: (handler) => onSessionsInvalidated(handler),
  onSessionEntryChanged: (handler) => onSessionEntryChanged(() => handler()),
  onScanFinished: (handler) =>
    onScanEvent((_status, phase) => {
      if (phase === "finished") handler()
    }),
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

/** The newest sessions first, cut to the Overview page's row count. */
function overviewRecentSessions(payloads: readonly ActivityEntryPayload[]): SessionListEntry[] {
  return toActivityEntries(payloads)
    .sort((left, right) => right.timestamp.localeCompare(left.timestamp))
    .slice(0, OVERVIEW_RECENT_SESSION_COUNT)
}

/**
 * Own the Overview section's reads: local provider usage, the live provider
 * limits, the Burn checks report, and the newest sessions. The section is
 * the main window's landing page, so the store loads only while the window
 * is visible and a viewer is active, and it refreshes after every scan and
 * every live usage update.
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
  private recentVersion = 0
  private visible = false
  private initialized = false
  private refreshTask: Promise<void> | null = null
  private refreshDirty = false
  private consumerId: string | null = null

  constructor(adapter: MainOverviewAdapter = productionAdapter) {
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
        this.adapter.onLiveUsageChanged((liveUsage) => {
          if (generation === this.generation && this.snapshot.active) this.update({ liveUsage })
        }),
      ),
      this.listen(
        generation,
        this.adapter.onChecksReportChanged(whenCurrent(this.refreshReport)),
      ),
      this.listen(generation, this.adapter.onSessionsInvalidated(whenCurrent(this.refresh))),
      this.listen(
        generation,
        this.adapter.onSessionEntryChanged(whenCurrent(this.refreshRecentSessions)),
      ),
      this.listen(generation, this.adapter.onScanFinished(whenCurrent(this.refresh))),
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

  /** Re-read the newest sessions alone. The newest read wins. */
  refreshRecentSessions = (): void => {
    if (!this.snapshot.active) return
    void this.loadRecentSessions(this.workVersion, ++this.recentVersion)
  }

  private async loadRecentSessions(work: number, version: number): Promise<void> {
    try {
      const payloads = await this.adapter.listRecentSessions()
      if (work === this.workVersion && version === this.recentVersion) {
        this.update({ recentSessions: overviewRecentSessions(payloads) })
      }
    } catch {
      // The sessions panel keeps its last rows.
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

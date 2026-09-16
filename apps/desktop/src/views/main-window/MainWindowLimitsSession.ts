import {
  getLiveUsage,
  getMainWindowVisible,
  onLiveUsageChanged,
  onMainWindowVisibilityChanged,
} from "../../lib/ipc"
import type { LiveUsageSummaryPayload } from "../../lib/providerUsageIpc"

export interface MainWindowLimitsAdapter {
  getLiveUsage(): Promise<LiveUsageSummaryPayload>
  getVisible(): Promise<boolean>
  onVisible(handler: (visible: boolean) => void): Promise<() => void>
  onLiveUsageChanged(handler: (usage: LiveUsageSummaryPayload) => void): Promise<() => void>
}

const productionAdapter: MainWindowLimitsAdapter = {
  getLiveUsage: () => getLiveUsage(),
  getVisible: () => getMainWindowVisible(),
  onVisible: (handler) => onMainWindowVisibilityChanged(handler),
  onLiveUsageChanged: (handler) => onLiveUsageChanged(handler),
}

export interface MainWindowLimitsSnapshot {
  liveUsage: LiveUsageSummaryPayload | null
  /** True until the first read answers, so the sidebar shows its skeleton. */
  loading: boolean
}

/**
 * The live provider limits for the main window's sidebar.
 *
 * The sidebar shows the limits in every section, so this store belongs to
 * the window and not to one section. A section store stops its updates when
 * the reader leaves it, which would freeze the meters in the sidebar.
 *
 * The store reads while the window is visible. A hidden window shows nobody
 * a meter, so it must not keep the provider's figures warm.
 */
export class MainWindowLimitsSession {
  private readonly adapter: MainWindowLimitsAdapter
  private snapshot: MainWindowLimitsSnapshot = { liveUsage: null, loading: true }
  private readonly listeners = new Set<() => void>()
  private readonly stops: Array<() => void> = []
  private generation = 0
  private visible = false
  /** Counts the visibility events this start has seen. */
  private visibleRevision = 0

  constructor(adapter: MainWindowLimitsAdapter = productionAdapter) {
    this.adapter = adapter
  }

  getSnapshot = (): MainWindowLimitsSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.dispose()
    }
  }

  private update(patch: Partial<MainWindowLimitsSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    for (const listener of this.listeners) listener()
  }

  private async listen(generation: number, pending: Promise<() => void>): Promise<void> {
    const stop = await pending.catch(() => null)
    if (!stop) return
    if (generation !== this.generation) stop()
    else this.stops.push(stop)
  }

  private async start(): Promise<void> {
    const generation = ++this.generation
    await Promise.all([
      this.listen(
        generation,
        this.adapter.onVisible((visible) => {
          if (generation !== this.generation) return
          const gained = visible && !this.visible
          this.visibleRevision += 1
          this.visible = visible
          if (gained) void this.load(generation)
        }),
      ),
      this.listen(
        generation,
        this.adapter.onLiveUsageChanged((liveUsage) => {
          if (generation === this.generation && this.visible) {
            this.update({ liveUsage, loading: false })
          }
        }),
      ),
    ])
    const revision = this.visibleRevision
    const visible = await this.adapter.getVisible().catch(() => false)
    if (generation !== this.generation) return
    // The first read asks for the state at the moment the store started. An
    // event that arrives while it is in flight carries a later state, so the
    // read must not write over it.
    if (revision !== this.visibleRevision) return
    this.visible = visible
    if (visible) await this.load(generation)
  }

  private async load(generation: number): Promise<void> {
    try {
      const liveUsage = await this.adapter.getLiveUsage()
      if (generation === this.generation) this.update({ liveUsage, loading: false })
    } catch {
      // The limits panel shows its own empty state. A failed read must not
      // take the sidebar's navigation down with it.
      if (generation === this.generation) this.update({ loading: false })
    }
  }

  private dispose(): void {
    this.generation += 1
    for (const stop of this.stops.splice(0)) stop()
    this.visible = false
    this.visibleRevision = 0
    this.snapshot = { liveUsage: null, loading: true }
  }
}

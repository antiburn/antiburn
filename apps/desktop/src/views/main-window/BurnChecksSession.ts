import {
  cancelChecksReport,
  getBurnCheckAggregateWins,
  getChecksReport,
  listBurnCheckTargets,
  onChecksReportChanged,
  type AggregateWinsPayload,
  type BurnCheckDetectorId,
  type BurnCheckTargetListPayload,
  type ChecksReportPayload,
} from "../../lib/insightsIpc"
import {
  getMainWindowVisible,
  noteInteraction,
  onMainWindowVisibilityChanged,
} from "../../lib/ipc"
import { SurfaceExposureTracker } from "../../lib/surfaceExposure"

export interface BurnChecksAdapter {
  getReport(consumerId: string): Promise<ChecksReportPayload | null>
  getAggregateWins(): Promise<AggregateWinsPayload | null>
  getTargets(detector: BurnCheckDetectorId): Promise<BurnCheckTargetListPayload | null>
  cancelReport(consumerId: string): Promise<void>
  getVisible(): Promise<boolean>
  onVisible(handler: (visible: boolean) => void): Promise<() => void>
  onChanged(handler: () => void): Promise<() => void>
}

const productionAdapter: BurnChecksAdapter = {
  getReport: (consumerId) => getChecksReport(consumerId),
  getAggregateWins: () => getBurnCheckAggregateWins(),
  getTargets: (detector) => listBurnCheckTargets(detector),
  cancelReport: (consumerId) => cancelChecksReport(consumerId),
  getVisible: () => getMainWindowVisible(),
  onVisible: (handler) => onMainWindowVisibilityChanged(handler),
  onChanged: (handler) => onChecksReportChanged(handler),
}

export interface BurnCheckTargetState {
  data: BurnCheckTargetListPayload | null
  loading: boolean
  error: boolean
}

export interface BurnChecksSnapshot {
  active: boolean
  report: ChecksReportPayload | null
  aggregate: AggregateWinsPayload | null
  loading: boolean
  refreshing: boolean
  error: boolean
  targets: Partial<Record<BurnCheckDetectorId, BurnCheckTargetState>>
}

let nextConsumer = 0

/** Own the main window's Checks reads without sharing the popover consumer. */
export class BurnChecksSession {
  private readonly adapter: BurnChecksAdapter
  private snapshot: BurnChecksSnapshot = {
    active: false,
    report: null,
    aggregate: null,
    loading: false,
    refreshing: false,
    error: false,
    targets: {},
  }
  private consumerId: string | null = null
  private readonly listeners = new Set<() => void>()
  private readonly activeListeners = new Set<() => void>()
  private readonly stops: Array<() => void> = []
  private generation = 0
  private workVersion = 0
  private refreshVersion = 0
  private visible = false
  private initialized = false
  private refreshTask: Promise<void> | null = null
  private refreshDirty = false
  private targetTasks = new Map<BurnCheckDetectorId, Promise<void>>()
  private targetVersions = new Map<BurnCheckDetectorId, number>()
  private readonly exposure = new SurfaceExposureTracker()
  private exposureGeneration: number | null = null
  private readonly visibleTargets = new Set<BurnCheckDetectorId>()
  private readonly observedTargets = new Set<BurnCheckDetectorId>()
  private readonly observedOutcomes = new Set<string>()

  constructor(adapter: BurnChecksAdapter = productionAdapter) {
    this.adapter = adapter
  }

  getSnapshot = (): BurnChecksSnapshot => this.snapshot
  subscribe = (listener: () => void): (() => void) => this.attach(listener, true)
  subscribeInactive = (listener: () => void): (() => void) => this.attach(listener, false)

  private update(patch: Partial<BurnChecksSnapshot>): void {
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
        this.adapter.onChanged(() => {
          if (generation === this.generation) this.refresh()
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
    this.update({ active, loading: active && !this.snapshot.report, refreshing: false })
    if (!active) {
      this.exposure.conceal("burn_checks", this.exposureGeneration ?? undefined)
      this.exposureGeneration = null
      this.observedOutcomes.clear()
      this.targetTasks.clear()
      const consumerId = this.consumerId
      this.consumerId = null
      if (consumerId) void this.adapter.cancelReport(consumerId).catch(() => undefined)
      return
    }
    this.exposureGeneration = this.exposure.expose({
      surface: "burn_checks",
      origin: "user",
      state: this.reportState(),
    })
    this.observeOutcomes()
    this.consumerId = `main-burn-checks-${++nextConsumer}`
    this.refresh()
    for (const detector of this.visibleTargets) {
      this.loadTargets(detector, true)
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
      const consumerId = this.consumerId
      if (!consumerId) return
      this.update({
        loading: !this.snapshot.report,
        refreshing: !!this.snapshot.report,
      })
      void this.loadAggregate(work, version)
      try {
        const report = await this.adapter.getReport(consumerId)
        if (work !== this.workVersion || version !== this.refreshVersion) continue
        if (!report) throw new Error("Checks are unavailable")
        this.update({ report, loading: false, refreshing: false, error: false })
        this.exposure.observe(
          this.reportState() ?? "empty",
          this.exposureGeneration ?? undefined,
        )
        this.observeOutcomes()
        for (const detector of this.visibleTargets) {
          this.loadTargets(detector, true)
        }
      } catch {
        if (work === this.workVersion && version === this.refreshVersion) {
          this.update({ loading: false, refreshing: false, error: true })
          this.exposure.observe("error", this.exposureGeneration ?? undefined)
        }
      }
    }
  }

  private async loadAggregate(work: number, version: number): Promise<void> {
    try {
      const aggregate = await this.adapter.getAggregateWins()
      if (
        aggregate &&
        work === this.workVersion &&
        version === this.refreshVersion &&
        this.snapshot.active
      ) {
        this.update({ aggregate })
        this.observeOutcomes()
      }
    } catch {
      // Aggregate savings are optional and must not hide the checks report.
    }
  }

  loadTargets = (detector: BurnCheckDetectorId, force = false): void => {
    const current = this.snapshot.targets[detector]
    if (!force && (current?.data || current?.loading)) return
    const version = (this.targetVersions.get(detector) ?? 0) + 1
    this.targetVersions.set(detector, version)
    this.update({
      targets: {
        ...this.snapshot.targets,
        [detector]: { data: current?.data ?? null, loading: true, error: false },
      },
    })
    if (!this.snapshot.active || this.targetTasks.has(detector)) return
    const task = this.runTargetLoad(detector).finally(() => {
      this.targetTasks.delete(detector)
      const latest = this.snapshot.targets[detector]
      if (latest?.loading && this.snapshot.active && this.visibleTargets.has(detector))
        this.loadTargets(detector, true)
    })
    this.targetTasks.set(detector, task)
  }

  setTargetsVisible = (
    detector: BurnCheckDetectorId,
    visible: boolean,
    observeOutcomes = true,
  ): void => {
    if (visible) {
      this.visibleTargets.add(detector)
      if (observeOutcomes) this.observedTargets.add(detector)
      this.loadTargets(detector)
      this.observeOutcomes()
    } else {
      this.visibleTargets.delete(detector)
      this.observedTargets.delete(detector)
      this.targetVersions.set(detector, (this.targetVersions.get(detector) ?? 0) + 1)
      const current = this.snapshot.targets[detector]
      if (current?.loading) {
        this.update({
          targets: {
            ...this.snapshot.targets,
            [detector]: { ...current, loading: false },
          },
        })
      }
    }
  }

  private async runTargetLoad(detector: BurnCheckDetectorId): Promise<void> {
    const work = this.workVersion
    const version = this.targetVersions.get(detector) ?? 0
    try {
      const data = await this.adapter.getTargets(detector)
      if (
        work !== this.workVersion ||
        version !== this.targetVersions.get(detector) ||
        !this.snapshot.active
      )
        return
      if (!data) throw new Error("Targets are unavailable")
      this.update({
        targets: {
          ...this.snapshot.targets,
          [detector]: { data, loading: false, error: false },
        },
      })
      this.observeOutcomes()
    } catch {
      if (
        work === this.workVersion &&
        version === this.targetVersions.get(detector) &&
        this.snapshot.active
      ) {
        const current = this.snapshot.targets[detector]
        this.update({
          targets: {
            ...this.snapshot.targets,
            [detector]: { data: current?.data ?? null, loading: false, error: true },
          },
        })
      }
    }
  }

  dispose = (): void => {
    this.generation += 1
    this.workVersion += 1
    this.initialized = false
    this.visible = false
    this.refreshTask = null
    this.targetTasks.clear()
    this.visibleTargets.clear()
    this.observedTargets.clear()
    this.observedOutcomes.clear()
    this.exposure.conceal("burn_checks", this.exposureGeneration ?? undefined)
    this.exposureGeneration = null
    for (const stop of this.stops.splice(0)) stop()
    const consumerId = this.consumerId
    this.consumerId = null
    if (consumerId) void this.adapter.cancelReport(consumerId).catch(() => undefined)
    this.update({ active: false, loading: false, refreshing: false })
  }

  private reportState(): "ready" | "empty" | null {
    const report = this.snapshot.report
    if (!report) return null
    return report.categories.some((category) => category.finding > 0 || category.clean > 0)
      ? "ready"
      : "empty"
  }

  private observeOutcomes(): void {
    if (!this.snapshot.active) return
    for (const win of this.snapshot.aggregate?.wins ?? []) {
      this.observeOutcome("verified", win.origin)
    }
    for (const detector of this.observedTargets) {
      for (const target of this.snapshot.targets[detector]?.data?.targets ?? []) {
        const watch = target.watch
        if (!watch) continue
        const status = watch.verification.status
        if (status === "fixed") this.observeOutcome("verified", watch.origin)
        if (status === "recurred") this.observeOutcome("recurred", watch.origin)
      }
    }
  }

  private observeOutcome(outcome: "verified" | "recurred", origin: "passive" | "action"): void {
    const key = `${outcome}:${origin}`
    if (this.observedOutcomes.has(key)) return
    this.observedOutcomes.add(key)
    noteInteraction({ kind: "burnCheckOutcomeObserved", outcome, origin })
  }
}

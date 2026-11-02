import { getMainWindowVisible, onMainWindowVisibilityChanged } from "./mainWindowIpc"
import { countAgentMemories, type AgentMemoriesReport } from "./memoriesIpc"

const MAX_AGE_MS = 60_000

interface MemoryCountAdapter {
  count(): Promise<number>
  now(): number
  getVisible(): Promise<boolean>
  onVisible(callback: (visible: boolean) => void): Promise<() => void>
  onFocus(callback: () => void): () => void
}

const productionAdapter: MemoryCountAdapter = {
  count: countAgentMemories,
  now: Date.now,
  getVisible: getMainWindowVisible,
  onVisible: onMainWindowVisibilityChanged,
  onFocus(callback) {
    window.addEventListener("focus", callback)
    return () => window.removeEventListener("focus", callback)
  },
}

export class MemoryCountStore {
  private count: number | null = null
  private revision = 0
  private generation = 0
  private attemptedAt = -Infinity
  private visible = false
  private pending = false
  private listeners = new Set<() => void>()
  private stops: Array<() => void> = []

  private readonly adapter: MemoryCountAdapter

  constructor(adapter: MemoryCountAdapter = productionAdapter) {
    this.adapter = adapter
  }

  getSnapshot = (): number | null => this.count
  getRevision = (): number => this.revision

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size > 0) return
      this.generation += 1
      this.revision += 1
      this.visible = false
      if (this.pending) this.attemptedAt = -Infinity
      for (const stop of this.stops.splice(0)) stop()
    }
  }

  acceptReport(report: AgentMemoriesReport, revision: number): void {
    if (revision !== this.revision) return
    this.revision += 1
    this.attemptedAt = this.adapter.now()
    this.publish(report.projects.reduce((total, project) => total + project.memories.length, 0))
  }

  invalidate(): void {
    this.revision += 1
    this.attemptedAt = -Infinity
    this.refresh()
  }

  private publish(count: number): void {
    this.count = count
    for (const listener of this.listeners) listener()
  }

  private refresh = (): void => {
    if (!this.visible || this.listeners.size === 0 || this.pending) return
    if (this.adapter.now() - this.attemptedAt < MAX_AGE_MS) return
    const revision = this.revision
    const generation = this.generation
    this.attemptedAt = this.adapter.now()
    this.pending = true
    void this.adapter
      .count()
      .then((count) => {
        if (revision === this.revision && generation === this.generation) this.publish(count)
      })
      .catch(() => undefined)
      .finally(() => {
        this.pending = false
        // An edit during this read requires a new count.
        if (this.attemptedAt === -Infinity) this.refresh()
      })
  }

  private start(): void {
    const generation = ++this.generation
    let visibilityRevision = 0
    const setVisible = (visible: boolean) => {
      if (generation !== this.generation) return
      this.visible = visible
      if (visible) this.refresh()
    }
    this.stops.push(this.adapter.onFocus(this.refresh))
    void this.adapter
      .onVisible((visible) => {
        visibilityRevision += 1
        setVisible(visible)
      })
      .then((stop) => {
        if (generation !== this.generation) stop()
        else this.stops.push(stop)
      })
      .catch(() => undefined)
    const requestedAt = visibilityRevision
    void this.adapter
      .getVisible()
      .then((visible) => {
        if (requestedAt === visibilityRevision) setVisible(visible)
      })
      .catch(() => undefined)
  }
}

export const memoryCountStore = new MemoryCountStore()

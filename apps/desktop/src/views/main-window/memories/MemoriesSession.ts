import { revealSource } from "../../../lib/ipc"
import {
  listAgentMemories,
  type AgentMemoriesReport,
  type MemoryEntry,
} from "../../../lib/memoriesIpc"
import { SurfaceExposureTracker } from "../../../lib/surfaceExposure"
import { readCollapsedProjects, writeCollapsedProjects } from "./memoriesViewPrefs"

export interface MemoriesSnapshot {
  /** The last loaded report, or null until the first load ends. */
  report: AgentMemoriesReport | null
  error: boolean
  loading: boolean
  collapsedProjects: ReadonlySet<string>
  expandedMemories: ReadonlySet<string>
}

export interface MemoriesAdapter {
  listMemories(): Promise<AgentMemoriesReport>
  reveal(path: string): Promise<void>
  now(): number
}

const productionAdapter: MemoriesAdapter = {
  listMemories: () => listAgentMemories(),
  reveal: (path) => revealSource(path),
  now: () => Date.now(),
}

/** Compare two nullable times, oldest first, with a missing time last. */
function compareNullableAscending(left: number | null, right: number | null): number {
  if (left == null && right == null) return 0
  if (left == null) return 1
  if (right == null) return -1
  return left - right
}

/** Order memories by the least recently referenced first, then the least
 *  recently written, then by title. A missing time sorts last. */
export function sortMemories(entries: readonly MemoryEntry[]): MemoryEntry[] {
  return [...entries].sort(
    (left, right) =>
      compareNullableAscending(left.facts.lastReferencedMs, right.facts.lastReferencedMs) ||
      compareNullableAscending(left.facts.lastWrittenMs, right.facts.lastWrittenMs) ||
      left.title.localeCompare(right.title),
  )
}

/** Toggle one member of a set without changing the set it received. */
function toggled(set: ReadonlySet<string>, member: string): Set<string> {
  const next = new Set(set)
  if (!next.delete(member)) next.add(member)
  return next
}

/**
 * Own the Memories section's read: every project's memories. Loads when a
 * viewer becomes active, and again each time the view is left and entered.
 * The previous report stays on screen during a reload. This screen does not
 * live-update.
 */
export class MemoriesSession {
  private readonly adapter: MemoriesAdapter
  private snapshot: MemoriesSnapshot
  private readonly listeners = new Set<() => void>()
  private readonly activeListeners = new Set<() => void>()
  private loadVersion = 0
  private active = false
  private readonly exposure = new SurfaceExposureTracker()

  constructor(adapter: MemoriesAdapter = productionAdapter) {
    this.adapter = adapter
    this.snapshot = {
      report: null,
      error: false,
      loading: false,
      collapsedProjects: new Set(readCollapsedProjects()),
      expandedMemories: new Set(),
    }
  }

  getSnapshot = (): MemoriesSnapshot => this.snapshot
  subscribe = (listener: () => void): (() => void) => this.attach(listener, true)
  subscribeInactive = (listener: () => void): (() => void) => this.attach(listener, false)

  private update(patch: Partial<MemoriesSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...patch }
    this.syncExposure()
    for (const listener of this.listeners) listener()
  }

  private attach(listener: () => void, active: boolean): () => void {
    this.listeners.add(listener)
    if (active) this.activeListeners.add(listener)
    this.syncActive()
    return () => {
      this.listeners.delete(listener)
      this.activeListeners.delete(listener)
      this.syncActive()
    }
  }

  private syncActive(): void {
    const active = this.activeListeners.size > 0
    if (active === this.active) return
    this.active = active
    // A load that started in an earlier activity period must not land.
    this.loadVersion += 1
    if (!active) {
      this.exposure.conceal("memories")
      this.snapshot = { ...this.snapshot, loading: false }
      return
    }
    this.syncExposure()
    void this.load()
  }

  /** Read the report again. Exposed for a Retry button. */
  refresh = (): void => {
    if (this.active) void this.load()
  }

  private async load(): Promise<void> {
    const version = ++this.loadVersion
    this.update({ loading: true, error: false })
    try {
      const report = await this.adapter.listMemories()
      if (version !== this.loadVersion) return
      this.update({ report, loading: false })
    } catch {
      if (version !== this.loadVersion) return
      this.update({ error: true, loading: false })
    }
  }

  toggleProject = (slug: string): void => {
    const collapsedProjects = toggled(this.snapshot.collapsedProjects, slug)
    writeCollapsedProjects(collapsedProjects)
    this.update({ collapsedProjects })
  }

  toggleMemory = (path: string): void => {
    this.update({ expandedMemories: toggled(this.snapshot.expandedMemories, path) })
  }

  now = (): number => this.adapter.now()

  reveal = (path: string): Promise<void> => this.adapter.reveal(path).catch(() => undefined)

  private syncExposure(): void {
    if (!this.active) return
    const generation = this.exposure.expose({ surface: "memories", origin: "user" })
    const state = this.memoriesState()
    if (state) this.exposure.observe(state, generation)
  }

  private memoriesState(): "ready" | "empty" | "error" | null {
    const { error, report } = this.snapshot
    if (error) return "error"
    if (!report) return null
    return report.projects.length === 0 ? "empty" : "ready"
  }

  dispose = (): void => {
    this.loadVersion += 1
    this.active = false
    this.activeListeners.clear()
    this.listeners.clear()
    this.exposure.conceal("memories")
  }
}

import {
  onMainWindowSectionTarget,
  takeMainWindowSectionTarget,
  type MainWindowSectionId,
  type MainWindowSectionRequest,
} from "../../lib/ipc"

export type MainWindowNavigationSnapshot = {
  selected: MainWindowSectionId
  visited: readonly MainWindowSectionId[]
  /** Bumps on every cross-window request `apply` accepts, even one that
   *  retargets the section already selected, so a listener can tell a fresh
   *  request apart from a `selected` value that merely stayed the same. */
  requests: number
}

/** Own cross-window section requests for the retained main renderer. */
export class MainWindowNavigationSession {
  private snapshot: MainWindowNavigationSnapshot = {
    selected: "overview",
    visited: ["overview"],
    requests: 0,
  }
  private revision = 0
  private generation = 0
  private listeners = new Set<() => void>()
  private stop: (() => void) | null = null

  getSnapshot = (): MainWindowNavigationSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.dispose()
    }
  }

  select(section: MainWindowSectionId): void {
    if (section === this.snapshot.selected) return
    this.snapshot = {
      ...this.snapshot,
      selected: section,
      visited: this.snapshot.visited.includes(section)
        ? this.snapshot.visited
        : [...this.snapshot.visited, section],
    }
    for (const listener of this.listeners) listener()
  }

  /** Unlike [`select`], a cross-window request always notifies, even one
   *  that retargets the section already selected: `requests` bumps every
   *  time so a listener parked on another local view (e.g. Limits) still
   *  learns a fresh request landed and leaves it. */
  private apply(request: MainWindowSectionRequest): void {
    if (request.revision <= this.revision) return
    this.revision = request.revision
    const section = request.section
    this.snapshot = {
      selected: section,
      visited: this.snapshot.visited.includes(section)
        ? this.snapshot.visited
        : [...this.snapshot.visited, section],
      requests: this.snapshot.requests + 1,
    }
    for (const listener of this.listeners) listener()
  }

  private async start(): Promise<void> {
    const generation = ++this.generation
    const stop = await onMainWindowSectionTarget((request) => {
      if (generation !== this.generation) return
      this.apply(request)
      void this.take(generation)
    }).catch(() => null)
    if (generation !== this.generation) {
      stop?.()
      return
    }
    this.stop = stop
    await this.take(generation)
  }

  private async take(generation: number): Promise<void> {
    const request = await takeMainWindowSectionTarget().catch(() => null)
    if (generation === this.generation && request) this.apply(request)
  }

  private dispose(): void {
    this.generation += 1
    this.stop?.()
    this.stop = null
  }
}

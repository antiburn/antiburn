import {
  onMainWindowSectionTarget,
  takeMainWindowSectionTarget,
  type MainWindowSectionId,
  type MainWindowSectionRequest,
} from "../../lib/ipc"

export type MainWindowNavigationSnapshot = {
  selected: MainWindowSectionId
  visited: readonly MainWindowSectionId[]
}

/** Own cross-window section requests for the retained main renderer. */
export class MainWindowNavigationSession {
  private snapshot: MainWindowNavigationSnapshot = {
    selected: "burnChecks",
    visited: ["burnChecks"],
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
      selected: section,
      visited: this.snapshot.visited.includes(section)
        ? this.snapshot.visited
        : [...this.snapshot.visited, section],
    }
    for (const listener of this.listeners) listener()
  }

  private apply(request: MainWindowSectionRequest): void {
    if (request.revision <= this.revision) return
    this.revision = request.revision
    this.select(request.section)
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

import {
  getCheckDirectories,
  openCheckDirectory,
  type CheckDirectory,
} from "../../lib/checkDirectories"

type Snapshot = {
  directories: CheckDirectory[] | null
  loading: boolean
  error: string | null
  opening: ReadonlySet<string>
  openErrors: ReadonlyMap<string, string>
}

function initialSnapshot(): Snapshot {
  return {
    directories: null,
    loading: true,
    error: null,
    opening: new Set(),
    openErrors: new Map(),
  }
}

export class CheckDirectorySession {
  private listeners = new Set<() => void>()
  private generation = 0
  private pending = false
  private state = initialSnapshot()
  getSnapshot = () => this.state
  subscribe = (listener: () => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) {
      this.generation++
      this.publish(initialSnapshot())
      void this.refresh()
    }
    return () => {
      this.listeners.delete(listener)
      if (!this.listeners.size) {
        this.generation++
        this.state = initialSnapshot()
      }
    }
  }
  private publish(update: Partial<Snapshot>) {
    this.state = { ...this.state, ...update }
    this.listeners.forEach((listener) => listener())
  }
  private refresh = async () => {
    if (this.pending) return
    this.pending = true
    const generation = this.generation
    try {
      const directories = await getCheckDirectories()
      if (generation === this.generation) this.publish({ directories, error: null })
    } catch {
      if (generation === this.generation)
        this.publish({
          error: "Could not load check folders. Reopen Settings → Checks to try again.",
        })
    } finally {
      this.pending = false
      if (generation === this.generation) {
        this.publish({ loading: false })
      } else if (this.listeners.size) {
        void this.refresh()
      }
    }
  }
  open = async (reference: string) => {
    if (!this.listeners.size || this.state.opening.has(reference)) return
    const generation = this.generation
    const opening = new Set(this.state.opening).add(reference)
    const openErrors = new Map(this.state.openErrors)
    openErrors.delete(reference)
    this.publish({ opening, openErrors })
    try {
      await openCheckDirectory(reference)
    } catch {
      if (generation === this.generation)
        this.publish({
          openErrors: new Map(this.state.openErrors).set(
            reference,
            "Could not open this check folder. Reopen Settings → Checks to try again.",
          ),
        })
    } finally {
      if (generation === this.generation) {
        const remaining = new Set(this.state.opening)
        remaining.delete(reference)
        this.publish({ opening: remaining })
      }
    }
  }
}

import {
  getCheckAvailability,
  onCheckAvailabilityChanged,
  type CheckAvailability,
  type CheckAvailabilityEvent,
} from "../../lib/checkAvailability"

export interface AppSearchCheckAvailabilityState {
  snapshot: CheckAvailability | null
  loading: boolean
  error: boolean
}

export class AppSearchCheckAvailability {
  private state: AppSearchCheckAvailabilityState = {
    snapshot: null,
    loading: true,
    error: false,
  }
  private disposed = false
  private revision = 0
  private stop: (() => void) | null = null
  private subscription: Promise<boolean> | null = null
  private readonly onChange: (state: AppSearchCheckAvailabilityState) => void

  constructor(onChange: (state: AppSearchCheckAvailabilityState) => void) {
    this.onChange = onChange
  }

  private update(state: AppSearchCheckAvailabilityState): void {
    if (this.disposed) return
    this.state = state
    this.onChange(state)
  }

  private onEvent = (event: CheckAvailabilityEvent): void => {
    if (this.disposed) return
    this.revision += 1
    this.update({
      snapshot: event.status === "updated" ? event.snapshot : this.state.snapshot,
      loading: false,
      error: event.status === "failed",
    })
  }

  private connect(): Promise<boolean> {
    if (this.stop) return Promise.resolve(true)
    if (!this.subscription) {
      this.subscription = onCheckAvailabilityChanged(this.onEvent)
        .then((stop) => {
          if (this.disposed) stop()
          else this.stop = stop
          return true
        })
        .catch(() => false)
        .finally(() => {
          this.subscription = null
        })
    }
    return this.subscription
  }

  refresh = async (): Promise<void> => {
    if (this.disposed) return
    const revision = ++this.revision
    this.update({ ...this.state, loading: true })
    const subscribed = await this.connect()
    if (this.disposed || revision !== this.revision) return
    try {
      const snapshot = await getCheckAvailability()
      if (this.disposed || revision !== this.revision) return
      this.update({ snapshot, loading: false, error: !subscribed })
    } catch {
      if (this.disposed || revision !== this.revision) return
      this.update({ ...this.state, loading: false, error: true })
    }
  }

  dispose(): void {
    this.disposed = true
    this.stop?.()
  }
}

import type { AnchorRegion } from "./anchorRegion"

export type AnchoredTriggerActivation = "idle" | "hovered" | "selected"

export interface AnchoredWindowRequest<T> {
  generation: number
  target: T | null
  retargetCommitRequired: boolean
}

export interface AnchoredWindowRenderRequest<T, P> extends AnchoredWindowRequest<T> {
  initialPresentation: P | null
}

export interface AnchoredWindowState<T> {
  generation: number
  target: T | null
  rendererReady: boolean
  visible: boolean
  awaitingRetargetCommit: boolean
  awaitingPresentation: boolean
  awaitingConcealment: boolean
}

export interface AnchoredWindowLifecycleEvent<T> {
  companionLabel: string
  state: AnchoredWindowState<T>
}

export interface AnchoredTriggerSnapshot<T> {
  activation: AnchoredTriggerActivation
  target: T | null
  generation: number
}

export interface AnchoredTriggerBridge<T, P = undefined> {
  request: (
    target: T,
    anchor: AnchorRegion,
    presentation: P | undefined,
  ) => Promise<AnchoredWindowRequest<T>>
  conceal: () => Promise<void>
  listen: (handler: (event: AnchoredWindowLifecycleEvent<T>) => void) => Promise<() => void>
  state: () => Promise<AnchoredWindowState<T>>
}

interface ScheduledRequest<T, P> {
  revision: number
  target: T
  anchor: AnchorRegion
  presentation: P | undefined
  resolve: () => void
}

const LISTENER_RETRY_MS = 250

/** Retains one trigger's activation while its anchored window remains active. */
export class AnchoredTriggerController<T, P = undefined> {
  private snapshot: AnchoredTriggerSnapshot<T> = {
    activation: "idle",
    target: null,
    generation: 0,
  }
  private readonly listeners = new Set<() => void>()
  private stopListening: (() => void) | null = null
  private listenerGeneration = 0
  private retryTimer: ReturnType<typeof setTimeout> | null = null
  private requestRevision = 0
  private lastLeaveRevision = 0
  private activeRequest: ScheduledRequest<T, P> | null = null
  private queuedRequest: ScheduledRequest<T, P> | null = null
  private latestGeneration = 0
  private readonly companionLabel: string
  private readonly sameTarget: (left: T, right: T) => boolean
  private readonly bridge: AnchoredTriggerBridge<T, P>

  constructor(
    companionLabel: string,
    sameTarget: (left: T, right: T) => boolean,
    bridge: AnchoredTriggerBridge<T, P>,
  ) {
    this.companionLabel = companionLabel
    this.sameTarget = sameTarget
    this.bridge = bridge
  }

  getSnapshot = (): AnchoredTriggerSnapshot<T> => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.startListening()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  hover(target: T, anchor: AnchorRegion, presentation?: P): Promise<void> {
    const activation =
      this.matchesSnapshot(target) && this.snapshot.activation === "selected"
        ? "selected"
        : "hovered"
    return this.activate(target, anchor, activation, presentation)
  }

  select(target: T, anchor: AnchorRegion, presentation?: P): Promise<void> {
    return this.activate(target, anchor, "selected", presentation)
  }

  leave(): Promise<void> {
    this.lastLeaveRevision = this.requestRevision
    return this.bridge.conceal().catch(() => undefined)
  }

  private activate(
    target: T,
    anchor: AnchorRegion,
    activation: Exclude<AnchoredTriggerActivation, "idle">,
    presentation: P | undefined,
  ): Promise<void> {
    const revision = ++this.requestRevision
    this.setSnapshot({ ...this.snapshot, activation, target })
    return new Promise((resolve) => {
      const next = { revision, target, anchor, presentation, resolve }
      const active = this.activeRequest
      if (!active) {
        this.startRequest(next)
        return
      }
      if (this.sameTarget(active.target, target) && this.lastLeaveRevision < active.revision) {
        this.queuedRequest?.resolve()
        this.queuedRequest = null
        resolve()
        return
      }
      this.queuedRequest?.resolve()
      this.queuedRequest = next
    })
  }

  private startRequest(scheduled: ScheduledRequest<T, P>): void {
    this.activeRequest = scheduled
    void this.bridge
      .request(scheduled.target, scheduled.anchor, scheduled.presentation)
      .then((request) => this.acceptRequest(scheduled.target, request))
      .catch(() => this.rejectRequest(scheduled.target))
      .finally(() => this.finishRequest(scheduled))
  }

  private acceptRequest(target: T, request: AnchoredWindowRequest<T>): void {
    if (!this.matchesSnapshot(target)) return
    if (request.target && !this.sameTarget(request.target, target)) return
    this.latestGeneration = Math.max(this.latestGeneration, request.generation)
    if (request.generation === this.snapshot.generation) return
    this.setSnapshot({ ...this.snapshot, generation: request.generation })
  }

  private rejectRequest(target: T): void {
    if (!this.matchesSnapshot(target) || this.queuedTargetsSnapshot()) return
    this.setSnapshot({ activation: "idle", target: null, generation: this.latestGeneration })
  }

  private finishRequest(scheduled: ScheduledRequest<T, P>): void {
    if (this.activeRequest?.revision !== scheduled.revision) return
    this.activeRequest = null
    scheduled.resolve()
    const queued = this.queuedRequest
    this.queuedRequest = null
    if (queued) this.startRequest(queued)
  }

  private acceptLifecycle = (event: AnchoredWindowLifecycleEvent<T>): void => {
    if (event.companionLabel !== this.companionLabel) return
    const { state } = event
    if (state.generation < this.latestGeneration) return

    if (!state.target) {
      if (this.requestTargetsSnapshot()) return
      this.latestGeneration = state.generation
      this.setSnapshot({ activation: "idle", target: null, generation: state.generation })
      return
    }

    if (!this.matchesSnapshot(state.target)) return
    this.latestGeneration = state.generation
    this.setSnapshot({ ...this.snapshot, generation: state.generation })
  }

  private async startListening(): Promise<void> {
    const generation = ++this.listenerGeneration
    try {
      const unlisten = await this.bridge.listen(this.acceptLifecycle)
      if (generation !== this.listenerGeneration || this.listeners.size === 0) {
        unlisten()
        return
      }
      this.stopListening = unlisten
      await this.readState(generation)
    } catch {
      if (generation !== this.listenerGeneration || this.listeners.size === 0) return
      this.retryTimer = setTimeout(() => {
        this.retryTimer = null
        if (generation === this.listenerGeneration && this.listeners.size > 0) {
          void this.startListening()
        }
      }, LISTENER_RETRY_MS)
    }
  }

  private async readState(listenerGeneration: number): Promise<void> {
    const requestRevision = this.requestRevision
    try {
      const state = await this.bridge.state()
      if (
        listenerGeneration === this.listenerGeneration &&
        requestRevision === this.requestRevision &&
        this.listeners.size > 0
      ) {
        this.acceptLifecycle({ companionLabel: this.companionLabel, state })
      }
    } catch {
      // The active event listener remains the recovery path for a failed state read.
    }
  }

  private stop(): void {
    this.listenerGeneration += 1
    if (this.retryTimer != null) clearTimeout(this.retryTimer)
    this.retryTimer = null
    this.stopListening?.()
    this.stopListening = null
  }

  private matchesSnapshot(target: T): boolean {
    return this.snapshot.target != null && this.sameTarget(this.snapshot.target, target)
  }

  private queuedTargetsSnapshot(): boolean {
    return this.queuedRequest != null && this.matchesSnapshot(this.queuedRequest.target)
  }

  private requestTargetsSnapshot(): boolean {
    if (this.requestRevision <= this.lastLeaveRevision || this.snapshot.target == null)
      return false
    return (
      (this.activeRequest != null && this.matchesSnapshot(this.activeRequest.target)) ||
      this.queuedTargetsSnapshot()
    )
  }

  private setSnapshot(snapshot: AnchoredTriggerSnapshot<T>): void {
    if (
      snapshot.activation === this.snapshot.activation &&
      snapshot.generation === this.snapshot.generation &&
      ((snapshot.target === null && this.snapshot.target === null) ||
        (snapshot.target !== null && this.matchesSnapshot(snapshot.target)))
    ) {
      return
    }
    this.snapshot = snapshot
    this.listeners.forEach((listener) => listener())
  }
}

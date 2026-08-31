import {
  getPopoverPeekData,
  getPopoverPeekState,
  onPopoverPeekRequest,
  popoverPeekReady,
  type PopoverPeekData,
  type PopoverPeekRequest,
  type PopoverPeekTarget,
} from "../../lib/popoverPeekIpc"

export interface PopoverPeekSnapshot {
  requested: PopoverPeekRequest
  presented: PopoverPeekPresentation | null
  candidate: PopoverPeekCandidate | null
  coldLoading: boolean
  failed: PopoverPeekActiveRequest | null
}

export interface PopoverPeekActiveRequest extends PopoverPeekRequest {
  target: PopoverPeekTarget
}

export interface PopoverPeekCandidate {
  request: PopoverPeekActiveRequest
  data: PopoverPeekData
}

export interface PopoverPeekPresentation {
  request: PopoverPeekActiveRequest
  data: PopoverPeekData | null
  unavailable: boolean
}

interface PopoverPeekBridge {
  listen: typeof onPopoverPeekRequest
  state: typeof getPopoverPeekState
  data: typeof getPopoverPeekData
  ready: typeof popoverPeekReady
}

const DEFAULT_BRIDGE: PopoverPeekBridge = {
  listen: onPopoverPeekRequest,
  state: getPopoverPeekState,
  data: getPopoverPeekData,
  ready: popoverPeekReady,
}

const LISTENER_RETRY_MS = 250

const EMPTY_REQUEST: PopoverPeekRequest = {
  generation: 0,
  target: null,
  retargetCommitRequired: false,
  initialPresentation: null,
}

function sameTarget(left: PopoverPeekTarget | null, right: PopoverPeekTarget | null): boolean {
  if (left === null || right === null) return left === right
  return left.provider === right.provider && left.utcOffsetMinutes === right.utcOffsetMinutes
}

/** Owns one renderer's request listener and rejects stale async results. */
export class PopoverPeekController {
  private snapshot: PopoverPeekSnapshot = {
    requested: EMPTY_REQUEST,
    presented: null,
    candidate: null,
    coldLoading: false,
    failed: null,
  }
  private readonly listeners = new Set<() => void>()
  private stopListening: (() => void) | null = null
  private startGeneration = 0
  private readonly bridge: PopoverPeekBridge
  private retryTimer: ReturnType<typeof setTimeout> | null = null
  private readyRetryTimer: ReturnType<typeof setTimeout> | null = null
  private rendererGeneration: number | null = null
  private readyGeneration: number | null = null
  private activeLoad: PopoverPeekActiveRequest | null = null
  private pendingLoad: PopoverPeekActiveRequest | null = null

  constructor(bridge: PopoverPeekBridge = DEFAULT_BRIDGE) {
    this.bridge = bridge
  }

  getSnapshot = (): PopoverPeekSnapshot => this.snapshot

  commitRenderer(node: HTMLDivElement | null): void {
    if (!node) return
    const generation = window.__ANTIBURN_WINDOW_GENERATION__
    if (typeof generation !== "number" || !Number.isSafeInteger(generation)) return
    this.rendererGeneration = generation
    void this.reportReady(this.startGeneration)
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) void this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) {
        this.startGeneration += 1
        if (this.retryTimer != null) clearTimeout(this.retryTimer)
        this.retryTimer = null
        if (this.readyRetryTimer != null) clearTimeout(this.readyRetryTimer)
        this.readyRetryTimer = null
        this.stopListening?.()
        this.stopListening = null
      }
    }
  }

  accept = (request: PopoverPeekRequest): void => {
    if (request.generation < this.snapshot.requested.generation) return
    const repeatsCurrent =
      request.generation === this.snapshot.requested.generation &&
      sameTarget(request.target, this.snapshot.requested.target)
    if (repeatsCurrent && request.initialPresentation == null) {
      return
    }
    if (!request.target) {
      this.pendingLoad = null
      this.snapshot = {
        requested: request,
        presented: null,
        candidate: null,
        coldLoading: false,
        failed: null,
      }
      this.publish()
      return
    }

    const activeRequest: PopoverPeekActiveRequest = { ...request, target: request.target }
    if (request.initialPresentation) {
      this.pendingLoad = null
      this.snapshot = {
        requested: activeRequest,
        presented: {
          request: activeRequest,
          data: request.initialPresentation,
          unavailable: false,
        },
        candidate: null,
        coldLoading: false,
        failed: null,
      }
      this.publish()
      return
    }
    if (repeatsCurrent) return
    this.snapshot = {
      requested: activeRequest,
      presented: this.snapshot.presented,
      candidate: null,
      coldLoading: this.snapshot.presented == null,
      failed: null,
    }
    this.publish()
    if (this.activeLoad == null) this.startLoad(activeRequest)
    else this.pendingLoad = activeRequest
  }

  promote = (generation: number): void => {
    if (generation !== this.snapshot.requested.generation) return
    const candidate = this.snapshot.candidate
    if (candidate?.request.generation === generation) {
      this.snapshot = {
        ...this.snapshot,
        presented: { request: candidate.request, data: candidate.data, unavailable: false },
        candidate: null,
        coldLoading: false,
      }
      this.publish()
      return
    }
    const failed = this.snapshot.failed
    if (failed?.generation !== generation) return
    this.snapshot = {
      ...this.snapshot,
      presented: { request: failed, data: null, unavailable: true },
      coldLoading: false,
      failed: null,
    }
    this.publish()
  }

  discard = (generation: number): void => {
    if (
      this.snapshot.candidate?.request.generation !== generation &&
      this.snapshot.failed?.generation !== generation
    ) {
      return
    }
    this.snapshot = {
      ...this.snapshot,
      candidate:
        this.snapshot.candidate?.request.generation === generation
          ? null
          : this.snapshot.candidate,
      failed: this.snapshot.failed?.generation === generation ? null : this.snapshot.failed,
    }
    this.publish()
  }

  private async start(): Promise<void> {
    const startGeneration = ++this.startGeneration
    let unlisten: () => void
    try {
      unlisten = await this.bridge.listen(this.accept)
    } catch {
      await this.readState(startGeneration)
      this.scheduleListenerRetry(startGeneration)
      return
    }
    if (startGeneration !== this.startGeneration || this.listeners.size === 0) {
      unlisten()
      return
    }
    this.stopListening = unlisten
    void this.reportReady(startGeneration)
    await this.readState(startGeneration)
  }

  private async reportReady(startGeneration: number): Promise<void> {
    const generation = this.rendererGeneration
    if (
      generation == null ||
      this.readyGeneration === generation ||
      startGeneration !== this.startGeneration ||
      this.stopListening == null ||
      this.listeners.size === 0
    ) {
      return
    }
    this.readyGeneration = generation
    try {
      await this.bridge.ready(generation)
    } catch {
      if (this.readyGeneration !== generation) return
      this.readyGeneration = null
      if (startGeneration !== this.startGeneration || this.listeners.size === 0) return
      this.readyRetryTimer = setTimeout(() => {
        this.readyRetryTimer = null
        void this.reportReady(startGeneration)
      }, LISTENER_RETRY_MS)
    }
  }

  private async readState(startGeneration: number): Promise<void> {
    try {
      const state = await this.bridge.state()
      if (startGeneration === this.startGeneration) {
        this.accept({
          generation: state.generation,
          target: state.target,
          retargetCommitRequired: state.awaitingRetargetCommit,
          initialPresentation: null,
        })
      }
    } catch {
      // The active event listener remains the recovery path for a failed state read.
    }
  }

  private scheduleListenerRetry(startGeneration: number): void {
    if (startGeneration !== this.startGeneration || this.listeners.size === 0) return
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null
      if (startGeneration === this.startGeneration && this.listeners.size > 0) {
        void this.start()
      }
    }, LISTENER_RETRY_MS)
  }

  private startLoad(request: PopoverPeekActiveRequest): void {
    this.activeLoad = request
    void this.load(request).finally(() => {
      if (this.activeLoad !== request) return
      this.activeLoad = null
      const pending = this.pendingLoad
      this.pendingLoad = null
      if (pending?.target && this.isCurrent(pending)) {
        this.startLoad(pending)
      }
    })
  }

  private async load(request: PopoverPeekActiveRequest): Promise<void> {
    try {
      const data = await this.bridge.data(request.generation)
      if (!this.isCurrent(request)) return
      if (this.snapshot.presented?.request.generation === request.generation) return
      this.snapshot = { ...this.snapshot, candidate: { request, data }, failed: null }
      this.publish()
    } catch {
      if (!this.isCurrent(request)) return
      this.snapshot = {
        ...this.snapshot,
        candidate: null,
        failed: request,
      }
      this.publish()
    }
  }

  private isCurrent(request: PopoverPeekRequest): boolean {
    return (
      request.generation === this.snapshot.requested.generation &&
      sameTarget(request.target, this.snapshot.requested.target)
    )
  }

  private publish(): void {
    this.listeners.forEach((listener) => listener())
  }
}

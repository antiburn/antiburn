import {
  fetchSessionEvidence,
  type EvidenceReference,
  type SessionEvidenceContextResponse,
} from "../../lib/sessionEvidenceIpc"
import { onSessionIndexChanged, onSessionUpdated } from "../../lib/sessionIpc"

type ListenerSource = (handler: () => void) => Promise<() => void>

export interface SessionEvidenceDependencies {
  fetch: typeof fetchSessionEvidence
  onSessionIndexChanged: ListenerSource
  onSessionUpdated: ListenerSource
}

export interface SessionEvidenceSnapshot {
  phase: "loading" | "ready" | "unavailable" | "error"
  context: SessionEvidenceContextResponse | null
}

const DEFAULT_DEPENDENCIES: SessionEvidenceDependencies = {
  fetch: fetchSessionEvidence,
  onSessionIndexChanged: (handler) => onSessionIndexChanged(handler),
  onSessionUpdated: (handler) => onSessionUpdated(handler),
}

export class SessionEvidenceSession {
  private readonly reference: EvidenceReference
  private readonly deps: SessionEvidenceDependencies
  private snapshot: SessionEvidenceSnapshot = { phase: "loading", context: null }
  private listeners = new Set<() => void>()
  private stops: Array<() => void> = []
  private lifetime = 0
  private revision = 0
  private running = false
  private pending = false

  constructor(reference: EvidenceReference, deps = DEFAULT_DEPENDENCIES) {
    this.reference = reference
    this.deps = deps
  }

  getSnapshot = (): SessionEvidenceSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (this.listeners.size === 1) this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size) return
      this.lifetime += 1
      this.revision += 1
      this.pending = false
      for (const stop of this.stops.splice(0)) stop()
      this.snapshot = { phase: "loading", context: null }
    }
  }

  retry = (): void => {
    if (!this.listeners.size) return
    this.revision += 1
    if (this.snapshot.phase !== "ready") this.publish({ phase: "loading", context: null })
    this.pending = true
    if (!this.running) void this.read()
  }

  private start(): void {
    const lifetime = ++this.lifetime
    for (const source of [this.deps.onSessionIndexChanged, this.deps.onSessionUpdated]) {
      void source(this.retry)
        .then((stop) => {
          if (this.lifetime !== lifetime || !this.listeners.size) stop()
          else this.stops.push(stop)
        })
        .catch(() => {})
    }
    this.retry()
  }

  private async read(): Promise<void> {
    this.running = true
    try {
      while (this.pending && this.listeners.size) {
        this.pending = false
        const revision = this.revision
        try {
          const context = await this.deps.fetch(this.reference)
          if (revision !== this.revision || !this.listeners.size) continue
          this.publish({
            phase: context.available && context.match ? "ready" : "unavailable",
            context,
          })
        } catch {
          if (revision === this.revision && this.listeners.size)
            this.publish({ phase: "error", context: null })
        }
      }
    } finally {
      this.running = false
    }
  }

  private publish(snapshot: SessionEvidenceSnapshot): void {
    this.snapshot = snapshot
    for (const listener of this.listeners) listener()
  }
}

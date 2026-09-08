import {
  cancelInsightsReport,
  getInsightsReport,
  getInsightsStatus,
  type InsightsReportPayload,
  type InsightsStatusPayload,
} from "../../lib/insightsIpc"
import { SurfaceExposureTracker } from "../../lib/surfaceExposure"

/** Where the report fetch stands. `ready` with a null report means the
 *  shell is absent (browser mode), which the pane names explicitly. */
type InsightsPhase = "loading" | "ready" | "error"

export type InsightsSnapshot = {
  phase: InsightsPhase
  report: InsightsReportPayload | null
  status: InsightsStatusPayload | null
  error: string | null
}

/** How often the session re-reads the processing status while mounted. */
const STATUS_POLL_MS = 5_000

type InsightsObservedState = "ready" | "empty" | "error" | null

function observedState(snapshot: InsightsSnapshot): InsightsObservedState {
  if (snapshot.phase === "loading") return null
  if (snapshot.phase === "error" || snapshot.report === null) return "error"
  if (snapshot.report.coverage.discovered === 0) return "empty"
  if (snapshot.report.coverage.ready === 0) return null
  return "ready"
}

/**
 * The imperative boundary behind the Insights pane.
 *
 * React reads immutable snapshots through `useSyncExternalStore`; the
 * report IPC call, its in-flight and error state, and the processing-status
 * poll all live here rather than in a component lifecycle — see
 * `SourcesSession` and `SettingsWindowSession` for the same shape.
 *
 * Ref-counted like those sessions: the first subscriber starts the work,
 * and the last unsubscribe stops it and asks the shell to cancel a report
 * reduction still in flight, so closing the pane never leaves the shell
 * computing for nobody.
 *
 * Unmounting stops the status poll and cancels report work. The session also
 * pauses on `visibilitychange` when the operating system hides the window.
 */
export class InsightsSession {
  private listeners = new Set<() => void>()
  private started = false
  private hostVisible: boolean
  private workActive = false
  private generation = 0
  private pollTimer: ReturnType<typeof setTimeout> | null = null
  private readonly exposure = new SurfaceExposureTracker()
  private exposureGeneration: number | null = null
  private exposureIdentity: number | null = null

  private snapshot: InsightsSnapshot = {
    phase: "loading",
    report: null,
    status: null,
    error: null,
  }

  constructor(hostVisible = true) {
    this.hostVisible = hostVisible
  }

  getSnapshot = (): InsightsSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) this.start()
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) this.stop()
    }
  }

  /** Recompute the report on demand. */
  refresh = async (): Promise<void> => {
    const generation = this.generation
    const exposureGeneration = this.exposureGeneration
    this.update({ phase: "loading", error: null })
    await this.loadReport(generation, exposureGeneration)
  }

  setHostVisible = (visible: boolean): void => {
    if (visible === this.hostVisible) return
    this.hostVisible = visible
    if (!this.started) return
    if (!visible) this.pauseWork()
    else if (document.visibilityState !== "hidden") this.startWork()
  }

  private start(): void {
    this.started = true
    document.addEventListener("visibilitychange", this.handleVisibility)
    if (this.hostVisible && document.visibilityState !== "hidden") this.startWork()
  }

  private stop(): void {
    this.started = false
    document.removeEventListener("visibilitychange", this.handleVisibility)
    this.pauseWork(false)
  }

  private handleVisibility = (): void => {
    if (!this.started) return
    if (document.visibilityState === "hidden") this.pauseWork()
    else if (this.hostVisible) this.startWork()
  }

  private startWork(): void {
    if (this.workActive) return
    this.workActive = true
    const generation = ++this.generation
    const state = observedState(this.snapshot)
    const identity = this.exposureIdentity ?? generation
    this.exposureIdentity = identity
    this.exposureGeneration = this.exposure.expose({
      surface: "insights",
      origin: "user",
      identity,
      ...(state ? { state } : {}),
    })
    void this.pollStatus(generation)
    void this.loadReport(generation, this.exposureGeneration)
  }

  private pauseWork(concealExposure = true): void {
    if (!this.workActive) return
    this.workActive = false
    this.generation += 1
    if (concealExposure && this.exposureGeneration !== null) {
      this.exposure.conceal("insights", this.exposureGeneration)
      this.exposureGeneration = null
      this.exposureIdentity = null
    } else {
      this.exposure.suspend()
    }
    if (this.pollTimer !== null) {
      clearTimeout(this.pollTimer)
      this.pollTimer = null
    }
    // Closing or hiding the pane cancels report work in the shell
    // (FR-16). The reduction is read-only, so this can never lose
    // stored evidence.
    void cancelInsightsReport()
  }

  private loadReport = async (
    generation: number,
    exposureGeneration: number | null,
  ): Promise<void> => {
    try {
      const report = await getInsightsReport()
      if (generation !== this.generation) return
      this.update({ phase: "ready", report, error: null })
      const state = observedState(this.snapshot)
      if (state && exposureGeneration !== null) {
        this.exposure.observe(state, exposureGeneration)
      }
    } catch (error) {
      if (generation !== this.generation) return
      // An error snapshot, never an empty or clean one: the pane renders
      // this as a failure with a retry, not as "no findings".
      this.update({ phase: "error", error: String(error) })
      if (exposureGeneration !== null) this.exposure.observe("error", exposureGeneration)
    }
  }

  private pollStatus = async (generation: number): Promise<void> => {
    const status = await getInsightsStatus().catch(() => null)
    if (generation !== this.generation) return
    if (status) {
      const previous = this.snapshot.status
      this.update({ status })
      // When the backlog drains, recompute once: the report on screen
      // predates the work that just finished. The transition guard keeps
      // this from looping — after the refresh, `previous` is already
      // drained.
      const drained =
        previous !== null &&
        previous.pending + previous.processing > 0 &&
        status.pending + status.processing === 0 &&
        !status.calculating
      if (drained && this.snapshot.phase === "ready") {
        void this.loadReport(generation, this.exposureGeneration)
      }
    }
    this.pollTimer = setTimeout(() => {
      void this.pollStatus(generation)
    }, STATUS_POLL_MS)
  }

  private update(change: Partial<InsightsSnapshot>): void {
    this.snapshot = { ...this.snapshot, ...change }
    for (const listener of this.listeners) listener()
  }
}

import {
  noteInteraction,
  type LiveUsageProvider,
  type LiveUsageState,
  type StateSurface,
  type SurfaceOrigin,
  type SurfaceState,
} from "./ipc"
import type { LiveUsageSummaryPayload } from "./providerUsageIpc"
import {
  liveDisplayableProviders,
  liveProviderStatus,
  liveUnavailableProviders,
  liveWindows,
} from "./presentation/liveUsage"

const INITIAL_LOAD_TIMEOUT_MS = 10_000

export type SurfaceExposure = {
  surface: StateSurface
  origin: SurfaceOrigin
  /** Local identity for visible content within one surface. This value never leaves the renderer. */
  identity?: string | number
  state?: SurfaceState | null
}

export type LiveUsageObservation = {
  provider: LiveUsageProvider
  state: LiveUsageState
}

type ActiveExposure = {
  generation: number
  surface: StateSurface
  origin: SurfaceOrigin
  identity: string | number | undefined
  states: Set<SurfaceState>
  liveUsageStates: Set<string>
}

function supportedProvider(provider: string): provider is LiveUsageProvider {
  return provider === "anthropic" || provider === "openai" || provider === "google"
}

function failedState(category: string): LiveUsageState {
  switch (category) {
    case "authentication":
      return "authentication"
    case "rateLimited":
      return "rate_limited"
    default:
      return "unavailable"
  }
}

/** Return the provider states that the supplied usage surface presents. */
export function liveUsageObservations(
  summary: LiveUsageSummaryPayload,
  onlyProvider?: string,
): LiveUsageObservation[] {
  const observations: LiveUsageObservation[] = []
  const seen = new Set<LiveUsageProvider>()
  for (const meter of summary.meters) {
    if (!meter.shown || (onlyProvider && meter.provider !== onlyProvider)) continue
    if (!supportedProvider(meter.provider) || seen.has(meter.provider)) continue
    seen.add(meter.provider)

    const providers = liveDisplayableProviders(summary).filter(
      (entry) => entry.provider === meter.provider && liveWindows(entry).length > 0,
    )
    for (const provider of providers) {
      const status = liveProviderStatus(summary, provider)
      if (status.kind === "live") {
        observations.push({
          provider: meter.provider,
          state: provider.freshness === "stale" ? "stale" : "fresh",
        })
      } else if (status.kind === "grace") {
        observations.push({ provider: meter.provider, state: "stale" })
        observations.push({ provider: meter.provider, state: failedState(status.category) })
      } else {
        observations.push({ provider: meter.provider, state: failedState(status.category) })
      }
    }

    const unavailable = liveUnavailableProviders(summary).find(
      (entry) => entry.provider === meter.provider,
    )
    if (unavailable) {
      observations.push({ provider: meter.provider, state: failedState(unavailable.category) })
    }
  }
  return observations.filter(
    (observation, index) =>
      observations.findIndex(
        (candidate) =>
          candidate.provider === observation.provider && candidate.state === observation.state,
      ) === index,
  )
}

/** Own visible exposure generations and their bounded analytics state. */
export class SurfaceExposureTracker {
  private generation = 0
  private active: ActiveExposure | null = null
  private timeout: ReturnType<typeof setTimeout> | null = null
  private timeoutDeadline: number | null = null

  expose(exposure: SurfaceExposure): number {
    const current = this.active
    if (
      current &&
      current.surface === exposure.surface &&
      current.origin === exposure.origin &&
      current.identity === exposure.identity
    ) {
      if (exposure.state) this.observe(exposure.state, current.generation)
      else this.resumeTimeout(current.generation)
      return current.generation
    }

    this.clearTimeout()
    const generation = ++this.generation
    this.active = {
      generation,
      surface: exposure.surface,
      origin: exposure.origin,
      identity: exposure.identity,
      states: new Set(),
      liveUsageStates: new Set(),
    }
    if (exposure.surface !== "insights") {
      noteInteraction({
        kind: "surfaceViewed",
        surface: exposure.surface,
        origin: exposure.origin,
      })
    }
    if (exposure.state) {
      this.observe(exposure.state, generation)
    } else {
      this.timeoutDeadline = Date.now() + INITIAL_LOAD_TIMEOUT_MS
      this.resumeTimeout(generation)
    }
    return generation
  }

  observe(state: SurfaceState, generation = this.active?.generation): void {
    const active = this.active
    if (!active || generation !== active.generation || active.states.has(state)) return
    active.states.add(state)
    if (state !== "loading_timeout") this.clearTimeout()
    noteInteraction({
      kind: "surfaceStateObserved",
      surface: active.surface,
      state,
      origin: active.origin,
    })
  }

  observeLiveUsage(
    summary: LiveUsageSummaryPayload,
    onlyProvider?: string,
    generation = this.active?.generation,
  ): void {
    const active = this.active
    if (!active || active.origin !== "user" || generation !== active.generation) return
    for (const observation of liveUsageObservations(summary, onlyProvider)) {
      const key = `${observation.provider}:${observation.state}`
      if (active.liveUsageStates.has(key)) continue
      active.liveUsageStates.add(key)
      noteInteraction({ kind: "liveUsageStateObserved", ...observation, origin: active.origin })
    }
  }

  conceal(surface?: StateSurface, generation = this.active?.generation): void {
    const active = this.active
    if (!active || generation !== active.generation) return
    if (surface && surface !== active.surface) return
    this.clearTimeout()
    this.active = null
    this.generation += 1
  }

  /** Pause timer work while a controller has no subscriber. */
  suspend(): void {
    this.clearTimer()
  }

  private clearTimeout(): void {
    this.clearTimer()
    this.timeoutDeadline = null
  }

  private clearTimer(): void {
    if (this.timeout === null) return
    clearTimeout(this.timeout)
    this.timeout = null
  }

  private resumeTimeout(generation: number): void {
    if (this.timeout !== null || this.timeoutDeadline === null) return
    this.timeout = setTimeout(
      () => {
        this.timeout = null
        this.timeoutDeadline = null
        this.observe("loading_timeout", generation)
      },
      Math.max(0, this.timeoutDeadline - Date.now()),
    )
  }
}

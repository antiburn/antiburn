import {
  getOnboardingWindowVisible,
  getOnboardingHudVariant,
  type OnboardingHudVariant,
  onOnboardingWindowVisibilityChanged,
} from "./onboardingWindowIpc"
import { getOnboardingState, setOnboardingState, type OnboardingState } from "./ipc"

import { noteOnboardingAction, noteOnboardingScreenViewed } from "./onboardingAnalytics"

export interface DesktopOnboardingSnapshot {
  visible: boolean
  hudVariant: OnboardingHudVariant
  state: OnboardingState | null
  busy: boolean
  error: string | null
}

export interface DesktopOnboardingPort {
  getHudVariant?(): Promise<OnboardingHudVariant>
  getVisible?(): Promise<boolean>
  onVisible?(listener: (visible: boolean) => void): Promise<() => void>
  read(): Promise<OnboardingState>
  write(state: OnboardingState): Promise<OnboardingState>
}

/** Keep onboarding saves separate from the Settings snapshot. */
export class DesktopOnboardingSession {
  private snapshot: DesktopOnboardingSnapshot = {
    state: null,
    busy: true,
    error: null,
    visible: false,
    hudVariant: "unsupported",
  }
  private listeners = new Set<() => void>()
  private generation = 0
  private started = false
  private unlisten: (() => void) | null = null
  private viewed: OnboardingState | null = null

  private readonly port: DesktopOnboardingPort

  constructor(
    port: DesktopOnboardingPort = {
      getHudVariant: getOnboardingHudVariant,
      getVisible: getOnboardingWindowVisible,
      onVisible: onOnboardingWindowVisibilityChanged,
      read: getOnboardingState,
      write: setOnboardingState,
    },
  ) {
    this.port = port
  }

  getSnapshot = (): DesktopOnboardingSnapshot => this.snapshot

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener)
    if (!this.started) {
      this.started = true
      void this.load()
      void this.watchVisibility(this.generation)
      const generation = this.generation
      void this.port
        .getHudVariant?.()
        .then((hudVariant) => {
          if (generation === this.generation) this.publish({ ...this.snapshot, hudVariant })
        })
        .catch(() => undefined)
    }
    return () => {
      this.listeners.delete(listener)
      if (this.listeners.size === 0) {
        this.generation += 1
        this.started = false
        this.unlisten?.()
        this.unlisten = null
        this.viewed = null
      }
    }
  }

  private publish(snapshot: DesktopOnboardingSnapshot): void {
    this.snapshot = snapshot
    for (const listener of this.listeners) listener()
    if (
      snapshot.visible &&
      snapshot.state &&
      snapshot.state !== "completed" &&
      this.viewed !== snapshot.state
    ) {
      this.viewed = snapshot.state
      noteOnboardingScreenViewed(snapshot.state)
    }
  }

  private async watchVisibility(generation: number): Promise<void> {
    let revision = 0
    const update = (visible: boolean) => {
      if (generation !== this.generation) return
      revision += 1
      if (!visible) this.viewed = null
      this.publish({ ...this.snapshot, visible })
    }
    if (this.port.onVisible) {
      const unlisten = await this.port.onVisible(update).catch(() => null)
      if (generation !== this.generation) {
        unlisten?.()
        return
      }
      this.unlisten = unlisten
    }
    const before = revision
    const visible = await (this.port.getVisible?.() ?? Promise.resolve(true)).catch(() => false)
    if (revision === before) update(visible)
  }

  load = async (): Promise<void> => {
    const generation = this.generation
    this.publish({ ...this.snapshot, busy: true, error: null })
    try {
      const state = await this.port.read()
      if (generation !== this.generation) return
      this.publish({ ...this.snapshot, state, busy: false, error: null })
    } catch {
      if (generation !== this.generation) return
      this.publish({
        ...this.snapshot,
        busy: false,
        error: "Could not load your introduction. Try again.",
      })
    }
  }

  change = async (state: OnboardingState): Promise<void> => {
    if (
      this.snapshot.busy ||
      this.snapshot.state === null ||
      (this.snapshot.state === "completed" && state !== "completed")
    )
      return
    const generation = this.generation
    const previous = this.snapshot.state
    const order: OnboardingState[] = [
      "welcome",
      "sessions",
      "limits",
      "hud",
      "checks",
      "cloud",
      "cloudUnavailable",
      "completed",
    ]
    const action =
      state === "completed"
        ? "use_locally"
        : state === "cloudUnavailable"
          ? "set_up_cloud"
          : order.indexOf(state) < order.indexOf(previous)
            ? "back"
            : previous === "welcome"
              ? "get_started"
              : "continue"
    if (previous !== "completed" && previous !== "cloudUnavailable")
      noteOnboardingAction(previous, action)
    this.publish({ ...this.snapshot, busy: true, error: null })
    try {
      const saved = await this.port.write(state)
      if (generation !== this.generation) return
      this.publish({ ...this.snapshot, state: saved, busy: false, error: null })
    } catch {
      if (generation !== this.generation) return
      this.publish({
        ...this.snapshot,
        busy: false,
        error:
          state === "completed"
            ? "Could not finish the introduction. Try again."
            : "Could not save your progress. Try again.",
      })
    }
  }
}

import { beforeEach, describe, expect, it, vi } from "vitest"

import { noteInteraction, type Interaction } from "./ipc"
import {
  noteOnboardingAction,
  noteOnboardingCloudUnavailable,
  noteOnboardingScreenViewed,
} from "./onboardingAnalytics"

const tauriInvoke = vi.hoisted(() => vi.fn())

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauriInvoke,
  isTauri: () => false,
}))

const nativeInvoke = vi.fn()

describe("native peek analytics", () => {
  beforeEach(() => {
    tauriInvoke.mockReset()
    nativeInvoke.mockReset()
    nativeInvoke.mockResolvedValue(undefined)
    Object.defineProperty(window, "__ANTIBURN_NATIVE_PEEK__", {
      configurable: true,
      value: { invoke: nativeInvoke, listen: vi.fn() },
    })
  })

  it("sends preview exposure events through the restricted native command", () => {
    const interactions: Interaction[] = [
      { kind: "surfaceViewed", surface: "provider_preview", origin: "user" },
      {
        kind: "surfaceStateObserved",
        surface: "provider_preview",
        state: "ready",
        origin: "user",
      },
      { kind: "liveUsageStateObserved", provider: "openai", state: "fresh", origin: "user" },
    ]
    interactions.forEach(noteInteraction)

    expect(nativeInvoke.mock.calls).toEqual(
      interactions.map((interaction) => ["note_interaction", { interaction }]),
    )
    expect(tauriInvoke).not.toHaveBeenCalled()
  })

  it("does not widen the native command beyond preview observations", () => {
    noteInteraction({ kind: "sessionOpened", agent: "codex", environment: "native" })
    noteInteraction({ kind: "surfaceViewed", surface: "activity", origin: "user" })
    noteOnboardingScreenViewed("cloud")
    noteOnboardingAction("cloud", "set_up_cloud")
    noteOnboardingCloudUnavailable()

    expect(nativeInvoke).not.toHaveBeenCalled()
    expect(tauriInvoke).not.toHaveBeenCalled()
  })

  it("keeps onboarding analytics inert in a browser without the shell", () => {
    Reflect.deleteProperty(window, "__ANTIBURN_NATIVE_PEEK__")

    noteOnboardingScreenViewed("cloud")
    noteOnboardingAction("cloud", "set_up_cloud")
    noteOnboardingCloudUnavailable()

    expect(nativeInvoke).not.toHaveBeenCalled()
    expect(tauriInvoke).not.toHaveBeenCalled()
  })
})

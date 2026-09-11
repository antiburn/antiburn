import { beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "./ipc"
import type { LiveUsageSummaryPayload } from "./providerUsageIpc"
import { SurfaceExposureTracker, liveUsageObservations } from "./surfaceExposure"

const noteInteraction = vi.hoisted(() => vi.fn())

vi.mock("./ipc", async (importOriginal) => {
  const actual = await importOriginal<typeof Ipc>()
  return { ...actual, noteInteraction }
})

function liveUsage(overrides: Partial<LiveUsageSummaryPayload> = {}): LiveUsageSummaryPayload {
  return {
    providers: [],
    errors: [],
    meters: [{ provider: "anthropic", displayName: "Claude", shown: true }],
    generatedAt: "2026-09-08T00:05:00Z",
    ...overrides,
  }
}

function provider(freshness: "fresh" | "stale" = "fresh") {
  return {
    provider: "anthropic",
    accountKey: null,
    displayName: "Claude",
    support: "live" as const,
    freshness,
    sourceLabel: "Claude",
    observedAt: "2026-09-08T00:04:00Z",
    windows: [
      {
        id: "five-hour",
        role: "primaryShort",
        kind: "rolling",
        scopeModel: null,
        usedPercent: 20,
        startsAt: "2026-09-07T20:00:00Z",
        resetsAt: "2026-09-08T01:00:00Z",
        hasNonzeroUsageInCurrentPeriod: true,
        forecast: {
          unavailableReason: null,
          confidence: "high",
          consumptionRate: 4,
          paceRatio: 1,
          paceTrend: 1,
          runwayAt: null,
          usedToday: 20,
        },
      },
    ],
    extraUsage: null,
    resetCredits: null,
    plan: null,
    accountUuid: null,
    accountEmail: null,
  }
}

beforeEach(() => {
  noteInteraction.mockReset()
  vi.useRealTimers()
})

describe("SurfaceExposureTracker", () => {
  it("records one view and each presented state once per exposure", () => {
    const tracker = new SurfaceExposureTracker()
    const generation = tracker.expose({ surface: "activity", origin: "user" })

    tracker.observe("ready", generation)
    tracker.observe("ready", generation)
    tracker.expose({ surface: "activity", origin: "user", state: "ready" })

    expect(noteInteraction.mock.calls).toEqual([
      [{ kind: "surfaceViewed", surface: "activity", origin: "user" }],
      [
        {
          kind: "surfaceStateObserved",
          surface: "activity",
          state: "ready",
          origin: "user",
        },
      ],
    ])
  })

  it("treats a new local identity as a new visible detail exposure", () => {
    const tracker = new SurfaceExposureTracker()

    tracker.expose({ surface: "session_detail", origin: "user", identity: "one" })
    tracker.expose({ surface: "session_detail", origin: "user", identity: "two" })

    expect(noteInteraction).toHaveBeenCalledTimes(2)
    expect(noteInteraction).toHaveBeenNthCalledWith(2, {
      kind: "surfaceViewed",
      surface: "session_detail",
      origin: "user",
    })
  })

  it("cancels hidden timeouts and ignores stale results", () => {
    vi.useFakeTimers()
    const tracker = new SurfaceExposureTracker()
    const hidden = tracker.expose({ surface: "activity", origin: "user" })
    tracker.conceal("activity", hidden)
    const current = tracker.expose({ surface: "session_detail", origin: "user" })

    tracker.observe("ready", hidden)
    vi.advanceTimersByTime(10_000)

    expect(noteInteraction).toHaveBeenCalledTimes(3)
    expect(noteInteraction).toHaveBeenLastCalledWith({
      kind: "surfaceStateObserved",
      surface: "session_detail",
      state: "loading_timeout",
      origin: "user",
    })

    tracker.observe("ready", current)
    expect(noteInteraction).toHaveBeenLastCalledWith({
      kind: "surfaceStateObserved",
      surface: "session_detail",
      state: "ready",
      origin: "user",
    })
  })

  it("preserves the timeout deadline across a controller remount", () => {
    vi.useFakeTimers()
    vi.setSystemTime("2026-09-08T00:00:00Z")
    const tracker = new SurfaceExposureTracker()
    tracker.expose({ surface: "activity", origin: "user" })
    vi.advanceTimersByTime(6_000)
    tracker.suspend()
    vi.advanceTimersByTime(3_000)

    tracker.expose({ surface: "activity", origin: "user" })
    vi.advanceTimersByTime(1_000)

    expect(noteInteraction).toHaveBeenLastCalledWith({
      kind: "surfaceStateObserved",
      surface: "activity",
      state: "loading_timeout",
      origin: "user",
    })
  })

  it("records Insights state without inventing a surface view", () => {
    const tracker = new SurfaceExposureTracker()

    tracker.expose({ surface: "insights", origin: "user", state: "empty" })

    expect(noteInteraction).toHaveBeenCalledOnce()
    expect(noteInteraction).toHaveBeenCalledWith({
      kind: "surfaceStateObserved",
      surface: "insights",
      state: "empty",
      origin: "user",
    })
  })

  it("reports live provider states only for a deliberate exposure", () => {
    const summary = liveUsage({ providers: [provider()] })
    const tracker = new SurfaceExposureTracker()
    const automatic = tracker.expose({ surface: "hud", origin: "automatic", state: "ready" })
    tracker.observeLiveUsage(summary, undefined, automatic)
    tracker.conceal("hud", automatic)
    const user = tracker.expose({ surface: "activity", origin: "user", state: "ready" })

    tracker.observeLiveUsage(summary, undefined, user)
    tracker.observeLiveUsage(summary, undefined, user)

    expect(noteInteraction.mock.calls.flatMap(([interaction]) => interaction)).toContainEqual({
      kind: "liveUsageStateObserved",
      provider: "anthropic",
      state: "fresh",
      origin: "user",
    })
    expect(
      noteInteraction.mock.calls.filter(
        ([interaction]) => interaction.kind === "liveUsageStateObserved",
      ),
    ).toHaveLength(1)
  })
})

describe("liveUsageObservations", () => {
  it("reports stale cached data and its visible authentication failure separately", () => {
    const summary = liveUsage({
      providers: [provider()],
      errors: [
        {
          source: "claude",
          provider: "anthropic",
          displayName: "Claude",
          category: "authentication",
        },
      ],
    })

    expect(liveUsageObservations(summary)).toEqual([
      { provider: "anthropic", state: "stale" },
      { provider: "anthropic", state: "authentication" },
    ])
  })

  it("does not infer missing credentials from an empty provider result", () => {
    expect(liveUsageObservations(liveUsage())).toEqual([])
  })

  it("ignores disabled and unknown providers", () => {
    const summary = liveUsage({
      providers: [provider()],
      meters: [
        { provider: "anthropic", displayName: "Claude", shown: false },
        { provider: "future", displayName: "Future", shown: true },
      ],
    })

    expect(liveUsageObservations(summary)).toEqual([])
  })
})

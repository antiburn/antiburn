import { beforeEach, describe, expect, it, vi } from "vitest"

import type { PopoverPeekData } from "../../lib/popoverPeekIpc"
import { emptyBurnCheckPresentation } from "../../lib/presentation/burnChecks"
import { PopoverPeekController } from "./PopoverPeekController"

const analytics = vi.hoisted(() => ({
  conceal: vi.fn(),
  expose: vi.fn((_options: unknown) => 41),
  observeLiveUsage: vi.fn(),
  suspend: vi.fn(),
}))

vi.mock("../../lib/surfaceExposure", () => ({
  SurfaceExposureTracker: class {
    expose(options: unknown) {
      return analytics.expose(options)
    }
    observeLiveUsage(summary: unknown, provider: unknown, generation: unknown) {
      analytics.observeLiveUsage(summary, provider, generation)
    }
    conceal() {
      analytics.conceal()
    }
    suspend() {
      analytics.suspend()
    }
  },
}))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((settle, fail) => {
    resolve = settle
    reject = fail
  })
  return { promise, reject, resolve }
}

function providerTarget(provider: string) {
  return { kind: "provider" as const, provider, utcOffsetMinutes: 600 }
}

function request(generation: number, provider: string | null) {
  return {
    generation,
    target: provider == null ? null : providerTarget(provider),
    retargetCommitRequired: false,
    initialPresentation: null,
  }
}

function providerData(generatedAt: string): PopoverPeekData {
  return {
    kind: "provider",
    summary: { providers: [], generatedAt },
    live: { providers: [], errors: [], meters: [], generatedAt },
  }
}

function checksData(): PopoverPeekData {
  return {
    kind: "checks",
    presentation: {
      failures: [],
      wins: [],
      unavailable: [],
      refreshUnavailable: false,
      estimate: { tokenBurnBasisPoints: null },
      burnChecks: emptyBurnCheckPresentation("pending"),
    },
  }
}

function expiredProviderData(): PopoverPeekData {
  const generatedAt = new Date("2026-09-08T01:00:00Z").toISOString()
  return {
    kind: "provider",
    summary: { providers: [], generatedAt },
    live: {
      providers: [
        {
          provider: "anthropic",
          accountKey: null,
          displayName: "Claude",
          support: "live",
          freshness: "stale",
          sourceLabel: "cached usage",
          observedAt: new Date("2026-09-08T00:40:00Z").toISOString(),
          windows: [
            {
              id: "five-hour",
              role: "primaryShort",
              kind: "rolling",
              scopeModel: null,
              usedPercent: 50,
              startsAt: null,
              resetsAt: null,
              hasNonzeroUsageInCurrentPeriod: true,
              forecast: {
                unavailableReason: "stale",
                confidence: null,
                consumptionRate: null,
                paceRatio: null,
                paceTrend: null,
                runwayAt: null,
                usedToday: null,
              },
            },
          ],
          extraUsage: null,
          resetCredits: null,
          plan: null,
          accountUuid: null,
          accountEmail: null,
        },
      ],
      errors: [
        {
          source: "claude",
          provider: "anthropic",
          displayName: "Claude",
          category: "rateLimited",
        },
      ],
      meters: [{ provider: "anthropic", displayName: "Claude", shown: true }],
      generatedAt,
    },
  }
}

function controllerWith(data: (generation: number) => Promise<PopoverPeekData>) {
  return new PopoverPeekController({
    data,
    listen: vi.fn(async () => () => undefined),
    ready: vi.fn(async () => true),
    state: vi.fn(),
  })
}

describe("PopoverPeekController", () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it("records a preview only after its candidate is promoted", async () => {
    const loaded = deferred<PopoverPeekData>()
    const controller = controllerWith(vi.fn(() => loaded.promise))

    controller.accept(request(1, "first"))
    loaded.resolve(providerData("first"))
    await loaded.promise
    await Promise.resolve()
    expect(analytics.expose).not.toHaveBeenCalled()

    controller.confirmPresented(1)
    expect(analytics.expose).not.toHaveBeenCalled()
    controller.promote(1)

    expect(analytics.expose).toHaveBeenCalledWith({
      surface: "provider_preview",
      origin: "user",
      identity: 1,
      state: "empty",
    })
    expect(analytics.observeLiveUsage).toHaveBeenCalledWith(
      expect.objectContaining({ generatedAt: "first" }),
      "first",
      41,
    )
  })

  it("does not record a promoted candidate without native visibility confirmation", async () => {
    const loaded = deferred<PopoverPeekData>()
    const controller = controllerWith(vi.fn(() => loaded.promise))

    controller.accept(request(1, "first"))
    loaded.resolve(providerData("first"))
    await loaded.promise
    await Promise.resolve()
    controller.promote(1)

    expect(controller.getSnapshot().presented).toMatchObject({ request: { generation: 1 } })
    expect(analytics.expose).not.toHaveBeenCalled()
  })

  it("starts the timeout when a cold loading preview becomes visible", async () => {
    const ready = vi.fn(async () => true)
    const controller = new PopoverPeekController({
      data: vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
      listen: vi.fn(async () => () => undefined),
      ready,
      state: vi.fn(async () => ({
        generation: 0,
        target: null,
        rendererReady: false,
        visible: false,
        awaitingRetargetCommit: false,
        awaitingPresentation: false,
        awaitingConcealment: false,
      })),
    })
    Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
      configurable: true,
      value: 9,
    })
    controller.commitRenderer(document.createElement("div"))
    const unsubscribe = controller.subscribe(() => undefined)
    await vi.waitFor(() => expect(ready).toHaveBeenCalledWith(9))
    await Promise.resolve()

    controller.accept(request(1, "first"))

    expect(analytics.expose).toHaveBeenCalledWith({
      surface: "provider_preview",
      origin: "user",
      identity: 1,
    })
    unsubscribe()
  })

  it("waits for native confirmation before recording seeded content", () => {
    const controller = controllerWith(
      vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
    )
    controller.accept({
      ...request(2, "second"),
      initialPresentation: providerData("seeded"),
    })

    expect(analytics.expose).not.toHaveBeenCalled()
    controller.confirmPresented(2)

    expect(analytics.expose).toHaveBeenCalledWith(
      expect.objectContaining({ surface: "provider_preview", identity: 2 }),
    )
  })

  it("records an empty checks preview after native confirmation", () => {
    const controller = controllerWith(
      vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
    )
    controller.accept({
      generation: 3,
      target: { kind: "checks" },
      retargetCommitRequired: true,
      initialPresentation: checksData(),
    })

    controller.confirmPresented(3)

    expect(analytics.expose).toHaveBeenCalledWith({
      surface: "checks_preview",
      origin: "user",
      identity: 3,
      state: "empty",
    })
  })

  it("does not treat unavailable-only checks as ready data", () => {
    const controller = controllerWith(
      vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
    )
    const data = checksData()
    if (data.kind === "checks") {
      data.presentation.unavailable.push({
        id: "cacheChurn",
        finding: 0,
        clean: 0,
        unavailable: 3,
        estimatedTokenBurnBasisPoints: null,
      })
    }
    controller.accept({
      generation: 3,
      target: { kind: "checks" },
      retargetCommitRequired: true,
      initialPresentation: data,
    })

    controller.confirmPresented(3)

    expect(analytics.expose).toHaveBeenCalledWith(expect.objectContaining({ state: "empty" }))
  })

  it("reports assessed checks as ready data", () => {
    const controller = controllerWith(
      vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
    )
    const data = checksData()
    if (data.kind === "checks") {
      data.presentation.wins.push({
        id: "cacheChurn",
        finding: 0,
        clean: 3,
        unavailable: 0,
        estimatedTokenBurnBasisPoints: 0,
      })
    }
    controller.accept({
      generation: 4,
      target: { kind: "checks" },
      retargetCommitRequired: true,
      initialPresentation: data,
    })

    controller.confirmPresented(4)

    expect(analytics.expose).toHaveBeenCalledWith(expect.objectContaining({ state: "ready" }))
  })

  it("reports an expired provider cache as an error", () => {
    const controller = controllerWith(
      vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
    )
    controller.accept({
      ...request(4, "anthropic"),
      initialPresentation: expiredProviderData(),
    })

    controller.confirmPresented(4)

    expect(analytics.expose).toHaveBeenCalledWith(expect.objectContaining({ state: "error" }))
  })

  it("ends the preview exposure when the shell conceals it", () => {
    const controller = controllerWith(
      vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
    )
    controller.accept(request(1, "first"))
    analytics.conceal.mockClear()

    controller.accept(request(2, null))

    expect(analytics.conceal).toHaveBeenCalledOnce()
  })

  it("marks a cold request as loading without a presented payload", () => {
    const controller = controllerWith(
      vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
    )

    controller.accept(request(1, "first"))

    expect(controller.getSnapshot()).toMatchObject({
      requested: { generation: 1, target: providerTarget("first") },
      presented: null,
      candidate: null,
      coldLoading: true,
      failed: null,
    })
  })

  it("reports renderer readiness only after the request listener attaches", async () => {
    const listening = deferred<() => void>()
    const ready = vi.fn(async () => true)
    const controller = new PopoverPeekController({
      data: vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
      listen: vi.fn(() => listening.promise),
      ready,
      state: vi.fn(async () => ({
        generation: 0,
        target: null,
        rendererReady: false,
        visible: false,
        awaitingRetargetCommit: false,
        awaitingPresentation: false,
        awaitingConcealment: false,
      })),
    })
    Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
      configurable: true,
      value: 9,
    })
    controller.commitRenderer(document.createElement("div"))
    const unsubscribe = controller.subscribe(() => undefined)

    expect(ready).not.toHaveBeenCalled()
    listening.resolve(() => undefined)
    await listening.promise
    await Promise.resolve()

    expect(ready).toHaveBeenCalledWith(9)
    unsubscribe()
  })

  it("retries renderer readiness after a transient command failure", async () => {
    vi.useFakeTimers()
    const ready = vi
      .fn()
      .mockRejectedValueOnce(new Error("command unavailable"))
      .mockResolvedValueOnce(true)
    const controller = new PopoverPeekController({
      data: vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
      listen: vi.fn(async () => () => undefined),
      ready,
      state: vi.fn(async () => ({
        generation: 0,
        target: null,
        rendererReady: false,
        visible: false,
        awaitingRetargetCommit: false,
        awaitingPresentation: false,
        awaitingConcealment: false,
      })),
    })
    Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
      configurable: true,
      value: 9,
    })
    controller.commitRenderer(document.createElement("div"))
    const unsubscribe = controller.subscribe(() => undefined)

    try {
      await Promise.resolve()
      await vi.advanceTimersByTimeAsync(250)

      expect(ready).toHaveBeenCalledTimes(2)
    } finally {
      unsubscribe()
      vi.useRealTimers()
    }
  })

  it("replays readiness after resubscribing around an older pending attempt", async () => {
    const firstReady = deferred<boolean>()
    const ready = vi.fn().mockReturnValueOnce(firstReady.promise).mockResolvedValueOnce(true)
    const controller = new PopoverPeekController({
      data: vi.fn(() => new Promise<PopoverPeekData>(() => undefined)),
      listen: vi.fn(async () => () => undefined),
      ready,
      state: vi.fn(async () => ({
        generation: 0,
        target: null,
        rendererReady: false,
        visible: false,
        awaitingRetargetCommit: false,
        awaitingPresentation: false,
        awaitingConcealment: false,
      })),
    })
    Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
      configurable: true,
      value: 9,
    })
    controller.commitRenderer(document.createElement("div"))

    const unsubscribeFirst = controller.subscribe(() => undefined)
    await vi.waitFor(() => expect(ready).toHaveBeenCalledTimes(1))
    unsubscribeFirst()
    const unsubscribeSecond = controller.subscribe(() => undefined)
    await vi.waitFor(() => expect(ready).toHaveBeenCalledTimes(2))

    firstReady.reject(new Error("stale attempt failed"))
    await firstReady.promise.catch(() => undefined)
    await Promise.resolve()

    expect(ready).toHaveBeenCalledTimes(2)
    unsubscribeSecond()
  })

  it("uses an initial presentation immediately without starting another load", () => {
    const data = vi.fn(() => new Promise<PopoverPeekData>(() => undefined))
    const controller = controllerWith(data)
    const seeded = providerData("seeded")

    controller.accept({
      ...request(2, "second"),
      retargetCommitRequired: true,
      initialPresentation: seeded,
    })

    expect(controller.getSnapshot()).toMatchObject({
      requested: { generation: 2, target: providerTarget("second") },
      presented: { request: { generation: 2 }, data: seeded },
      candidate: null,
      coldLoading: false,
    })
    expect(data).not.toHaveBeenCalled()
  })

  it("retains A while B loads and promotes B only after presentation", async () => {
    const first = deferred<PopoverPeekData>()
    const second = deferred<PopoverPeekData>()
    const data = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const controller = controllerWith(data)

    controller.accept(request(1, "first"))
    first.resolve(providerData("first"))
    await first.promise
    await Promise.resolve()
    controller.promote(1)

    controller.accept(request(2, "second"))

    expect(controller.getSnapshot()).toMatchObject({
      requested: { generation: 2, target: providerTarget("second") },
      presented: { request: { generation: 1 }, data: { summary: { generatedAt: "first" } } },
      candidate: null,
      coldLoading: false,
    })

    second.resolve(providerData("second"))
    await second.promise
    await Promise.resolve()

    expect(controller.getSnapshot()).toMatchObject({
      presented: { request: { generation: 1 } },
      candidate: { request: { generation: 2 }, data: { summary: { generatedAt: "second" } } },
    })

    controller.promote(2)
    expect(controller.getSnapshot()).toMatchObject({
      presented: { request: { generation: 2 }, data: { summary: { generatedAt: "second" } } },
      candidate: null,
    })
  })

  it("serializes loads, replaces the pending target, and rejects stale B", async () => {
    const first = deferred<PopoverPeekData>()
    const third = deferred<PopoverPeekData>()
    const data = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(third.promise)
    const controller = controllerWith(data)

    controller.accept(request(1, "first"))
    controller.accept(request(2, "second"))
    controller.accept(request(3, "third"))

    expect(data).toHaveBeenCalledTimes(1)
    expect(data).toHaveBeenLastCalledWith(1)

    first.resolve(providerData("first"))
    await first.promise
    await Promise.resolve()

    expect(controller.getSnapshot().requested.generation).toBe(3)
    expect(controller.getSnapshot().candidate).toBeNull()
    expect(data).toHaveBeenCalledTimes(2)
    expect(data).toHaveBeenLastCalledWith(3)

    third.resolve(providerData("third"))
    await third.promise
    await Promise.resolve()

    expect(controller.getSnapshot().candidate).toMatchObject({
      request: { generation: 3 },
      data: { summary: { generatedAt: "third" } },
    })
  })

  it("ignores a same-target lifecycle echo without loading or publishing", () => {
    const data = vi.fn(() => new Promise<PopoverPeekData>(() => undefined))
    const controller = controllerWith(data)
    const listener = vi.fn()
    const unsubscribe = controller.subscribe(listener)

    controller.accept(request(4, "same"))
    listener.mockClear()
    controller.accept(request(4, "same"))

    expect(listener).not.toHaveBeenCalled()
    expect(data).toHaveBeenCalledOnce()
    unsubscribe()
  })

  it("publishes only a current failure as an unavailable candidate", async () => {
    const first = deferred<PopoverPeekData>()
    const second = deferred<PopoverPeekData>()
    const data = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    const controller = controllerWith(data)

    controller.accept(request(1, "first"))
    controller.accept(request(2, "second"))
    first.reject(new Error("stale"))
    await first.promise.catch(() => undefined)
    await Promise.resolve()

    expect(controller.getSnapshot().failed).toBeNull()

    second.reject(new Error("current"))
    await second.promise.catch(() => undefined)
    await Promise.resolve()

    expect(controller.getSnapshot().failed).toMatchObject({
      generation: 2,
      target: providerTarget("second"),
    })
    expect(controller.getSnapshot().candidate).toBeNull()
  })

  it("clears all presentation state synchronously on conceal", async () => {
    const loaded = deferred<PopoverPeekData>()
    const controller = controllerWith(vi.fn(() => loaded.promise))

    controller.accept(request(1, "first"))
    loaded.resolve(providerData("first"))
    await loaded.promise
    await Promise.resolve()
    controller.promote(1)

    controller.accept(request(2, null))

    expect(controller.getSnapshot()).toEqual({
      requested: {
        generation: 2,
        target: null,
        retargetCommitRequired: false,
        initialPresentation: null,
      },
      presented: null,
      candidate: null,
      coldLoading: false,
      failed: null,
    })
  })
})

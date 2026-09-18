import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../lib/ipc"
import type { LiveUsageSummaryPayload } from "../lib/ipc"
import type * as HudIpc from "../lib/hudIpc"
import type { HudDetailState } from "../lib/hudIpc"
import { OverlayWindow } from "./OverlayWindow"

const REFRESH_TEST_MS = 60_000

const getLiveUsage = vi.hoisted(() => vi.fn())
const getLiveSessions = vi.hoisted(() => vi.fn())
const isOverlayWorkActive = vi.hoisted(() => vi.fn())
const getHudTokenMap = vi.hoisted(() => vi.fn())
const refreshLiveUsage = vi.hoisted(() => vi.fn())
const showHudDetail = vi.hoisted(() => vi.fn(async (_state: HudDetailState) => {}))
const hideHudDetail = vi.hoisted(() => vi.fn(async () => {}))
const resizeOverlayWindow = vi.hoisted(() => vi.fn(async () => {}))
const livePush = vi.hoisted(() => ({
  emit: null as ((usage: unknown) => void) | null,
}))
const onLiveUsageChanged = vi.hoisted(() =>
  vi.fn(async (handler: (usage: unknown) => void) => {
    livePush.emit = handler
    return () => {
      livePush.emit = null
    }
  }),
)
vi.mock("../lib/ipc", async () => {
  const actual = await vi.importActual<typeof Ipc>("../lib/ipc")
  return {
    ...actual,
    getLiveUsage,
    getLiveSessions,
    isOverlayWorkActive,
    refreshLiveUsage,
    hideHudDetail,
    resizeOverlayWindow,
    onLiveUsageChanged,
  }
})
vi.mock("../lib/hudIpc", async () => {
  const actual = await vi.importActual<typeof HudIpc>("../lib/hudIpc")
  return { ...actual, getHudTokenMap, showHudDetail }
})

const invoke = vi.hoisted(() => vi.fn(async (..._args: unknown[]) => {}))
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

const takeHudAnalyticsOrigin = vi.hoisted(() => vi.fn())
vi.mock("../lib/overlayWindow", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  return { ...actual, takeHudAnalyticsOrigin }
})

const analytics = vi.hoisted(() => ({
  conceal: vi.fn(),
  expose: vi.fn(),
  nextGeneration: 0,
  observe: vi.fn(),
  observeLiveUsage: vi.fn(),
  suspend: vi.fn(),
}))
vi.mock("../lib/surfaceExposure", () => ({
  SurfaceExposureTracker: class {
    expose(options: unknown) {
      analytics.nextGeneration += 1
      analytics.expose(options, analytics.nextGeneration)
      return analytics.nextGeneration
    }
    observe(state: unknown, generation: unknown) {
      analytics.observe(state, generation)
    }
    observeLiveUsage(summary: unknown, provider: unknown, generation: unknown) {
      analytics.observeLiveUsage(summary, provider, generation)
    }
    conceal(surface: unknown, generation: unknown) {
      analytics.conceal(surface, generation)
    }
    suspend() {
      analytics.suspend()
    }
  },
}))

const nativeEvents = vi.hoisted(
  () => new Map<string, Set<(event: { payload: unknown }) => void>>(),
)
const listenNative = vi.hoisted(() => vi.fn())
const hover = vi.hoisted(() => ({
  emit: (next: boolean) => {
    for (const handler of nativeEvents.get("overlay_hover") ?? []) {
      handler({ payload: next })
    }
  },
}))
vi.mock("@tauri-apps/api/event", () => ({ listen: listenNative }))

function emitNative(event: string, payload: unknown): void {
  for (const handler of nativeEvents.get(event) ?? []) handler({ payload })
}

const lifecycle = { seq: 0 }

/**
 * Push one sequenced lifecycle envelope at the mocked native listener, with
 * the batch counts the registry stamps on its last lifecycle event.
 */
function emitLifecycle(
  event: Record<string, unknown>,
  aggregate: { working: number; total: number; anonymous: number },
): void {
  lifecycle.seq += 1
  emitNative("session:lifecycle", {
    seq: lifecycle.seq,
    aggregate: {
      ...aggregate,
      sweep: sweep(aggregate.working, aggregate.anonymous, String(event.agent)),
    },
    ...event,
  })
}

const setPosition = vi.hoisted(() => vi.fn(async () => {}))
const outerPosition = vi.hoisted(() => vi.fn(async () => ({ x: 600, y: 40 })))
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ outerPosition, setPosition }),
  currentMonitor: async () => ({
    scaleFactor: 1,
    position: { x: 0, y: 0 },
    size: { width: 1512, height: 982 },
  }),
}))
vi.mock("@tauri-apps/api/dpi", () => ({
  LogicalPosition: class {
    x: number
    y: number
    constructor(x: number, y: number) {
      this.x = x
      this.y = y
    }
  },
}))

const stored = new Map<string, string>()
const storage = {
  getItem: (key: string) => stored.get(key) ?? null,
  setItem: (key: string, value: string) => stored.set(key, value),
  removeItem: (key: string) => stored.delete(key),
  clear: () => stored.clear(),
  key: (index: number) => [...stored.keys()][index] ?? null,
  get length() {
    return stored.size
  },
}

function summary(): LiveUsageSummaryPayload {
  // One instant for the whole snapshot. Sampling the clock twice lets a
  // millisecond fall between the reset time and the generated time, which
  // moves the notch off the round percentage the tests state.
  const at = Date.now()
  return {
    providers: [
      {
        provider: "anthropic",
        accountKey: null,
        displayName: "Anthropic",
        support: "live",
        freshness: "fresh",
        sourceLabel: "cached usage",
        observedAt: new Date(at).toISOString(),
        windows: [
          {
            id: "five-hour",
            role: "primaryShort",
            kind: "rolling",
            scopeModel: null,
            usedPercent: 81,
            startsAt: null,
            resetsAt: new Date(at + 2 * 3_600_000).toISOString(),
            hasNonzeroUsageInCurrentPeriod: true,
            forecast: {
              unavailableReason: "sparseHistory",
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
    errors: [],
    meters: [],
    generatedAt: new Date(at).toISOString(),
  }
}

/** A summary whose one limit is used up, resetting in `resetInMs`. */
function blocked(resetInMs: number): LiveUsageSummaryPayload {
  const payload = summary()
  const window = payload.providers[0]!.windows[0]!
  window.usedPercent = 100
  window.resetsAt = new Date(Date.now() + resetInMs).toISOString()
  return payload
}

function withSecondBar(): LiveUsageSummaryPayload {
  const payload = summary()
  payload.providers[0]!.windows.push({
    ...payload.providers[0]!.windows[0]!,
    id: "weekly",
    role: "primaryLong",
  })
  return payload
}

function frame(container: HTMLElement): HTMLElement {
  return container.firstElementChild as HTMLElement
}

function panel(container: HTMLElement): HTMLElement {
  return frame(container).firstElementChild as HTMLElement
}

// The close control is commented out for now. Its tests below are skipped
// with it, so they come back with the button.
function closeButton(): HTMLElement {
  return screen.getByRole("button", { name: "Close overlay" })
}

function panelRect(element: HTMLElement): DOMRect {
  const barCount = Math.max(
    1,
    element.querySelectorAll(".pointer-events-none .rounded-full").length / 20,
  )
  const height = 12 + barCount * 16
  return {
    x: 0,
    y: 0,
    top: 0,
    right: 176,
    bottom: height,
    left: 0,
    width: 176,
    height,
    toJSON: () => ({}),
  }
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

const SESSION_REF = { environmentKey: "native", agent: "claude-code", sessionId: "session-1" }

function liveSession(
  sessionId = "session-1",
  agent = "claude-code",
  model: string | null = "claude-opus-4-6",
  providerRoute: string | null = "anthropic",
) {
  return {
    session: { ...SESSION_REF, agent, sessionId },
    providerRoute,
    agent,
    lastActivityAt: Math.floor(Date.now() / 1000),
    model,
  }
}

function sweep(
  working: number,
  anonymous = 0,
  agent = "claude-code",
  model: string | null = "claude-opus-4-6",
  providerRoute: string | null = "anthropic",
) {
  return working + anonymous === 0
    ? []
    : [
        {
          agent,
          working,
          anonymous,
          modelPendingWorking: model ? 0 : working,
          modelFailedWorking: 0,
          modelNoneWorking: 0,
          models: model
            ? [
                {
                  model,
                  working,
                  providerRoute,
                  recordedProvider: providerRoute,
                  modelVendor: null,
                },
              ]
            : [],
        },
      ]
}

function liveSnapshot(row = liveSession()) {
  return {
    seq: 0,
    working: 1,
    total: 1,
    anonymous: [],
    sessions: [{ ...row, quiet: false }],
    sweep: sweep(1, 0, row.agent, row.model, row.providerRoute),
  }
}

describe("OverlayWindow", () => {
  let rectSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    vi.stubGlobal("localStorage", storage)
    getLiveUsage.mockReset()
    getLiveUsage.mockResolvedValue(summary())
    getLiveSessions.mockReset()
    getLiveSessions.mockResolvedValue({
      seq: 0,
      working: 0,
      total: 0,
      sessions: [],
      anonymous: [],
    })
    lifecycle.seq = 0
    isOverlayWorkActive.mockReset()
    isOverlayWorkActive.mockResolvedValue(true)
    getHudTokenMap.mockReset()
    getHudTokenMap.mockResolvedValue(null)
    refreshLiveUsage.mockReset()
    refreshLiveUsage.mockResolvedValue(null)
    showHudDetail.mockClear()
    hideHudDetail.mockClear()
    resizeOverlayWindow.mockClear()
    invoke.mockClear()
    takeHudAnalyticsOrigin.mockReset()
    takeHudAnalyticsOrigin.mockResolvedValue("automatic")
    analytics.nextGeneration = 0
    analytics.conceal.mockClear()
    analytics.expose.mockClear()
    analytics.observe.mockClear()
    analytics.observeLiveUsage.mockClear()
    livePush.emit = null
    nativeEvents.clear()
    listenNative.mockReset()
    listenNative.mockImplementation(
      async (event: string, handler: (event: { payload: unknown }) => void) => {
        const listeners = nativeEvents.get(event) ?? new Set()
        listeners.add(handler)
        nativeEvents.set(event, listeners)
        return () => {
          listeners.delete(handler)
        }
      },
    )
    onLiveUsageChanged.mockClear()
    analytics.suspend.mockClear()
    outerPosition.mockReset()
    outerPosition.mockResolvedValue({ x: 600, y: 40 })
    stored.clear()
    rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        return panelRect(this)
      })
  })

  afterEach(() => {
    rectSpy.mockRestore()
  })

  it("marks only its own document body as transparent", () => {
    const { unmount } = render(<OverlayWindow />)
    expect(document.body.dataset.transparentWindow).toBe("true")
    unmount()
    expect(document.body.dataset.transparentWindow).toBeUndefined()
  })

  it("does no polling or subscription work while native policy keeps it hidden", async () => {
    isOverlayWorkActive.mockResolvedValue(false)
    vi.useFakeTimers()
    try {
      const { unmount } = render(<OverlayWindow />)
      await advance(0)

      expect(getLiveUsage).not.toHaveBeenCalled()
      expect(getLiveSessions).not.toHaveBeenCalled()
      expect(nativeEvents.get("overlay_hover")?.size ?? 0).toBe(0)
      expect(nativeEvents.get("session:lifecycle")?.size ?? 0).toBe(0)
      await advance(5 * 60_000)
      expect(getLiveUsage).not.toHaveBeenCalled()
      unmount()
    } finally {
      vi.useRealTimers()
    }
  })

  it("parks on hide and starts one fresh work set on every reshow", async () => {
    isOverlayWorkActive.mockResolvedValue(false)
    vi.useFakeTimers()
    try {
      render(<OverlayWindow />)
      await advance(0)

      act(() => emitNative("overlay_work_changed", true))
      await advance(0)
      expect(getLiveUsage).toHaveBeenCalledTimes(1)
      expect(getLiveSessions).toHaveBeenCalledTimes(1)
      expect(nativeEvents.get("session:lifecycle")?.size ?? 0).toBe(1)

      act(() => emitNative("overlay_work_changed", false))
      expect(nativeEvents.get("overlay_hover")?.size ?? 0).toBe(0)
      expect(nativeEvents.get("session:lifecycle")?.size ?? 0).toBe(0)
      await advance(2 * REFRESH_TEST_MS)
      expect(getLiveUsage).toHaveBeenCalledTimes(1)

      act(() => emitNative("overlay_work_changed", true))
      await advance(0)
      expect(getLiveUsage).toHaveBeenCalledTimes(2)
      expect(getLiveSessions).toHaveBeenCalledTimes(2)

      act(() => emitNative("overlay_work_changed", false))
      act(() => emitNative("overlay_work_changed", true))
      await advance(0)
      expect(getLiveUsage).toHaveBeenCalledTimes(3)
      expect(nativeEvents.get("overlay_hover")?.size).toBe(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("reports layout readiness again when a reshow returns identical bars", async () => {
    vi.useFakeTimers()
    try {
      render(<OverlayWindow />)
      await advance(0)
      const initialResizeCount = resizeOverlayWindow.mock.calls.length
      expect(initialResizeCount).toBeGreaterThan(0)

      act(() => emitNative("overlay_work_changed", false))
      act(() => emitNative("overlay_work_changed", true))
      await advance(0)

      expect(resizeOverlayWindow.mock.calls.length).toBeGreaterThan(initialResizeCount)
      expect(getLiveUsage).toHaveBeenCalledTimes(2)
    } finally {
      vi.useRealTimers()
    }
  })

  it("ignores a delayed initial load after native hide", async () => {
    let resolveUsage!: (usage: LiveUsageSummaryPayload) => void
    getLiveUsage.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveUsage = resolve
        }),
    )
    render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalledTimes(1))
    const resizeCount = resizeOverlayWindow.mock.calls.length

    act(() => emitNative("overlay_work_changed", false))
    await act(async () => resolveUsage(withSecondBar()))

    expect(resizeOverlayWindow).toHaveBeenCalledTimes(resizeCount)
    expect(document.querySelectorAll(".pointer-events-none .rounded-full")).toHaveLength(20)
  })

  it("does not let a stale active-state read restart hidden work", async () => {
    let resolveActive!: (active: boolean) => void
    isOverlayWorkActive.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveActive = resolve
        }),
    )
    render(<OverlayWindow />)
    await waitFor(() => expect(isOverlayWorkActive).toHaveBeenCalledTimes(1))

    act(() => emitNative("overlay_work_changed", false))
    await act(async () => resolveActive(true))

    expect(getLiveUsage).not.toHaveBeenCalled()
    expect(getLiveSessions).not.toHaveBeenCalled()
  })

  it("retries a transient native work-listener failure", async () => {
    listenNative.mockRejectedValueOnce(new Error("listener unavailable"))
    render(<OverlayWindow />)

    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    expect(listenNative.mock.calls[0]?.[0]).toBe("overlay_work_changed")
    expect(listenNative.mock.calls[1]?.[0]).toBe("overlay_work_changed")
    expect(nativeEvents.get("overlay_work_changed")?.size).toBe(1)
  })

  it("lights working activity from the registry snapshot and clears it on quiet", async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date("2026-09-08T00:00:00Z"))
    getLiveSessions.mockResolvedValue({
      seq: 1,
      working: 1,
      sweep: sweep(1),
      total: 1,
      sessions: [
        {
          session: { environmentKey: "native", agent: "claude-code", sessionId: "busy" },
          agent: "claude-code",
          lastActivityAt: Math.floor(Date.now() / 1000),
          quiet: false,
        },
      ],
      anonymous: [],
    })
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      expect(container.querySelector(".led-blink")).not.toBeNull()

      // The snapshot already carries sequence 1; the delta must be newer.
      lifecycle.seq = 1
      // The registry, not a local timer, ends the working state.
      act(() =>
        emitLifecycle(
          {
            kind: "quiet",
            session: { environmentKey: "native", agent: "claude-code", sessionId: "busy" },
            agent: "claude-code",
            at: Math.floor(Date.now() / 1000) + 30,
          },
          { working: 0, total: 1, anonymous: 0 },
        ),
      )
      expect(container.querySelector(".led-blink")).toBeNull()
      expect(getLiveSessions).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("lights working activity from the exact counts when the snapshot rows are truncated", async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date("2026-09-08T00:00:00Z"))
    // The bounded rows hold nothing that works, but the registry's exact
    // counts say two sessions do. The HUD trusts the counts, not the rows.
    getLiveSessions.mockResolvedValue({
      seq: 1,
      working: 2,
      sweep: sweep(2),
      total: 300,
      sessions: [
        {
          session: { environmentKey: "native", agent: "claude-code", sessionId: "quiet-row" },
          agent: "claude-code",
          lastActivityAt: Math.floor(Date.now() / 1000) - 60,
          quiet: true,
        },
      ],
      anonymous: [],
    })
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      expect(container.querySelector(".led-blink")).not.toBeNull()

      lifecycle.seq = 1
      // A stamped delta about a session the rows never named still moves
      // the HUD, because the counts are what it reads.
      act(() =>
        emitLifecycle(
          {
            kind: "quiet",
            session: { environmentKey: "native", agent: "claude-code", sessionId: "unlisted" },
            agent: "claude-code",
            at: Math.floor(Date.now() / 1000),
          },
          { working: 1, total: 300, anonymous: 0 },
        ),
      )
      expect(container.querySelector(".led-blink")).not.toBeNull()
      act(() =>
        emitLifecycle(
          {
            kind: "quiet",
            session: {
              environmentKey: "native",
              agent: "claude-code",
              sessionId: "unlisted-2",
            },
            agent: "claude-code",
            at: Math.floor(Date.now() / 1000),
          },
          { working: 0, total: 300, anonymous: 0 },
        ),
      )
      expect(container.querySelector(".led-blink")).toBeNull()
      expect(getLiveSessions).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("clears anonymous agent activity only when the registry says so", async () => {
    const empty = summary()
    empty.providers = []
    getLiveUsage.mockResolvedValue(empty)
    vi.useFakeTimers()
    vi.setSystemTime(new Date("2026-09-08T00:00:00Z"))
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      expect(nativeEvents.get("session:lifecycle")?.size ?? 0).toBe(1)

      const at = Math.floor(Date.now() / 1000)
      act(() =>
        emitLifecycle(
          {
            kind: "activity",
            session: null,
            agent: "codex",
            at,
            resumed: false,
          },
          { working: 0, total: 0, anonymous: 1 },
        ),
      )
      expect(container.querySelector(".led-blink")).not.toBeNull()

      // No renderer timer ends anonymous activity: past the registry's
      // window the bar still blinks until the canonical clear arrives.
      await advance(30_000)
      expect(container.querySelector(".led-blink")).not.toBeNull()

      act(() =>
        emitLifecycle(
          {
            kind: "anonymous_cleared",
            agent: "codex",
            at: at + 30,
            cause: "expired",
          },
          { working: 0, total: 0, anonymous: 0 },
        ),
      )
      expect(container.querySelector(".led-blink")).toBeNull()
      expect(getLiveSessions).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("uses pushed lifecycle activity and cleans its subscriptions on hide", async () => {
    const { container, unmount } = render(<OverlayWindow />)
    await waitFor(() => expect(nativeEvents.get("session:lifecycle")?.size).toBe(1))

    act(() =>
      emitLifecycle(
        {
          kind: "activity",
          session: { environmentKey: "native", agent: "claude-code", sessionId: "live" },
          agent: "claude-code",
          at: Math.floor(Date.now() / 1000),
          resumed: false,
        },
        { working: 1, total: 1, anonymous: 0 },
      ),
    )
    await waitFor(() => expect(container.querySelector(".led-blink")).not.toBeNull())

    act(() => emitNative("overlay_work_changed", false))
    expect(nativeEvents.get("session:lifecycle")?.size ?? 0).toBe(0)
    expect(nativeEvents.get("scan:finished")?.size ?? 0).toBe(0)
    expect(nativeEvents.get("overlay_work_changed")?.size).toBe(1)

    unmount()
    expect(nativeEvents.get("overlay_work_changed")?.size ?? 0).toBe(0)
  })

  it("keeps a low-usage bar dark without a live session", async () => {
    const low = summary()
    low.providers[0]!.windows[0]!.usedPercent = 1
    getLiveUsage.mockResolvedValue(low)
    const { container } = render(<OverlayWindow />)

    await waitFor(() => expect(getLiveSessions).toHaveBeenCalled())
    expect(container.querySelector(".led-blink")).toBeNull()
  })

  it("rests with bars only and a hidden close control", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    expect(screen.queryByText("81%")).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Close overlay" })).toBeNull()
    expect(document.querySelectorAll(".pointer-events-none .rounded-full")).toHaveLength(20)
  })

  it("records an automatic HUD exposure and its visible data outcome", async () => {
    render(<OverlayWindow />)

    await waitFor(() =>
      expect(analytics.expose).toHaveBeenCalledWith(
        expect.objectContaining({ surface: "hud", origin: "automatic" }),
        1,
      ),
    )
    await waitFor(() => expect(analytics.observe).toHaveBeenCalledWith("ready", 1))
    expect(analytics.observeLiveUsage).not.toHaveBeenCalled()
  })

  it("starts the HUD exposure timeout while its first usage read is pending", async () => {
    getLiveUsage.mockImplementation(() => new Promise(() => undefined))
    render(<OverlayWindow />)

    await waitFor(() =>
      expect(analytics.expose).toHaveBeenCalledWith(
        expect.objectContaining({ surface: "hud", origin: "automatic" }),
        1,
      ),
    )
    expect(analytics.expose.mock.calls[0]?.[0]).not.toHaveProperty("state")
  })

  it("reports an empty HUD when no usage bars can be shown", async () => {
    getLiveUsage.mockResolvedValue({
      providers: [],
      errors: [],
      meters: [],
      generatedAt: new Date().toISOString(),
    })
    render(<OverlayWindow />)

    await waitFor(() => expect(analytics.observe).toHaveBeenCalledWith("empty", 1))
  })

  it("reports an error when the first visible HUD read fails", async () => {
    getLiveUsage.mockRejectedValue(new Error("usage unavailable"))
    render(<OverlayWindow />)

    await waitFor(() =>
      expect(analytics.expose).toHaveBeenCalledWith(
        expect.objectContaining({ surface: "hud", state: "error" }),
        1,
      ),
    )
  })

  it("records provider states only for a user-opened HUD", async () => {
    takeHudAnalyticsOrigin.mockResolvedValue("user")
    getLiveUsage.mockResolvedValue({
      ...summary(),
      meters: [{ provider: "anthropic", displayName: "Claude", shown: true }],
    })
    render(<OverlayWindow />)

    await waitFor(() => expect(analytics.observeLiveUsage).toHaveBeenCalled())

    expect(analytics.expose).toHaveBeenCalledWith(
      expect.objectContaining({ surface: "hud", origin: "user" }),
      1,
    )
  })

  it("ends the HUD exposure when the native window hides", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(nativeEvents.get("overlay_visibility_changed")?.size).toBe(1))
    await waitFor(() => expect(analytics.expose).toHaveBeenCalled())

    act(() => emitNative("overlay_visibility_changed", false))

    expect(analytics.conceal).toHaveBeenCalledWith("hud", 1)
  })

  it("ends exposure before parking and captures the next native reveal", async () => {
    takeHudAnalyticsOrigin.mockResolvedValueOnce("user").mockResolvedValue(null)
    render(<OverlayWindow />)
    await waitFor(() => expect(analytics.expose).toHaveBeenCalledTimes(1))

    act(() => emitNative("overlay_work_changed", false))
    act(() => emitNative("overlay_visibility_changed", false))
    expect(analytics.conceal).toHaveBeenCalledWith("hud", 1)
    expect(nativeEvents.get("overlay_visibility_changed")?.size).toBe(0)
    expect(nativeEvents.get("hud-detail:shown")?.size).toBe(0)

    analytics.expose.mockClear()
    act(() => emitNative("overlay_work_changed", true))
    await waitFor(() => expect(takeHudAnalyticsOrigin).toHaveBeenCalledTimes(2))
    expect(analytics.expose).not.toHaveBeenCalled()

    takeHudAnalyticsOrigin.mockResolvedValueOnce("user")
    act(() => emitNative("overlay_visibility_changed", true))
    await waitFor(() => expect(analytics.expose).toHaveBeenCalledTimes(1))
    expect(analytics.expose).toHaveBeenCalledWith(
      expect.objectContaining({ surface: "hud", origin: "user" }),
      2,
    )
    expect(nativeEvents.get("overlay_visibility_changed")?.size).toBe(1)
    expect(nativeEvents.get("hud-detail:shown")?.size).toBe(1)
  })

  it("ignores a pending exposure response after native visibility ends", async () => {
    let resolveOrigin!: (origin: "user") => void
    takeHudAnalyticsOrigin.mockImplementation(
      () => new Promise<"user">((resolve) => (resolveOrigin = resolve)),
    )
    render(<OverlayWindow />)
    await waitFor(() => expect(takeHudAnalyticsOrigin).toHaveBeenCalledTimes(1))

    act(() => emitNative("overlay_visibility_changed", false))
    await act(async () => resolveOrigin("user"))

    expect(analytics.expose).not.toHaveBeenCalled()
    expect(analytics.observeLiveUsage).not.toHaveBeenCalled()
  })

  it("drops a meter the moment settings turns it off, not on the next poll", async () => {
    // The HUD polls once a minute. A switch the reader just moved cannot wait
    // that long, so the shell pushes the new summary and the HUD takes it.
    getLiveUsage.mockResolvedValue(withSecondBar())
    render(<OverlayWindow />)
    await waitFor(() =>
      expect(document.querySelectorAll(".pointer-events-none .rounded-full")).toHaveLength(40),
    )

    await act(async () => {
      livePush.emit!({
        providers: [],
        errors: [],
        meters: [{ provider: "anthropic", displayName: "Claude", shown: false }],
        generatedAt: new Date().toISOString(),
      })
    })

    // The empty track: one bar's worth of segments, none of them lit.
    expect(document.querySelectorAll(".pointer-events-none .rounded-full")).toHaveLength(20)
    // The window shrinks with it, rather than keeping the old bars' height.
    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(28, false, true))
  })

  it("does not publish or resize for an equal pushed usage snapshot", async () => {
    const payload = summary()
    getLiveUsage.mockResolvedValue(payload)
    render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    const resizeCount = resizeOverlayWindow.mock.calls.length

    await act(async () => livePush.emit!(payload))

    expect(resizeOverlayWindow).toHaveBeenCalledTimes(resizeCount)
  })

  function mapSession(sessionId: string, tokensPerMin: number) {
    return {
      agent: "claude-code",
      sessionId,
      title: null,
      lastTurnEpoch: 990,
      tokensPerMin,
      modes: {
        looking: tokensPerMin * 5,
        running: 0,
        changing: 0,
        delegating: 0,
        thinking: 0,
        talking: 0,
        other: 0,
      },
      subagents: [],
    }
  }

  it("draws the token map above the bars when two sessions are live", async () => {
    getHudTokenMap.mockResolvedValue({
      nowEpoch: 1_000,
      windowSecs: 300,
      spend: null,
      sessions: [mapSession("s1", 1_000), mapSession("s2", 250)],
    })
    const { container } = render(<OverlayWindow />)
    await waitFor(() =>
      expect(container.querySelectorAll("svg[data-dot-value] circle")).toHaveLength(5),
    )
    const svg = container.querySelector("svg[data-dot-value]")!
    const bars = container.querySelector(".hud-leds")
    expect(svg.compareDocumentPosition(bars!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it("pins the blink period to a spend rate from the HUD Dev menu", async () => {
    getLiveSessions.mockResolvedValue(liveSnapshot())
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(container.querySelector(".led-blink")).not.toBeNull())
    const led = container.querySelector<HTMLElement>(".led-blink")!
    expect(led.style.getPropertyValue("--led-period")).toBe("3000ms")
    await act(async () => {
      emitNative("hud_dev", { kind: "spend", usdPerMinute: 2 })
    })
    expect(led.style.getPropertyValue("--led-period")).toBe("300ms")
  })

  it("celebrates a reset on demand from the HUD Dev menu", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(nativeEvents.get("hud_dev")?.size ?? 0).toBe(1))
    await act(async () => {
      emitNative("hud_dev", { kind: "celebrate" })
    })
    expect(screen.getByTestId("hud-celebration").textContent).toContain("usage reset")
  })

  it("leaves the map off for one session and lets the live LED carry it", async () => {
    getHudTokenMap.mockResolvedValue({
      nowEpoch: 1_000,
      windowSecs: 300,
      spend: null,
      sessions: [mapSession("s1", 1_000)],
    })
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getHudTokenMap).toHaveBeenCalled())
    await act(async () => {})
    expect(container.querySelector("svg[data-dot-value]")).toBeNull()
  })

  it("retargets the open detail card to the agent box under the pointer", async () => {
    getHudTokenMap.mockResolvedValue({
      nowEpoch: 1_000,
      windowSecs: 300,
      spend: null,
      sessions: [mapSession("s1", 1_000), mapSession("s2", 250)],
    })
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      expect(container.querySelector("svg[data-dot-value]")).not.toBeNull()
      fireEvent.mouseEnter(frame(container))
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledTimes(1)
      expect(showHudDetail.mock.calls[0]?.[0]).toMatchObject({
        reason: "show",
        target: "usage",
      })

      const box = container.querySelector('g[data-blob="claude-code:s2"]')!
      fireEvent.mouseEnter(box)
      expect(showHudDetail).toHaveBeenCalledTimes(2)
      expect(showHudDetail.mock.calls[1]?.[0]).toMatchObject({
        reason: "show",
        target: "claude-code:s2",
      })
      expect(showHudDetail.mock.calls[1]?.[0].map?.sessions[1]).toMatchObject({
        key: "claude-code:s2",
        agent: "claude-code",
        tokensPerMin: 250,
      })

      fireEvent.mouseLeave(box)
      expect(showHudDetail).toHaveBeenCalledTimes(3)
      expect(showHudDetail.mock.calls[2]?.[0]).toMatchObject({ target: "usage" })
    } finally {
      vi.useRealTimers()
    }
  })

  it("tears a docked HUD off when a drag starts", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseDown(panel(container), { clientX: 10, clientY: 10 })
      expect(invoke).toHaveBeenCalledWith("tear_off_overlay")
    } finally {
      vi.useRealTimers()
    }
  })

  it("wakes the HUD after two polls of hot spend, then waits for a drop", async () => {
    const hot = { usdPerMinute: 3, windowSecs: 300, pricedShare: 1 }
    getHudTokenMap.mockResolvedValue({
      nowEpoch: 1_000,
      windowSecs: 300,
      spend: hot,
      sessions: [],
    })
    vi.useFakeTimers()
    try {
      render(<OverlayWindow />)
      await advance(0)
      const wakes = () => invoke.mock.calls.filter(([command]) => command === "wake_overlay")
      expect(wakes()).toHaveLength(0)
      await advance(5_000)
      expect(wakes()).toEqual([["wake_overlay", { reason: "burn" }]])
      await advance(5_000)
      expect(wakes()).toHaveLength(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("blinks the live LED at the spend rate in the live session's mode colour", async () => {
    getLiveSessions.mockResolvedValue(liveSnapshot())
    getHudTokenMap.mockResolvedValue({
      nowEpoch: 1_000,
      windowSecs: 300,
      spend: { usdPerMinute: 5, windowSecs: 300, pricedShare: 1 },
      sessions: [
        {
          agent: "claude-code",
          sessionId: "s1",
          title: null,
          lastTurnEpoch: 990,
          tokensPerMin: 1_000,
          modes: {
            looking: 0,
            running: 0,
            changing: 5_000,
            delegating: 0,
            thinking: 0,
            talking: 0,
            other: 0,
          },
          subagents: [],
        },
      ],
    })
    const { container } = render(<OverlayWindow />)
    await waitFor(() => {
      const led = container.querySelector<HTMLElement>(".led-blink")
      expect(led).not.toBeNull()
      expect(led!.style.getPropertyValue("--led-period")).toBe("300ms")
      expect(led!.style.getPropertyValue("--led-on")).toBe("var(--color-mode-changing)")
    })
    expect(showHudDetail).not.toHaveBeenCalled()
  })

  it("blinks at the quiet period with no spend and no forecast", async () => {
    getLiveSessions.mockResolvedValue(liveSnapshot())
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(container.querySelector(".led-blink")).not.toBeNull())
    const led = container.querySelector<HTMLElement>(".led-blink")!
    expect(led.style.getPropertyValue("--led-period")).toBe("3000ms")
  })

  it("spells the map out in the detail payload", async () => {
    vi.useFakeTimers()
    try {
      getHudTokenMap.mockResolvedValue({
        nowEpoch: 1_000,
        windowSecs: 300,
        spend: null,
        sessions: [
          {
            agent: "claude-code",
            sessionId: "s1",
            title: "HUD token map",
            lastTurnEpoch: 990,
            tokensPerMin: 1_000,
            modes: {
              looking: 5_000,
              running: 0,
              changing: 0,
              delegating: 0,
              thinking: 0,
              talking: 0,
              other: 0,
            },
            subagents: [],
          },
        ],
      })
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledWith(
        expect.objectContaining({
          map: {
            dotValue: 250,
            sessions: [
              expect.objectContaining({
                label: "HUD token map",
                tokensPerMin: 1_000,
                topMode: "looking",
              }),
            ],
          },
        }),
      )
    } finally {
      vi.useRealTimers()
    }
  })

  it("draws no map when the preference is off", async () => {
    stored.set("antiburn.showHudTokenMap", "0")
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    expect(getHudTokenMap).not.toHaveBeenCalled()
    expect(container.querySelector("svg[data-dot-value]")).toBeNull()
  })

  it("reveals at the measured collapsed height", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(28, false, false))
  })

  it("resizes when refreshed data changes the bar count", async () => {
    getLiveUsage.mockResolvedValue(withSecondBar())
    render(<OverlayWindow />)
    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(44, false, true))
  })

  it("paints the same translucent frame at rest and on hover", async () => {
    // The frame groups the bars into one object without hiding the desktop.
    // The pointer does not change it.
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    expect(panel(container).classList.contains("bg-hud-frame")).toBe(true)
    expect(panel(container).classList.contains("hud-frame")).toBe(true)
    expect(panel(container).style.backgroundColor).toBe("")
    fireEvent.mouseEnter(frame(container))
    expect(panel(container).style.backgroundColor).toBe("")
    fireEvent.mouseLeave(frame(container))
    expect(panel(container).style.backgroundColor).toBe("")
  })

  it("marks how far through the window the clock has travelled", async () => {
    // The fixture window resets in two hours and its id states a five-hour
    // period, so three of its five hours have gone.
    render(<OverlayWindow />)
    await waitFor(() => expect(screen.getByTestId("led-bar-notch")).toBeInTheDocument())
    const offset = Number.parseFloat(screen.getByTestId("led-bar-notch").style.left)
    expect(offset).toBeCloseTo(60, 3)
  })

  it("shows the detail window after the hover delay", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      await advance(399)
      expect(showHudDetail).not.toHaveBeenCalled()
      await advance(1)
      expect(showHudDetail).toHaveBeenCalledWith(
        expect.objectContaining({
          reason: "show",
          bars: [expect.objectContaining({ label: "5-hour limit", percent: 81 })],
        }),
      )
      expect(
        analytics.expose.mock.calls.some(([options]) => options.surface === "hud_detail"),
      ).toBe(false)
      act(() => emitNative("hud-detail:shown", undefined))
      expect(analytics.expose).toHaveBeenCalledWith(
        expect.objectContaining({ surface: "hud_detail", origin: "user", state: "ready" }),
        expect.any(Number),
      )
    } finally {
      vi.useRealTimers()
    }
  })

  it("does not record detail when a pending reveal is canceled", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      await advance(400)
      fireEvent.mouseLeave(frame(container))

      act(() => emitNative("hud-detail:shown", undefined))

      expect(
        analytics.expose.mock.calls.some(([options]) => options.surface === "hud_detail"),
      ).toBe(false)
    } finally {
      vi.useRealTimers()
    }
  })

  it("does not record detail when its show request fails", async () => {
    vi.useFakeTimers()
    showHudDetail.mockRejectedValueOnce(new Error("detail unavailable"))
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      await advance(400)

      expect(
        analytics.expose.mock.calls.some(([options]) => options.surface === "hud_detail"),
      ).toBe(false)
    } finally {
      vi.useRealTimers()
    }
  })

  it("hides the detail window at once on leave", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      await advance(400)
      fireEvent.mouseLeave(frame(container))
      expect(hideHudDetail).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("does not touch the detail window when the pointer leaves early", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      await advance(200)
      fireEvent.mouseLeave(frame(container))
      await advance(1000)
      expect(showHudDetail).not.toHaveBeenCalled()
      expect(hideHudDetail).not.toHaveBeenCalled()
    } finally {
      vi.useRealTimers()
    }
  })

  it("accepts native hover edges while the app is in the background", async () => {
    vi.useFakeTimers()
    try {
      render(<OverlayWindow />)
      await advance(0)
      act(() => hover.emit(true))
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledTimes(1)
      act(() => hover.emit(false))
      expect(hideHudDetail).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("draws the collapsed island as the notch row alone and keeps the detail shut", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      expect(container.querySelector("[data-island]")).toBeNull()

      act(() =>
        emitNative("hud-island:state", {
          island: "collapsed",
          wing: 30,
          fillet: 6,
          notch: 200,
          height: 32,
        }),
      )
      const island = container.querySelector("[data-island]")
      expect(island?.getAttribute("data-island")).toBe("collapsed")
      expect(island?.classList.contains("hud-island-fillets")).toBe(true)
      expect(screen.getByTestId("island-live-led")).toBeTruthy()
      // No priced spend in the summary, so the right wing shows the usage LED.
      expect(screen.getByTestId("island-usage-led")).toBeTruthy()
      expect(document.querySelectorAll(".pointer-events-none .rounded-full")).toHaveLength(0)

      act(() => hover.emit(true))
      await advance(400)
      expect(showHudDetail).not.toHaveBeenCalled()

      act(() =>
        emitNative("hud-island:state", {
          island: "expanded",
          wing: 30,
          fillet: 19,
          notch: 200,
          height: 32,
        }),
      )
      expect(
        container.querySelector("[data-island]")?.classList.contains("hud-island-open"),
      ).toBe(true)
      expect(document.querySelectorAll(".pointer-events-none .rounded-full")).toHaveLength(20)
      // The pointer is still on the island, so the expansion opens the detail.
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledTimes(1)

      act(() =>
        emitNative("hud-island:state", {
          island: "off",
          wing: 0,
          fillet: 0,
          notch: 0,
          height: 0,
        }),
      )
      expect(container.querySelector("[data-island]")).toBeNull()
      expect(container.querySelector(".hud-frame")).not.toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })

  it("keeps hover active inside the transparent frame margin", async () => {
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(nativeEvents.get("overlay_hover")?.size).toBe(1))
    act(() => hover.emit(true))
    fireEvent.mouseLeave(panel(container), { relatedTarget: frame(container) })
    // Hover intent survives the leave, so the detail window still opens.
    await waitFor(() => expect(showHudDetail).toHaveBeenCalledTimes(1))
    expect(hideHudDetail).not.toHaveBeenCalled()
  })

  it("cancels the detail timer for a drag and restarts it on mouse up", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      await advance(200)
      fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
      await advance(1000)
      expect(showHudDetail).not.toHaveBeenCalled()
      fireEvent.mouseUp(window)
      await advance(399)
      expect(showHudDetail).not.toHaveBeenCalled()
      await advance(1)
      expect(showHudDetail).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("settles a drag when the pointer is released during native setup", async () => {
    let resolvePosition!: (position: { x: number; y: number }) => void
    outerPosition.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolvePosition = resolve
        }),
    )
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    fireEvent.mouseEnter(frame(container))
    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(1))
    fireEvent.mouseUp(window)
    await act(async () => resolvePosition({ x: 600, y: 40 }))

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(2))
    fireEvent.mouseUp(window)
  })

  it("ignores an old drag position after hide and reshow", async () => {
    let resolveOldPosition!: (position: { x: number; y: number }) => void
    outerPosition
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOldPosition = resolve
          }),
      )
      .mockResolvedValueOnce({ x: 600, y: 40 })
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(1))
    act(() => emitNative("overlay_work_changed", false))
    act(() => emitNative("overlay_work_changed", true))
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalledTimes(2))

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(2))
    await act(async () => resolveOldPosition({ x: 100, y: 10 }))
    fireEvent.mouseMove(window, { screenX: 710, screenY: 110 })

    await waitFor(() => expect(setPosition).toHaveBeenCalled())
    expect(setPosition).toHaveBeenLastCalledWith(expect.objectContaining({ x: 610, y: 50 }))
    fireEvent.mouseUp(window)
  })

  it("remembers where a settled drag left the HUD", async () => {
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(1))
    fireEvent.mouseUp(window)

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("record_hud_position"))
    const records = invoke.mock.calls.filter(([command]) => command === "record_hud_position")
    expect(records).toHaveLength(1)
  })

  it("does not remember a position when no drag was running", async () => {
    render(<OverlayWindow />)
    fireEvent.mouseUp(window)
    await act(async () => {})
    expect(invoke).not.toHaveBeenCalledWith("record_hud_position")
  })

  it("survives a rejected position record", async () => {
    invoke.mockRejectedValueOnce(new Error("no window"))
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(1))
    fireEvent.mouseUp(window)

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("record_hud_position"))
    fireEvent.mouseEnter(frame(container))
    expect(frame(container)).toBeInTheDocument()
  })

  it("cleans up drag listeners when native setup rejects", async () => {
    const removeListener = vi.spyOn(window, "removeEventListener")
    outerPosition.mockRejectedValueOnce(new Error("position unavailable"))
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })

    await waitFor(() =>
      expect(removeListener).toHaveBeenCalledWith("mousemove", expect.any(Function)),
    )
    expect(removeListener).toHaveBeenCalledWith("mouseup", expect.any(Function), true)
    expect(removeListener).toHaveBeenCalledWith("blur", expect.any(Function))
    removeListener.mockRestore()
  })

  it("hides a visible detail window when a drag starts", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      await advance(400)
      fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
      expect(hideHudDetail).toHaveBeenCalledTimes(1)
      fireEvent.mouseUp(window)
    } finally {
      vi.useRealTimers()
    }
  })

  it.skip("closes the visible detail window with the HUD", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      localStorage.setItem("antiburn.showFloatingHud", "1")
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledTimes(1)

      fireEvent.click(closeButton())

      expect(hideHudDetail).toHaveBeenCalledTimes(1)
      expect(closeButton()).toHaveClass("opacity-0")
      expect(localStorage.getItem("antiburn.showFloatingHud")).toBe("0")
      await act(async () => {})
      expect(invoke).toHaveBeenCalledWith("hide_overlay_window")
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("counts down to the reset while a limit blocks the tool", async () => {
    vi.useFakeTimers()
    try {
      getLiveUsage.mockResolvedValue(blocked(90 * 60_000))
      render(<OverlayWindow />)
      await advance(0)
      expect(screen.getByTestId("hud-countdown").textContent).toBe(
        "resets in 1h 30m · 5-hour limit",
      )
      await advance(65_000)
      expect(screen.getByTestId("hud-countdown").textContent).toBe(
        "resets in 1h 29m · 5-hour limit",
      )
      expect(refreshLiveUsage).not.toHaveBeenCalled()
    } finally {
      vi.useRealTimers()
    }
  })

  it("asks for a fresh read once the reset time passes, then celebrates", async () => {
    vi.useFakeTimers()
    try {
      getLiveUsage.mockResolvedValue(blocked(7_000))
      refreshLiveUsage.mockResolvedValue(summary())
      render(<OverlayWindow />)
      await advance(0)
      expect(screen.getByTestId("hud-countdown")).toBeTruthy()

      await advance(10_000)
      expect(refreshLiveUsage).toHaveBeenCalledTimes(1)
      expect(screen.queryByTestId("hud-countdown")).toBeNull()
      expect(screen.getByTestId("hud-celebration").textContent).toBe("anthropic usage reset")
      expect(invoke).toHaveBeenCalledWith("wake_overlay", { reason: "reset" })

      await advance(6_000)
      expect(screen.queryByTestId("hud-celebration")).toBeNull()
      // The next ticks do not ask again for a reset that was read.
      await advance(10_000)
      expect(refreshLiveUsage).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })
})

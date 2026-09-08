import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../lib/ipc"
import type { LiveUsageSummaryPayload } from "../lib/ipc"
import { providerBarColor } from "../lib/usageBars"
import { OverlayWindow } from "./OverlayWindow"

const REFRESH_TEST_MS = 60_000

const getLiveUsage = vi.hoisted(() => vi.fn())
const getLiveSessions = vi.hoisted(() => vi.fn())
const isOverlayWorkActive = vi.hoisted(() => vi.fn())
const showHudDetail = vi.hoisted(() => vi.fn(async () => {}))
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
    showHudDetail,
    hideHudDetail,
    resizeOverlayWindow,
    onLiveUsageChanged,
  }
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

function liveSession(sessionId = "session-1", agent = "claude-code") {
  return {
    session: { ...SESSION_REF, agent, sessionId },
    agent,
    lastActivityAt: Math.floor(Date.now() / 1000),
  }
}

function lifecycle(
  kind: "started" | "activity" | "idle",
  sessionId: string | null = "session-1",
): Record<string, unknown> {
  return {
    kind,
    session: sessionId == null ? null : { ...SESSION_REF, sessionId },
    agent: "claude-code",
    at: Math.floor(Date.now() / 1000),
  }
}

describe("OverlayWindow", () => {
  let rectSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    vi.stubGlobal("localStorage", storage)
    getLiveUsage.mockReset()
    getLiveUsage.mockResolvedValue(summary())
    getLiveSessions.mockReset()
    getLiveSessions.mockResolvedValue([])
    isOverlayWorkActive.mockReset()
    isOverlayWorkActive.mockResolvedValue(true)
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

      act(() => emitNative("overlay_work_changed", false))
      expect(nativeEvents.get("overlay_hover")?.size ?? 0).toBe(0)
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

  it("turns the blink off at the bus's idle event, with no timer of its own", async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date("2026-09-08T00:00:00Z"))
    getLiveSessions.mockResolvedValue([liveSession()])
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      expect(container.querySelector(".led-blink")).not.toBeNull()

      await advance(10 * 60_000)
      expect(container.querySelector(".led-blink")).not.toBeNull()

      act(() => emitNative("session:lifecycle", lifecycle("idle")))
      expect(container.querySelector(".led-blink")).toBeNull()
      expect(getLiveSessions).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("keeps blinking while another session is still live", async () => {
    getLiveSessions.mockResolvedValue([liveSession("session-1"), liveSession("session-2")])
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(container.querySelector(".led-blink")).not.toBeNull())

    act(() => emitNative("session:lifecycle", lifecycle("idle", "session-1")))
    expect(container.querySelector(".led-blink")).not.toBeNull()
    act(() => emitNative("session:lifecycle", lifecycle("idle", "session-2")))
    expect(container.querySelector(".led-blink")).toBeNull()
  })

  it("expires keyless agent activity after the active window", async () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date("2026-09-08T00:00:00Z"))
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      expect(container.querySelector(".led-blink")).toBeNull()

      act(() => emitNative("session:lifecycle", lifecycle("activity", null)))
      expect(container.querySelector(".led-blink")).not.toBeNull()

      await advance(180_001)
      expect(container.querySelector(".led-blink")).toBeNull()
      expect(getLiveSessions).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("re-reads the live set after a scan pass", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(nativeEvents.get("scan:finished")?.size).toBe(1))
    expect(getLiveSessions).toHaveBeenCalledTimes(1)

    act(() => emitNative("scan:finished", {}))
    expect(getLiveSessions).toHaveBeenCalledTimes(2)
  })

  it("uses pushed session activity and cleans its subscriptions on hide", async () => {
    const { container, unmount } = render(<OverlayWindow />)
    await waitFor(() => expect(nativeEvents.get("session:lifecycle")?.size).toBe(1))

    act(() => emitNative("session:lifecycle", lifecycle("activity")))
    expect(container.querySelector(".led-blink")).not.toBeNull()

    act(() => emitNative("overlay_work_changed", false))
    expect(nativeEvents.get("session:lifecycle")?.size ?? 0).toBe(0)
    expect(nativeEvents.get("scan:finished")?.size ?? 0).toBe(0)
    expect(nativeEvents.get("sessions:invalidated")?.size ?? 0).toBe(0)
    expect(nativeEvents.get("overlay_work_changed")?.size).toBe(1)

    unmount()
    expect(nativeEvents.get("overlay_work_changed")?.size ?? 0).toBe(0)
  })

  it("blinks the first segment when usage is too low to light one", async () => {
    const low = summary()
    low.providers[0]!.windows[0]!.usedPercent = 1
    getLiveUsage.mockResolvedValue(low)
    getLiveSessions.mockResolvedValue([liveSession()])
    const { container } = render(<OverlayWindow />)

    await waitFor(() => expect(container.querySelector(".led-blink")).not.toBeNull())
    const dots = container.querySelectorAll(".pointer-events-none .rounded-full")
    expect(dots).toHaveLength(20)
    expect(container.querySelectorAll(".led-blink")).toHaveLength(1)
    expect(dots[0]).toHaveClass("led-blink", "bg-led-off")
    expect((dots[0] as HTMLElement).style.getPropertyValue("--led-rest")).toBe(
      "var(--color-led-off)",
    )
  })

  it("blinks the first segment when there are no bars", async () => {
    const empty = summary()
    empty.providers = []
    getLiveUsage.mockResolvedValue(empty)
    getLiveSessions.mockResolvedValue([liveSession()])
    const { container } = render(<OverlayWindow />)

    await waitFor(() => expect(container.querySelector(".led-blink")).not.toBeNull())
    const dots = container.querySelectorAll(".pointer-events-none .rounded-full")
    expect(dots).toHaveLength(20)
    expect(dots[0]).toHaveClass("led-blink")
    expect(container.querySelectorAll(".led-blink")).toHaveLength(1)
  })

  it("blinks every bar of the live provider, one step apart from the top", async () => {
    getLiveUsage.mockResolvedValue(withSecondBar())
    getLiveSessions.mockResolvedValue([liveSession()])
    const { container } = render(<OverlayWindow />)

    await waitFor(() => expect(container.querySelectorAll(".led-blink")).toHaveLength(2))
    const [first, second] = Array.from(container.querySelectorAll<HTMLElement>(".led-blink"))
    expect(first?.dataset["ledStep"]).toBeUndefined()
    expect(second?.dataset["ledStep"]).toBe("1")
  })

  it("keeps the bars dark while the live session draws on another provider", async () => {
    getLiveSessions.mockResolvedValue([liveSession("session-1", "cursor")])
    const { container } = render(<OverlayWindow />)

    await waitFor(() => expect(getLiveSessions).toHaveBeenCalled())
    await waitFor(() =>
      expect(container.querySelectorAll(".pointer-events-none .rounded-full")).toHaveLength(20),
    )
    expect(container.querySelector(".led-blink")).toBeNull()
  })

  it("keeps a low-usage bar dark without a live session", async () => {
    const low = summary()
    low.providers[0]!.windows[0]!.usedPercent = 1
    getLiveUsage.mockResolvedValue(low)
    const { container } = render(<OverlayWindow />)

    await waitFor(() => expect(getLiveSessions).toHaveBeenCalled())
    expect(container.querySelector(".led-blink")).toBeNull()
  })

  it("blinks the last lit segment with its own colour as the rest state", async () => {
    getLiveSessions.mockResolvedValue([liveSession()])
    const { container } = render(<OverlayWindow />)

    await waitFor(() => expect(container.querySelector(".led-blink")).not.toBeNull())
    const dots = container.querySelectorAll(".pointer-events-none .rounded-full")
    // 81% of 20 segments rounds to 16 lit, so the blink sits on index 15.
    expect(dots[15]).toHaveClass("led-blink")
    expect(dots[15]).not.toHaveClass("bg-led-off")
    const lit = dots[15] as HTMLElement
    expect(lit.style.getPropertyValue("--led-rest")).toBe(providerBarColor("anthropic"))
    expect(lit.style.backgroundColor).not.toBe("")
  })

  it("rests with bars only and a hidden close control", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    expect(screen.queryByText("81%")).not.toBeInTheDocument()
    expect(closeButton()).toHaveClass("opacity-0", "pointer-events-none")
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

  it("reveals at the measured collapsed height", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(28, false, false))
  })

  it("resizes when refreshed data changes the bar count", async () => {
    getLiveUsage.mockResolvedValue(withSecondBar())
    render(<OverlayWindow />)
    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(44, false, true))
  })

  it("takes a surface on hover and drops it on leave", async () => {
    // At rest the bars sit on the desktop with nothing behind them. The
    // surface arrives with the pointer and groups them into one object.
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    expect(panel(container).style.backgroundColor).toBe("")
    fireEvent.mouseEnter(frame(container))
    expect(panel(container).style.backgroundColor).toBe("var(--color-bg-hud-hover)")
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

  it("shows the close control at once and the detail window after the delay", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(frame(container))
      expect(closeButton()).toHaveClass("opacity-100")
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
      expect(closeButton()).toHaveClass("opacity-0")
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

  it("keeps hover active inside the transparent frame margin", async () => {
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(nativeEvents.get("overlay_hover")?.size).toBe(1))
    act(() => hover.emit(true))
    await waitFor(() => expect(closeButton()).toHaveClass("opacity-100"))
    fireEvent.mouseLeave(panel(container), { relatedTarget: frame(container) })
    expect(closeButton()).toHaveClass("opacity-100")
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

  it("closes the visible detail window with the HUD", async () => {
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
})

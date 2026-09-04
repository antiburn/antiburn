import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../lib/ipc"
import type { LiveUsageSummaryPayload } from "../lib/ipc"
import { OverlayWindow } from "./OverlayWindow"

const getLiveUsage = vi.hoisted(() => vi.fn())
const getLatestSessionActivity = vi.hoisted(() => vi.fn())
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
    getLatestSessionActivity,
    showHudDetail,
    hideHudDetail,
    resizeOverlayWindow,
    onLiveUsageChanged,
  }
})

const invoke = vi.hoisted(() => vi.fn(async (..._args: unknown[]) => {}))
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

const hover = vi.hoisted(() => ({ emit: null as ((next: boolean) => void) | null }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_event: string, handler: (event: { payload: boolean }) => void) => {
    hover.emit = (next: boolean) => handler({ payload: next })
    return () => {}
  }),
}))

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
  return {
    providers: [
      {
        provider: "anthropic",
        accountKey: null,
        displayName: "Anthropic",
        support: "live",
        freshness: "fresh",
        sourceLabel: "cached usage",
        observedAt: new Date().toISOString(),
        windows: [
          {
            id: "five-hour",
            role: "primaryShort",
            kind: "rolling",
            scopeModel: null,
            usedPercent: 81,
            startsAt: null,
            resetsAt: new Date(Date.now() + 2 * 3_600_000).toISOString(),
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
      },
    ],
    errors: [],
    meters: [],
    generatedAt: new Date().toISOString(),
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

describe("OverlayWindow", () => {
  let rectSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    vi.stubGlobal("localStorage", storage)
    getLiveUsage.mockReset()
    getLiveUsage.mockResolvedValue(summary())
    getLatestSessionActivity.mockReset()
    getLatestSessionActivity.mockResolvedValue(null)
    showHudDetail.mockClear()
    hideHudDetail.mockClear()
    resizeOverlayWindow.mockClear()
    invoke.mockClear()
    livePush.emit = null
    onLiveUsageChanged.mockClear()
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

  it("rests with bars only and a hidden close control", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    expect(screen.queryByText("81%")).not.toBeInTheDocument()
    expect(closeButton()).toHaveClass("opacity-0", "pointer-events-none")
    expect(document.querySelectorAll(".pointer-events-none .rounded-full")).toHaveLength(20)
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
    expect(screen.getByTestId("led-bar-notch")).toHaveStyle({ left: "60%" })
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
      act(() => hover.emit!(true))
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledTimes(1)
      act(() => hover.emit!(false))
      expect(hideHudDetail).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("keeps hover active inside the transparent frame margin", async () => {
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(hover.emit).not.toBeNull())
    act(() => hover.emit!(true))
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
    fireEvent.mouseEnter(frame(container))
    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(1))
    fireEvent.mouseUp(window)
    await act(async () => resolvePosition({ x: 600, y: 40 }))

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(2))
    fireEvent.mouseUp(window)
  })

  it("remembers where a settled drag left the HUD", async () => {
    const { container } = render(<OverlayWindow />)
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

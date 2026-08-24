// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../lib/ipc"
import type { LiveUsageSummaryPayload } from "../lib/ipc"
import { OverlayWindow } from "./OverlayWindow"

const getLiveUsage = vi.hoisted(() => vi.fn())
const getLatestSessionActivity = vi.hoisted(() => vi.fn())
const openSettingsWindow = vi.hoisted(() => vi.fn(async () => {}))
const resizeOverlayWindow = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("../lib/ipc", async () => {
  const actual = await vi.importActual<typeof Ipc>("../lib/ipc")
  return {
    ...actual,
    getLiveUsage,
    getLatestSessionActivity,
    openSettingsWindow,
    resizeOverlayWindow,
  }
})

const invoke = vi.hoisted(() => vi.fn(async () => {}))
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
const nativeWindow = vi.hoisted(() => ({ y: 40 }))
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    outerPosition,
    setPosition,
  }),
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
      },
    ],
    errors: [],
    generatedAt: new Date().toISOString(),
  }
}

function panel(container: HTMLElement): HTMLElement {
  return container.firstElementChild!.firstElementChild as HTMLElement
}

function panelRect(element: HTMLElement): DOMRect {
  const barCount = Math.max(1, element.querySelectorAll(".rounded-full").length / 20)
  const height = element.classList.contains("bevel") ? 60 + barCount * 60 : 12 + barCount * 16
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

async function expand(container: HTMLElement) {
  fireEvent.mouseEnter(panel(container))
  await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
}

describe("OverlayWindow", () => {
  let rectSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    vi.stubGlobal("localStorage", storage)
    getLiveUsage.mockReset()
    getLiveUsage.mockResolvedValue(summary())
    getLatestSessionActivity.mockReset()
    getLatestSessionActivity.mockResolvedValue(null)
    openSettingsWindow.mockClear()
    resizeOverlayWindow.mockClear()
    invoke.mockClear()
    nativeWindow.y = 40
    outerPosition.mockReset()
    outerPosition.mockImplementation(async () => ({ x: 600, y: nativeWindow.y }))
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

  it("rests with bars and inaccessible chrome", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Close overlay" }).closest("div")).toHaveClass(
      "opacity-0",
      "pointer-events-none",
    )
    expect(document.querySelectorAll(".rounded-full")).toHaveLength(20)
  })

  it("waits 250ms before expansion and collapses immediately", async () => {
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    fireEvent.mouseEnter(panel(container))
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
    expect(screen.getByText("81%")).toBeInTheDocument()
    expect(screen.getByText(/^resets in /)).toBeInTheDocument()
    fireEvent.mouseLeave(panel(container))
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
  })

  it("cancels hover intent before the full dwell elapses", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await act(async () => {
        await Promise.resolve()
      })
      fireEvent.mouseEnter(panel(container))
      await act(async () => {
        await vi.advanceTimersByTimeAsync(249)
      })
      expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()

      fireEvent.mouseLeave(container.firstElementChild!)
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1)
      })
      expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    } finally {
      vi.useRealTimers()
    }
  })

  it("accepts native hover edges while the app is in the background", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(hover.emit).not.toBeNull())
    act(() => hover.emit!(true))
    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
    act(() => hover.emit!(false))
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
  })

  it("stays expanded when the pointer moves into a transparent frame margin", async () => {
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(hover.emit).not.toBeNull())
    act(() => hover.emit!(true))
    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())

    fireEvent.mouseLeave(panel(container), {
      relatedTarget: container.firstElementChild,
    })

    expect(screen.getByText("5-hour limit")).toBeInTheDocument()
    act(() => hover.emit!(false))
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
  })

  it("draws collapsed during the full manual drag", async () => {
    const { container } = render(<OverlayWindow />)
    await expand(container)
    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument())
    fireEvent.mouseUp(window)
    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
  })

  it("clears the stored preference when its close button hides the HUD", async () => {
    const { container } = render(<OverlayWindow />)
    localStorage.setItem("antiburn.showFloatingHud", "1")
    await expand(container)
    fireEvent.click(screen.getByRole("button", { name: "Close overlay" }))
    expect(localStorage.getItem("antiburn.showFloatingHud")).toBe("0")
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("hide_overlay_window"))
  })

  it("opens General settings from the wordmark", async () => {
    const { container } = render(<OverlayWindow />)
    await expand(container)
    fireEvent.click(screen.getByRole("button", { name: "Open antiburn settings" }))
    expect(openSettingsWindow).toHaveBeenCalledWith("general")
  })

  it("shows the exact empty copy only after expansion", async () => {
    getLiveUsage.mockResolvedValue({ providers: [], errors: [], generatedAt: "" })
    const { container } = render(<OverlayWindow />)
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    expect(screen.queryByText("No usage limits detected yet.")).not.toBeInTheDocument()
    fireEvent.mouseEnter(panel(container))
    await waitFor(() =>
      expect(screen.getByText("No usage limits detected yet.")).toBeInTheDocument(),
    )
  })

  it("reveals at the measured collapsed height", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(28, false, false))
  })

  it("keeps the top edge while it expands with room below", async () => {
    const { container } = render(<OverlayWindow />)
    await expand(container)
    expect(resizeOverlayWindow).toHaveBeenCalledWith(120, false, true)
  })

  it("keeps the bottom edge while it expands and collapses near the screen bottom", async () => {
    nativeWindow.y = 940
    const { container } = render(<OverlayWindow />)
    await expand(container)
    expect(resizeOverlayWindow).toHaveBeenCalledWith(120, true, true)

    fireEvent.mouseLeave(panel(container))
    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(28, true, true))
  })

  it("collapses without animation before a drag reads the window origin", async () => {
    nativeWindow.y = 940
    const { container } = render(<OverlayWindow />)
    await expand(container)

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })

    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(28, true, false))
  })

  it("keeps the upward anchor when a drag interrupts animated collapse", async () => {
    nativeWindow.y = 940
    const { container } = render(<OverlayWindow />)
    await expand(container)
    fireEvent.mouseLeave(container.firstElementChild!)
    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(28, true, true))

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })

    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(28, true, false))
  })

  it("finishes a drag when the pointer is released during native setup", async () => {
    let resolveResize!: () => void
    const { container } = render(<OverlayWindow />)
    await expand(container)
    resizeOverlayWindow.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          resolveResize = resolve
        }),
    )

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    fireEvent.mouseUp(window)

    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
    await act(async () => {
      resolveResize()
      await Promise.resolve()
    })
    expect(screen.getByText("5-hour limit")).toBeInTheDocument()
  })

  it("settles collapsed when native drag setup rejects", async () => {
    const removeListener = vi.spyOn(window, "removeEventListener")
    const { container } = render(<OverlayWindow />)
    await expand(container)
    outerPosition.mockRejectedValueOnce(new Error("position unavailable"))

    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() =>
      expect(removeListener).toHaveBeenCalledWith("mousemove", expect.any(Function)),
    )
    expect(removeListener).toHaveBeenCalledWith("mouseup", expect.any(Function), true)
    expect(removeListener).toHaveBeenCalledWith("blur", expect.any(Function))
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    fireEvent.mouseLeave(container.firstElementChild!)
    fireEvent.mouseEnter(container.firstElementChild!)

    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
    removeListener.mockRestore()
  })

  it("settles collapsed when release direction lookup rejects", async () => {
    const { container } = render(<OverlayWindow />)
    await expand(container)
    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(2))
    outerPosition.mockRejectedValueOnce(new Error("position unavailable"))

    fireEvent.mouseUp(window)
    await waitFor(() => expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument())
    fireEvent.mouseLeave(container.firstElementChild!)
    fireEvent.mouseEnter(container.firstElementChild!)

    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
  })

  it("retries drag release direction when hover leaves and re-enters", async () => {
    const { container } = render(<OverlayWindow />)
    await expand(container)
    fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
    await waitFor(() => expect(outerPosition).toHaveBeenCalledTimes(2))

    let resolvePosition!: (position: { x: number; y: number }) => void
    outerPosition.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolvePosition = resolve
        }),
    )
    fireEvent.mouseUp(window)
    fireEvent.mouseLeave(container.firstElementChild!)
    fireEvent.mouseEnter(container.firstElementChild!)
    await act(async () => {
      resolvePosition({ x: 600, y: 40 })
      await Promise.resolve()
    })

    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
    expect(outerPosition).toHaveBeenCalledTimes(4)
  })

  it("resizes when a data refresh changes the collapsed bar count", async () => {
    const payload = summary()
    payload.providers[0]!.windows.push({
      ...payload.providers[0]!.windows[0]!,
      id: "weekly",
      role: "primaryLong",
    })
    getLiveUsage.mockResolvedValue(payload)

    render(<OverlayWindow />)

    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(44, false, true))
  })

  it("rechecks the anchor when refreshed data makes an expanded HUD taller", async () => {
    let resolveUsage!: (payload: LiveUsageSummaryPayload) => void
    getLiveUsage.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveUsage = resolve
        }),
    )
    nativeWindow.y = 850
    const { container } = render(<OverlayWindow />)
    fireEvent.mouseEnter(panel(container))
    await waitFor(() =>
      expect(screen.getByText("No usage limits detected yet.")).toBeInTheDocument(),
    )

    const payload = summary()
    payload.providers[0]!.windows.push({
      ...payload.providers[0]!.windows[0]!,
      id: "weekly",
      role: "primaryLong",
    })
    await act(async () => {
      resolveUsage(payload)
      await Promise.resolve()
    })

    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(180, true, true))
  })

  it("uses the current anchor when refreshed-data positioning is unavailable", async () => {
    let resolveUsage!: (payload: LiveUsageSummaryPayload) => void
    getLiveUsage.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveUsage = resolve
        }),
    )
    const { container } = render(<OverlayWindow />)
    fireEvent.mouseEnter(panel(container))
    await waitFor(() =>
      expect(screen.getByText("No usage limits detected yet.")).toBeInTheDocument(),
    )
    outerPosition.mockRejectedValueOnce(new Error("position unavailable"))

    const payload = summary()
    payload.providers[0]!.windows.push({
      ...payload.providers[0]!.windows[0]!,
      id: "weekly",
      role: "primaryLong",
    })
    await act(async () => {
      resolveUsage(payload)
      await Promise.resolve()
    })

    await waitFor(() => expect(resizeOverlayWindow).toHaveBeenCalledWith(180, false, true))
  })

  it("disables native frame animation when reduced motion is requested", async () => {
    const originalMatchMedia = window.matchMedia
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({ matches: true })),
    })
    const { container } = render(<OverlayWindow />)

    await expand(container)

    expect(resizeOverlayWindow).toHaveBeenCalledWith(120, false, false)
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: originalMatchMedia,
    })
  })

  it("does not expand when the pointer leaves during direction lookup", async () => {
    let resolvePosition!: (position: { x: number; y: number }) => void
    outerPosition.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolvePosition = resolve
        }),
    )
    const { container } = render(<OverlayWindow />)

    fireEvent.mouseEnter(panel(container))
    await new Promise((resolve) => window.setTimeout(resolve, 275))
    fireEvent.mouseLeave(panel(container))
    resolvePosition({ x: 600, y: 40 })

    await waitFor(() => expect(outerPosition).toHaveBeenCalled())
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    expect(resizeOverlayWindow).not.toHaveBeenCalledWith(120, false, true)
  })

  it("starts a new hover intent after leaving during direction lookup", async () => {
    let resolvePosition!: (position: { x: number; y: number }) => void
    outerPosition.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolvePosition = resolve
        }),
    )
    const { container } = render(<OverlayWindow />)

    fireEvent.mouseEnter(panel(container))
    await new Promise((resolve) => window.setTimeout(resolve, 275))
    fireEvent.mouseLeave(panel(container))
    fireEvent.mouseEnter(panel(container))
    await act(async () => {
      resolvePosition({ x: 600, y: 40 })
      await Promise.resolve()
    })

    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
    expect(outerPosition).toHaveBeenCalledTimes(2)
  })
})

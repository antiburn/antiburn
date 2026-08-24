// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../lib/ipc"
import type { LiveUsageSummaryPayload } from "../lib/ipc"
import { OverlayWindow } from "./OverlayWindow"

const getLiveUsage = vi.hoisted(() => vi.fn())
const getLatestSessionActivity = vi.hoisted(() => vi.fn())
const showHudDetail = vi.hoisted(() => vi.fn(async () => {}))
const hideHudDetail = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("../lib/ipc", async () => {
  const actual = await vi.importActual<typeof Ipc>("../lib/ipc")
  return { ...actual, getLiveUsage, getLatestSessionActivity, showHudDetail, hideHudDetail }
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

const hide = vi.hoisted(() => vi.fn(async () => {}))
const setPosition = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    outerPosition: async () => ({ x: 600, y: 40 }),
    setPosition,
    hide,
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

function closeButton(): HTMLElement {
  return screen.getByRole("button", { name: "Close overlay" })
}

/** Advance fake time inside act, so timer work lands in a React batch. */
async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

describe("OverlayWindow", () => {
  beforeEach(() => {
    vi.stubGlobal("localStorage", storage)
    getLiveUsage.mockReset()
    getLiveUsage.mockResolvedValue(summary())
    getLatestSessionActivity.mockReset()
    getLatestSessionActivity.mockResolvedValue(null)
    showHudDetail.mockClear()
    hideHudDetail.mockClear()
    hide.mockClear()
    invoke.mockClear()
    stored.clear()
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
    // The descendant selector counts LED segments and not the round close chip.
    expect(document.querySelectorAll(".pointer-events-none .rounded-full")).toHaveLength(20)
  })

  it("shows the close control at once and the detail window after the delay", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(panel(container))
      expect(closeButton()).toHaveClass("opacity-100")
      await advance(399)
      expect(showHudDetail).not.toHaveBeenCalled()
      await advance(1)
      expect(showHudDetail).toHaveBeenCalledTimes(1)
      expect(showHudDetail).toHaveBeenCalledWith(
        expect.objectContaining({
          reason: "show",
          bars: [
            expect.objectContaining({
              label: "5-hour limit",
              percent: 81,
              resetsAt: expect.any(String),
            }),
          ],
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
      fireEvent.mouseEnter(panel(container))
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledTimes(1)
      fireEvent.mouseLeave(panel(container))
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
      fireEvent.mouseEnter(panel(container))
      await advance(200)
      fireEvent.mouseLeave(panel(container))
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
      expect(hover.emit).not.toBeNull()
      act(() => hover.emit!(true))
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledTimes(1)
      act(() => hover.emit!(false))
      expect(hideHudDetail).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("cancels the timer for the whole drag and restarts it on mouse up", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(panel(container))
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

  it("hides a visible detail window when a drag starts", async () => {
    vi.useFakeTimers()
    try {
      const { container } = render(<OverlayWindow />)
      await advance(0)
      fireEvent.mouseEnter(panel(container))
      await advance(400)
      expect(showHudDetail).toHaveBeenCalledTimes(1)
      fireEvent.mouseDown(panel(container), { screenX: 700, screenY: 100 })
      expect(hideHudDetail).toHaveBeenCalledTimes(1)
      fireEvent.mouseUp(window)
    } finally {
      vi.useRealTimers()
    }
  })

  it("clears the stored preference when its close button hides the HUD", async () => {
    render(<OverlayWindow />)
    localStorage.setItem("antiburn.showFloatingHud", "1")
    await waitFor(() => expect(getLiveUsage).toHaveBeenCalled())
    fireEvent.click(closeButton())
    expect(localStorage.getItem("antiburn.showFloatingHud")).toBe("0")
    await waitFor(() => expect(hide).toHaveBeenCalled())
  })

  it("reports the visible panel bounds to the native watcher", async () => {
    render(<OverlayWindow />)
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_overlay_hover_region", {
        top: expect.any(Number),
        bottom: expect.any(Number),
      }),
    )
  })
})

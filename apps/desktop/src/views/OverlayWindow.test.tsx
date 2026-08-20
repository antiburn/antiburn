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
const openSettingsWindow = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("../lib/ipc", async () => {
  const actual = await vi.importActual<typeof Ipc>("../lib/ipc")
  return { ...actual, getLiveUsage, getLatestSessionActivity, openSettingsWindow }
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
      },
    ],
    errors: [],
    generatedAt: new Date().toISOString(),
  }
}

function panel(container: HTMLElement): HTMLElement {
  return container.firstElementChild!.firstElementChild as HTMLElement
}

async function expand(container: HTMLElement) {
  fireEvent.mouseEnter(panel(container))
  await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
}

describe("OverlayWindow", () => {
  beforeEach(() => {
    vi.stubGlobal("localStorage", storage)
    getLiveUsage.mockReset()
    getLiveUsage.mockResolvedValue(summary())
    getLatestSessionActivity.mockReset()
    getLatestSessionActivity.mockResolvedValue(null)
    openSettingsWindow.mockClear()
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

  it("accepts native hover edges while the app is in the background", async () => {
    render(<OverlayWindow />)
    await waitFor(() => expect(hover.emit).not.toBeNull())
    act(() => hover.emit!(true))
    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
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
    await waitFor(() => expect(hide).toHaveBeenCalled())
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

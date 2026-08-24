// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../../lib/ipc"
import type { HudDetailState } from "../../lib/ipc"
import { HudDetailView } from "./HudDetailView"

const getHudDetailState = vi.hoisted(() => vi.fn())
const setHudDetailSize = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("../../lib/ipc", async () => {
  const actual = await vi.importActual<typeof Ipc>("../../lib/ipc")
  return { ...actual, getHudDetailState, setHudDetailSize }
})

const push = vi.hoisted(() => ({ emit: null as ((state: HudDetailState) => void) | null }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (_event: string, handler: (event: { payload: HudDetailState }) => void) => {
    push.emit = (state: HudDetailState) => handler({ payload: state })
    return () => {}
  }),
}))

function detailState(overrides: Partial<HudDetailState> = {}): HudDetailState {
  return {
    reason: "show",
    now: Date.now(),
    bars: [
      {
        key: "anthropic:five-hour",
        label: "5-hour limit",
        percent: 81,
        resetsAt: new Date(Date.now() + 2 * 3_600_000).toISOString(),
        color: "var(--color-burn)",
      },
    ],
    ...overrides,
  }
}

describe("HudDetailView", () => {
  beforeEach(() => {
    getHudDetailState.mockReset()
    getHudDetailState.mockResolvedValue(null)
    setHudDetailSize.mockClear()
    push.emit = null
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
      height: 120,
    } as DOMRect)
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it("marks only its own document body as transparent", () => {
    const { unmount } = render(<HudDetailView />)
    expect(document.body.dataset.transparentWindow).toBe("true")
    unmount()
    expect(document.body.dataset.transparentWindow).toBeUndefined()
  })

  it("renders the payload stored before this webview existed", async () => {
    getHudDetailState.mockResolvedValue(detailState())
    render(<HudDetailView />)
    await waitFor(() => expect(screen.getByText("5-hour limit")).toBeInTheDocument())
    expect(screen.getByText("81%")).toBeInTheDocument()
    expect(screen.getByText(/^resets in /)).toBeInTheDocument()
  })

  it("reports its measured height after each payload, not before", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    expect(setHudDetailSize).not.toHaveBeenCalled()
    act(() => push.emit!(detailState()))
    expect(setHudDetailSize).toHaveBeenCalledWith(120)
  })

  it("repaints from a pushed refresh without new bars appearing twice", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState()))
    act(() =>
      push.emit!(
        detailState({
          reason: "refresh",
          bars: [
            {
              key: "anthropic:five-hour",
              label: "5-hour limit",
              percent: 82,
              resetsAt: null,
              color: "var(--color-burn)",
            },
          ],
        }),
      ),
    )
    expect(screen.getByText("82%")).toBeInTheDocument()
    expect(screen.queryByText("81%")).not.toBeInTheDocument()
  })

  it("shows the exact empty copy when no limits exist", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState({ bars: [] })))
    expect(screen.getByText("No usage limits detected yet.")).toBeInTheDocument()
  })
})

import { act, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as HudIpc from "../../lib/hudIpc"
import type { HudDetailState } from "../../lib/hudIpc"
import type * as Ipc from "../../lib/ipc"
import { HudDetailView } from "./HudDetailView"

const getHudDetailState = vi.hoisted(() => vi.fn())
const setHudDetailSize = vi.hoisted(() => vi.fn(async () => {}))
const concealHudDetail = vi.hoisted(() => vi.fn(async () => {}))
vi.mock("../../lib/ipc", async () => {
  const actual = await vi.importActual<typeof Ipc>("../../lib/ipc")
  return { ...actual, setHudDetailSize, concealHudDetail }
})
vi.mock("../../lib/hudIpc", async () => {
  const actual = await vi.importActual<typeof HudIpc>("../../lib/hudIpc")
  return { ...actual, getHudDetailState }
})

const push = vi.hoisted(() => ({
  emit: null as ((state: HudDetailState) => void) | null,
  conceal: null as (() => void) | null,
}))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, handler: (received: { payload: unknown }) => void) => {
    if (event === "hud-detail:state") {
      push.emit = (state: HudDetailState) => handler({ payload: state })
    }
    if (event === "hud-detail:conceal") {
      push.conceal = () => handler({ payload: undefined })
    }
    return () => {}
  }),
}))

function detailState(overrides: Partial<HudDetailState> = {}): HudDetailState {
  return {
    reason: "show",
    now: Date.now(),
    noMeterSelected: false,
    bars: [
      {
        key: "anthropic:five-hour",
        label: "5-hour limit",
        percent: 81,
        resetsAt: new Date(Date.now() + 2 * 3_600_000).toISOString(),
        color: "#D97757",
        expectedFraction: 0.6,
      },
    ],
    map: null,
    spend: null,
    target: "usage",
    ...overrides,
  }
}

function detailMap(): NonNullable<HudDetailState["map"]> {
  return {
    dotValue: 500,
    sessions: [
      {
        key: "claude:hud",
        label: "HUD token map",
        agent: "claude-code",
        tokensPerMin: 9_200,
        topMode: "looking",
        frameColor: "var(--color-label-tertiary)",
        modes: {
          looking: 30_000,
          running: 0,
          changing: 16_000,
          delegating: 0,
          thinking: 0,
          talking: 0,
          other: 0,
        },
        subagents: [
          {
            subagentId: "abcdef1234",
            tokensPerMin: 800,
            modes: {
              looking: 0,
              running: 4_000,
              changing: 0,
              delegating: 0,
              thinking: 0,
              talking: 0,
              other: 0,
            },
          },
        ],
      },
      {
        key: "codex:quiet",
        label: "codex",
        agent: "codex",
        tokensPerMin: 60,
        topMode: "talking",
        frameColor: "var(--color-system-red)",
        modes: {
          looking: 0,
          running: 0,
          changing: 0,
          delegating: 0,
          thinking: 0,
          talking: 300,
          other: 0,
        },
        subagents: [],
      },
    ],
  }
}

describe("HudDetailView", () => {
  beforeEach(() => {
    getHudDetailState.mockReset()
    getHudDetailState.mockResolvedValue(null)
    setHudDetailSize.mockClear()
    concealHudDetail.mockClear()
    push.emit = null
    push.conceal = null
    vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
      height: 120,
    } as DOMRect)
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it("carries the linear-use notch through to the card", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState()))
    expect(screen.getByTestId("led-bar-notch")).toHaveStyle({ left: "60%" })
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
              color: "#D97757",
              expectedFraction: null,
            },
          ],
        }),
      ),
    )
    expect(screen.getByText("82%")).toBeInTheDocument()
    expect(screen.queryByText("81%")).not.toBeInTheDocument()
  })

  it("clears the card on conceal and reports back after the paint", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.conceal).not.toBeNull())
    act(() => push.emit!(detailState()))
    expect(screen.getByText("5-hour limit")).toBeInTheDocument()
    act(() => push.conceal!())
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    await waitFor(() => expect(concealHudDetail).toHaveBeenCalledTimes(1))
  })

  it("shows the card again after a conceal", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.conceal).not.toBeNull())
    act(() => push.emit!(detailState()))
    act(() => push.conceal!())
    setHudDetailSize.mockClear()
    act(() => push.emit!(detailState()))
    expect(screen.getByText("5-hour limit")).toBeInTheDocument()
    expect(setHudDetailSize).toHaveBeenCalledWith(120)
  })

  it("lists each live session with its rate, top mode, and the dot value", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState({ map: detailMap() })))
    expect(screen.getByText("HUD token map")).toBeInTheDocument()
    expect(screen.getByText("9.2k/min")).toBeInTheDocument()
    expect(screen.getByText("60/min")).toBeInTheDocument()
    expect(screen.getByLabelText("mostly looking")).toBeInTheDocument()
    expect(screen.getByLabelText("mostly talking")).toBeInTheDocument()
    expect(screen.getByText("● = 500 tokens/min")).toBeInTheDocument()
    // The mode key is gone; each row carries its own top-mode dot.
    expect(screen.queryByText("delegating")).toBeNull()
  })

  it("lights the sub-agent under the pointer and names its top mode", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() =>
      push.emit!(
        detailState({ map: detailMap(), target: "claude:hud", subagent: "abcdef1234" }),
      ),
    )
    const row = screen.getByTestId("hud-detail-session").querySelector("li[data-lit]")!
    expect(row).not.toBeNull()
    expect(row).toHaveTextContent("sub-agent abcdef12 · mostly running")
  })

  it("shows one agent's card when the target is its box", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState({ map: detailMap(), target: "claude:hud" })))
    const card = screen.getByTestId("hud-detail-session")
    expect(card).toHaveTextContent("HUD token map")
    expect(card).toHaveTextContent("9.2k/min")
    expect(card).toHaveTextContent("claude-code · mostly looking")
    expect(card).toHaveTextContent("sub-agent abcdef12")
    expect(card).toHaveTextContent("800/min")
    expect(card).toHaveTextContent("● = 500 tokens/min")
    // The mode row lights the whole bar, looking first, then changing.
    const lit = card.querySelectorAll(".rounded-full")
    expect(lit.length).toBe(20)
    // The usage meter and the other session stay off this card.
    expect(screen.queryByText("5-hour limit")).not.toBeInTheDocument()
    expect(screen.queryByText("codex")).not.toBeInTheDocument()
  })

  it("falls back to the usage card when the target left the map", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState({ map: detailMap(), target: "gone:key" })))
    expect(screen.queryByTestId("hud-detail-session")).not.toBeInTheDocument()
    expect(screen.getByText("5-hour limit")).toBeInTheDocument()
  })

  it("states the spend rate in words when the payload carries one", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState({ spend: "Spending about $0.420/min." })))
    expect(screen.getByTestId("hud-detail-spend")).toHaveTextContent(
      "Spending about $0.420/min.",
    )
    act(() => push.emit!(detailState({ reason: "refresh" })))
    expect(screen.queryByTestId("hud-detail-spend")).toBeNull()
  })

  it("takes the dark theme on the island and gives it back after", async () => {
    document.documentElement.dataset["theme"] = "light"
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState({ island: true })))
    expect(document.documentElement.dataset["theme"]).toBe("dark")
    act(() => push.emit!(detailState({ island: false })))
    expect(document.documentElement.dataset["theme"]).toBe("light")
    delete document.documentElement.dataset["theme"]
  })

  it("draws no map section when the payload carries none", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState()))
    expect(screen.queryByTestId("hud-detail-map")).not.toBeInTheDocument()
  })

  it("shows the exact empty copy when no limits exist", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState({ bars: [] })))
    expect(screen.getByText("No usage limits detected yet.")).toBeInTheDocument()
  })

  it("separates a reader's own choice from a shortage of readings", async () => {
    render(<HudDetailView />)
    await waitFor(() => expect(push.emit).not.toBeNull())
    act(() => push.emit!(detailState({ bars: [], noMeterSelected: true })))
    expect(screen.getByText("No meter selected.")).toBeInTheDocument()
    expect(screen.queryByText("No usage limits detected yet.")).not.toBeInTheDocument()
  })
})

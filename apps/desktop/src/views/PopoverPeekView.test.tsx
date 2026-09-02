import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { PopoverPeekData } from "../lib/popoverPeekIpc"
import { PopoverPeekView } from "./PopoverPeekView"

const harness = vi.hoisted(() => ({
  emit: null as
    | ((request: {
        generation: number
        target: unknown
        retargetCommitRequired: boolean
        initialPresentation: PopoverPeekData | null
      }) => void)
    | null,
}))
const getPopoverPeekState = vi.hoisted(() => vi.fn())
const getPopoverPeekData = vi.hoisted(() => vi.fn())
const popoverPeekConcealed = vi.hoisted(() => vi.fn(async () => true))
const popoverPeekPresented = vi.hoisted(() => vi.fn(async () => true))
const popoverPeekReady = vi.hoisted(() => vi.fn(async () => true))
const popoverPeekRetargetReady = vi.hoisted(() => vi.fn(async () => true))

vi.mock("../lib/popoverPeekIpc", () => ({
  getPopoverPeekState,
  getPopoverPeekData,
  popoverPeekConcealed,
  popoverPeekPresented,
  popoverPeekReady,
  popoverPeekRetargetReady,
  onPopoverPeekRequest: vi.fn(
    async (
      handler: (request: {
        generation: number
        target: unknown
        retargetCommitRequired: boolean
        initialPresentation: PopoverPeekData | null
      }) => void,
    ) => {
      harness.emit = handler
      return () => undefined
    },
  ),
}))

const PROVIDER_DATA: PopoverPeekData = {
  kind: "provider",
  summary: { providers: [], generatedAt: "2026-08-27T00:00:00Z" },
  live: { providers: [], errors: [], meters: [], generatedAt: "2026-08-27T00:00:00Z" },
}

function providerData(provider: string, displayName: string) {
  const usage = { tokensIn: 1, tokensOut: 1, cacheRead: 0, estimatedUsd: 0.01, sessionCount: 1 }
  return {
    kind: "provider" as const,
    summary: {
      providers: [
        {
          provider,
          displayName,
          accountKey: null,
          agents: [],
          state: "estimated" as const,
          staleness: "fresh" as const,
          windows: { today: usage, week: usage, monthToDate: usage, last30Days: usage },
          lastActivityAt: null,
        },
      ],
      generatedAt: "2026-08-27T00:00:00Z",
    },
    live: { providers: [], errors: [], meters: [], generatedAt: "2026-08-27T00:00:00Z" },
  }
}

let frames: FrameRequestCallback[] = []

function flushFrames(): void {
  const pending = frames
  frames = []
  pending.forEach((callback) => callback(0))
}

function providerTarget(provider: string) {
  return { kind: "provider", provider, utcOffsetMinutes: 600 }
}

describe("PopoverPeekView", () => {
  beforeEach(() => {
    frames = []
    harness.emit = null
    vi.clearAllMocks()
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      frames.push(callback)
      return frames.length
    })
    vi.stubGlobal("cancelAnimationFrame", vi.fn())
    Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
      value: 9,
      configurable: true,
    })
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({
      height: 196,
    } as DOMRect)
    getPopoverPeekState.mockResolvedValue({
      generation: 1,
      target: providerTarget("openai"),
      awaitingRetargetCommit: false,
    })
  })

  afterEach(() => {
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  it("shows the cold skeleton without an entrance or native content acknowledgement", async () => {
    let resolveState: (value: unknown) => void = () => undefined
    let resolveData: (value: unknown) => void = () => undefined
    getPopoverPeekState.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveState = resolve
        }),
    )
    getPopoverPeekData.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveData = resolve
        }),
    )

    render(<PopoverPeekView />)

    expect(screen.getByTestId("popover-peek-standby")).toBeInTheDocument()
    await waitFor(() => expect(getPopoverPeekState).toHaveBeenCalled())
    await act(async () => {
      resolveState({
        generation: 1,
        target: providerTarget("openai"),
        awaitingRetargetCommit: false,
      })
    })

    const skeleton = await screen.findByTestId("popover-peek-loading")
    expect(skeleton).toHaveAttribute("data-loading-state", "quiet")
    expect(skeleton.closest("[data-slot-state]")).toHaveAttribute("data-slot-state", "stable")
    expect(screen.getByTestId("anchored-content-presenter")).toHaveAttribute(
      "data-presenter-phase",
      "idle",
    )
    expect(screen.getByRole("status")).toHaveTextContent("Loading preview")
    expect(popoverPeekReady).toHaveBeenCalledWith(9)
    expect(popoverPeekPresented).not.toHaveBeenCalled()
    expect(popoverPeekRetargetReady).not.toHaveBeenCalled()

    await act(async () => resolveData(PROVIDER_DATA))

    expect(await screen.findByText("No local evidence yet")).toBeInTheDocument()
    await waitFor(() => expect(popoverPeekPresented).toHaveBeenCalledWith(1, 196))
    expect(screen.queryByRole("heading", { name: "Usage" })).not.toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "Back to activity" })).not.toBeInTheDocument()
  })

  it("acknowledges a cold seeded request without showing a loading shell", async () => {
    getPopoverPeekState.mockImplementation(() => new Promise(() => undefined))
    getPopoverPeekData.mockImplementation(() => new Promise(() => undefined))
    render(<PopoverPeekView />)
    await waitFor(() => expect(harness.emit).not.toBeNull())

    act(() =>
      harness.emit?.({
        generation: 1,
        target: providerTarget("openai"),
        retargetCommitRequired: false,
        initialPresentation: PROVIDER_DATA,
      }),
    )

    expect(screen.queryByTestId("popover-peek-loading")).not.toBeInTheDocument()
    expect(document.querySelector('[data-generation="1"]')).toHaveTextContent(
      "No local evidence yet",
    )
    expect(getPopoverPeekData).not.toHaveBeenCalled()
    await waitFor(() => expect(popoverPeekPresented).toHaveBeenCalledWith(1, 196))
  })

  it("replaces hydrated loading with a same-generation seed", async () => {
    getPopoverPeekData.mockImplementation(() => new Promise(() => undefined))
    render(<PopoverPeekView />)
    expect(await screen.findByTestId("popover-peek-loading")).toBeInTheDocument()

    act(() =>
      harness.emit?.({
        generation: 1,
        target: providerTarget("openai"),
        retargetCommitRequired: false,
        initialPresentation: PROVIDER_DATA,
      }),
    )

    expect(screen.queryByTestId("popover-peek-loading")).not.toBeInTheDocument()
    expect(document.querySelector('[data-generation="1"]')).toHaveTextContent(
      "No local evidence yet",
    )
    await waitFor(() => expect(popoverPeekPresented).toHaveBeenCalledWith(1, 196))
  })

  it("paints a hydrated same-generation seed before measured retarget commit", async () => {
    getPopoverPeekState.mockResolvedValue({
      generation: 2,
      target: providerTarget("anthropic"),
      awaitingRetargetCommit: true,
    })
    getPopoverPeekData.mockImplementation(() => new Promise(() => undefined))
    render(<PopoverPeekView />)
    expect(await screen.findByTestId("popover-peek-loading")).toBeInTheDocument()

    act(() =>
      harness.emit?.({
        generation: 2,
        target: providerTarget("anthropic"),
        retargetCommitRequired: true,
        initialPresentation: PROVIDER_DATA,
      }),
    )

    expect(screen.queryByTestId("popover-peek-loading")).not.toBeInTheDocument()
    expect(document.querySelector('[data-generation="2"]')).toHaveTextContent(
      "No local evidence yet",
    )
    expect(popoverPeekRetargetReady).not.toHaveBeenCalled()
    act(flushFrames)
    act(flushFrames)
    expect(popoverPeekRetargetReady).toHaveBeenCalledWith(2, 196)
    expect(popoverPeekPresented).not.toHaveBeenCalled()
  })

  it("paints seeded B before committing its measured native retarget", async () => {
    getPopoverPeekData.mockResolvedValue(PROVIDER_DATA)
    render(<PopoverPeekView />)
    await waitFor(() => expect(popoverPeekPresented).toHaveBeenCalledWith(1, 196))
    act(flushFrames)
    fireEvent.transitionEnd(document.querySelector('[data-generation="1"]')!, {
      propertyName: "opacity",
    })

    act(() =>
      harness.emit?.({
        generation: 2,
        target: providerTarget("anthropic"),
        retargetCommitRequired: true,
        initialPresentation: PROVIDER_DATA,
      }),
    )

    expect(document.querySelector('[data-generation="1"]')).not.toBeInTheDocument()
    expect(document.querySelector('[data-generation="2"]')).toHaveTextContent(
      "No local evidence yet",
    )
    expect(screen.queryByTestId("popover-peek-loading")).not.toBeInTheDocument()
    expect(getPopoverPeekData).toHaveBeenCalledTimes(1)
    expect(popoverPeekPresented).toHaveBeenCalledTimes(1)
    expect(popoverPeekRetargetReady).not.toHaveBeenCalled()

    act(flushFrames)
    expect(popoverPeekRetargetReady).not.toHaveBeenCalled()
    act(flushFrames)
    expect(popoverPeekRetargetReady).toHaveBeenCalledWith(2, 196)
    expect(popoverPeekPresented).toHaveBeenCalledTimes(1)
  })

  it("holds immediately resolved B content behind the painted shell barrier", async () => {
    getPopoverPeekData.mockResolvedValue(PROVIDER_DATA)
    render(<PopoverPeekView />)
    await waitFor(() => expect(popoverPeekPresented).toHaveBeenCalledWith(1, 196))
    act(flushFrames)
    fireEvent.transitionEnd(document.querySelector('[data-generation="1"]')!, {
      propertyName: "opacity",
    })

    act(() =>
      harness.emit?.({
        generation: 2,
        target: providerTarget("anthropic"),
        retargetCommitRequired: true,
        initialPresentation: null,
      }),
    )
    await waitFor(() => expect(getPopoverPeekData).toHaveBeenCalledWith(2))

    expect(popoverPeekPresented).toHaveBeenCalledTimes(1)
    expect(document.querySelector('[data-generation="2"]')).not.toBeInTheDocument()
    expect(document.querySelector('[data-generation="1"]')).toHaveTextContent(
      "No local evidence yet",
    )
    expect(screen.queryByTestId("popover-peek-loading")).not.toBeInTheDocument()

    act(flushFrames)
    expect(popoverPeekRetargetReady).not.toHaveBeenCalled()
    await act(async () => flushFrames())

    expect(popoverPeekRetargetReady).toHaveBeenCalledWith(2, 196)
    await waitFor(() => expect(popoverPeekPresented).toHaveBeenCalledWith(2, 196))
  })

  it("pairs rapid seeded retargets and paints only C without a loading shell", async () => {
    getPopoverPeekData.mockResolvedValue(PROVIDER_DATA)
    render(<PopoverPeekView />)
    await waitFor(() => expect(popoverPeekPresented).toHaveBeenCalledWith(1, 196))
    act(flushFrames)
    fireEvent.transitionEnd(document.querySelector('[data-generation="1"]')!, {
      propertyName: "opacity",
    })

    act(() => {
      harness.emit?.({
        generation: 2,
        target: providerTarget("anthropic"),
        retargetCommitRequired: true,
        initialPresentation: providerData("anthropic", "Claude B"),
      })
      harness.emit?.({
        generation: 3,
        target: providerTarget("google"),
        retargetCommitRequired: true,
        initialPresentation: providerData("google", "Gemini C"),
      })
    })

    expect(screen.queryByText("Claude B")).not.toBeInTheDocument()
    expect(screen.getByText("Gemini C")).toBeInTheDocument()
    expect(screen.getByRole("region", { name: "Recently used" })).toContainElement(
      screen.getByText("Gemini C"),
    )
    expect(screen.queryByRole("heading", { name: "Recently used" })).not.toBeInTheDocument()
    expect(screen.queryByTestId("popover-peek-loading")).not.toBeInTheDocument()
    expect(getPopoverPeekData).toHaveBeenCalledTimes(1)
    act(flushFrames)
    act(flushFrames)

    expect(popoverPeekRetargetReady).toHaveBeenCalledOnce()
    expect(popoverPeekRetargetReady).toHaveBeenCalledWith(3, 196)
  })

  it("measures a current failure as unavailable content", async () => {
    getPopoverPeekData.mockRejectedValue(new Error("unavailable"))

    render(<PopoverPeekView />)

    expect(await screen.findByText(/preview unavailable/i)).toBeInTheDocument()
    await waitFor(() => expect(popoverPeekPresented).toHaveBeenCalledWith(1, 196))
    expect(document.querySelector('[data-generation="1"]')).toHaveAttribute(
      "data-slot-state",
      "staged",
    )
    act(flushFrames)
    fireEvent.transitionEnd(document.querySelector('[data-generation="1"]')!, {
      propertyName: "opacity",
    })
    expect(screen.getByRole("status")).toHaveTextContent(/preview unavailable/i)
  })

  it("clears a crossfade before acknowledging concealment", async () => {
    getPopoverPeekData.mockResolvedValue(PROVIDER_DATA)
    render(<PopoverPeekView />)
    await waitFor(() => expect(popoverPeekPresented).toHaveBeenCalledWith(1, 196))
    act(flushFrames)
    expect(screen.getByTestId("anchored-content-presenter")).toHaveAttribute(
      "data-presenter-phase",
      "crossfading",
    )

    act(() =>
      harness.emit?.({
        generation: 2,
        target: null,
        retargetCommitRequired: false,
        initialPresentation: null,
      }),
    )

    expect(screen.queryByText("No local evidence yet")).not.toBeInTheDocument()
    expect(screen.queryByTestId("anchored-content-presenter")).not.toBeInTheDocument()
    await waitFor(() => expect(popoverPeekConcealed).toHaveBeenCalledWith(2))
  })
})

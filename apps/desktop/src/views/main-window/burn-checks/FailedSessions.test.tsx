import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { BurnCheckSamplePayload } from "../../../lib/insightsIpc"
import type * as InsightsIpcModule from "../../../lib/insightsIpc"
import { FailedSessions } from "./BurnCheckTargetPresentation"

const openSample = vi.hoisted(() => vi.fn())
vi.mock("../../../lib/insightsIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof InsightsIpcModule>()),
  openBurnCheckSample: openSample,
}))

function sample(overrides: Partial<BurnCheckSamplePayload> = {}): BurnCheckSamplePayload {
  return {
    navigationHandle: "opaque-claude",
    title: "Review the usage summary",
    agent: "claude-code",
    surface: "cli",
    observedAtMs: 1000,
    repo: "demo",
    timestamp: "2026-09-14T12:00:00Z",
    isActive: false,
    hasForkParent: false,
    forkChildCount: 0,
    cost: { totalUsd: 2, inputUsd: 1, outputUsd: 1, cacheReadUsd: 0, cacheWriteUsd: 0 },
    models: ["claude-sonnet-4-6"],
    modelRuns: [{ model: "claude-sonnet-4-6", thinkingMode: "high" }],
    hygiene: {
      evidenceState: "ready",
      unusedResources: null,
      badges: [
        { id: "modelOverthinking", status: "finding", notAssessedReason: null },
        { id: "obsoleteModel", status: "clean", notAssessedReason: null },
      ],
    },
    ...overrides,
  }
}

beforeEach(() => openSample.mockReset().mockResolvedValue({ outcome: "opened" }))

describe("FailedSessions", () => {
  it("shows every card and limits lists longer than five to five measured rows", () => {
    let rowHeight = 72
    const observers: TestResizeObserver[] = []
    class TestResizeObserver {
      readonly callback: () => void
      constructor(callback: () => void) {
        this.callback = callback
        observers.push(this)
      }
      observe = vi.fn()
      disconnect = vi.fn()
    }
    vi.stubGlobal("ResizeObserver", TestResizeObserver)
    const rect = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        const row = this.hasAttribute("data-session-row")
          ? this.textContent?.match(/Row (\d+)/)
          : null
        return DOMRect.fromRect({
          x: 0,
          y: Number(row?.[1] ?? 0) * (rowHeight + 8),
          width: 400,
          height: rowHeight,
        })
      })
    try {
      const samples = Array.from({ length: 7 }, (_, index) =>
        sample({
          navigationHandle: `opaque-${index}`,
          title: `Row ${index}`,
        }),
      )
      const view = render(<FailedSessions samples={samples} total={7} />)
      const list = screen.getByRole("region", { name: "Failed sessions" })
      expect(screen.getAllByRole("button", { name: /Row \d/ })).toHaveLength(7)
      expect(list).toHaveClass("pr-3", "scroll-edge-fade-top", "scroll-edge-fade-bottom")
      expect(list).toHaveStyle({ maxHeight: "392px" })
      expect(list).toHaveAttribute("tabindex", "0")
      expect(screen.queryByRole("button", { name: /Failed sessions/ })).toBeNull()
      expect(screen.queryByText(/Showing/)).toBeNull()
      const observer = observers.find((observer) =>
        observer.observe.mock.calls.some(
          ([element]) => element === list.querySelector("[data-failed-session-cards]"),
        ),
      )!
      expect(observer).toBeDefined()
      rowHeight = 96
      observer.callback()
      expect(list).toHaveStyle({ maxHeight: "512px" })

      view.rerender(<FailedSessions samples={samples.slice(0, 5)} total={5} />)
      expect(screen.getAllByRole("button", { name: /Row \d/ })).toHaveLength(5)
      expect(screen.getByRole("region", { name: "Failed sessions" })).not.toHaveClass(
        "ui-scroll-viewport",
      )
      expect(list.style.maxHeight).toBe("")
      expect(observer.disconnect).toHaveBeenCalledOnce()
    } finally {
      rect.mockRestore()
      vi.unstubAllGlobals()
    }
  })

  it("shows shared cards, real check results, and both source agents", () => {
    const samples = [
      sample(),
      sample({
        navigationHandle: "opaque-codex",
        agent: "codex",
        title: "Review the settings",
        modelRuns: [{ model: "gpt-5.4", thinkingMode: "high" }],
      }),
      sample({ navigationHandle: "opaque-third", title: "Review the limit states" }),
    ]
    const { container } = render(<FailedSessions samples={samples} total={7} />)
    expect(screen.queryByRole("button", { name: /Failed sessions/ })).toBeNull()
    const first = screen.getByRole("button", { name: /Review the usage summary/ })
    expect(first).toHaveClass("session-card", "bg-session-card")
    expect(within(first).getByText("1 failed")).toBeVisible()
    expect(within(first).getByText("1 passed")).toBeVisible()
    expect(within(first).getByText("Claude Code")).toBeVisible()
    expect(within(first).getByText("$2.00")).toBeVisible()
    expect(screen.getByText("Codex")).toBeVisible()
    expect(container.querySelectorAll("[data-session-vendor-watermark]")).toHaveLength(3)
    expect(container.querySelectorAll("[data-session-row]")).toHaveLength(3)
    expect(container.querySelector("[data-session-row-compact]")).toBeNull()
    expect(screen.queryByText(/All Burn Checks failed/)).toBeNull()
  })

  it("opens opaque handles with Enter and Space, and blocks repeat navigation while busy", async () => {
    let finish!: (value: { outcome: "opened" }) => void
    openSample.mockReturnValueOnce(
      new Promise((resolve) => {
        finish = resolve
      }),
    )
    render(<FailedSessions samples={[sample()]} total={1} />)
    const row = screen.getByRole("button", { name: /Review the usage summary/ })
    fireEvent.keyDown(row, { key: "Enter" })
    expect(openSample).toHaveBeenCalledWith("opaque-claude")
    expect(row).toHaveAttribute("aria-busy", "true")
    expect(row).toHaveAttribute("aria-disabled", "true")
    fireEvent.click(row)
    fireEvent.keyDown(row, { key: " " })
    expect(openSample).toHaveBeenCalledTimes(1)
    await act(async () => finish({ outcome: "opened" }))
    expect(row).not.toHaveAttribute("aria-disabled")
    fireEvent.keyDown(row, { key: " " })
    expect(openSample).toHaveBeenCalledTimes(2)
    await act(async () => {})
  })

  it.each([
    ["deleted", "This session was deleted."],
    ["expired", "This session is no longer available."],
    ["unavailable", "This session is unavailable."],
  ])("reports %s navigation without exposing backend details", async (outcome, message) => {
    openSample.mockResolvedValueOnce({ outcome })
    render(<FailedSessions samples={[sample()]} total={1} />)
    fireEvent.click(screen.getByRole("button", { name: /Review the usage summary/ }))
    expect(await screen.findByRole("status")).toHaveTextContent(message)
  })

  it("keeps missing metadata and pending checks honest", () => {
    render(
      <FailedSessions
        samples={[
          sample({
            cost: null,
            modelRuns: [],
            models: [],
            hygiene: { evidenceState: "pending", unusedResources: null, badges: [] },
          }),
        ]}
        total={1}
      />,
    )
    expect(screen.getByText("Running Burn Checks…")).toBeVisible()
    expect(screen.queryByText("1 failed")).toBeNull()
    expect(screen.queryByText("$2.00")).toBeNull()
    expect(screen.getByText("Claude Code")).toBeVisible()
    expect(screen.queryByText(/Showing/)).toBeNull()
  })

  it("handles zero failures and unavailable session records", () => {
    const view = render(<FailedSessions samples={[]} total={0} />)
    expect(view.container).toBeEmptyDOMElement()
    view.rerender(<FailedSessions samples={[]} total={7} />)
    expect(screen.queryByText(/No failed sessions/)).toBeNull()
    expect(screen.queryByText(/Showing/)).toBeNull()
  })
})

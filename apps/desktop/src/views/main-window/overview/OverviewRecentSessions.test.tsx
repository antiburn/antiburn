import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { SessionListEntry } from "../../../components/session/SessionList"
import * as SnoozedBurnChecks from "../../../lib/snoozedBurnChecks"
import { OverviewRecentSessions } from "./OverviewRecentSessions"

function entry(sessionId: string, title: string): SessionListEntry {
  return {
    agent: "claude",
    sessionId,
    repo: "antiburn",
    timestamp: "2026-09-14T08:00:00Z",
    isActive: false,
    title,
  }
}

afterEach(() => vi.useRealTimers())

describe("OverviewRecentSessions", () => {
  it("updates idle ages, pauses when inactive, and displays new activity", () => {
    vi.useFakeTimers()
    vi.setSystemTime(new Date("2026-09-14T08:03:00Z"))
    const item = entry("s1", "Current session")
    const props = {
      entries: [item],
      onSelect: vi.fn(),
      onOpenAll: vi.fn(),
      metric: "cost" as const,
    }
    const { rerender } = render(<OverviewRecentSessions {...props} />)
    expect(screen.getByText("3m ago")).toBeVisible()
    act(() => vi.advanceTimersByTime(120_000))
    expect(screen.getByText("5m ago")).toBeVisible()
    rerender(<OverviewRecentSessions {...props} active={false} />)
    act(() => vi.advanceTimersByTime(120_000))
    expect(screen.getByText("5m ago")).toBeInTheDocument()
    rerender(<OverviewRecentSessions {...props} />)
    expect(screen.getByText("7m ago")).toBeVisible()
    rerender(
      <OverviewRecentSessions
        {...props}
        entries={[{ ...item, timestamp: new Date().toISOString(), isActive: true }]}
      />,
    )
    expect(screen.getByText("active")).toBeVisible()
    expect(screen.queryByText("7m ago")).toBeNull()
  })

  it("renders one session row per entry and reports the clicked entry", () => {
    const onSelect = vi.fn()
    const onOpenAll = vi.fn()
    const entries = [
      entry("s1", "Refine keyboard navigation"),
      entry("s2", "Simplify the settings flow"),
      entry("s3", "Count Codex spawn_agent calls"),
    ]
    render(
      <OverviewRecentSessions
        metric="cost"
        entries={entries}
        onSelect={onSelect}
        onOpenAll={onOpenAll}
      />,
    )
    const panel = screen.getByRole("region", { name: "Recent sessions" })
    const rows = within(panel).getAllByRole("button", { name: /Refine|Simplify|Count/ })
    expect(rows).toHaveLength(3)
    fireEvent.keyDown(rows[0]!, { key: "Enter" })
    expect(onSelect).toHaveBeenLastCalledWith(entries[0])
    fireEvent.keyDown(rows[2]!, { key: " " })
    expect(onSelect).toHaveBeenLastCalledWith(entries[2])
    fireEvent.click(rows[1]!)
    expect(onSelect).toHaveBeenCalledWith(entries[1])
    fireEvent.click(within(panel).getByRole("button", { name: "All sessions" }))
    expect(onOpenAll).toHaveBeenCalledOnce()
  })

  it("explains an empty list and marks the panel busy while it loads", () => {
    const { rerender } = render(
      <OverviewRecentSessions
        metric="cost"
        entries={null}
        loading
        onSelect={vi.fn()}
        onOpenAll={vi.fn()}
      />,
    )
    const panel = screen.getByRole("region", { name: "Recent sessions" })
    expect(panel).toHaveAttribute("aria-busy", "true")
    rerender(
      <OverviewRecentSessions
        metric="cost"
        entries={[]}
        onSelect={vi.fn()}
        onOpenAll={vi.fn()}
      />,
    )
    expect(within(panel).getByText("No sessions yet.")).toBeVisible()
  })

  it("withholds rows until snoozes load and shows the unavailable state after failure", () => {
    const hook = vi
      .spyOn(SnoozedBurnChecks, "useSnoozedBurnChecks")
      .mockReturnValue({ status: "loading", records: [] })
    const props = {
      entries: [entry("s1", "Stored failing session")],
      onSelect: vi.fn(),
      onOpenAll: vi.fn(),
      metric: "cost" as const,
    }
    const view = render(<OverviewRecentSessions {...props} />)
    expect(screen.queryByText("Stored failing session")).toBeNull()

    hook.mockReturnValue({ status: "error", records: [] })
    view.rerender(<OverviewRecentSessions {...props} />)
    expect(screen.getByText("Recent sessions are unavailable.")).toBeVisible()
    expect(screen.queryByText("No sessions yet.")).toBeNull()
    hook.mockRestore()
  })

  it("gives the list and the loading skeleton the same row class and count, so the height query hides the same rows either way", () => {
    const { container, rerender } = render(
      <OverviewRecentSessions
        metric="cost"
        entries={null}
        loading
        onSelect={vi.fn()}
        onOpenAll={vi.fn()}
      />,
    )
    const skeleton = container.querySelector(".overview-recent-rows")
    expect(skeleton?.children).toHaveLength(6)

    const entries = Array.from({ length: 6 }, (_, index) =>
      entry(`s${index}`, `Session ${index}`),
    )
    rerender(
      <OverviewRecentSessions
        metric="cost"
        entries={entries}
        onSelect={vi.fn()}
        onOpenAll={vi.fn()}
      />,
    )
    const list = container.querySelector(".overview-recent-rows")
    expect(list?.children).toHaveLength(6)
  })
})

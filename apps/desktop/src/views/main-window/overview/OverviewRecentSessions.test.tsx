import { fireEvent, render, screen, within } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import type { SessionListEntry } from "../../../components/session/SessionList"
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

describe("OverviewRecentSessions", () => {
  it("renders one session row per entry and reports the clicked entry", () => {
    const onSelect = vi.fn()
    const onOpenAll = vi.fn()
    const entries = [
      entry("s1", "Refine keyboard navigation"),
      entry("s2", "Simplify the settings flow"),
      entry("s3", "Count Codex spawn_agent calls"),
    ]
    render(
      <OverviewRecentSessions entries={entries} onSelect={onSelect} onOpenAll={onOpenAll} />,
    )
    const panel = screen.getByRole("region", { name: "Recent sessions" })
    const rows = within(panel).getAllByRole("button", { name: /Refine|Simplify|Count/ })
    expect(rows).toHaveLength(3)
    fireEvent.click(rows[1]!)
    expect(onSelect).toHaveBeenCalledWith(entries[1])
    fireEvent.click(within(panel).getByRole("button", { name: "All sessions" }))
    expect(onOpenAll).toHaveBeenCalledOnce()
  })

  it("explains an empty list and marks the panel busy while it loads", () => {
    const { rerender } = render(
      <OverviewRecentSessions entries={null} loading onSelect={vi.fn()} onOpenAll={vi.fn()} />,
    )
    const panel = screen.getByRole("region", { name: "Recent sessions" })
    expect(panel).toHaveAttribute("aria-busy", "true")
    rerender(<OverviewRecentSessions entries={[]} onSelect={vi.fn()} onOpenAll={vi.fn()} />)
    expect(within(panel).getByText("No sessions yet.")).toBeVisible()
  })
})

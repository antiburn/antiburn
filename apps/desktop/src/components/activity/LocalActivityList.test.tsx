// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import {
  LocalActivityList,
  type LocalActivityEntry,
  type LocalActivityListProps,
} from "./LocalActivityList"

afterEach(cleanup)

const NOW = new Date("2026-03-04T12:00:00.000Z")

function at(daysAgo: number, hour = 12): string {
  const date = new Date(NOW)
  date.setDate(date.getDate() - daysAgo)
  date.setHours(hour, 0, 0, 0)
  return date.toISOString()
}

function entry(over: Partial<LocalActivityEntry> = {}): LocalActivityEntry {
  return {
    agent: "claude-code",
    sessionId: "session-1",
    repo: "avery/widgets",
    timestamp: at(0),
    isActive: false,
    ...over,
  }
}

function list(over: Partial<LocalActivityListProps> = {}) {
  const props: LocalActivityListProps = {
    entries: [entry()],
    days: 7,
    now: NOW,
    ...over,
  }
  return render(<LocalActivityList {...props} />)
}

describe("LocalActivityList — rows", () => {
  it("names a session by its title, then a short id, then its agent", () => {
    list({
      entries: [
        entry({ sessionId: "a", title: "Fix the flaky test" }),
        entry({ sessionId: "abcdef1234567", title: "  " }),
        entry({ sessionId: undefined, agent: "amp-code" }),
      ],
    })
    expect(screen.getByText("Fix the flaky test")).toBeTruthy()
    expect(screen.getByText("Session abcdef1")).toBeTruthy()
    expect(screen.getByText("Amp")).toBeTruthy()
  })

  it("shows the repository, extra repositories, and branch", () => {
    list({
      entries: [
        entry({ repo: "avery/widgets", additionalRepos: ["avery/docs"], branch: "main" }),
      ],
    })
    expect(screen.getByText("avery/widgets +1")).toBeTruthy()
    expect(screen.getByText("main")).toBeTruthy()
  })

  it("omits the identity line entirely when there is nothing to say", () => {
    list({ entries: [entry({ repo: "", branch: undefined })] })
    expect(screen.queryByText("avery/widgets")).toBeNull()
  })

  it("marks a running session for both sighted and screen-reader users", () => {
    const { container } = list({ entries: [entry({ isActive: true, title: "Running" })] })
    expect(screen.getByText("Active session")).toBeTruthy()
    expect(container.querySelector(".activity-row-active")).toBeTruthy()
    expect(screen.getByText("Running").className).toContain("activity-row-title-shimmer")
  })

  it("marks fork relationships in both directions", () => {
    list({ entries: [entry({ hasForkParent: true, forkChildCount: 2 })] })
    expect(screen.getByLabelText("Forked from another session")).toBeTruthy()
    expect(screen.getByLabelText("2 direct forks")).toBeTruthy()
  })

  it('says "fork" in the singular for one child', () => {
    list({ entries: [entry({ forkChildCount: 1 })] })
    expect(screen.getByLabelText("1 direct fork")).toBeTruthy()
  })

  it("marks a WSL origin", () => {
    list({ entries: [entry({ wslDistro: "Ubuntu-24.04" })] })
    expect(
      screen.getByLabelText("Found in Ubuntu-24.04 on Windows Subsystem for Linux"),
    ).toBeTruthy()
  })

  it("shows the local cost and active-time pills it is given", () => {
    list({
      entries: [
        entry({
          cost: { totalUsd: 2.4, figureLabel: "Estimated cost" },
          activeTime: { activeSecs: 5400, durationSecs: 28_800 },
        }),
      ],
    })
    expect(screen.getByLabelText("Estimated cost $2.40")).toBeTruthy()
    expect(screen.getAllByText("1h 30m").length).toBeGreaterThan(0)
  })

  it("omits the badge line when a session has no measurements", () => {
    const { container } = list({ entries: [entry()] })
    expect(container.querySelector(".min-h-\\[15px\\]")).toBeNull()
  })

  it("states the last-activity time in the corner", () => {
    list({ entries: [entry({ timestamp: at(0, 9) })] })
    expect(screen.getByText(/^Last activity /)).toBeTruthy()
  })
})

describe("LocalActivityList — navigation", () => {
  it("opens a session by click and by keyboard from the row itself", () => {
    const onOpenSession = vi.fn()
    list({ entries: [entry({ title: "Fix the flaky test" })], onOpenSession })
    const row = screen.getByRole("button", { name: /Fix the flaky test/ })

    fireEvent.click(row)
    fireEvent.keyDown(row, { key: "Enter" })
    fireEvent.keyDown(row, { key: " " })
    expect(onOpenSession).toHaveBeenCalledTimes(3)
    expect(onOpenSession).toHaveBeenLastCalledWith(
      expect.objectContaining({ sessionId: "session-1" }),
    )
  })

  it("leaves a row inert when there is nothing to open", () => {
    list({ entries: [entry({ title: "Fix the flaky test" })] })
    expect(screen.queryByRole("button", { name: /Fix the flaky test/ })).toBeNull()
  })

  it("leaves a session with no transcript id inert", () => {
    list({
      entries: [entry({ sessionId: undefined, title: "Untracked" })],
      onOpenSession: vi.fn(),
    })
    expect(screen.queryByRole("button", { name: /Untracked/ })).toBeNull()
  })

  it("opens the roster from the fan-out pill without also opening the row", () => {
    const onOpenSession = vi.fn()
    const onOpenOrchestration = vi.fn()
    list({
      entries: [entry({ subagentCount: 4 })],
      onOpenSession,
      onOpenOrchestration,
    })
    fireEvent.click(screen.getByLabelText("Orchestrated 4 sub-agents, view roster"))
    expect(onOpenOrchestration).toHaveBeenCalledOnce()
    expect(onOpenSession).not.toHaveBeenCalled()
  })

  it("does not offer a fan-out pill below the orchestration threshold", () => {
    list({ entries: [entry({ subagentCount: 1 })], onOpenSession: vi.fn() })
    expect(screen.queryByLabelText(/view roster/)).toBeNull()
  })

  it("renders an agent icon only from the injected renderer", () => {
    const renderAgentIcon = vi.fn(() => <span data-testid="agent-icon" />)
    list({ entries: [entry({ surface: "cli" })], renderAgentIcon })
    expect(renderAgentIcon).toHaveBeenCalledWith("claude-code", 18, "cli")
    expect(screen.getByTestId("agent-icon")).toBeTruthy()
  })
})

describe("LocalActivityList — grouping and range", () => {
  it("buckets rows by calendar day and pins the newest label", () => {
    list({
      entries: [
        entry({ sessionId: "a", title: "Today one", timestamp: at(0) }),
        entry({ sessionId: "b", title: "Yesterday one", timestamp: at(1) }),
        entry({ sessionId: "c", title: "Older one", timestamp: at(3) }),
      ],
    })
    expect(screen.getByTestId("activity-pinned-group-label").textContent).toBe("Today")
    // The first heading is announced but not painted twice.
    expect(screen.getByText("Yesterday")).toBeTruthy()
    expect(screen.getByText("3 days ago")).toBeTruthy()
  })

  it("orders the newest session first within a day", () => {
    list({
      entries: [
        entry({ sessionId: "early", title: "Morning", timestamp: at(0, 9) }),
        entry({ sessionId: "late", title: "Afternoon", timestamp: at(0, 11) }),
      ],
      onOpenSession: vi.fn(),
    })
    const rows = screen.getAllByRole("button", { name: /Morning|Afternoon/ })
    expect(within(rows[0] as HTMLElement).getByText("Afternoon")).toBeTruthy()
  })

  it("drops sessions outside the visible window", () => {
    list({
      entries: [
        entry({ sessionId: "inside", title: "Inside", timestamp: at(2) }),
        entry({ sessionId: "outside", title: "Outside", timestamp: at(9) }),
      ],
    })
    expect(screen.getByText("Inside")).toBeTruthy()
    expect(screen.queryByText("Outside")).toBeNull()
  })

  it("states the visible range and opens settings to change it", () => {
    const onOpenSettings = vi.fn()
    list({ onOpenSettings })
    fireEvent.click(screen.getByLabelText(/Recent activity range: Last 7 days/))
    expect(onOpenSettings).toHaveBeenCalledOnce()
  })

  it("renders the range as plain text when there is nowhere to change it", () => {
    list({ days: 1 })
    expect(screen.getByText("Last 1 day")).toBeTruthy()
    expect(screen.queryByLabelText(/Recent activity range/)).toBeNull()
  })

  it("accepts a caller-supplied range label", () => {
    list({ rangeLabel: "This week" })
    expect(screen.getByText("This week")).toBeTruthy()
  })
})

describe("LocalActivityList — filters and empty state", () => {
  it("renders a plain title when there is nothing to filter by", () => {
    list({ title: "Sessions" })
    expect(screen.getByText("Sessions")).toBeTruthy()
    expect(screen.queryByLabelText(/Activity filter/)).toBeNull()
  })

  it("offers the supplied filters and reports a selection", () => {
    const onFilterChange = vi.fn()
    list({
      filters: [
        { value: "all", label: "All sessions" },
        { value: "active", label: "Running now" },
      ],
      selectedFilter: "all",
      onFilterChange,
    })
    const trigger = screen.getByLabelText("Activity filter, current view: All sessions")
    // The menu opens from the keyboard; jsdom has no pointer capture for the
    // pointer path the trigger prefers.
    fireEvent.keyDown(trigger, { key: "Enter" })
    fireEvent.click(screen.getByText("Running now"))
    expect(onFilterChange).toHaveBeenCalledWith("active")
  })

  it("explains an empty range and announces it once", () => {
    list({ entries: [] })
    expect(screen.getAllByText("No sessions in the last 7 days").length).toBe(2)
    expect(
      screen.getByText("Coding sessions appear here as they are discovered on this machine."),
    ).toBeTruthy()
  })

  it('says "today" for a one-day window', () => {
    list({ entries: [], days: 1 })
    expect(screen.getAllByText("No sessions today").length).toBe(2)
  })

  it("accepts caller-supplied empty copy", () => {
    list({ entries: [], emptyTitle: "Nothing here", emptyDescription: "Try a wider range." })
    expect(screen.getAllByText("Nothing here").length).toBe(2)
    expect(screen.getByText("Try a wider range.")).toBeTruthy()
  })

  it("shows no day heading at all when the list is empty", () => {
    list({ entries: [] })
    expect(screen.queryByTestId("activity-pinned-group-label")).toBeNull()
  })
})

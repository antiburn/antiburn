import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { SessionList, type SessionListEntry, type SessionListProps } from "./SessionList"

afterEach(cleanup)

const NOW = new Date("2026-03-04T12:00:00.000Z")

function at(daysAgo: number, hour = 12): string {
  const date = new Date(NOW)
  date.setDate(date.getDate() - daysAgo)
  date.setHours(hour, 0, 0, 0)
  return date.toISOString()
}

function entry(over: Partial<SessionListEntry> = {}): SessionListEntry {
  return {
    agent: "claude-code",
    sessionId: "session-1",
    repo: "avery/widgets",
    timestamp: at(0),
    isActive: false,
    ...over,
  }
}

function list(over: Partial<SessionListProps> = {}) {
  const props: SessionListProps = {
    entries: [entry()],
    days: 7,
    now: NOW,
    ...over,
  }
  return render(<SessionList {...props} />)
}

describe("SessionList — rows", () => {
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

  it("shows up to two lines of the primary text", () => {
    list({ entries: [entry({ title: "A long session title" })] })
    expect(screen.getByText("A long session title").className).toContain("truncated-text-lines")
  })

  it("marks a hot spend with the brand pill, and a usual cost without one", () => {
    list({
      entries: [
        entry({
          sessionId: "hot",
          cost: { totalUsd: 24, figureLabel: "Estimated cost", isHighCost: true },
        }),
        entry({ sessionId: "usual", cost: { totalUsd: 2.4, figureLabel: "Estimated cost" } }),
      ],
    })
    expect(
      screen.getByLabelText("Estimated cost $24.00, higher than usual").className,
    ).toContain("bg-brand-tint")
    expect(screen.getByLabelText("Estimated cost $2.40").className).not.toContain(
      "bg-brand-tint",
    )
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

  it("shows the local cost pill it is given", () => {
    list({
      entries: [
        entry({
          cost: { totalUsd: 2.4, figureLabel: "Estimated cost" },
        }),
      ],
    })
    expect(screen.getByLabelText("Estimated cost $2.40")).toBeTruthy()
  })

  it("shows short model names in the supplied parent-first order", () => {
    list({
      entries: [
        entry({
          modelRuns: [
            { model: "gpt-5.6-sol", thinkingMode: "xhigh" },
            { model: "claude-fable-5", thinkingMode: "high" },
          ],
        }),
      ],
    })
    const models = screen.getByText("5.6-sol").closest("[title]")
    expect(models?.getAttribute("title")).toBe("gpt-5.6-sol/xhigh\nclaude-fable-5/high")
    expect(models?.textContent).toBe("5.6-sol xhigh · fable-5 high")
  })

  it("includes the relative-time suffix", () => {
    list({ entries: [entry({ timestamp: at(0, 9) })] })
    expect(screen.getByText(/ ago$/)).toBeTruthy()
  })

  it("shows evidence computation instead of a synthetic hygiene verdict", () => {
    list({ entries: [entry({ sessionId: "session-pending" })] })
    const verdict = screen.getByLabelText("Computing session hygiene checks")
    expect(verdict.textContent).toBe("Computing checks")
    expect(verdict.style.color).toBe("var(--color-label-tertiary)")
    expect(screen.queryByLabelText("All checks pass")).toBeNull()
  })

  it("states the last-activity time", () => {
    list({ entries: [entry({ timestamp: at(0, 9) })] })
    expect(screen.getByLabelText(/^Last activity /)).toBeTruthy()
  })
})

describe("SessionList — navigation", () => {
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

  it("renders an agent icon only from the injected renderer", () => {
    const renderAgentIcon = vi.fn(() => <span data-testid="agent-icon" />)
    list({ entries: [entry({ surface: "cli" })], renderAgentIcon })
    expect(renderAgentIcon).toHaveBeenCalledWith("claude-code", 14, "cli")
    expect(screen.getByTestId("agent-icon")).toBeTruthy()
  })
})

describe("SessionList — grouping", () => {
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
})

describe("SessionList — empty state", () => {
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

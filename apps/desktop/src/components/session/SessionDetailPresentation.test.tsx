// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import {
  inclusiveCostSubject,
  type LocalSessionCost,
} from "../../lib/presentation/sessionCosts"
import type {
  ActiveSessionsSummary,
  SessionBucket,
  SessionMetrics,
} from "../../lib/types/session"
import {
  SessionDetailPresentation,
  type SessionDetailPresentationProps,
} from "./SessionDetailPresentation"

afterEach(cleanup)

function bucket(over: Partial<SessionBucket> = {}): SessionBucket {
  return {
    tokensIn: 1000,
    tokensOut: 200,
    contextTokens: 40_000,
    isCompactionBoundary: false,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    isCacheRehydration: false,
    subagentLaunches: 0,
    ...over,
  }
}

function metrics(over: Partial<SessionMetrics> = {}): SessionMetrics {
  return {
    agent: "claude-code",
    sessionId: "session-1",
    durationSecs: 3600,
    activeSecs: 1800,
    eventCount: 42,
    tokensIn: 120_000,
    tokensOut: 8_000,
    peakContextTokens: 90_000,
    contextAvailable: true,
    contextWindow: 200_000,
    toolMix: { edit: 10, read: 8, search: 3, test: 2, bash: 5, other: 1 },
    grepCount: 3,
    buckets: [bucket(), bucket()],
    ...over,
  }
}

function summary(over: Partial<ActiveSessionsSummary> = {}): ActiveSessionsSummary {
  return {
    sessionCount: 1,
    avgDurationSecs: 3600,
    avgActiveSecs: 1800,
    toolMix: { edit: 10, read: 8, search: 3, test: 2, bash: 5, other: 1 },
    grepTotal: 3,
    tokensInTotal: 120_000,
    tokensOutTotal: 8_000,
    peakContextTokens: 90_000,
    contextAvailable: true,
    contextWindow: 200_000,
    buckets: [bucket(), bucket()],
    sessions: [metrics()],
    ...over,
  }
}

function cost(totalCostUsd = 2.4): LocalSessionCost {
  return {
    subject: inclusiveCostSubject("claude-code", "session-1"),
    inputTokens: 1,
    outputTokens: 2,
    cacheReadTokens: 3,
    cacheCreationTokens: 4,
    totalTokens: 10,
    inputCostUsd: 0.3,
    outputCostUsd: 0.8,
    cacheReadCostUsd: 1.1,
    cacheWriteCostUsd: 0.2,
    totalCostUsd,
    isActive: false,
  }
}

function presentationProps(
  over: Partial<SessionDetailPresentationProps> = {},
): SessionDetailPresentationProps {
  return {
    summary: summary(),
    loading: false,
    error: false,
    onBack: () => {},
    session: {
      agent: "claude-code",
      sessionId: "session-1",
      title: "Fix the flaky test",
      wslDistro: null,
    },
    supportsAnalytics: true,
    cost: null,
    costSplit: null,
    orchestration: null,
    modelRuns: [],
    relations: null,
    onOpenSubagent: () => {},
    onOpenOrchestrator: () => {},
    onOpenRelatedSession: () => {},
    onExportSession: () => {},
    onDeleteSession: () => {},
    renderAgentIcon: () => null,
    ...over,
  }
}

function view(over: Partial<SessionDetailPresentationProps> = {}) {
  return render(<SessionDetailPresentation {...presentationProps(over)} />)
}

describe("SessionDetailPresentation — chrome", () => {
  it("renders the useful card hierarchy for a settled session", () => {
    view({ cost: cost() })
    expect(screen.getByText("Fix the flaky test")).toBeTruthy()
    expect(screen.getByText("Context")).toBeTruthy()
    expect(screen.getByText("Cost")).toBeTruthy()
    expect(screen.getByText("Tools")).toBeTruthy()
  })

  it("omits the Cost card when nothing priced the session", () => {
    view({ cost: null })
    expect(screen.getByText("Context")).toBeTruthy()
    expect(screen.queryByText("Cost")).toBeNull()
  })

  it("shows the session title on no more than three lines", () => {
    view({
      session: {
        agent: "claude-code",
        sessionId: "session-1",
        title: "A session title that can continue across more than one line",
        wslDistro: null,
      },
    })
    const title = screen.getByText(
      "A session title that can continue across more than one line",
    )
    expect(title.className).toContain("truncated-text-lines")
    expect(title.className).toContain("break-all")
    expect(title.style.getPropertyValue("--truncated-text-lines")).toBe("3")
  })

  it("arranges list metadata around the session title", () => {
    const timestamp = new Date(Date.now() - 11 * 60_000).toISOString()
    view({
      session: {
        agent: "claude-code",
        sessionId: "session-1",
        repo: "antiburn",
        timestamp,
        title: "Simplify the session detail",
        wslDistro: null,
      },
      modelRuns: [{ model: "gpt-5.6-sol", thinkingMode: "high" }],
      cost: cost(),
    })

    const summaryRow = screen.getByLabelText("Session summary")
    expect(summaryRow).toHaveTextContent("antiburn")
    expect(summaryRow).toHaveTextContent("$2.40")
    expect(summaryRow).toHaveTextContent("11m ago")

    const detailRow = screen.getByLabelText("Session timing and models")
    expect(detailRow).toHaveTextContent("30m active")
    expect(detailRow).toHaveTextContent("last 11m ago")
    expect(detailRow).toHaveTextContent("5.6-sol/high")
    expect(screen.getByLabelText("Session hygiene checks").children).toHaveLength(6)
  })

  it("names the back control for what it does, not for the view it leaves", () => {
    view()
    // The heading and the back control are separate elements.
    // The screen reader announces the view and the control correctly.
    expect(screen.getByRole("heading", { name: "Session Detail" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Back" })).toBeTruthy()
  })

  it("navigates back through the callback", () => {
    const onBack = vi.fn()
    view({ onBack })
    fireEvent.click(screen.getByRole("button", { name: "Back" }))
    expect(onBack).toHaveBeenCalledOnce()
  })

  it("disables traversal that has no adjacent session, and wires the arrow keys", () => {
    const onNext = vi.fn()
    view({ onNext })
    expect(screen.getByLabelText("Newer session").hasAttribute("disabled")).toBe(true)
    expect(screen.getByLabelText("Older session").hasAttribute("disabled")).toBe(false)

    fireEvent.keyDown(document, { key: "ArrowRight" })
    expect(onNext).toHaveBeenCalledOnce()
    fireEvent.keyDown(document, { key: "ArrowLeft" })
    expect(onNext).toHaveBeenCalledOnce()
  })

  it("leaves the arrow keys alone while typing", () => {
    const onNext = vi.fn()
    view({ onNext })
    const input = document.createElement("input")
    document.body.appendChild(input)
    input.focus()
    fireEvent.keyDown(input, { key: "ArrowRight" })
    expect(onNext).not.toHaveBeenCalled()
    input.remove()
  })
})

describe("SessionDetailPresentation — states", () => {
  it("holds the skeleton back on a fast load and shows it on a slow one", () => {
    vi.useFakeTimers()
    try {
      const { rerender } = render(
        <SessionDetailPresentation {...presentationProps({ summary: null, loading: true })} />,
      )
      expect(screen.queryByTestId("session-analytics-skeleton")).toBeNull()

      act(() => {
        vi.advanceTimersByTime(250)
      })
      expect(screen.getByTestId("session-analytics-skeleton")).toBeTruthy()

      // Once shown it holds for its minimum-visible window even after the
      // load finishes, so it cannot flicker.
      rerender(
        <SessionDetailPresentation
          {...presentationProps({ summary: summary(), loading: false })}
        />,
      )
      expect(screen.getByTestId("session-analytics-skeleton")).toBeTruthy()

      act(() => {
        vi.advanceTimersByTime(500)
      })
      expect(screen.queryByTestId("session-analytics-skeleton")).toBeNull()
    } finally {
      vi.useRealTimers()
    }
  })

  it("reports a failure without pretending the session was empty", () => {
    view({ summary: null, error: true })
    expect(screen.getByText("Couldn't read this session.")).toBeTruthy()
    expect(screen.queryByText("No session analysis available")).toBeNull()
  })

  it("explains an empty session, and an unsupported agent differently", () => {
    const { unmount } = view({ summary: summary({ sessionCount: 0 }) })
    expect(screen.getByText("No session analysis available")).toBeTruthy()
    unmount()

    view({
      summary: summary({ sessionCount: 0 }),
      supportsAnalytics: false,
      session: { agent: "kiro", sessionId: "s1", wslDistro: null },
    })
    expect(screen.getByText(/Session analysis for Kiro sessions/)).toBeTruthy()
  })

  it("blames the fork parent when a fork has no activity of its own", () => {
    view({
      summary: summary({ sessionCount: 0 }),
      relations: {
        parent: { identity: { agent: "claude-code", sessionId: "p1" }, available: true },
        children: [],
      },
    })
    expect(screen.getByText(/This fork has no analyzable child activity yet/)).toBeTruthy()
  })

  it("still shows the price of a session it could not analyze", () => {
    view({
      summary: summary({ sessionCount: 0 }),
      cost: cost(),
    })
    expect(screen.getByLabelText("Estimated cost $2.40")).toBeTruthy()
  })
})

describe("SessionDetailPresentation — session facts", () => {
  it("shows the local cost badge and its breakdown", () => {
    view({
      cost: cost(),
    })
    expect(screen.getByLabelText("Estimated cost $2.40")).toBeTruthy()
    expect(screen.getByText("Input")).toBeTruthy()
    // The pill and the breakdown headline are the same figure, by design.
    expect(screen.getAllByText("$2.40").length).toBeGreaterThan(1)
  })

  it("marks a WSL session origin in the header", () => {
    view({
      session: { agent: "claude-code", sessionId: "s1", title: "T", wslDistro: "Ubuntu-24.04" },
    })
    expect(
      screen.getByLabelText("Found in Ubuntu-24.04 on Windows Subsystem for Linux"),
    ).toBeTruthy()
  })

  it("offers the orchestrator roster and opens a sub-agent from it", () => {
    const onOpenSubagent = vi.fn()
    view({
      onOpenSubagent,
      orchestration: {
        orchestrating: true,
        orchestratorAgent: "claude-code",
        orchestratorSessionId: "session-1",
        subagentCount: 2,
        members: [
          { agent: "claude-code", subagentId: "a", label: "Investigate" },
          { agent: "claude-code", subagentId: "b", label: "Write tests" },
        ],
      },
    })
    fireEvent.click(screen.getByText("Orchestrated 2 agents"))
    fireEvent.click(screen.getByText("Write tests"))
    expect(onOpenSubagent).toHaveBeenCalledWith("b", "Write tests")
  })

  it("marks a sub-agent view and links up to its orchestrator", () => {
    const onOpenOrchestrator = vi.fn()
    view({
      onOpenOrchestrator,
      session: {
        agent: "claude-code",
        sessionId: "child-1",
        wslDistro: null,
        subagent: {
          parentTitle: "Ship the release",
        },
      },
    })
    expect(screen.getByText("Sub-agent")).toBeTruthy()
    fireEvent.click(screen.getByText("Autonomous sub-agent"))
    fireEvent.click(screen.getByText("Ship the release"))
    expect(onOpenOrchestrator).toHaveBeenCalledOnce()
  })

  it("opens a fork parent through the callback", () => {
    const onOpenRelatedSession = vi.fn()
    const parent = {
      identity: { agent: "claude-code", sessionId: "p1" },
      title: "Original run",
      available: true,
    }
    view({ relations: { parent, children: [] }, onOpenRelatedSession })
    fireEvent.click(screen.getByLabelText("Open fork parent"))
    expect(onOpenRelatedSession).toHaveBeenCalledWith(parent, "Original run")
  })

  it("marks a fork parent whose transcript is gone as unavailable", () => {
    view({
      relations: {
        parent: { identity: { agent: "claude-code", sessionId: "p1" }, available: false },
        children: [],
      },
    })
    expect(screen.getByLabelText("Fork parent is unavailable locally")).toBeTruthy()
    expect(screen.queryByLabelText("Open fork parent")).toBeNull()
  })

  it("collects several forks behind one control", () => {
    view({
      relations: {
        parent: null,
        children: [
          { identity: { agent: "claude-code", sessionId: "c1" }, title: "A", available: true },
          { identity: { agent: "claude-code", sessionId: "c2" }, title: "B", available: true },
        ],
      },
    })
    expect(screen.getByLabelText("Show 2 direct forks")).toBeTruthy()
  })

  it("falls back to a short session id when a relation has no title", () => {
    const onOpenRelatedSession = vi.fn()
    view({
      relations: {
        parent: {
          identity: { agent: "claude-code", sessionId: "abcdef1234567" },
          available: true,
        },
        children: [],
      },
      onOpenRelatedSession,
    })
    fireEvent.click(screen.getByLabelText("Open fork parent"))
    expect(onOpenRelatedSession).toHaveBeenCalledWith(expect.anything(), "Session abcdef1")
  })

  it("still renders the Context card, with just the token layer, when context occupancy is unavailable", () => {
    expect(() => view({ summary: summary({ contextAvailable: false }) })).not.toThrow()
    expect(screen.getByText("Context")).toBeTruthy()
  })

  it("adds the initial-context card when the session has initial context", () => {
    const withContext = summary({
      sessions: [
        metrics({
          initialContext: {
            trackingStatus: "tracked",
            totalTokens: 12_000,
            sources: [
              { source: "skill_instructions", sourceName: "research", tokenCount: 12_000 },
            ],
          },
        }),
      ],
    })
    const { unmount } = view({ summary: withContext })
    expect(screen.getByText("Initial context")).toBeTruthy()
    unmount()

    view({ summary: summary() })
    expect(screen.queryByText("Initial context")).toBeNull()
  })
})

describe("SessionDetailPresentation — host actions", () => {
  it("always shows export and delete, but only shows reveal when it is available", () => {
    view()
    expect(screen.getByLabelText("Export this session")).toBeTruthy()
    expect(screen.getByLabelText("Delete this session")).toBeTruthy()
    expect(screen.queryByLabelText("Reveal in file manager")).toBeNull()
  })

  it("wires export, delete, and reveal to their callbacks", () => {
    const onExportSession = vi.fn()
    const onDeleteSession = vi.fn()
    const onRevealSource = vi.fn()
    view({ onExportSession, onDeleteSession, onRevealSource })

    fireEvent.click(screen.getByLabelText("Export this session"))
    fireEvent.click(screen.getByLabelText("Delete this session"))
    fireEvent.click(screen.getByLabelText("Reveal in file manager"))
    expect(onExportSession).toHaveBeenCalledOnce()
    expect(onDeleteSession).toHaveBeenCalledOnce()
    expect(onRevealSource).toHaveBeenCalledOnce()
  })

  it("renders the agent icon from the app renderer", () => {
    const renderAgentIcon = vi.fn(() => <span data-testid="agent-icon" />)
    view({ renderAgentIcon })
    expect(screen.getByTestId("agent-icon")).toBeTruthy()
    expect(renderAgentIcon).toHaveBeenCalledWith("claude-code", 20)
  })
})

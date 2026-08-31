import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { INITIAL_SESSION_HYGIENE } from "../../lib/presentation/sessionHygiene"
import {
  inclusiveCostSubject,
  type LocalSessionCost,
} from "../../lib/presentation/sessionCosts"
import type {
  ActiveSessionsSummary,
  SessionBucket,
  SessionMetrics,
} from "../../lib/types/session"
import { subagentsExpandedStore } from "./analysis/subagentsExpandedStore"
import {
  SessionDetailPresentation,
  type SessionDetailPresentationProps,
} from "./SessionDetailPresentation"

afterEach(cleanup)

// The Cost card's sub-agent roster now remembers its open/closed state in a
// module-level store, shared across every test in this file. Start each test
// from the same collapsed state so an earlier test's click cannot leak in.
beforeEach(() => {
  subagentsExpandedStore.set(false)
})

function bucket(over: Partial<SessionBucket> = {}): SessionBucket {
  return {
    tokensIn: 1000,
    tokensOut: 200,
    subagentTokens: 0,
    contextTokens: 40_000,
    isCompactionBoundary: false,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    rewriteTokens: 0,
    isCacheRehydration: false,
    isCacheRoutingMiss: false,
    secsSincePriorTurn: null,
    subagentLaunches: 0,
    userPrompts: 0,
    lastTool: null,
    model: null,
    thinkingMode: null,
    speed: null,
    hasThinking: false,
    compactionTrigger: null,
    compactionPreTokens: null,
    compactionPostTokens: null,
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
    buckets: [bucket(), bucket()],
    ...over,
  }
}

function summary(over: Partial<ActiveSessionsSummary> = {}): ActiveSessionsSummary {
  return {
    sessionCount: 1,
    avgDurationSecs: 3600,
    avgActiveSecs: 1800,
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
    hygiene: {
      badges: INITIAL_SESSION_HYGIENE.badges.map((badge) => ({
        ...badge,
        status: "clean",
        notAssessedReason: null,
      })),
      evidenceState: "ready",
    },
    error: false,
    onBack: () => {},
    session: {
      agent: "claude-code",
      sessionId: "session-1",
      title: "Fix the flaky test",
      wslDistro: null,
    },
    supportsAnalysis: true,
    analysisPending: false,
    cost: null,
    costSplit: null,
    efficiency: null,
    subagentCount: 0,
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
  })

  it("renders a distinct not-assessed hygiene pip with an accessible label", () => {
    view({
      hygiene: {
        badges: [
          {
            id: "sessionOverdepth",
            status: "notAssessed",
            notAssessedReason: "incompleteEvidence",
          },
          {
            id: "modelOverthinking",
            status: "clean",
            notAssessedReason: null,
          },
          {
            id: "overpoweredSubagents",
            status: "finding",
            notAssessedReason: null,
          },
          {
            id: "obsoleteModel",
            status: "clean",
            notAssessedReason: null,
          },
          {
            id: "fastModeOveruse",
            status: "clean",
            notAssessedReason: null,
          },
          {
            id: "excessCacheRehydration",
            status: "clean",
            notAssessedReason: null,
          },
        ],
        evidenceState: "ready",
      },
    })

    const pip = screen.getByLabelText("Session overdepth not assessed")
    expect(pip.textContent).toBe("?")
    expect(pip.className).toContain("border-dashed")
    expect(pip.className).toContain("text-label-tertiary")
    expect(screen.getByLabelText("No model overthinking detected").textContent).toBe("✓")
    expect(screen.getByLabelText("Overpowered subagents detected").textContent).toBe("×")
  })

  it("adds the routing-miss count from the session metrics to the Context hint", () => {
    view({
      cost: cost(),
      summary: summary({ sessions: [metrics({ cacheRoutingMissCount: 2 })] }),
    })
    expect(screen.getByText(/2 routing misses/)).toBeTruthy()
  })

  it("omits the Cost card when nothing priced the session", () => {
    view({ cost: null })
    expect(screen.getByText("Context")).toBeTruthy()
    expect(screen.queryByText("Cost")).toBeNull()
  })

  it("shows the Efficiency card under the Cost card with a definition", () => {
    view({
      cost: cost(),
      efficiency: {
        totalUsd: 10,
        newWorkUsd: 3.4,
        carryUsd: 5.4,
        rewriteUsd: 1.2,
        growthTokens: 200_000,
        outputTokens: 50_000,
        pricedTurns: 12,
        unpricedTurns: 3,
      },
    })
    expect(screen.getByText("Efficiency")).toBeTruthy()
    expect(screen.getByText("$/MTok")).toBeTruthy()
    expect(
      screen.getByText("Relative to real work: context growth and output tokens."),
    ).toBeTruthy()
  })

  it("omits the Efficiency card when the session is unpriced", () => {
    view({ cost: null, efficiency: null })
    expect(screen.queryByText("Efficiency")).toBeNull()
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
    const hygiene = screen.getByLabelText("Session hygiene checks")
    expect(hygiene.children).toHaveLength(6)
    expect(screen.getByLabelText("No session overdepth detected")).toBeTruthy()
    expect(screen.getByLabelText("No model overthinking detected")).toBeTruthy()
    expect(screen.getByLabelText("No overpowered subagents detected")).toBeTruthy()
    expect(screen.getByLabelText("No obsolete model detected")).toBeTruthy()
    expect(screen.getByLabelText("No fast mode overuse detected")).toBeTruthy()
    expect(screen.getByLabelText("No excess cache rehydration detected")).toBeTruthy()
  })

  it("names the back control for what it does, not for the view it leaves", () => {
    view()
    // The heading and the back control are separate elements.
    // The screen reader announces the view and the control correctly.
    expect(screen.getByRole("heading", { name: "Session Detail" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Back" })).toBeTruthy()
  })

  it("shows a header spinner while a newer analysis is on its way", () => {
    view({ refreshing: true })
    expect(screen.getByRole("status")).toHaveTextContent("Refreshing session analysis")
    expect(screen.getByRole("heading", { name: "Session Detail" })).toBeTruthy()
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
      expect(screen.queryByTestId("session-analysis-skeleton")).toBeNull()

      act(() => {
        vi.advanceTimersByTime(250)
      })
      expect(screen.getByTestId("session-analysis-skeleton")).toBeTruthy()

      // Once shown it holds for its minimum-visible window even after the
      // load finishes, so it cannot flicker.
      rerender(
        <SessionDetailPresentation
          {...presentationProps({ summary: summary(), loading: false })}
        />,
      )
      expect(screen.getByTestId("session-analysis-skeleton")).toBeTruthy()

      act(() => {
        vi.advanceTimersByTime(500)
      })
      expect(screen.queryByTestId("session-analysis-skeleton")).toBeNull()
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
      supportsAnalysis: false,
      session: { agent: "kiro", sessionId: "s1", wslDistro: null },
    })
    expect(screen.getByText(/Session analysis for Kiro sessions/)).toBeTruthy()
  })

  it("shows an indexing message while the drilldown is pending, not the empty-transcript copy", () => {
    view({ summary: null, analysisPending: true })
    expect(screen.getByText("Analyzing this session…")).toBeTruthy()
    expect(screen.queryByText("No session analysis available")).toBeNull()
    expect(
      screen.queryByText("This session has no analyzable messages in its local transcript."),
    ).toBeNull()
  })

  it("renders supported Pi analysis instead of the generic unsupported state", () => {
    view({
      session: { agent: "pi", sessionId: "pi-1", wslDistro: null },
      supportsAnalysis: true,
    })
    expect(screen.getByText("Context")).toBeTruthy()
    expect(screen.queryByText(/Session analysis for Pi sessions/)).toBeNull()
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

  it("shows no orchestrator banner, and opens a sub-agent from the Cost card instead", () => {
    const onOpenSubagent = vi.fn()
    const members = [
      {
        agent: "claude-code",
        subagentId: "a",
        label: "Investigate",
        cost: { totalUsd: 3, inputUsd: 1, outputUsd: 1, cacheReadUsd: 0.5, cacheWriteUsd: 0.5 },
        tokens: {
          inputTokens: 100,
          outputTokens: 50,
          cacheReadTokens: 0,
          cacheCreationTokens: 0,
        },
        startedAtEpoch: null,
        modelRuns: [{ model: "claude-sonnet-4-6" }],
      },
      {
        agent: "claude-code",
        subagentId: "b",
        label: "Write tests",
        cost: null,
        tokens: null,
        startedAtEpoch: null,
        modelRuns: [],
      },
    ]
    view({
      onOpenSubagent,
      cost: cost(41.45),
      costSplit: {
        parent: cost(32.95),
        subagents: cost(8.5),
        subagentCount: 2,
        members,
        sessionStartedAtEpoch: null,
      },
      subagentCount: 2,
    })

    expect(screen.queryByText(/Orchestrated \d+ agents/)).toBeNull()

    fireEvent.click(screen.getByText("2 sub-agents"))
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

  it("adds the Skills, MCPs and tools card when the session has initial context", () => {
    const withContext = summary({
      sessions: [
        metrics({
          initialContext: {
            sources: [
              {
                source: "skill_instructions",
                sourceName: "research",
                tokenCount: 12_000,
                useCount: 1,
              },
            ],
          },
        }),
      ],
    })
    const { unmount } = view({ summary: withContext })
    expect(screen.getByText("Skills, MCPs and tools")).toBeTruthy()
    unmount()

    view({ summary: summary() })
    expect(screen.queryByText("Skills, MCPs and tools")).toBeNull()
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

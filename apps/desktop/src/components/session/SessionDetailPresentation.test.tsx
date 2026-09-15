import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { SharedTooltipOwnerContext } from "../presentation/Tooltip"
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
    onDeleteSession: () => {},
    renderAgentIcon: () => null,
    ...over,
  }
}

function view(over: Partial<SessionDetailPresentationProps> = {}) {
  return render(<SessionDetailPresentation {...presentationProps(over)} />)
}

describe("SessionDetailPresentation — chrome", () => {
  it("renders the settled view: title, overview stats, and the tab strip", () => {
    view({ cost: cost() })
    expect(screen.getByText("Fix the flaky test")).toBeTruthy()
    expect(screen.getByText("In")).toBeTruthy()
    expect(screen.getByRole("tab", { name: /^Cost/ })).toBeTruthy()
  })

  it("renders only assessed hygiene checks", () => {
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

    fireEvent.click(screen.getByRole("tab", { name: /^Cost/ }))

    // Each assessed check shows its verdict and opens its explanation on focus.
    const hygiene = screen.getByLabelText("Session hygiene checks")
    expect(hygiene.children).toHaveLength(5)
    expect(screen.queryByRole("button", { name: "4/5 passed" })).toBeNull()
    fireEvent.focus(screen.getByRole("group", { name: "Overpowered subagents" }))
    expect(screen.queryByText(/Past about 200k tokens/)).toBeNull()
    expect(screen.getByRole("group", { name: "Overpowered subagents" })).toHaveAttribute(
      "tabindex",
      "0",
    )
    expect(screen.getByRole("group", { name: "Overpowered subagents" })).toHaveTextContent(
      "Failed",
    )

    // A check nobody could assess leaves the list rather than claiming a verdict.
    expect(screen.queryByRole("group", { name: "Session overdepth" })).toBeNull()
    expect(screen.getByRole("group", { name: "Model overthinking" })).toHaveTextContent(
      "Passed",
    )
  })

  it("keeps the Cost tab free of evidence-state chrome", () => {
    view({
      hygiene: {
        ...INITIAL_SESSION_HYGIENE,
        evidenceState: "stale",
      },
    })

    // The status bar carries the evidence state, so the tab does not repeat it.
    fireEvent.click(screen.getByRole("tab", { name: /^Cost/ }))
    expect(screen.queryByText("Burn checks")).toBeNull()
    expect(screen.queryByText("Refreshing")).toBeNull()
    expect(screen.queryByText("0/0")).toBeNull()
  })

  it("omits the Checks section when no check was assessed", () => {
    view({
      hygiene: {
        ...INITIAL_SESSION_HYGIENE,
        evidenceState: "ready",
      },
    })

    fireEvent.click(screen.getByRole("tab", { name: /^Cost/ }))
    expect(screen.queryByText("Checks")).toBeNull()
    expect(screen.queryByLabelText("Session hygiene checks")).toBeNull()
    expect(screen.queryByText(/not assessed/i)).toBeNull()
  })

  it("adds the provider-cache-miss count from the session metrics to the Context stats", () => {
    view({
      cost: cost(),
      summary: summary({ sessions: [metrics({ cacheRoutingMissCount: 2 })] }),
    })
    const cell = screen.getByText("Cache misses").closest("button")
    expect(cell).toHaveTextContent("2")
  })

  it("explains an unpriced session on the Cost tab instead of showing cost rows", () => {
    view({ cost: null })
    fireEvent.click(screen.getByRole("tab", { name: /^Cost/ }))
    expect(screen.getByText("No cost has been recorded for this session.")).toBeTruthy()
    expect(screen.queryByText("Input")).toBeNull()
  })

  it("splits the efficiency readings: composition under the chart, $/MTok on Cost", () => {
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
    // The composition sits under the chart it explains. The $/MTok scale
    // lives with the cost rows.
    expect(screen.getByText("Real Work %")).toBeTruthy()
    expect(screen.queryByText("$/MTOK")).toBeNull()
    fireEvent.click(screen.getByRole("tab", { name: /^Cost/ }))
    expect(screen.getByText("$/MTOK")).toBeTruthy()
    expect(screen.queryByText("Real Work %")).toBeNull()
    expect(screen.getByText("Efficiency")).toBeTruthy()
    expect(
      screen.queryByText("Cost for real work: context growth and output tokens."),
    ).toBeNull()
  })

  it("omits the efficiency readings when the session is unpriced", () => {
    view({ cost: null, efficiency: null })
    fireEvent.click(screen.getByRole("tab", { name: /^Cost/ }))
    expect(screen.queryByText("$/MTOK")).toBeNull()
  })

  it("keeps the session title on one line", () => {
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
    expect(title.className).toContain("truncate")
    expect(title.className).toContain("break-words")
    expect(title.style.getPropertyValue("--truncated-text-lines")).toBe("")
  })

  it("arranges the session facts in the hero and each figure at the head of its tab", () => {
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

    const hero = screen.getByLabelText("Session summary")
    expect(hero).toHaveTextContent("antiburn")
    expect(hero).toHaveTextContent("30m")
    expect(hero).toHaveTextContent("5.6-sol high")
    expect(hero).toHaveTextContent("11m ago")
    // Each figure heads its own tab, so the hero states the facts alone.
    expect(hero).not.toHaveTextContent("$2.40")
    expect(screen.getByRole("tab", { name: /^Cost/ })).not.toHaveTextContent("$2.40")

    fireEvent.click(screen.getByRole("tab", { name: /^Cost/ }))
    expect(screen.getByRole("tabpanel")).toHaveTextContent("Estimated cost")
    expect(screen.queryByRole("button", { name: "6/6 passed" })).toBeNull()
    expect(screen.getByRole("group", { name: "Session overdepth" })).toBeTruthy()
    expect(screen.getByRole("group", { name: "Model overthinking" })).toBeTruthy()
    expect(screen.getByRole("group", { name: "Overpowered subagents" })).toBeTruthy()
    expect(screen.getByRole("group", { name: "Obsolete model" })).toBeTruthy()
    expect(screen.getByRole("group", { name: "Fast mode overuse" })).toBeTruthy()
    expect(screen.getByRole("group", { name: "Excess cache rehydration" })).toBeTruthy()
  })

  it("names the back control for what it does, not for the view it leaves", () => {
    view()
    // The heading and the back control are separate elements.
    // The screen reader announces the view and the control correctly.
    expect(screen.getByRole("heading", { name: "Fix the flaky test" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Back" })).toBeTruthy()
  })

  it("shows a header spinner while a newer analysis is on its way", () => {
    view({ refreshing: true })
    expect(screen.getByRole("status")).toHaveTextContent("Refreshing session analysis")
    expect(screen.getByRole("heading", { name: "Fix the flaky test" })).toBeTruthy()
  })

  it("navigates back through the callback", () => {
    const onBack = vi.fn()
    view({ onBack })
    fireEvent.click(screen.getByRole("button", { name: "Back" }))
    expect(onBack).toHaveBeenCalledOnce()
  })

  it("wires the arrow keys to session traversal", () => {
    const onNext = vi.fn()
    view({ onNext })

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
    expect(screen.getByText("In")).toBeTruthy()
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

describe("SessionDetailPresentation — chart key", () => {
  it("draws the key under the chart it explains", () => {
    const { container } = view({})
    const panel = screen.getByRole("tabpanel")
    const chart = container.querySelector(".recharts-responsive-container")
    const key = screen.getByText("120.0k").closest("div")
    expect(chart).not.toBeNull()
    expect(key).not.toBeNull()
    // The key follows the plot in document order, so the reader meets the
    // shape first and the figures that name it second.
    expect(
      panel.compareDocumentPosition(chart!) & Node.DOCUMENT_POSITION_CONTAINED_BY,
    ).toBeTruthy()
    expect(chart!.compareDocumentPosition(key!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it("names the context area first, at its peak in tokens", () => {
    view({})
    expect(screen.getByText("90.0k")).toBeTruthy()
  })
})

describe("SessionDetailPresentation — session facts", () => {
  it("heads the Cost tab with the total and follows it with the breakdown", () => {
    view({
      cost: cost(),
    })
    const costTab = screen.getByRole("tab", { name: /^Cost/ })
    // The nav carries the label alone. The figure heads the panel behind it.
    expect(costTab).not.toHaveTextContent("$2.40")

    fireEvent.click(costTab)
    const panel = screen.getByRole("tabpanel")
    expect(panel).toHaveTextContent("Estimated cost")
    expect(screen.getByText("Input")).toBeTruthy()
    expect(screen.getByText("$2.40")).toBeTruthy()
  })

  it("marks a WSL session origin in the header", () => {
    view({
      session: { agent: "claude-code", sessionId: "s1", title: "T", wslDistro: "Ubuntu-24.04" },
    })
    expect(
      screen.getByLabelText("Found in Ubuntu-24.04 on Windows Subsystem for Linux"),
    ).toBeTruthy()
  })

  it("shows no orchestrator banner, and opens a sub-agent from the Cost tab instead", () => {
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

    fireEvent.click(screen.getByRole("tab", { name: /^Cost/ }))
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
    expect(screen.getByText("Autonomous sub-agent")).toBeTruthy()
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

  it("still renders the token stats and chart when context occupancy is unavailable", () => {
    expect(() => view({ summary: summary({ contextAvailable: false }) })).not.toThrow()
    expect(screen.getByText("In")).toBeTruthy()
  })

  it("shows Skills, MCPs and tools on the Tools tab when the session has initial context", () => {
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
    fireEvent.click(screen.getByRole("tab", { name: /^Tools/ }))
    expect(screen.getByText("research")).toBeTruthy()
    unmount()

    view({ summary: summary() })
    fireEvent.click(screen.getByRole("tab", { name: /^Tools/ }))
    expect(screen.queryByText("research")).toBeNull()
    expect(
      screen.getByText("No startup context has been recorded for this session."),
    ).toBeTruthy()
  })

  it("reddens the wasted-token figure only for a large share of a large context", () => {
    function wastedContext(unusedTokens: number, usedTokens: number) {
      return summary({
        sessions: [
          metrics({
            initialContext: {
              sources: [
                {
                  source: "skill_instructions",
                  sourceName: "idle",
                  tokenCount: unusedTokens,
                  useCount: 0,
                },
                {
                  source: "skill_instructions",
                  sourceName: "busy",
                  tokenCount: usedTokens,
                  useCount: 1,
                },
              ],
            },
          }),
        ],
      })
    }

    function figureClass() {
      fireEvent.click(screen.getByRole("tab", { name: /^Tools/ }))
      return screen.getByTestId("tools-wasted-figure").className
    }

    // Three quarters of the startup context, and far past the floor.
    const severe = view({ summary: wastedContext(30_000, 10_000) })
    expect(figureClass()).toContain("text-system-red-text")
    severe.unmount()

    // The same share of a context too small for the waste to matter.
    const small = view({ summary: wastedContext(3_000, 1_000) })
    expect(figureClass()).toContain("text-waste-warn")
    small.unmount()

    // Past the floor, but a small share of a large context.
    view({ summary: wastedContext(12_000, 200_000) })
    expect(figureClass()).toContain("text-waste-warn")
  })
})

describe("SessionDetailPresentation — presentation", () => {
  const efficiency = {
    totalUsd: 10,
    newWorkUsd: 3.4,
    carryUsd: 5.4,
    rewriteUsd: 1.2,
    growthTokens: 200_000,
    outputTokens: 50_000,
    pricedTurns: 12,
    unpricedTurns: 0,
  }

  function detailView(over: Partial<SessionDetailPresentationProps> = {}) {
    const props = presentationProps({ cost: cost(), efficiency, ...over })
    delete props.onBack
    return render(<SessionDetailPresentation {...props} />)
  }

  it("keeps the summary and host actions in the toolbar and floats the section picker over the content", () => {
    const onDeleteSession = vi.fn()
    const onRevealSource = vi.fn()
    const { container } = detailView({ onDeleteSession, onRevealSource, refreshing: true })

    expect(screen.queryByRole("button", { name: "Back" })).toBeNull()
    expect(screen.queryByText("Session Detail")).toBeNull()

    const toolbar = container.querySelector<HTMLElement>(".session-detail-toolbar")!
    expect(toolbar).toHaveClass("flex", "px-10")
    expect(within(toolbar).getByText("Fix the flaky test")).toBeTruthy()
    expect(within(toolbar).getByLabelText("Session summary")).toBeTruthy()
    fireEvent.click(within(toolbar).getByLabelText("Delete this session"))
    fireEvent.click(within(toolbar).getByLabelText("Reveal in file manager"))
    expect(onDeleteSession).toHaveBeenCalledTimes(1)
    expect(onRevealSource).toHaveBeenCalledTimes(1)
    expect(within(toolbar).getByRole("status")).toBeTruthy()

    // The picker sits over the bottom of the tab panel, not in the toolbar,
    // and the panel keeps room under its last row for it.
    const tablist = screen.getByRole("tablist", { name: "Session detail sections" })
    expect(within(toolbar).queryByRole("tablist")).toBeNull()
    const panel = screen.getByRole("tabpanel")
    expect(panel.parentElement).toBe(tablist.parentElement!.parentElement)
    expect(panel.parentElement).toHaveClass("relative")
    expect(panel).toHaveClass("pb-20")
    expect(tablist.parentElement).toHaveClass("absolute", "bottom-0", "justify-center")
    expect(tablist).toHaveClass("session-detail-floating-tabs", "pointer-events-auto")
  })

  it.each([
    { loading: true, error: false },
    { loading: false, error: true },
    { loading: false, error: false },
  ])("keeps the toolbar usable without analysis: %j", (state) => {
    const onBack = vi.fn()
    const onDeleteSession = vi.fn()
    view({ embedded: true, summary: null, onBack, onDeleteSession, ...state })
    expect(screen.getByRole("heading", { name: "Fix the flaky test" })).toBeTruthy()
    expect(screen.queryByRole("tablist")).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Back" }))
    fireEvent.click(screen.getByRole("button", { name: "Delete this session" }))
    expect(onBack).toHaveBeenCalledOnce()
    expect(onDeleteSession).toHaveBeenCalledOnce()
  })

  it("separates the toolbar and gives the tab bar its content width", () => {
    const { container } = detailView()
    expect(container.querySelector(".session-detail-toolbar")).toHaveClass("border-separator")
    expect(screen.getByRole("tablist", { name: "Session detail sections" })).toHaveClass(
      "inline-grid",
    )
  })

  it("places composition below the growing chart and its key", () => {
    const { container } = detailView()
    const key = screen.getByTestId("chart-key")
    expect(
      Array.from(screen.getByRole("tabpanel").querySelectorAll("h3")).map(
        (heading) => heading.textContent,
      ),
    ).toEqual(["Context over time", "Cost composition"])
    expect(key).toHaveClass("grid")
    expect(container.querySelector(".min-h-48")).toHaveClass("flex-1")
    expect(screen.getByTestId("efficiency-composition").dataset.height).toBe("bar")
    expect(screen.getByTestId("composition-legend")).toHaveClass("flex-col")
  })

  it("keeps the Cost tab's sections apart with spacing, not rules", () => {
    const { container } = detailView()
    fireEvent.click(screen.getByRole("tab", { name: /^Cost/ }))
    expect(screen.getByRole("heading", { name: "Checks" })).toHaveClass("sr-only")
    expect(screen.getByText("Efficiency")).toBeTruthy()
    // Spacing separates the cost, checks, and efficiency sections.
    const sections = Array.from(container.querySelectorAll("section"))
    expect(sections).toHaveLength(3)
    expect(sections.map((section) => section.querySelector("h3")?.textContent)).toEqual([
      "Cost",
      "Checks",
      "Efficiency",
    ])
    for (const section of sections) expect(section).not.toHaveClass("border-separator")
  })

  it("lays the Tools tab out in two columns", () => {
    detailView({
      summary: summary({
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
                {
                  source: "skill_instructions",
                  sourceName: "deploy",
                  tokenCount: 8_000,
                  useCount: 0,
                },
              ],
            },
          }),
        ],
      }),
    })
    fireEvent.click(screen.getByRole("tab", { name: /^Tools/ }))
    expect(screen.getByTestId("skills-mcp-list").dataset.columns).toBe("2")
  })
})

describe("SessionDetailPresentation — host actions", () => {
  it("always shows delete, but only shows reveal when it is available", () => {
    view()
    expect(screen.getByLabelText("Delete this session")).toBeTruthy()
    expect(screen.queryByLabelText("Reveal in file manager")).toBeNull()
  })

  it("shows reveal when onRevealSource is set", () => {
    view({ onRevealSource: () => {} })
    expect(screen.getByLabelText("Reveal in file manager")).toBeTruthy()
  })

  it("wires delete and reveal to their callbacks", () => {
    const onDeleteSession = vi.fn()
    const onRevealSource = vi.fn()
    view({ onDeleteSession, onRevealSource })

    fireEvent.click(screen.getByLabelText("Delete this session"))
    fireEvent.click(screen.getByLabelText("Reveal in file manager"))
    expect(onDeleteSession).toHaveBeenCalledOnce()
    expect(onRevealSource).toHaveBeenCalledOnce()
  })

  it("renders the agent icon from the app renderer in the sub-agent badge", () => {
    const renderAgentIcon = vi.fn(() => <span data-testid="agent-icon" />)
    view({
      renderAgentIcon,
      session: {
        agent: "claude-code",
        sessionId: "child-1",
        wslDistro: null,
        subagent: { parentTitle: "Ship the release" },
      },
    })
    fireEvent.click(screen.getByText("Autonomous sub-agent"))
    expect(screen.getByTestId("agent-icon")).toBeTruthy()
    expect(renderAgentIcon).toHaveBeenCalledWith("claude-code", 14)
  })
})

describe("SessionDetailPresentation — copy path", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  /** Click the copy control and let its clipboard promise settle. */
  async function clickCopy() {
    await act(async () => {
      fireEvent.click(screen.getByLabelText("Copy path"))
    })
  }

  it("hides copy when the session has no source path", () => {
    view()
    expect(screen.queryByLabelText("Copy path")).toBeNull()
  })

  it("shows copy beside reveal when a source path exists", () => {
    view({ onRevealSource: () => {}, onCopySourcePath: async () => {} })
    expect(screen.getByLabelText("Reveal in file manager")).toBeTruthy()
    expect(screen.getByLabelText("Copy path")).toBeTruthy()
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
  })

  it("shows a tick for two seconds after a successful copy, then the copy icon again", async () => {
    const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
    view({ onCopySourcePath })

    await clickCopy()
    expect(onCopySourcePath).toHaveBeenCalledOnce()
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()

    act(() => vi.advanceTimersByTime(1_999))
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()

    act(() => vi.advanceTimersByTime(1))
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
    expect(screen.getByLabelText("Copy path")).toBeTruthy()
  })

  it("shows no tick when the clipboard write fails", async () => {
    const onCopySourcePath = vi.fn().mockRejectedValue(new Error("denied"))
    view({ onCopySourcePath })

    await clickCopy()
    expect(onCopySourcePath).toHaveBeenCalledOnce()
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()

    // No timer was scheduled, so nothing can flip to success later.
    act(() => vi.advanceTimersByTime(5_000))
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
  })

  it("drops an earlier success tick when a repeated copy fails", async () => {
    const onCopySourcePath = vi
      .fn()
      .mockResolvedValueOnce(undefined)
      .mockRejectedValueOnce(new Error("denied"))
    view({ onCopySourcePath })

    await clickCopy()
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()

    await clickCopy()
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
    act(() => vi.advanceTimersByTime(5_000))
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
  })

  it("restarts the two-second window on a repeated successful copy", async () => {
    const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
    view({ onCopySourcePath })

    await clickCopy()
    act(() => vi.advanceTimersByTime(1_500))
    await clickCopy()
    expect(onCopySourcePath).toHaveBeenCalledTimes(2)

    // 1.5s after the second copy the restarted window is still open.
    act(() => vi.advanceTimersByTime(1_500))
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()

    act(() => vi.advanceTimersByTime(500))
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
  })

  it("ignores clicks while a copy is in flight", async () => {
    let resolveCopy: () => void = () => {}
    const onCopySourcePath = vi.fn(
      () => new Promise<void>((resolve) => (resolveCopy = resolve)),
    )
    view({ onCopySourcePath })

    fireEvent.click(screen.getByLabelText("Copy path"))
    fireEvent.click(screen.getByLabelText("Copy path"))
    expect(onCopySourcePath).toHaveBeenCalledOnce()

    await act(async () => resolveCopy())
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()
  })

  it("resets the tick immediately when the session changes", async () => {
    const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
    const { rerender } = view({ onCopySourcePath })

    await clickCopy()
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()

    rerender(
      <SessionDetailPresentation
        {...presentationProps({
          onCopySourcePath,
          session: {
            agent: "claude-code",
            sessionId: "session-2",
            title: "Another session",
            wslDistro: null,
          },
        })}
      />,
    )
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()

    // The old session's timer is gone and cannot fire against the new one.
    expect(vi.getTimerCount()).toBe(0)
    act(() => vi.advanceTimersByTime(5_000))
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
  })

  it("ignores a copy that settles after the session changes", async () => {
    let resolveCopy: () => void = () => {}
    const onCopySourcePath = vi.fn(
      () => new Promise<void>((resolve) => (resolveCopy = resolve)),
    )
    const { rerender } = view({ onCopySourcePath })

    fireEvent.click(screen.getByLabelText("Copy path"))
    rerender(
      <SessionDetailPresentation
        {...presentationProps({
          onCopySourcePath,
          session: {
            agent: "claude-code",
            sessionId: "session-2",
            title: "Another session",
            wslDistro: null,
          },
        })}
      />,
    )

    await act(async () => resolveCopy())
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
    expect(vi.getTimerCount()).toBe(0)
  })

  it.each(["session-2", "session-1"])(
    "isolates pending copies after navigating through session-2 to %s",
    async (sessionId) => {
      let resolveOldCopy: () => void = () => {}
      let resolveNewCopy: () => void = () => {}
      const oldCopy = vi.fn(() => new Promise<void>((resolve) => (resolveOldCopy = resolve)))
      const newCopy = vi.fn(() => new Promise<void>((resolve) => (resolveNewCopy = resolve)))
      const props = presentationProps({ onCopySourcePath: oldCopy })
      const { rerender } = render(<SessionDetailPresentation {...props} />)

      fireEvent.click(screen.getByLabelText("Copy path"))
      expect(oldCopy).toHaveBeenCalledOnce()
      rerender(
        <SessionDetailPresentation
          {...props}
          session={{ ...props.session, sessionId: "session-2" }}
          onCopySourcePath={newCopy}
        />,
      )
      if (sessionId === "session-1") {
        rerender(<SessionDetailPresentation {...props} onCopySourcePath={newCopy} />)
      }

      fireEvent.click(screen.getByLabelText("Copy path"))
      expect(newCopy).toHaveBeenCalledOnce()
      expect(screen.queryByTestId("copy-path-tick")).toBeNull()

      await act(async () => resolveOldCopy())
      expect(screen.queryByTestId("copy-path-tick")).toBeNull()
      expect(screen.getByRole("status")).toBeEmptyDOMElement()
      expect(vi.getTimerCount()).toBe(0)

      await act(async () => resolveNewCopy())
      expect(screen.getByTestId("copy-path-tick")).toBeTruthy()
      expect(screen.getByRole("status")).toHaveTextContent("Path copied")
    },
  )

  it("clears the pending tick timer on unmount", async () => {
    const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
    const { unmount } = view({ onCopySourcePath })

    await clickCopy()
    expect(vi.getTimerCount()).toBe(1)

    unmount()
    expect(vi.getTimerCount()).toBe(0)
  })
})

describe("SessionDetailPresentation — discussion copy", () => {
  const promptLabel = "Copy prompt to discuss session with agent"
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it.each(["altKey", "metaKey", "ctrlKey"])(
    "copies a prompt on %s click without prior hover",
    async (modifier) => {
      const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
      const onCopyDiscussionPrompt = vi.fn().mockResolvedValue(undefined)
      view({ onCopySourcePath, onCopyDiscussionPrompt })
      await act(async () =>
        fireEvent.click(screen.getByLabelText("Copy path"), { [modifier]: true }),
      )
      expect(onCopyDiscussionPrompt).toHaveBeenCalledOnce()
      expect(onCopySourcePath).not.toHaveBeenCalled()
      expect(screen.getByRole("status")).toHaveTextContent("Prompt copied")
      act(() => vi.advanceTimersByTime(1999))
      expect(screen.getByTestId("copy-path-tick")).toBeTruthy()
      act(() => vi.advanceTimersByTime(1))
      expect(screen.queryByTestId("copy-path-tick")).toBeNull()
    },
  )

  it("copies on macOS Control-context-menu alone and keeps the two-second tick", async () => {
    const platform = vi.spyOn(window.navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
      const onCopyDiscussionPrompt = vi.fn().mockResolvedValue(undefined)
      view({ onCopySourcePath, onCopyDiscussionPrompt })
      const button = screen.getByLabelText("Copy path")
      fireEvent.mouseDown(button, { ctrlKey: true, button: 0 })
      await act(async () => {
        expect(fireEvent.contextMenu(button, { ctrlKey: true, button: 2 })).toBe(false)
      })
      fireEvent.mouseUp(button, { ctrlKey: true, button: 0 })
      expect(onCopyDiscussionPrompt).toHaveBeenCalledOnce()
      expect(onCopySourcePath).not.toHaveBeenCalled()
      expect(screen.getByRole("status")).toHaveTextContent("Prompt copied")
      act(() => vi.advanceTimersByTime(1999))
      expect(screen.getByTestId("copy-path-tick")).toBeTruthy()
      act(() => vi.advanceTimersByTime(1))
      expect(screen.queryByTestId("copy-path-tick")).toBeNull()
    } finally {
      platform.mockRestore()
    }
  })

  it.each([
    { first: "contextmenu", settleBetween: false },
    { first: "contextmenu", settleBetween: true },
    { first: "click", settleBetween: false },
    { first: "click", settleBetween: true },
  ])("deduplicates one Control gesture: %j", async ({ first, settleBetween }) => {
    const platform = vi.spyOn(window.navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      let resolveCopy = () => {}
      const onCopyDiscussionPrompt = vi.fn(
        () =>
          new Promise<void>((resolve) => {
            resolveCopy = resolve
          }),
      )
      const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
      view({ onCopySourcePath, onCopyDiscussionPrompt })
      const button = screen.getByLabelText("Copy path")
      const contextMenu = () => {
        expect(fireEvent.contextMenu(button, { ctrlKey: true, button: 2 })).toBe(false)
      }
      fireEvent.mouseDown(button, { ctrlKey: true, button: 0 })
      if (first === "contextmenu") contextMenu()
      fireEvent.mouseUp(button, { ctrlKey: true, button: 0 })
      if (first === "click") fireEvent.click(button, { ctrlKey: true, detail: 1 })
      if (settleBetween) await act(async () => resolveCopy())
      if (first === "contextmenu") {
        // The user can release Control before the companion click arrives.
        fireEvent.click(button, { detail: 1 })
      } else contextMenu()
      expect(onCopyDiscussionPrompt).toHaveBeenCalledOnce()
      expect(onCopySourcePath).not.toHaveBeenCalled()
      if (!settleBetween) await act(async () => resolveCopy())

      // A new press starts a new gesture, even while the success tick remains visible.
      fireEvent.mouseDown(button, { ctrlKey: true, button: 0 })
      fireEvent.mouseUp(button, { ctrlKey: true, button: 0 })
      fireEvent.click(button, { ctrlKey: true, detail: 1 })
      expect(onCopyDiscussionPrompt).toHaveBeenCalledTimes(2)
      await act(async () => resolveCopy())
    } finally {
      platform.mockRestore()
    }
  })

  it.each(["control", "plain", "shift", "keyboard"])(
    "allows a new %s activation after an unpaired Control-context-menu",
    async (activation) => {
      const platform = vi
        .spyOn(window.navigator, "userAgent", "get")
        .mockReturnValue("Macintosh")
      try {
        const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
        const onCopyDiscussionPrompt = vi.fn().mockResolvedValue(undefined)
        view({ onCopySourcePath, onCopyDiscussionPrompt })
        const button = screen.getByLabelText("Copy path")
        fireEvent.mouseDown(button, { ctrlKey: true })
        await act(async () => fireEvent.contextMenu(button, { ctrlKey: true, button: 2 }))
        fireEvent.mouseUp(button, { ctrlKey: true })
        const modifiers = {
          ctrlKey: activation === "control",
          shiftKey: activation === "shift",
        }
        if (activation === "keyboard") fireEvent.keyDown(button, { key: "Enter" })
        else {
          fireEvent.mouseDown(button, modifiers)
          fireEvent.mouseUp(button, modifiers)
        }
        await act(async () =>
          fireEvent.click(button, { ...modifiers, detail: activation === "keyboard" ? 0 : 1 }),
        )
        expect(onCopyDiscussionPrompt).toHaveBeenCalledTimes(activation === "control" ? 2 : 1)
        expect(onCopySourcePath).toHaveBeenCalledTimes(activation === "control" ? 0 : 1)
      } finally {
        platform.mockRestore()
      }
    },
  )

  it.each([
    { platformName: "Macintosh", ctrlKey: false, shiftKey: false, promptAvailable: true },
    { platformName: "Macintosh", ctrlKey: false, shiftKey: true, promptAvailable: true },
    { platformName: "Macintosh", ctrlKey: true, shiftKey: false, promptAvailable: false },
    { platformName: "Windows NT", ctrlKey: true, shiftKey: false, promptAvailable: true },
    { platformName: "Linux", ctrlKey: true, shiftKey: false, promptAvailable: true },
  ])(
    "leaves other context menus alone: %j",
    ({ platformName, ctrlKey, shiftKey, promptAvailable }) => {
      const platform = vi
        .spyOn(window.navigator, "userAgent", "get")
        .mockReturnValue(platformName)
      try {
        const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
        const onCopyDiscussionPrompt = vi.fn().mockResolvedValue(undefined)
        view({ onCopySourcePath, ...(promptAvailable ? { onCopyDiscussionPrompt } : {}) })
        const button = screen.getByLabelText("Copy path")
        fireEvent.mouseDown(button, { button: 2, ctrlKey, shiftKey })
        expect(fireEvent.contextMenu(button, { button: 2, ctrlKey, shiftKey })).toBe(true)
        fireEvent.mouseUp(button, { button: 2, ctrlKey, shiftKey })
        expect(onCopySourcePath).not.toHaveBeenCalled()
        expect(onCopyDiscussionPrompt).not.toHaveBeenCalled()
        expect(screen.queryByTestId("copy-path-tick")).toBeNull()
      } finally {
        platform.mockRestore()
      }
    },
  )

  it("does not carry Control-context-menu suppression into another session", async () => {
    const platform = vi.spyOn(window.navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
      const onCopyDiscussionPrompt = vi.fn().mockResolvedValue(undefined)
      const props = presentationProps({ onCopySourcePath, onCopyDiscussionPrompt })
      const { rerender } = render(<SessionDetailPresentation {...props} />)
      fireEvent.mouseDown(screen.getByLabelText("Copy path"), { ctrlKey: true })
      await act(async () =>
        fireEvent.contextMenu(screen.getByLabelText("Copy path"), { ctrlKey: true }),
      )
      rerender(
        <SessionDetailPresentation
          {...props}
          session={{ ...props.session, sessionId: "session-2" }}
        />,
      )
      expect(screen.queryByTestId("copy-path-tick")).toBeNull()
      await act(async () => fireEvent.click(screen.getByLabelText("Copy path")))
      expect(onCopySourcePath).toHaveBeenCalledOnce()
      expect(onCopyDiscussionPrompt).toHaveBeenCalledOnce()
    } finally {
      platform.mockRestore()
    }
  })

  it.each([{}, { shiftKey: true }])(
    "keeps an unmodified or Shift-only click as a path copy: %j",
    async (modifiers) => {
      const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
      const onCopyDiscussionPrompt = vi.fn().mockResolvedValue(undefined)
      view({ onCopySourcePath, onCopyDiscussionPrompt })
      await act(async () => fireEvent.click(screen.getByLabelText("Copy path"), modifiers))
      expect(onCopySourcePath).toHaveBeenCalledOnce()
      expect(onCopyDiscussionPrompt).not.toHaveBeenCalled()
    },
  )

  it.each(["altKey", "metaKey", "ctrlKey"])(
    "shows the wand when entering the toolbar with %s held",
    (modifier) => {
      view({ onCopySourcePath: async () => {}, onCopyDiscussionPrompt: async () => {} })
      const toolbar = screen.getByLabelText("Copy path").closest(".session-detail-toolbar")!
      fireEvent.mouseEnter(toolbar, { [modifier]: true })
      expect(
        screen.getByLabelText(promptLabel).querySelector(".lucide-wand-sparkles"),
      ).toBeTruthy()
      fireEvent.mouseLeave(toolbar)
      expect(screen.getByLabelText("Copy path").querySelector(".lucide-copy")).toBeTruthy()
    },
  )

  it("updates on keyboard changes while hovered and focused, and clears on blur", () => {
    view({ onCopySourcePath: async () => {}, onCopyDiscussionPrompt: async () => {} })
    const button = screen.getByLabelText("Copy path")
    const toolbar = button.closest(".session-detail-toolbar")!
    fireEvent.mouseEnter(toolbar, { shiftKey: true })
    expect(button).toHaveAttribute("aria-label", "Copy path")
    fireEvent.keyDown(window, { key: "Alt", altKey: true })
    expect(button).toHaveAttribute("aria-label", promptLabel)
    fireEvent.keyDown(window, { key: "Control", altKey: true, ctrlKey: true })
    fireEvent.keyUp(window, { key: "Alt", ctrlKey: true })
    expect(button).toHaveAttribute("aria-label", promptLabel)
    fireEvent.keyUp(window, { key: "Control" })
    expect(button).toHaveAttribute("aria-label", "Copy path")
    fireEvent.mouseLeave(toolbar)
    fireEvent.keyDown(window, { key: "Control", ctrlKey: true })
    expect(button).toHaveAttribute("aria-label", "Copy path")
    act(() => button.focus())
    expect(button).toHaveAttribute("aria-label", promptLabel)
    fireEvent.keyUp(window, { key: "Control" })
    expect(button).toHaveAttribute("aria-label", "Copy path")
    fireEvent.keyDown(button, { key: "Meta", metaKey: true })
    expect(button).toHaveAttribute("aria-label", promptLabel)
    act(() => button.blur())
    expect(button).toHaveAttribute("aria-label", "Copy path")
    fireEvent.mouseEnter(toolbar, { altKey: true })
    fireEvent.blur(window)
    expect(button).toHaveAttribute("aria-label", "Copy path")
  })

  it("keeps the success timer across shared-tooltip label changes and resets after another copy", async () => {
    const register = vi.fn(() => () => {})
    const onCopyDiscussionPrompt = vi.fn().mockResolvedValue(undefined)
    render(
      <SharedTooltipOwnerContext.Provider value={{ register }}>
        <SessionDetailPresentation
          {...presentationProps({ onCopySourcePath: async () => {}, onCopyDiscussionPrompt })}
        />
      </SharedTooltipOwnerContext.Provider>,
    )
    const button = screen.getByLabelText("Copy path")
    fireEvent.mouseEnter(button.closest(".session-detail-toolbar")!, { altKey: true })
    expect(register).toHaveBeenCalledWith(
      button,
      expect.objectContaining({ label: promptLabel }),
    )
    await act(async () => fireEvent.click(button, { altKey: true }))
    act(() => vi.advanceTimersByTime(1500))
    fireEvent.keyUp(window, { key: "Alt" })
    fireEvent.keyDown(window, { key: "Control", ctrlKey: true })
    await act(async () => fireEvent.click(button, { ctrlKey: true }))
    act(() => vi.advanceTimersByTime(1999))
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()
    act(() => vi.advanceTimersByTime(1))
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
    expect(onCopyDiscussionPrompt).toHaveBeenCalledTimes(2)
  })

  it("clears success on prompt rejection and never adds a success timer", async () => {
    const onCopyDiscussionPrompt = vi.fn().mockRejectedValue(new Error("denied"))
    view({ onCopySourcePath: async () => {}, onCopyDiscussionPrompt })
    const button = screen.getByLabelText("Copy path")
    await act(async () => fireEvent.click(button))
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()
    await act(async () => fireEvent.click(button, { altKey: true }))
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
    expect(vi.getTimerCount()).toBe(0)
  })

  it("isolates pending prompt writes and listeners across navigation, deactivation, and unmount", async () => {
    const add = vi.spyOn(window, "addEventListener")
    const remove = vi.spyOn(window, "removeEventListener")
    let resolveOld = () => {}
    let resolveNew = () => {}
    const oldCopy = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveOld = resolve
        }),
    )
    const newCopy = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveNew = resolve
        }),
    )
    const onCopySourcePath = vi.fn().mockResolvedValue(undefined)
    const props = presentationProps({ onCopySourcePath, onCopyDiscussionPrompt: oldCopy })
    const { rerender, unmount } = render(<SessionDetailPresentation {...props} />)
    fireEvent.click(screen.getByLabelText("Copy path"), { metaKey: true })
    fireEvent.click(screen.getByLabelText("Copy path"))
    expect(oldCopy).toHaveBeenCalledOnce()
    expect(onCopySourcePath).not.toHaveBeenCalled()
    rerender(
      <SessionDetailPresentation
        {...props}
        session={{ ...props.session, sessionId: "session-2" }}
        onCopyDiscussionPrompt={newCopy}
      />,
    )
    fireEvent.click(screen.getByLabelText("Copy path"), { altKey: true })
    await act(async () => resolveOld())
    expect(screen.queryByTestId("copy-path-tick")).toBeNull()
    await act(async () => resolveNew())
    expect(screen.getByTestId("copy-path-tick")).toBeTruthy()
    rerender(<SessionDetailPresentation {...props} active={false} />)
    fireEvent.keyDown(window, { key: "Alt", altKey: true })
    expect(screen.getByLabelText("Copy path")).toBeTruthy()
    unmount()
    expect(vi.getTimerCount()).toBe(0)
    for (const [event, listener] of add.mock.calls.filter(([event]) => event === "keyup")) {
      expect(remove).toHaveBeenCalledWith(event, listener)
    }
    add.mockRestore()
    remove.mockRestore()
  })
})

describe("SessionDetailPresentation — embedded pane", () => {
  it("omits popover chrome and Back while retaining session content", () => {
    const { container } = view({ embedded: true, onBack: undefined })
    expect(screen.queryByRole("button", { name: "Back" })).toBeNull()
    expect(container.firstElementChild).not.toHaveClass("rounded-popover")
    expect(screen.queryByText("Session Detail")).toBeNull()
    expect(screen.getByRole("heading", { name: "Fix the flaky test" })).toBeTruthy()
  })

  it("limits adjacent navigation to the active detail pane", () => {
    const onNext = vi.fn()
    const props = presentationProps({ embedded: true, onNext })
    const { container, rerender } = render(<SessionDetailPresentation {...props} />)
    fireEvent.keyDown(window, { key: "ArrowRight" })
    expect(onNext).not.toHaveBeenCalled()
    fireEvent.keyDown(container.firstElementChild!, { key: "ArrowRight" })
    expect(onNext).toHaveBeenCalledOnce()
    rerender(<SessionDetailPresentation {...props} active={false} />)
    fireEvent.keyDown(container.firstElementChild!, { key: "ArrowRight" })
    expect(onNext).toHaveBeenCalledOnce()
  })
})

describe("SessionDetailPresentation — deferred keyboard entry", () => {
  it("accepts focus when a lazy detail replaces the focused loading pane", () => {
    const { container, rerender } = render(<section data-detail-pane tabIndex={-1} />)
    const pane = container.firstElementChild as HTMLElement
    pane.focus()
    rerender(
      <section data-detail-pane tabIndex={-1}>
        <SessionDetailPresentation {...presentationProps({ embedded: true })} />
      </section>,
    )
    expect(container.querySelector("[data-detail-focus-target]")).toHaveFocus()
  })
})

describe("SessionDetailPresentation — native drag toolbar", () => {
  it("only enables the embedded macOS toolbar, leaving controls interactive", () => {
    const agent = vi.spyOn(window.navigator, "userAgent", "get").mockReturnValue("Macintosh")
    try {
      const props = presentationProps({})
      const { container, rerender, unmount } = render(<SessionDetailPresentation {...props} />)
      expect(container.querySelector("[data-tauri-drag-region]")).toBeNull()
      rerender(<SessionDetailPresentation {...props} embedded />)
      const toolbar = screen
        .getByRole("heading", { name: "Fix the flaky test" })
        .closest("[data-tauri-drag-region]")
      expect(toolbar).toHaveAttribute("data-tauri-drag-region", "deep")
      expect(screen.getByRole("button", { name: "Delete this session" })).not.toHaveAttribute(
        "data-tauri-drag-region",
      )
      unmount()
      agent.mockReturnValue("Windows NT")
      const windows = render(<SessionDetailPresentation {...props} embedded />)
      expect(windows.container.querySelector("[data-tauri-drag-region]")).toBeNull()
    } finally {
      agent.mockRestore()
    }
  })
})

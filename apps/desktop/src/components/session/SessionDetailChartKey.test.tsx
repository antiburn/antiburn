import { cleanup, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { INITIAL_SESSION_HYGIENE } from "../../lib/presentation/sessionHygiene"
import type { ActiveSessionsSummary, SessionBucket } from "../../lib/types/session"
import {
  SessionDetailPresentation,
  type SessionDetailPresentationProps,
} from "./SessionDetailPresentation"

afterEach(cleanup)

// The real chart needs a measured layout that jsdom never supplies, so it
// draws nothing here. This stand-in reports the layer the key asked for, which
// is the one thing these tests are about. The rest and lit inks themselves are
// covered in the chart's own tests.
const highlights: Array<string | null> = []
vi.mock("./analysis/ContextTokensChart", () => ({
  ContextTokensChart: ({ highlight }: { highlight?: string | null }) => {
    highlights.push(highlight ?? null)
    return <div data-testid="chart" data-highlight={highlight ?? "none"} />
  },
}))

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
    postCompactionTokens: null,
    ...over,
  } as SessionBucket
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
    compactionCount: 3,
    buckets: [bucket(), bucket()],
    sessions: [
      {
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
      },
    ],
    ...over,
  } as ActiveSessionsSummary
}

function view(over: Partial<SessionDetailPresentationProps> = {}) {
  const props = {
    summary: summary(),
    loading: false,
    hygiene: {
      badges: INITIAL_SESSION_HYGIENE.badges.map((badge) => ({
        ...badge,
        status: "clean" as const,
        notAssessedReason: null,
      })),
      evidenceState: "ready" as const,
    },
    error: false,
    onBack: () => {},
    session: {
      agent: "claude-code" as const,
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
  } as SessionDetailPresentationProps
  return render(<SessionDetailPresentation {...props} />)
}

describe("the chart key lights one layer", () => {
  it("rests with no layer named", () => {
    view()
    expect(screen.getByTestId("chart").getAttribute("data-highlight")).toBe("none")
  })

  it("names the layer under the pointer, and drops it again on the way out", () => {
    view()
    const chart = () => screen.getByTestId("chart").getAttribute("data-highlight")

    fireEvent.mouseOver(screen.getByText("120.0k"))
    expect(chart()).toBe("in")

    fireEvent.mouseOut(screen.getByText("120.0k"))
    expect(chart()).toBe("none")

    fireEvent.mouseOver(screen.getByText("8.0k"))
    expect(chart()).toBe("out")
  })

  it("names the context area from its own key entry", () => {
    view()
    fireEvent.mouseOver(screen.getByText("90.0k"))
    expect(screen.getByTestId("chart").getAttribute("data-highlight")).toBe("context")
  })

  it("names the compaction marks from the compactions entry", () => {
    view()
    fireEvent.mouseOver(screen.getByText("3"))
    expect(screen.getByTestId("chart").getAttribute("data-highlight")).toBe("compaction")
  })

  it("pins a layer on click, and unpins it on the next click", () => {
    view()
    const chart = () => screen.getByTestId("chart").getAttribute("data-highlight")
    const chip = screen.getByText("120.0k").closest("button")!

    fireEvent.click(chip)
    fireEvent.mouseOut(chip)
    expect(chart()).toBe("in")
    expect(chip.getAttribute("aria-pressed")).toBe("true")

    // Another chip under the pointer wins while it is there.
    fireEvent.mouseOver(screen.getByText("8.0k"))
    expect(chart()).toBe("out")
    fireEvent.mouseOut(screen.getByText("8.0k"))
    expect(chart()).toBe("in")

    fireEvent.click(chip)
    fireEvent.mouseOut(chip)
    expect(chart()).toBe("none")
    expect(chip.getAttribute("aria-pressed")).toBe("false")
  })
})

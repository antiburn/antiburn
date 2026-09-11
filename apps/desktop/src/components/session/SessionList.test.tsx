import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type * as InsightsIpc from "../../lib/insightsIpc"
import type { LiveUsageSummaryPayload, LiveUsageWindowPayload } from "../../lib/ipc"
import { SessionList, type SessionListEntry, type SessionListProps } from "./SessionList"

const getSessionHygiene = vi.hoisted(() => vi.fn())

vi.mock("../../lib/insightsIpc", async (importOriginal) => ({
  ...(await importOriginal<typeof InsightsIpc>()),
  getSessionHygiene,
}))

beforeEach(() => {
  getSessionHygiene.mockReset()
  getSessionHygiene.mockResolvedValue(null)

  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    return this.classList.contains("ui-scroll-viewport") ? 356 : 340
  })
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockImplementation(function (
    this: HTMLElement,
  ) {
    if (this.classList.contains("ui-scroll-viewport")) return 320
    if (this.dataset.virtualKind === "heading") {
      return this.querySelector(".sr-only") ? 0 : 28
    }
    if (this.dataset.virtualKind === "row") {
      return this.textContent?.includes("Tall fixture") ? 120 : 72
    }
    return 0
  })
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockImplementation(function (
    this: Element,
  ) {
    const element = this as HTMLElement
    const viewport = element.classList.contains("ui-scroll-viewport")
      ? element
      : element.closest<HTMLElement>(".ui-scroll-viewport")
    const measuredItem = element.closest<HTMLElement>("[data-index]")
    const start = Number(measuredItem?.style.transform.match(/[-\d.]+/)?.[0] ?? 0)
    const top =
      viewport && !element.classList.contains("ui-scroll-viewport")
        ? start - viewport.scrollTop
        : 0
    const height = element.classList.contains("ui-scroll-viewport")
      ? element.offsetHeight
      : (measuredItem?.offsetHeight ?? element.offsetHeight)
    return {
      x: 0,
      y: top,
      top,
      left: 0,
      right: element.offsetWidth,
      bottom: top + height,
      width: element.offsetWidth,
      height,
      toJSON: () => ({}),
    }
  })
})

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.useRealTimers()
})

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

function entries(count: number): SessionListEntry[] {
  return Array.from({ length: count }, (_, index) =>
    entry({
      sessionId: `session-${index}`,
      title: `Fixture session ${index}`,
      timestamp: new Date(NOW.getTime() - index * 1_000).toISOString(),
    }),
  )
}

/** A live usage summary reporting one provider with one window. */
function liveUsageWithWindow(
  provider: string,
  displayName: string,
  window: Partial<LiveUsageWindowPayload> = {},
): LiveUsageSummaryPayload {
  return {
    generatedAt: NOW.toISOString(),
    errors: [],
    meters: [],
    providers: [
      {
        provider,
        accountKey: null,
        displayName,
        support: "live",
        freshness: "fresh",
        sourceLabel: "test",
        observedAt: NOW.toISOString(),
        windows: [
          {
            id: "five-hour",
            role: "primaryShort",
            kind: "rolling",
            scopeModel: null,
            usedPercent: 0,
            startsAt: null,
            resetsAt: null,
            hasNonzeroUsageInCurrentPeriod: false,
            forecast: {
              unavailableReason: "sparseHistory",
              confidence: null,
              consumptionRate: null,
              paceRatio: null,
              paceTrend: null,
              runwayAt: null,
              usedToday: null,
            },
            ...window,
          },
        ],
        extraUsage: null,
        resetCredits: null,
        plan: null,
        accountUuid: null,
        accountEmail: null,
      },
    ],
  }
}

describe("SessionList — rows", () => {
  it("shows the backend weekly allocation for the exact session identity", () => {
    list({
      badgeMetric: "weeklyPercent",
      sessionLimitAllocations: {
        generatedAt: NOW.toISOString(),
        allocations: [
          {
            agent: "claude-code",
            sessionId: "session-1",
            wslDistro: null,
            provider: "anthropic",
            displayName: "Claude",
            accountKey: "work",
            metric: "weekly",
            windowId: "weekly",
            percent: 12.345,
            confidence: "learned",
          },
        ],
      },
    })

    const badge = screen.getByText("12.3%")
    expect(badge.dataset.sessionLimitProvider).toBe("anthropic")
    expect(badge.dataset.sessionLimitWindow).toBe("weekly")
    expect(badge.dataset.sessionLimitPercent).toBe("12.3450")
    expect(badge).toHaveAttribute(
      "aria-label",
      "Estimated share of your Claude weekly limit. This session uses 5% or more of your limit.",
    )
  })

  it("shows a percent above 100 for a long session as is", () => {
    list({
      badgeMetric: "weeklyPercent",
      sessionLimitAllocations: {
        generatedAt: NOW.toISOString(),
        allocations: [
          {
            agent: "claude-code",
            sessionId: "session-1",
            wslDistro: null,
            provider: "anthropic",
            displayName: "Claude",
            accountKey: "work",
            metric: "weekly",
            windowId: "weekly",
            percent: 102.5,
            confidence: "seeded",
          },
        ],
      },
    })

    const badge = screen.getByText("102.5%")
    expect(badge).toHaveAttribute(
      "aria-label",
      "Estimated share of your Claude weekly limit. This session uses 5% or more of your limit.",
    )
  })

  it("shows the weekly limit as unknown when live usage has not loaded", async () => {
    list({
      entries: [
        entry({
          cost: { totalUsd: 1, figureLabel: "Estimated cost", models: ["gpt-5.6-sol"] },
        }),
      ],
      badgeMetric: "weeklyPercent",
    })

    expect(screen.queryByLabelText("Estimated cost $1.00")).toBeNull()
    const badge = screen.getByLabelText(
      "Share of your weekly limit is not known for this session.",
    )
    expect(badge).toHaveTextContent("unknown")
    fireEvent.focus(badge)
    await waitFor(() => {
      expect(screen.getByRole("tooltip")).toHaveTextContent(
        "Share of your weekly limit is not known for this session.",
      )
    })
  })

  it("does not fall back to cost when a controlled five-hour metric is unavailable", () => {
    list({
      entries: [
        entry({
          cost: { totalUsd: 1, figureLabel: "Estimated cost", models: ["gpt-5.6-sol"] },
        }),
      ],
      badgeMetric: "fiveHourPercent",
      onBadgeMetricChange: vi.fn(),
    })

    expect(screen.getByRole("radio", { name: "Cost" })).toHaveAttribute("aria-checked", "false")
    expect(screen.getByRole("radio", { name: "5h %" })).toHaveAttribute("aria-checked", "true")
    expect(screen.queryByLabelText("Estimated cost $1.00")).toBeNull()
    expect(
      screen.getByLabelText("Share of your 5-hour limit is not known for this session."),
    ).toHaveTextContent("unknown")
  })

  it("shows no limit when live usage confirms the provider reports none", () => {
    list({
      entries: [entry()],
      badgeMetric: "weeklyPercent",
      // The only reported window is a five-hour one, so a weekly badge can
      // affirmatively say Claude has no weekly limit for this session.
      liveUsage: liveUsageWithWindow("anthropic", "Claude", {
        id: "five-hour",
        kind: "rolling",
      }),
    })

    expect(screen.getByLabelText("No weekly limit for this session.")).toHaveTextContent(
      "no limit",
    )
  })

  it("shows the five-hour limit as unknown when a live window exists but this session has no share", () => {
    list({
      entries: [entry()],
      badgeMetric: "fiveHourPercent",
      liveUsage: liveUsageWithWindow("anthropic", "Claude", {
        id: "five-hour",
        kind: "rolling",
      }),
    })

    expect(
      screen.getByLabelText("Share of your 5-hour limit is not known for this session."),
    ).toHaveTextContent("unknown")
  })

  it("does not offer five-hour mode for another short rolling window", () => {
    list({
      entries: [entry()],
      onBadgeMetricChange: vi.fn(),
      liveUsage: {
        generatedAt: NOW.toISOString(),
        errors: [],
        meters: [],
        providers: [
          {
            provider: "openai",
            accountKey: null,
            displayName: "OpenAI",
            support: "live",
            freshness: "fresh",
            sourceLabel: "test",
            observedAt: NOW.toISOString(),
            windows: [
              {
                id: "burst-60m",
                role: "primaryShort",
                kind: "rolling",
                scopeModel: null,
                usedPercent: 20,
                startsAt: null,
                resetsAt: new Date(NOW.getTime() + 3_600_000).toISOString(),
                hasNonzeroUsageInCurrentPeriod: true,
                forecast: {
                  unavailableReason: "sparseHistory",
                  confidence: null,
                  consumptionRate: null,
                  paceRatio: null,
                  paceTrend: null,
                  runwayAt: null,
                  usedToday: null,
                },
              },
            ],
            extraUsage: null,
            resetCredits: null,
            plan: null,
            accountUuid: null,
            accountEmail: null,
          },
        ],
      },
    })

    expect(screen.queryByRole("radio", { name: "5h %" })).toBeNull()
  })

  it("offers five-hour mode when the usage panel shows an empty five-hour window", () => {
    list({
      onBadgeMetricChange: vi.fn(),
      liveUsage: {
        generatedAt: NOW.toISOString(),
        errors: [],
        meters: [],
        providers: [
          {
            provider: "anthropic",
            accountKey: null,
            displayName: "Claude",
            support: "live",
            freshness: "fresh",
            sourceLabel: "test",
            observedAt: NOW.toISOString(),
            windows: [
              {
                id: "five-hour",
                role: "primaryShort",
                kind: "rolling",
                scopeModel: null,
                usedPercent: 0,
                startsAt: null,
                resetsAt: null,
                hasNonzeroUsageInCurrentPeriod: false,
                forecast: {
                  unavailableReason: "sparseHistory",
                  confidence: null,
                  consumptionRate: null,
                  paceRatio: null,
                  paceTrend: null,
                  runwayAt: null,
                  usedToday: null,
                },
              },
            ],
            extraUsage: null,
            resetCredits: null,
            plan: null,
            accountUuid: null,
            accountEmail: null,
          },
        ],
      },
    })

    expect(screen.getByRole("radio", { name: "5h %" })).toBeInTheDocument()
  })

  it("keeps a five-hour allocation badge as time passes, since the factor never expires", () => {
    const props: SessionListProps = {
      entries: [entry()],
      days: 7,
      now: NOW,
      badgeMetric: "fiveHourPercent",
      onBadgeMetricChange: vi.fn(),
      sessionLimitAllocations: {
        generatedAt: NOW.toISOString(),
        allocations: [
          {
            agent: "claude-code",
            sessionId: "session-1",
            wslDistro: null,
            provider: "openai",
            displayName: "Codex",
            accountKey: null,
            metric: "fiveHour",
            windowId: "fiveHour",
            percent: 6.25,
            confidence: "learned",
          },
        ],
      },
    }
    const { rerender } = render(<SessionList {...props} />)

    expect(screen.getByRole("radio", { name: "5h %" })).toHaveAttribute("aria-checked", "true")
    expect(screen.getByText("6.3%")).toBeInTheDocument()

    rerender(<SessionList {...props} now={new Date(NOW.getTime() + 3_600_001)} />)
    expect(screen.getByRole("radio", { name: "5h %" })).toHaveAttribute("aria-checked", "true")
    expect(screen.getByRole("radio", { name: "Cost" })).toHaveAttribute("aria-checked", "false")
    expect(screen.getByText("6.3%")).toBeInTheDocument()
  })

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

  it("keeps the primary text on one line and prepares overflow for hover reveal", () => {
    list({ entries: [entry({ title: "A long session title" })] })
    const title = screen.getByText("A long session title")
    expect(title).toHaveClass("truncate")
    expect(title.closest("[data-scroll-on-hover]")).not.toBeNull()
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
    const renderAgentIcon = vi.fn(() => <span data-testid="agent-icon" />)
    list({
      entries: [
        entry({
          modelRuns: [
            { model: "gpt-5.6-sol", thinkingMode: "xhigh" },
            { model: "claude-fable-5", thinkingMode: "high" },
          ],
        }),
      ],
      renderAgentIcon,
    })
    const context = screen.getByLabelText(
      "Session source: Claude Code. Models: gpt-5.6-sol/xhigh, claude-fable-5/high.",
    )
    expect(context).toHaveTextContent(/5\.6-sol xhigh\s*\+1/)
    expect(context).not.toHaveTextContent("· +1")
    expect(context.querySelector("[data-additional-model-count]")).toHaveClass(
      "h-4",
      "min-w-4",
      "rounded-full",
      "bg-surface-tertiary/40",
      "font-mono",
      "type-metadata",
      "font-medium!",
      "tabular-nums",
      "text-label-tertiary",
    )
    expect(screen.getByText("5.6-sol").parentElement).toHaveClass(
      "shrink-0",
      "whitespace-nowrap",
    )
    expect(context).not.toHaveTextContent("fable-5 high")
    expect(context).not.toHaveTextContent("avery/widgets")
    expect(context).toHaveAttribute("data-shared-tooltip-trigger")

    fireEvent.focus(context)
    const tooltip = screen.getByRole("tooltip")
    const tooltipContent = tooltip.querySelector<HTMLElement>("[data-session-context-tooltip]")!
    expect(within(tooltipContent).getByText("Claude Code")).toHaveClass(
      "type-callout",
      "font-medium!",
      "text-label",
    )
    expect(within(tooltipContent).getByText("Models")).toHaveClass(
      "type-caption",
      "text-label-tertiary",
    )
    expect(within(tooltipContent).getByText("gpt-5.6-sol/xhigh")).toHaveClass(
      "font-mono",
      "type-footnote",
      "text-label",
    )
    expect(within(tooltipContent).queryByText("Repository")).toBeNull()
    expect(within(tooltipContent).queryByText("avery/widgets")).toBeNull()
    expect(within(tooltipContent).queryByText("Source")).toBeNull()
    expect(tooltipContent.querySelector('[data-testid="agent-icon"]')).toBeTruthy()
    expect(renderAgentIcon).toHaveBeenCalledWith("claude-code", 16, undefined, "neutral")
    expect(renderAgentIcon).toHaveBeenCalledWith("claude-code", 40, undefined, "neutral")
    expect(renderAgentIcon).not.toHaveBeenCalledWith("claude-code", 12, undefined, "neutral")
  })

  it("makes the title primary and moves vendor identity into a subtle watermark", () => {
    const renderAgentIcon = vi.fn(() => <span data-testid="agent-icon" />)
    list({
      entries: [entry({ title: "Supporting identity", modelRuns: [{ model: "gpt-5" }] })],
      renderAgentIcon,
    })

    const title = screen.getByText("Supporting identity")
    const titleContainer = title.closest("[data-scroll-on-hover]")
    const card = title.closest(".session-card")
    expect(titleContainer).toHaveClass("type-body", "font-medium!", "text-label")
    expect(card).toHaveClass(
      "gap-y-0.5",
      "py-3",
      "isolate",
      "overflow-hidden",
      "bg-session-card",
    )
    expect(card).not.toHaveClass("bg-surface-card/50")
    expect(card).not.toHaveClass("gap-y-1")
    expect(titleContainer).not.toHaveClass("type-body-large", "text-label-secondary")
    expect(screen.getByLabelText(/Burn Checks/)).toHaveClass("text-label-secondary")
    const watermark = screen
      .getByTestId("agent-icon")
      .closest("[data-session-vendor-watermark]")
    expect(watermark).toHaveClass(
      "pointer-events-none",
      "absolute",
      "-right-1.5",
      "-bottom-1.5",
      "h-10",
      "w-10",
      "session-vendor-watermark",
    )
    expect(watermark).toHaveAttribute("aria-hidden", "true")
    expect(
      screen
        .getByLabelText("Session source: Claude Code. Models: gpt-5.")
        .querySelector('[data-testid="agent-icon"]'),
    ).toBeNull()
    expect(screen.getByText("5")).toHaveClass("font-semibold!", "text-label-secondary")
    expect(screen.getByText("5")).not.toHaveClass("font-medium!")
    expect(screen.getByText("5")).not.toHaveClass("text-label")
    expect(
      screen.getByLabelText("Session source: Claude Code. Models: gpt-5."),
    ).toBeInTheDocument()
  })

  it("shows repositories only when they distinguish visible sessions", () => {
    const renderAgentIcon = () => <span data-testid="agent-icon" />
    const { rerender } = list({
      entries: [
        entry({ sessionId: "one", repo: "antiburn" }),
        entry({ sessionId: "two", repo: "antiburn" }),
      ],
      renderAgentIcon,
    })
    expect(screen.queryByText("antiburn")).toBeNull()

    rerender(
      <SessionList
        entries={[
          entry({
            sessionId: "one",
            repo: "antiburn",
            modelRuns: [{ model: "gpt-5.6-sol", thinkingMode: "medium" }],
          }),
          entry({
            sessionId: "two",
            repo: "another-repo",
            modelRuns: [{ model: "gpt-5.6-sol", thinkingMode: "medium" }],
          }),
        ]}
        days={7}
        now={NOW}
        renderAgentIcon={renderAgentIcon}
      />,
    )
    const firstRepository = screen.getByText("antiburn")
    const secondRepository = screen.getByText("another-repo")
    expect(firstRepository).toBeInTheDocument()
    expect(secondRepository).toBeInTheDocument()
    const firstMetadata = firstRepository.closest("[data-session-trailing-metadata]")
    const secondMetadata = secondRepository.closest("[data-session-trailing-metadata]")
    const firstContext = firstRepository.closest("[data-session-context-row]")
    expect(firstMetadata).toHaveClass(
      "font-mono",
      "type-metadata",
      "tabular-nums",
      "items-baseline",
      "gap-x-0.5",
      "ml-auto",
      "justify-end",
      "session-trailing-metadata",
    )
    expect(firstContext).toHaveClass("items-baseline", "gap-x-2")
    expect(secondMetadata).toHaveClass("font-mono", "type-metadata", "tabular-nums")
    expect(screen.getAllByText("medium")[0]).not.toHaveClass("type-caption")
    expect(within(firstMetadata as HTMLElement).getByText("·")).toBeInTheDocument()
    expect(firstContext?.textContent?.indexOf("5.6-sol")).toBeLessThan(
      firstContext?.textContent?.indexOf("antiburn") ?? -1,
    )
  })

  it("includes the relative-time suffix", () => {
    list({ entries: [entry({ timestamp: at(0, 9) })] })
    const time = screen.getByText(/ ago$/)
    expect(time).toHaveClass("shrink-0", "whitespace-nowrap")
    expect(time.closest("[data-session-trailing-metadata]")).toHaveClass(
      "font-mono",
      "type-metadata",
      "tabular-nums",
    )
  })

  it("compacts the timestamp beside a long repository name", () => {
    vi.useFakeTimers()
    vi.setSystemTime(NOW)
    list({
      entries: [
        entry({
          sessionId: "long-repo",
          repo: "what-does-this-do-basics",
          timestamp: at(0, 9),
        }),
        entry({ sessionId: "short-repo", repo: "antiburn", timestamp: at(0, 10) }),
      ],
    })

    const longMetadata = screen
      .getByText("what-does-this-do-basics")
      .closest("[data-session-trailing-metadata]") as HTMLElement
    const shortMetadata = screen
      .getByText("antiburn")
      .closest("[data-session-trailing-metadata]") as HTMLElement
    expect(within(longMetadata).getByText("3h")).toBeInTheDocument()
    expect(within(longMetadata).queryByText("3h ago")).toBeNull()
    expect(within(longMetadata).getByText("3h")).toHaveAttribute(
      "aria-label",
      "Last activity 3h ago",
    )
    expect(within(shortMetadata).getByText("2h ago")).toBeInTheDocument()
  })

  it("shows evidence computation instead of a synthetic hygiene verdict", () => {
    list({ entries: [entry({ sessionId: "session-pending" })] })
    const verdict = screen.getByLabelText(/Running Burn Checks/)
    expect(verdict).toHaveTextContent("Running Burn Checks…")
    expect(screen.queryByLabelText(/All Burn Checks passed/)).toBeNull()
  })

  it("renders finding and clean statuses returned by the batched IPC path", async () => {
    getSessionHygiene.mockResolvedValueOnce([
      {
        evidenceState: "ready",
        badges: [
          {
            id: "sessionOverdepth",
            status: "finding",
            notAssessedReason: null,
          },
          {
            id: "modelOverthinking",
            status: "clean",
            notAssessedReason: null,
          },
          {
            id: "overpoweredSubagents",
            status: "clean",
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
      },
    ])
    list({ entries: [entry({ sessionId: "synthetic-hygiene-result" })] })

    await waitFor(() => {
      expect(screen.getByLabelText(/Some Burn Checks failed/)).toHaveTextContent(
        "1 failed·5 passed",
      )
    })
    expect(getSessionHygiene).toHaveBeenCalledWith([
      {
        agent: "claude-code",
        sessionId: "synthetic-hygiene-result",
        wslDistro: null,
      },
    ])
  })

  it("keeps a cyan all-passed verdict above the session title", async () => {
    getSessionHygiene.mockResolvedValueOnce([
      {
        evidenceState: "ready",
        badges: [
          "sessionOverdepth",
          "modelOverthinking",
          "overpoweredSubagents",
          "obsoleteModel",
          "fastModeOveruse",
          "excessCacheRehydration",
        ].map((id) => ({ id, status: "clean", notAssessedReason: null })),
      },
    ])
    list({ entries: [entry({ sessionId: "clean-session", title: "Primary clean title" })] })

    const verdict = await screen.findByLabelText(/All Burn Checks passed/)
    expect(verdict).toHaveTextContent("All 6 passed")
    expect(screen.getByText("All 6 passed")).toHaveClass("text-burn-check-pass-fill")
    expect(
      screen.getByText("Primary clean title").closest("[data-session-status-bar]"),
    ).toBeNull()
  })

  it("keeps the last verdict on screen, marked stale, while a live session recomputes", async () => {
    getSessionHygiene.mockResolvedValueOnce([
      {
        evidenceState: "stale",
        badges: [
          { id: "sessionOverdepth", status: "finding", notAssessedReason: null },
          { id: "modelOverthinking", status: "clean", notAssessedReason: null },
          { id: "overpoweredSubagents", status: "clean", notAssessedReason: null },
          { id: "obsoleteModel", status: "clean", notAssessedReason: null },
          { id: "fastModeOveruse", status: "clean", notAssessedReason: null },
          { id: "excessCacheRehydration", status: "clean", notAssessedReason: null },
        ],
      },
    ])
    list({ entries: [entry({ sessionId: "synthetic-hygiene-stale" })] })

    await waitFor(() => {
      expect(screen.getByLabelText(/Refreshing/)).toHaveTextContent("1 failed·5 passed")
    })
    expect(screen.queryByText("Refreshing Burn Checks…")).toBeNull()
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

  it("renders a vendor watermark only from the injected renderer", () => {
    const renderAgentIcon = vi.fn(() => <span data-testid="agent-icon" />)
    list({ entries: [entry({ surface: "cli" })], renderAgentIcon })
    expect(renderAgentIcon).toHaveBeenCalledWith("claude-code", 40, "cli", "neutral")
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
    expect(screen.getByTestId("activity-pinned-group-label")).not.toHaveClass(
      "uppercase",
      "tracking-wide",
    )
    // The first heading is announced but not painted twice.
    expect(screen.getByText("Yesterday")).toBeTruthy()
    expect(screen.getByText("3 days ago")).toBeTruthy()
  })

  it("keeps the badge metric control inline with the pinned date label", () => {
    list({ entries: [entry()], onBadgeMetricChange: vi.fn() })
    const toolbar = document.querySelector<HTMLElement>("[data-list-display-toolbar]")!
    const control = screen.getByRole("radiogroup", { name: "Session metric" })

    expect(toolbar).toContainElement(control)
    expect(toolbar).toHaveTextContent("Today")
    expect(screen.queryByTestId("activity-pinned-group-label")).not.toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Sessions" })).not.toBeInTheDocument()
    expect(control).toHaveAttribute("data-variant", "text-tabs")
    expect(control).toHaveClass("inline-flex")
    expect(control).not.toHaveClass("bg-surface-secondary")
    expect(screen.getByRole("radio", { name: "Week %" })).toBeInTheDocument()
    expect(toolbar).not.toHaveTextContent("Show")
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

describe("SessionList — virtualization", () => {
  it("uses a 12px gap between cards in the same date group", async () => {
    const { container } = list({
      entries: [
        entry({ sessionId: "first", title: "First fixture", timestamp: at(0, 11) }),
        entry({ sessionId: "second", title: "Second fixture", timestamp: at(0, 10) }),
      ],
    })

    await waitFor(() => {
      const rows = [...container.querySelectorAll<HTMLElement>('[data-virtual-kind="row"]')]
      expect(rows).toHaveLength(2)
      expect(rows[0]).not.toHaveClass("pt-3")
      expect(rows[1]).toHaveClass("pt-3")
    })
  })

  it.each([225, 500])("bounds mounted rows for %i sessions", async (count) => {
    list({ entries: entries(count), onOpenSession: vi.fn() })

    await waitFor(() => {
      const mountedRows = screen.getAllByRole("button")
      expect(mountedRows.length).toBeGreaterThan(1)
      expect(mountedRows.length).toBeLessThan(20)
    })
    expect(screen.getByText("Fixture session 0")).toBeTruthy()
    expect(screen.queryByText(`Fixture session ${count - 1}`)).toBeNull()
  })

  it("measures variable row heights without overlap", async () => {
    const { container } = list({
      entries: [
        entry({ sessionId: "normal", title: "Normal fixture", timestamp: at(0, 11) }),
        entry({ sessionId: "tall", title: "Tall fixture", timestamp: at(0, 10) }),
        entry({ sessionId: "after", title: "After fixture", timestamp: at(0, 9) }),
      ],
    })

    await waitFor(() => {
      const rows = [...container.querySelectorAll<HTMLElement>('[data-virtual-kind="row"]')]
      expect(rows).toHaveLength(3)
      const tall = rows.find((row) => row.textContent?.includes("Tall fixture"))!
      const after = rows.find((row) => row.textContent?.includes("After fixture"))!
      const tallStart = Number(tall.style.transform.match(/[-\d.]+/)?.[0])
      const afterStart = Number(after.style.transform.match(/[-\d.]+/)?.[0])
      expect(afterStart).toBeGreaterThanOrEqual(tallStart + tall.offsetHeight)
    })
  })

  it("mounts the final row after scrolling to the end", async () => {
    const { container } = list({
      entries: entries(225),
      onOpenSession: vi.fn(),
      badgeMetric: "weeklyPercent",
      sessionLimitAllocations: {
        generatedAt: NOW.toISOString(),
        allocations: [
          {
            agent: "claude-code",
            sessionId: "session-224",
            wslDistro: null,
            provider: "anthropic",
            displayName: "Claude",
            accountKey: null,
            metric: "weekly",
            windowId: "weekly",
            percent: 7.25,
            confidence: "learned",
          },
        ],
      },
    })
    const viewport = container.querySelector<HTMLElement>(".ui-scroll-viewport")!

    viewport.scrollTop = 20_000
    fireEvent.scroll(viewport)

    await waitFor(() => {
      expect(screen.getByText("Fixture session 224")).toBeTruthy()
      expect(screen.getByText("7.3%")).toBeTruthy()
    })
    expect(screen.queryByText("Fixture session 0")).toBeNull()
    expect(screen.getAllByRole("button").length).toBeLessThan(20)
  })

  it("keeps the focused row mounted while the list scrolls", async () => {
    const { container } = list({ entries: entries(225), onOpenSession: vi.fn() })
    const firstRow = screen.getByRole("button", { name: /Fixture session 0/ })
    const viewport = container.querySelector<HTMLElement>(".ui-scroll-viewport")!

    firstRow.focus()
    viewport.scrollTop = 20_000
    fireEvent.scroll(viewport)

    await waitFor(() => {
      expect(screen.getByText("Fixture session 224")).toBeTruthy()
    })
    expect(firstRow).toBe(document.activeElement)
    expect(screen.getAllByRole("button").length).toBeLessThan(20)
  })

  it("moves Tab focus into the next virtual range", async () => {
    const { container } = list({ entries: entries(225), onOpenSession: vi.fn() })
    const viewport = container.querySelector<HTMLElement>(".ui-scroll-viewport")!
    Object.defineProperty(viewport, "scrollTo", {
      value: ({ top }: ScrollToOptions) => {
        viewport.scrollTop = top ?? 0
        fireEvent.scroll(viewport)
      },
    })
    const mountedRows = screen.getAllByRole("button")
    const lastMounted = mountedRows.at(-1)!
    const previousIndex = Number(
      lastMounted.closest<HTMLElement>("[data-index]")?.dataset.index,
    )
    expect(
      viewport.querySelector(`[data-virtual-kind="row"][data-index="${previousIndex + 1}"]`),
    ).toBeNull()
    lastMounted.focus()

    expect(fireEvent.keyDown(lastMounted, { key: "Tab" })).toBe(false)

    await waitFor(() => {
      const focusedRow = document.activeElement?.closest<HTMLElement>("[data-index]")
      expect(Number(focusedRow?.dataset.index)).toBeGreaterThan(previousIndex)
    })
  })

  it("updates the pinned label from measured group headings", async () => {
    const { container } = list({
      entries: [
        entry({ sessionId: "today", title: "Today fixture", timestamp: at(0) }),
        entry({ sessionId: "yesterday", title: "Yesterday fixture", timestamp: at(1) }),
      ],
    })
    const viewport = container.querySelector<HTMLElement>(".ui-scroll-viewport")!
    const yesterdayHeading = screen.getByRole("heading", { name: "Yesterday" })
    const headingItem = yesterdayHeading.closest<HTMLElement>("[data-index]")!

    viewport.scrollTop = Number(headingItem.style.transform.match(/[-\d.]+/)?.[0])
    fireEvent.scroll(viewport)

    await waitFor(() => {
      expect(screen.getByTestId("activity-pinned-group-label").textContent).toBe("Yesterday")
    })
  })

  it("preserves the viewport callback ref cleanup", () => {
    const cleanupRef = vi.fn()
    const viewportRef = vi.fn((node: HTMLDivElement | null) => (node ? cleanupRef : undefined))
    const { unmount } = list({ viewportRef })

    expect(viewportRef).toHaveBeenCalledWith(expect.any(HTMLDivElement))
    unmount()
    expect(cleanupRef).toHaveBeenCalledOnce()
  })
})

describe("SessionList — shared tooltips", () => {
  it("mounts one owner for every populated list size and none for an empty list", async () => {
    const { container, rerender } = list({ entries: entries(500) })

    await waitFor(() =>
      expect(container.querySelectorAll("[data-session-tooltip-owner]")).toHaveLength(1),
    )
    expect(container.querySelectorAll("[data-shared-tooltip-trigger]").length).toBeGreaterThan(
      1,
    )

    rerender(<SessionList entries={[]} days={7} now={NOW} />)
    expect(container.querySelectorAll("[data-session-tooltip-owner]")).toHaveLength(0)
  })

  it("shares rich status and cost content with fork, repository, and WSL labels", async () => {
    getSessionHygiene.mockResolvedValueOnce([
      {
        evidenceState: "ready",
        badges: [
          { id: "sessionOverdepth", status: "finding", notAssessedReason: null },
          { id: "modelOverthinking", status: "clean", notAssessedReason: null },
          { id: "overpoweredSubagents", status: "clean", notAssessedReason: null },
          { id: "obsoleteModel", status: "clean", notAssessedReason: null },
          { id: "fastModeOveruse", status: "clean", notAssessedReason: null },
          { id: "excessCacheRehydration", status: "clean", notAssessedReason: null },
        ],
      },
    ])
    const { container } = list({
      entries: [
        entry({
          additionalRepos: ["avery/docs"],
          hasForkParent: true,
          forkChildCount: 2,
          wslDistro: "Ubuntu-24.04",
          cost: {
            totalUsd: 2.4,
            figureLabel: "Estimated cost",
            models: ["claude-fable-5"],
            breakdownRows: [{ label: "Output", usd: 1.25 }],
          },
        }),
      ],
    })
    const status = await screen.findByLabelText(/Some Burn Checks failed/)
    const cost = screen.getByLabelText("Estimated cost $2.40")
    const fork = screen.getByLabelText("Forked from another session")
    const repository = screen.getByText("avery/widgets +1")
    const wsl = screen.getByLabelText("Found in Ubuntu-24.04 on Windows Subsystem for Linux")
    const row = status.closest(".group")!

    vi.useFakeTimers()

    fireEvent.pointerOver(status)
    act(() => vi.advanceTimersByTime(149))
    expect(document.querySelector(".ui-tooltip")).toBeNull()
    act(() => vi.advanceTimersByTime(1))
    expect(document.querySelector(".ui-tooltip")?.textContent).toContain("Burn Checks")
    expect(status.dataset.state).toBe("delayed-open")
    expect(status.getAttribute("aria-describedby")).toBe(
      document.querySelector(".ui-tooltip")?.id,
    )
    expect(row.querySelector('[data-state="delayed-open"]')).toBe(status)

    fireEvent.pointerOut(status)
    fireEvent.pointerOver(cost)
    act(() => vi.advanceTimersByTime(150))
    const costTooltip = document.querySelector<HTMLElement>(".ui-tooltip")!
    expect(costTooltip.dataset.side).toBe("bottom")
    expect(costTooltip.textContent).toContain("claude-fable-5")
    expect(costTooltip.textContent).toContain("Output")

    fireEvent.pointerOut(cost)
    fireEvent.pointerOver(fork)
    act(() => vi.advanceTimersByTime(500))
    expect(document.querySelector(".ui-tooltip")?.textContent).toBe(
      "Forked from another session",
    )

    fireEvent.pointerOut(fork)
    fireEvent.pointerOver(repository)
    act(() => vi.advanceTimersByTime(600))
    expect(document.querySelector(".ui-tooltip")).toBeNull()
    expect(repository).not.toHaveAttribute("data-shared-tooltip-trigger")

    fireEvent.pointerOver(wsl)
    act(() => vi.advanceTimersByTime(600))
    expect(document.querySelector(".ui-tooltip")?.textContent).toBe(
      "Found in Ubuntu-24.04 on Windows Subsystem for Linux",
    )
    expect(container.querySelectorAll("[data-session-tooltip-owner]")).toHaveLength(1)
  })

  it("dismisses and clears superseded timers without leaving stale trigger state", () => {
    const { container, unmount } = list({
      entries: [entry({ hasForkParent: true, repo: "avery/widgets", wslDistro: "Ubuntu" })],
    })
    const status = screen.getByLabelText(/Running Burn Checks/)
    const fork = screen.getByLabelText("Forked from another session")
    const repository = screen.getByText("avery/widgets")
    const wsl = screen.getByLabelText("Found in Ubuntu on Windows Subsystem for Linux")
    const viewport = container.querySelector<HTMLElement>(".ui-scroll-viewport")!
    vi.useFakeTimers()

    fireEvent.pointerOver(fork)
    act(() => vi.advanceTimersByTime(499))
    fireEvent.pointerOut(fork, { relatedTarget: repository })
    fireEvent.pointerOver(repository, { relatedTarget: fork })
    act(() => vi.advanceTimersByTime(101))
    expect(document.querySelector(".ui-tooltip")).toBeNull()
    expect(fork.dataset.state).toBe("closed")
    fireEvent.scroll(viewport)
    act(() => vi.advanceTimersByTime(499))
    expect(document.querySelector(".ui-tooltip")).toBeNull()

    fireEvent.scroll(viewport)
    expect(document.querySelector(".ui-tooltip")).toBeNull()
    expect(repository).not.toHaveAttribute("data-state")

    fireEvent.focus(status)
    expect(document.querySelector(".ui-tooltip")?.textContent).toContain("Not assessed · 6")
    expect(document.querySelector(".ui-tooltip")?.textContent).toContain("Session depth")
    fireEvent.blur(status)
    expect(document.querySelector(".ui-tooltip")).toBeNull()

    fireEvent.pointerOver(wsl)
    act(() => vi.advanceTimersByTime(600))
    fireEvent.keyDown(document, { key: "Escape" })
    expect(document.querySelector(".ui-tooltip")).toBeNull()
    expect(wsl.dataset.state).toBe("closed")

    fireEvent.pointerOver(repository)
    unmount()
    act(() => vi.runAllTimers())
    expect(document.querySelector(".ui-tooltip")).toBeNull()
  })

  it("does not open a tooltip from a touch pointer", () => {
    list({ entries: [entry({ hasForkParent: true })] })
    const fork = screen.getByLabelText("Forked from another session")
    vi.useFakeTimers()

    fireEvent.pointerOver(fork, { pointerType: "touch" })
    act(() => vi.runAllTimers())

    expect(document.querySelector(".ui-tooltip")).toBeNull()
    expect(fork.dataset.state).toBe("closed")
  })

  it("keeps an open rich tooltip through a list rerender", () => {
    const props: SessionListProps = {
      entries: [
        entry({
          cost: {
            totalUsd: 2.4,
            figureLabel: "Estimated cost",
            models: ["claude-fable-5"],
          },
        }),
      ],
      days: 7,
      now: NOW,
    }
    const { rerender } = render(<SessionList {...props} />)
    const cost = screen.getByLabelText("Estimated cost $2.40")

    fireEvent.focus(cost)
    expect(document.querySelector(".ui-tooltip")?.textContent).toContain("claude-fable-5")
    rerender(<SessionList {...props} />)

    expect(document.querySelector(".ui-tooltip")?.textContent).toContain("claude-fable-5")
    expect(cost.dataset.state).toBe("instant-open")
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

describe("SessionList — controlled main-window selection", () => {
  it("selects without invoking popover navigation and focuses detail on Enter", () => {
    const onSelect = vi.fn()
    const onOpenDetail = vi.fn()
    const onOpenSession = vi.fn()
    list({ entries: entries(3), selectedKey: null, onSelect, onOpenDetail, onOpenSession })
    const first = screen.getByRole("button", { name: /Fixture session 0/ })
    fireEvent.click(first)
    expect(first).toHaveFocus()
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ sessionId: "session-0" }))
    expect(onOpenSession).not.toHaveBeenCalled()
    fireEvent.keyDown(first, { key: "Enter" })
    expect(onOpenDetail).toHaveBeenCalledWith(
      expect.objectContaining({ sessionId: "session-0" }),
    )
  })

  it("moves selection across virtualized rows and lets Tab leave the collection", async () => {
    const onSelect = vi.fn()
    list({ entries: entries(225), selectedKey: null, onSelect })
    const first = screen.getByRole("button", { name: /Fixture session 0/ })
    first.focus()
    fireEvent.keyDown(first, { key: "End" })
    expect(onSelect).toHaveBeenLastCalledWith(
      expect.objectContaining({ sessionId: "session-224" }),
    )
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /Fixture session 224/ })).toHaveFocus(),
    )
    fireEvent.keyDown(document.activeElement!, { key: "Home" })
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /Fixture session 0/ })).toHaveFocus(),
    )
    expect(fireEvent.keyDown(document.activeElement!, { key: "Tab" })).toBe(true)
  })

  it("pauses hygiene work and active row motion while hidden", () => {
    const { container } = list({
      entries: [entry({ isActive: true })],
      active: false,
      onSelect: vi.fn(),
    })
    expect(getSessionHygiene).not.toHaveBeenCalled()
    expect(container.querySelector(".activity-row-active")).toBeNull()
  })
})

describe("SessionList — virtualized tab entry", () => {
  it("retains one row tab stop after scrolling away while focus is elsewhere", async () => {
    const { container } = list({ entries: entries(225), selectedKey: null, onSelect: vi.fn() })
    const viewport = container.querySelector<HTMLElement>(".ui-scroll-viewport")!
    viewport.scrollTop = 20_000
    fireEvent.scroll(viewport)
    await waitFor(() => expect(screen.getByText("Fixture session 224")).toBeTruthy())
    expect(screen.getByRole("button", { name: /Fixture session 0/ })).toHaveAttribute(
      "tabindex",
      "0",
    )
    expect(container.querySelectorAll('[data-session-row][tabindex="0"]')).toHaveLength(1)
    expect(container.querySelectorAll("[data-session-row]").length).toBeLessThan(20)
  })
})

describe("SessionList — native drag header", () => {
  it("opts in without marking session rows or metric buttons draggable", () => {
    const props = {
      entries: [entry({ isActive: true })],
      days: 7,
      onBadgeMetricChange: vi.fn(),
    }
    const { container, rerender } = render(<SessionList {...props} />)
    expect(container.querySelector("[data-tauri-drag-region]")).toBeNull()
    rerender(<SessionList {...props} draggableHeader />)
    expect(container.querySelector("[data-list-display-toolbar]")).toHaveAttribute(
      "data-tauri-drag-region",
      "deep",
    )
    for (const control of container.querySelectorAll("button, [role=option]")) {
      expect(control).not.toHaveAttribute("data-tauri-drag-region")
    }
  })
})

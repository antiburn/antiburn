import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { PopoverView } from "./PopoverView"

/**
 * The popover's flow, driven entirely through the mocked command layer.
 *
 * These are the tests that would catch a rename on either side of the IPC
 * boundary: every assertion names a command and the arguments it is called
 * with, so a shell-side signature change fails here rather than at runtime.
 */

const invoke = vi.hoisted(() => vi.fn())
const confirmDialog = vi.hoisted(() => vi.fn())
const saveDialog = vi.hoisted(() => vi.fn())
const openDialog = vi.hoisted(() => vi.fn())
/** Shell event handlers the view subscribed to, by event name. */
const listeners = vi.hoisted(() => new Map<string, ((event: { payload: unknown }) => void)[]>())

vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, handler: (event: { payload: unknown }) => void) => {
    listeners.set(name, [...(listeners.get(name) ?? []), handler])
    return () => {
      listeners.set(
        name,
        (listeners.get(name) ?? []).filter((each) => each !== handler),
      )
    }
  }),
}))
vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: confirmDialog,
  save: saveDialog,
  open: openDialog,
}))

const platform = vi.hoisted(() => ({ mac: false }))
vi.mock("../lib/platform", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  return { ...actual, isMacOS: () => platform.mac }
})

const hudPreference = vi.hoisted(() => ({
  enabled: false,
  overlayVisible: false,
  popoverVisible: false,
}))
const overlayVisibilityRead = vi.hoisted(() => vi.fn())
vi.mock("../lib/overlayWindow", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  return {
    ...actual,
    isCurrentWindowVisible: async () => hudPreference.popoverVisible,
    isFloatingHudEnabled: () => hudPreference.enabled,
    isOverlayWindowVisible: overlayVisibilityRead,
  }
})

/** Push a shell event at whatever subscribed to it. */
function emit(name: string, payload: unknown) {
  act(() => (listeners.get(name) ?? []).forEach((handler) => handler({ payload })))
}

const SETTINGS = {
  theme: "system" as const,
  activityWindowDays: 7,
  sessionDataRetentionDays: -1,
  onboardingCompleted: true,
  launchAtLogin: false,
  autoUpdate: true,
  discoveryPaused: false,
  // The collapsed state lets each test open Usage with one provider-pill click.
  // `UsageLimitsBar.test.tsx` covers the expanded state.
  overviewLimitsExpanded: false,
}

const SCAN_STATUS = {
  running: false,
  completedAgents: 11,
  totalAgents: 11,
  sessions: 4,
  finishedAt: new Date(Date.now() - 120_000).toISOString(),
  cancelled: false,
  error: null,
  agents: [],
}

function activityEntry(overrides: Record<string, unknown> = {}) {
  return {
    agent: "claude-code",
    sessionId: "session-abc-123",
    repo: "widgets",
    timestamp: new Date(Date.now() - 60_000).toISOString(),
    isActive: false,
    surface: "cli",
    wslDistro: null,
    title: "Wire the tray popover",
    hasForkParent: false,
    forkChildCount: 0,
    cost: {
      totalUsd: 1.25,
      inputUsd: 0.5,
      outputUsd: 0.5,
      cacheReadUsd: 0.15,
      cacheWriteUsd: 0.1,
    },
    models: ["claude-opus-4-6"],
    ...overrides,
  }
}

const ANALYTICS = {
  // A session with nothing analyzable is enough to exercise the flow: the view
  // still renders its chrome, which is what these tests navigate through.
  summary: null,
  supportsAnalysis: true,
  title: "Wire the tray popover",
  wslDistro: null,
  isActive: false,
  cost: null,
  models: [],
  modelRuns: [],
  orchestration: null,
  relations: null,
  sourcePath: "/home/avery/.claude/projects/widgets/session-abc-123.jsonl",
}

const USAGE_WINDOW = {
  tokensIn: 1_000,
  tokensOut: 200,
  cacheRead: 50,
  estimatedUsd: 1.25,
  sessionCount: 1,
}

const PROVIDER_USAGE = {
  providers: [
    {
      provider: "anthropic",
      accountKey: null,
      displayName: "Anthropic",
      state: "estimated",
      staleness: "fresh",
      windows: {
        today: USAGE_WINDOW,
        week: USAGE_WINDOW,
        monthToDate: USAGE_WINDOW,
        last30Days: USAGE_WINDOW,
      },
      agents: [],
      lastActivityAt: "2027-01-15T07:59:00Z",
    },
  ],
  generatedAt: "2027-01-15T08:00:00Z",
}

const LIVE_FORECAST = {
  unavailableReason: "sparseHistory",
  confidence: null,
  consumptionRate: null,
  paceRatio: null,
  paceTrend: null,
  runwayAt: null,
  usedToday: null,
}

// Use a different provider from the local spend fixture. The limits bar uses
// live readings, so only Codex gets a pill. This verifies that a live reading
// does not require local spend.
const LIVE_USAGE = {
  providers: [
    {
      provider: "openai",
      accountKey: null,
      displayName: "Codex",
      support: "live",
      freshness: "fresh",
      sourceLabel: "Asked Codex directly",
      observedAt: "2027-01-15T07:58:00Z",
      windows: [
        {
          id: "seven-day",
          role: "primaryLong",
          kind: "weekly",
          scopeModel: null,
          usedPercent: 40,
          startsAt: null,
          resetsAt: null,
          hasNonzeroUsageInCurrentPeriod: false,
          forecast: LIVE_FORECAST,
        },
      ],
      extraUsage: null,
      resetCredits: null,
      plan: null,
    },
  ],
  errors: [],
  meters: [{ provider: "openai", displayName: "Codex", shown: true }],
  generatedAt: "2027-01-15T08:00:00Z",
}

const HEALTHY_STORAGE = { failing: false, message: null }

const CHECKS_REPORT = {
  evidenceSettled: true,
  estimatedTokenBurnBasisPoints: 1_625,
  categories: [
    {
      id: "cacheChurn",
      finding: 7,
      clean: 7,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 1_250,
    },
    {
      id: "sessionsOverDepth",
      finding: 0,
      clean: 14,
      unavailable: 0,
      estimatedTokenBurnBasisPoints: 0,
    },
  ],
}

function repositoryPayload(overrides: Record<string, unknown> = {}) {
  return {
    key: "/home/avery/code/widgets",
    repoName: "widgets",
    fullName: "avery/widgets",
    status: "accessible",
    repoRoot: "/home/avery/code/widgets",
    suspectedPath: null,
    worktreeCount: 1,
    sessionCount: 3,
    wslDistro: null,
    enabled: true,
    ...overrides,
  }
}

function mockCommands(overrides: Record<string, unknown> = {}) {
  invoke.mockImplementation((command: string, args?: unknown) => {
    if (command in overrides) {
      const result = overrides[command]
      return result instanceof Error ? Promise.reject(result) : Promise.resolve(result)
    }
    switch (command) {
      case "app_info":
        return Promise.resolve({ appVersion: "0.1.0", debugBuild: true })
      case "get_settings":
        return Promise.resolve(SETTINGS)
      case "list_recent_sessions":
        return Promise.resolve([activityEntry()])
      case "get_session_analysis":
        return Promise.resolve(ANALYTICS)
      case "get_provider_usage":
        return Promise.resolve(PROVIDER_USAGE)
      case "get_live_usage":
      case "refresh_live_usage":
        return Promise.resolve(LIVE_USAGE)
      case "get_session_limit_allocations":
        return Promise.resolve({ generatedAt: "2027-01-15T08:00:00Z", allocations: [] })
      case "get_checks_report":
        return Promise.resolve(CHECKS_REPORT)
      case "get_scan_status":
      case "scan_now":
      case "cancel_scan":
        return Promise.resolve(SCAN_STATUS)
      case "set_settings":
        return Promise.resolve((args as Record<string, unknown> | undefined)?.["settings"])
      case "list_scan_roots":
      case "default_scan_roots":
      case "list_repositories":
      case "set_repository_enabled":
        return Promise.resolve([])
      case "get_storage_health":
        return Promise.resolve(HEALTHY_STORAGE)
      case "set_popover_height":
        return Promise.resolve(true)
      case "show_popover_peek":
        return Promise.resolve({
          generation: 1,
          target: (args as { target: unknown }).target,
        })
      default:
        return Promise.resolve(null)
    }
  })
}

describe("PopoverView", () => {
  beforeEach(() => {
    invoke.mockReset()
    confirmDialog.mockReset()
    saveDialog.mockReset()
    openDialog.mockReset()
    listeners.clear()
    Reflect.deleteProperty(window, "__ANTIBURN_WINDOW_GENERATION__")
    Object.defineProperty(HTMLElement.prototype, "scrollTo", {
      configurable: true,
      value(this: HTMLElement, options: ScrollToOptions | number, y?: number) {
        this.scrollTop = typeof options === "number" ? (y ?? 0) : (options.top ?? 0)
        this.dispatchEvent(new Event("scroll"))
      },
    })
    mockCommands()
  })

  it("reports content readiness after activity and cached usage settle", async () => {
    let resolveEntries: ((entries: unknown[]) => void) | null = null
    let resolveUsage: ((usage: unknown) => void) | null = null
    const entries = new Promise<unknown[]>((resolve) => {
      resolveEntries = resolve
    })
    const usage = new Promise<unknown>((resolve) => {
      resolveUsage = resolve
    })
    Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
      configurable: true,
      value: 7,
    })
    mockCommands({ list_recent_sessions: entries, get_provider_usage: usage })

    render(<PopoverView />)

    await act(async () => {
      resolveEntries?.([])
      await Promise.resolve()
    })
    expect(invoke).not.toHaveBeenCalledWith("popover_content_ready", { generation: 7 })

    await act(async () => {
      resolveUsage?.(PROVIDER_USAGE)
      await Promise.resolve()
    })
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("popover_content_ready", { generation: 7 }),
    )
    expect(
      invoke.mock.calls.filter(([command]) => command === "popover_content_ready"),
    ).toHaveLength(1)

    emit("popover:shown", undefined)
    await act(async () => Promise.resolve())
    expect(
      invoke.mock.calls.filter(([command]) => command === "popover_content_ready"),
    ).toHaveLength(1)
  })

  it("renders the backend limit allocation for a session row", async () => {
    mockCommands({
      get_settings: { ...SETTINGS, sessionBadgeMetric: "weeklyPercent" },
      get_session_limit_allocations: {
        generatedAt: "2027-01-15T08:00:00Z",
        allocations: [
          {
            agent: "claude-code",
            sessionId: "session-abc-123",
            wslDistro: null,
            provider: "anthropic",
            displayName: "Claude",
            accountKey: "work",
            metric: "weekly",
            windowId: "weekly-main",
            resetsAt: "2027-01-20T08:00:00Z",
            percent: 12.345,
          },
        ],
      },
    })

    render(<PopoverView />)

    expect(await screen.findByText("12.3%")).toHaveAttribute(
      "data-session-limit-window",
      "weekly-main",
    )
    expect(invoke).toHaveBeenCalledWith("get_session_limit_allocations")
  })

  it("keeps the last allocation when a refresh fails", async () => {
    const allocationSummary = {
      generatedAt: "2027-01-15T08:00:00Z",
      allocations: [
        {
          agent: "claude-code",
          sessionId: "session-abc-123",
          wslDistro: null,
          provider: "anthropic",
          displayName: "Claude",
          accountKey: null,
          metric: "weekly",
          windowId: "weekly-main",
          resetsAt: "2027-01-20T08:00:00Z",
          percent: 12.5,
        },
      ],
    }
    mockCommands({
      get_settings: { ...SETTINGS, sessionBadgeMetric: "weeklyPercent" },
      get_session_limit_allocations: allocationSummary,
    })
    const baseInvoke = invoke.getMockImplementation()!
    let allocationRefreshFails = false
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "get_session_limit_allocations" && allocationRefreshFails) {
        return Promise.reject(new Error("temporary allocation failure"))
      }
      return baseInvoke(command, args)
    })
    render(<PopoverView />)
    expect(await screen.findByText("12.5%")).toBeInTheDocument()
    const allocationCallsBefore = invoke.mock.calls.filter(
      ([command]) => command === "get_session_limit_allocations",
    ).length

    allocationRefreshFails = true
    emit("popover:shown", undefined)

    await waitFor(() =>
      expect(
        invoke.mock.calls.filter(([command]) => command === "get_session_limit_allocations")
          .length,
      ).toBeGreaterThan(allocationCallsBefore),
    )
    expect(screen.getByText("12.5%")).toBeInTheDocument()
  })

  it("retries a failed hidden content report when the popover appears", async () => {
    Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
      configurable: true,
      value: 7,
    })
    const baseInvoke = invoke.getMockImplementation()
    let contentReadyCalls = 0
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "popover_content_ready") {
        contentReadyCalls += 1
        return contentReadyCalls === 1
          ? Promise.reject(new Error("renderer not available"))
          : Promise.resolve(null)
      }
      return baseInvoke?.(command, args)
    })

    render(<PopoverView />)
    await waitFor(() => expect(contentReadyCalls).toBe(1))
    await act(async () => Promise.resolve())

    emit("popover:shown", undefined)
    await waitFor(() => expect(contentReadyCalls).toBe(2))
  })

  it("retries once when reveal arrives during a report that later fails", async () => {
    Object.defineProperty(window, "__ANTIBURN_WINDOW_GENERATION__", {
      configurable: true,
      value: 7,
    })
    const baseInvoke = invoke.getMockImplementation()
    let rejectFirstReport: ((error: Error) => void) | null = null
    let contentReadyCalls = 0
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "popover_content_ready") {
        contentReadyCalls += 1
        if (contentReadyCalls === 1) {
          return new Promise((_resolve, reject) => {
            rejectFirstReport = reject
          })
        }
        return Promise.resolve(null)
      }
      return baseInvoke?.(command, args)
    })

    render(<PopoverView />)
    await waitFor(() => expect(contentReadyCalls).toBe(1))
    emit("popover:shown", undefined)
    expect(contentReadyCalls).toBe(1)

    await act(async () => {
      rejectFirstReport?.(new Error("renderer not available"))
      await Promise.resolve()
    })
    await waitFor(() => expect(contentReadyCalls).toBe(2))
  })

  it("lists the sessions the shell reports for the stored window", async () => {
    render(<PopoverView />)

    expect(await screen.findByText("Wire the tray popover")).toBeInTheDocument()
    expect(invoke).toHaveBeenCalledWith("list_recent_sessions", { windowDays: 7 })
    // The cost pill's wording is derived here from the payload's components;
    // the shell sends values, never copy.
    expect(screen.getByLabelText("Estimated cost $1.25")).toBeInTheDocument()
  })

  it("opens a session, loads its analysis, and comes back to the list", async () => {
    render(<PopoverView />)

    fireEvent.click(await screen.findByText("Wire the tray popover"))

    expect(await screen.findByRole("heading", { name: "Session Detail" })).toBeInTheDocument()
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("get_session_analysis", {
        agent: "claude-code",
        sessionId: "session-abc-123",
        wslDistro: null,
      }),
    )

    // The session pane is a lazy-loaded chunk. Its own "Session Detail"
    // heading briefly shares text with the Suspense fallback's, so wait for a
    // control unique to the loaded pane before treating it as ready.
    fireEvent.click(await screen.findByRole("button", { name: "Back" }, { timeout: 5_000 }))

    expect(await screen.findByText("Wire the tray popover")).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "Session Detail" })).not.toBeInTheDocument()
  })

  it("updates the one row a sessions:entry-changed event names, without re-listing", async () => {
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")

    const listCallsBefore = invoke.mock.calls.filter(
      ([command]) => command === "list_recent_sessions",
    ).length
    const allocationCallsBefore = invoke.mock.calls.filter(
      ([command]) => command === "get_session_limit_allocations",
    ).length

    emit("sessions:entry-changed", {
      ...activityEntry(),
      modelRuns: [{ model: "claude-fable-5", thinkingMode: "high" }],
    })

    expect(await screen.findByTitle("claude-fable-5/high")).toBeInTheDocument()
    expect(
      invoke.mock.calls.filter(([command]) => command === "list_recent_sessions"),
    ).toHaveLength(listCallsBefore)
    await waitFor(() =>
      expect(
        invoke.mock.calls.filter(([command]) => command === "get_session_limit_allocations")
          .length,
      ).toBeGreaterThan(allocationCallsBefore),
    )
  })

  it("keeps a row's high-cost flag after a sessions:entry-changed event replaces it", async () => {
    const cheapEntries = Array.from({ length: 7 }, (_, i) =>
      activityEntry({
        sessionId: `session-cheap-${i}`,
        title: `Cheap session ${i}`,
        cost: {
          totalUsd: 0.1,
          inputUsd: 0.05,
          outputUsd: 0.03,
          cacheReadUsd: 0.01,
          cacheWriteUsd: 0.01,
        },
      }),
    )
    const expensiveEntry = activityEntry({
      cost: { totalUsd: 10, inputUsd: 5, outputUsd: 3, cacheReadUsd: 1, cacheWriteUsd: 1 },
    })
    mockCommands({ list_recent_sessions: [expensiveEntry, ...cheapEntries] })

    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")
    expect(await screen.findByLabelText(/higher than usual/i)).toBeInTheDocument()

    emit("sessions:entry-changed", {
      ...expensiveEntry,
      modelRuns: [{ model: "claude-fable-5", thinkingMode: "high" }],
    })

    expect(await screen.findByTitle("claude-fable-5/high")).toBeInTheDocument()
    // The row is rebuilt from the pushed payload alone, so its high-cost flag
    // must be recomputed against the cohort rather than defaulting to false.
    expect(screen.getByLabelText(/higher than usual/i)).toBeInTheDocument()
  })

  it("leaves the list unchanged when a sessions:entry-changed event names an unknown session", async () => {
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")

    emit("sessions:entry-changed", {
      ...activityEntry({ sessionId: "session-unknown" }),
      modelRuns: [{ model: "claude-fable-5", thinkingMode: "high" }],
    })

    expect(screen.queryByText("fable-5/high")).not.toBeInTheDocument()
    expect(screen.getByText("Wire the tray popover")).toBeInTheDocument()
  })

  it("keeps the list at the same offset through repeated session navigation", async () => {
    const scrollTo = vi.fn(function (
      this: HTMLElement,
      options: ScrollToOptions | number,
      y?: number,
    ) {
      this.scrollTop = typeof options === "number" ? (y ?? 0) : (options.top ?? 0)
      this.dispatchEvent(new Event("scroll"))
    })
    Object.defineProperty(HTMLElement.prototype, "scrollTo", {
      configurable: true,
      value: scrollTo,
    })
    mockCommands({
      list_recent_sessions: Array.from({ length: 12 }, (_, index) =>
        activityEntry({
          sessionId: `session-${index}`,
          title: index === 0 ? "Wire the tray popover" : `Session fixture ${index}`,
          timestamp: new Date(Date.now() - index * 1_000).toISOString(),
        }),
      ),
    })
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")

    const viewportOf = () =>
      screen
        .getByRole("region", { name: "Sessions" })
        .querySelector<HTMLElement>(".ui-scroll-viewport")
    const viewport = viewportOf()
    expect(viewport).not.toBeNull()
    viewport!.scrollTop = 240
    fireEvent.scroll(viewport!)

    for (let cycle = 0; cycle < 3; cycle += 1) {
      fireEvent.click(screen.getByText("Wire the tray popover"))
      fireEvent.click(await screen.findByRole("button", { name: "Back" }, { timeout: 5_000 }))

      await screen.findByText("Wire the tray popover")
      await waitFor(() => expect(viewportOf()?.scrollTop).toBe(240))
    }
    expect(
      scrollTo.mock.calls.some(
        ([options]) => typeof options !== "number" && (options.top ?? 0) === 240,
      ),
    ).toBe(true)
  })

  it("folds Usage and Checks as one measured Activity header", async () => {
    render(<PopoverView />)

    const summary = await screen.findByRole("region", { name: "Usage and spend" })
    expect(summary).not.toHaveAttribute("title")
    const foldTarget = summary.parentElement
    expect(foldTarget).toContainElement(screen.getByTestId("usage-limits-bar"))
    expect(foldTarget).toContainElement(screen.getByText("All checks").closest("[tabindex]"))
    expect(foldTarget?.parentElement?.children).toHaveLength(1)
  })

  it("shows the API pricing caveat when a live account reports a subscription", async () => {
    mockCommands({
      get_live_usage: {
        ...LIVE_USAGE,
        providers: LIVE_USAGE.providers.map((provider) => ({
          ...provider,
          plan: { name: "plus", tier: null },
        })),
      },
    })
    render(<PopoverView />)

    expect(
      await screen.findByTitle(
        "You're on subscription, so these are just estimated dollar values.",
      ),
    ).toHaveAccessibleName("Usage and spend")
  })

  it("notes an opened session as an agent and an environment, and nothing else", async () => {
    render(<PopoverView />)

    fireEvent.click(await screen.findByText("Wire the tray popover"))

    const notes = invoke.mock.calls.filter(([name]) => name === "note_interaction")
    // Exactly one. The card is the only thing instrumented: the newer/older
    // traversal inside a session replaces the top of the stack and is
    // deliberately silent, because counting it would drown out the question
    // this event exists to answer — how often the list leads anywhere at all.
    expect(notes).toHaveLength(1)
    expect(invoke).toHaveBeenCalledWith("note_interaction", {
      interaction: { kind: "sessionOpened", agent: "claude-code", environment: "native" },
    })
    // The shape is the guarantee. Nothing identifying the session itself may
    // ride along, and the shell would refuse it if it did — this asserts the
    // caller does not even try.
    const [, payload] = invoke.mock.calls.find(([name]) => name === "note_interaction") ?? []
    expect(Object.keys((payload as { interaction: object }).interaction).sort()).toEqual([
      "agent",
      "environment",
      "kind",
    ])
  })

  it("confirms a removal, deletes only local records, and returns to the list", async () => {
    confirmDialog.mockResolvedValue(true)
    render(<PopoverView />)

    fireEvent.click(await screen.findByText("Wire the tray popover"))
    fireEvent.click(await screen.findByRole("button", { name: "Delete this session" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("delete_session_data", {
        agent: "claude-code",
        sessionId: "session-abc-123",
        wslDistro: null,
      }),
    )
    const [message] = confirmDialog.mock.calls[0] as [string]
    expect(message).toMatch(/transcript file is not touched/i)
    expect(await screen.findByText("Wire the tray popover")).toBeInTheDocument()
  })

  it("reveals the provider transcript rather than a copy of it", async () => {
    render(<PopoverView />)

    fireEvent.click(await screen.findByText("Wire the tray popover"))
    fireEvent.click(await screen.findByRole("button", { name: "Reveal in file manager" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("reveal_source", {
        path: "/home/avery/.claude/projects/widgets/session-abc-123.jsonl",
      }),
    )
  })

  it("asks for provider usage with the reader own offset and shows the usage-limits bar", async () => {
    render(<PopoverView />)

    expect(await screen.findByTestId("usage-limits-bar")).toBeInTheDocument()
    // "Today" and "this month" are the reader's calendar days, so the offset
    // travels with the request rather than being guessed shell-side.
    expect(invoke).toHaveBeenCalledWith("get_provider_usage", {
      utcOffsetMinutes: -new Date().getTimezoneOffset(),
    })
    expect(screen.getByRole("button", { name: "Codex at 40 percent" })).toBeInTheDocument()
  })

  it("shows Checks in one passive anchored preview", async () => {
    render(<PopoverView />)

    await screen.findByText("1 check failed")
    const trigger = (await screen.findByText("All checks")).closest("[tabindex]")!
    fireEvent.mouseEnter(trigger)

    expect(screen.queryByRole("heading", { name: "Checks" })).not.toBeInTheDocument()
    await waitFor(() => {
      expect(trigger.parentElement).toHaveAttribute("data-state", "active")
      const requests = invoke.mock.calls.filter(([command]) => command === "show_popover_peek")
      expect(requests).toHaveLength(1)
      expect(requests[0]?.[1]).toMatchObject({
        target: { kind: "checks" },
        initialPresentation: {
          kind: "checks",
          presentation: {
            failures: [expect.objectContaining({ id: "cacheChurn" })],
            wins: [expect.objectContaining({ id: "sessionsOverDepth" })],
            refreshUnavailable: false,
          },
        },
      })
    })
  })

  it("conceals the Checks preview when the Activity list scrolls", async () => {
    render(<PopoverView />)
    await screen.findByText("1 check failed")
    fireEvent.mouseEnter((await screen.findByText("All checks")).closest("[tabindex]")!)

    const viewport = screen
      .getByRole("region", { name: "Sessions" })
      .querySelector<HTMLElement>(".ui-scroll-viewport")!
    viewport.scrollTop = 20
    fireEvent.scroll(viewport)

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("hide_popover_peek"))
  })

  it("loads a trailing Checks report when evidence settles during reduction", async () => {
    let resolveFirst: ((report: typeof CHECKS_REPORT) => void) | null = null
    const first = new Promise<typeof CHECKS_REPORT>((resolve) => {
      resolveFirst = resolve
    })
    const original = invoke.getMockImplementation()!
    let checksCalls = 0
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "get_checks_report") {
        checksCalls += 1
        return checksCalls === 1 ? first : Promise.resolve(CHECKS_REPORT)
      }
      return original(command, args)
    })
    render(<PopoverView />)
    await waitFor(() => expect(checksCalls).toBe(1))

    emit("checks:report-changed", null)
    expect(checksCalls).toBe(1)
    resolveFirst!(CHECKS_REPORT)

    await waitFor(() => expect(checksCalls).toBe(2))
    expect(await screen.findByText("1 check failed")).toBeInTheDocument()
  })

  it("refreshes Checks after the evidence worker queue settles", async () => {
    render(<PopoverView />)
    await screen.findByText("1 check failed")
    const callsBefore = invoke.mock.calls.filter(
      ([command]) => command === "get_checks_report",
    ).length
    const hidesBefore = invoke.mock.calls.filter(
      ([command]) => command === "hide_popover_peek",
    ).length

    emit("checks:report-changed", null)

    await waitFor(() => {
      const calls = invoke.mock.calls.filter(
        ([command]) => command === "get_checks_report",
      ).length
      expect(calls).toBe(callsBefore + 1)
    })
    expect(
      invoke.mock.calls.filter(([command]) => command === "hide_popover_peek"),
    ).toHaveLength(hidesBefore + 1)
  })

  it("publishes the current report while evidence is still processing", async () => {
    const original = invoke.getMockImplementation()!
    let settled = false
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "get_checks_report") {
        return Promise.resolve({ ...CHECKS_REPORT, evidenceSettled: settled })
      }
      return original(command, args)
    })
    render(<PopoverView />)
    await waitFor(() => expect(listeners.has("checks:report-changed")).toBe(true))
    await waitFor(() =>
      expect(invoke.mock.calls.some(([command]) => command === "get_checks_report")).toBe(true),
    )
    expect(await screen.findByText("1 check failed")).toBeInTheDocument()

    emit("scan:finished", SCAN_STATUS)
    await waitFor(() => expect(screen.getByText("1 check failed")).toBeInTheDocument())

    settled = true
    emit("checks:report-changed", null)
    expect(await screen.findByText("1 check failed")).toBeInTheDocument()
  })

  it("keeps the verdict stable while new evidence is still processing", async () => {
    const original = invoke.getMockImplementation()!
    let settled = true
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "get_checks_report") {
        return Promise.resolve({ ...CHECKS_REPORT, evidenceSettled: settled })
      }
      return original(command, args)
    })
    render(<PopoverView />)
    await screen.findByText("1 check failed")

    settled = false
    emit("scan:finished", SCAN_STATUS)

    expect(await screen.findByText("1 check failed")).toBeInTheDocument()
    expect(screen.queryByText("Checking local sessions…")).not.toBeInTheDocument()
  })

  it("marks a retained verdict when a later Checks request fails", async () => {
    const original = invoke.getMockImplementation()!
    let fail = false
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "get_checks_report") {
        return fail
          ? Promise.reject(new Error("report failed"))
          : Promise.resolve({ ...CHECKS_REPORT, evidenceSettled: false })
      }
      return original(command, args)
    })
    render(<PopoverView />)
    expect(await screen.findByText("1 check failed")).toBeInTheDocument()

    fail = true
    emit("checks:report-changed", null)

    expect(await screen.findByText(/1 check failed · refresh unavailable/)).toBeInTheDocument()
    expect(screen.queryByText(/refreshing/i)).not.toBeInTheDocument()
  })

  it("refreshes after a settled scan that queues no evidence work", async () => {
    render(<PopoverView />)
    await screen.findByText("1 check failed")
    const callsBefore = invoke.mock.calls.filter(
      ([command]) => command === "get_checks_report",
    ).length

    emit("scan:finished", SCAN_STATUS)

    await waitFor(() => {
      const calls = invoke.mock.calls.filter(
        ([command]) => command === "get_checks_report",
      ).length
      expect(calls).toBe(callsBefore + 1)
    })
  })

  it("cancels Checks work when the popover session stops", async () => {
    const view = render(<PopoverView />)
    await screen.findByText("1 check failed")

    view.unmount()

    expect(invoke).toHaveBeenCalledWith("cancel_checks_report", {
      consumerId: expect.any(String),
    })
  })

  it("uses a new Checks consumer ID when the popover session restarts", async () => {
    const firstView = render(<PopoverView />)
    await screen.findByText("1 check failed")
    const firstId = invoke.mock.calls.find(([command]) => command === "get_checks_report")?.[1]
      ?.consumerId
    firstView.unmount()

    const secondView = render(<PopoverView />)
    await screen.findByText("1 check failed")
    const ids = invoke.mock.calls
      .filter(([command]) => command === "get_checks_report")
      .map(([, args]) => args.consumerId)
    secondView.unmount()

    expect(ids).toContain(firstId)
    expect(ids.some((id) => id !== firstId)).toBe(true)
  })

  it("requests a provider preview after the pointer rests on its trigger", async () => {
    render(<PopoverView />)

    const trigger = await screen.findByRole("button", { name: "Codex at 40 percent" })
    vi.useFakeTimers()
    try {
      fireEvent.mouseEnter(trigger)

      await act(() => vi.advanceTimersByTimeAsync(149))
      expect(invoke).not.toHaveBeenCalledWith("show_popover_peek", expect.anything())

      await act(() => vi.advanceTimersByTimeAsync(1))
      expect(invoke).toHaveBeenCalledWith("show_popover_peek", {
        target: {
          kind: "provider",
          provider: "openai",
          utcOffsetMinutes: -new Date().getTimezoneOffset(),
        },
        anchor: expect.any(Object),
        initialPresentation: {
          kind: "provider",
          summary: { ...PROVIDER_USAGE, providers: [] },
          live: LIVE_USAGE,
        },
      })
    } finally {
      vi.useRealTimers()
    }
  })

  it("cancels a pending provider preview when its click opens Usage", async () => {
    render(<PopoverView />)

    const trigger = await screen.findByRole("button", { name: "Codex at 40 percent" })
    vi.useFakeTimers()
    try {
      fireEvent.mouseEnter(trigger)
      fireEvent.click(trigger)

      expect(screen.getByRole("heading", { name: "Usage" })).toBeInTheDocument()
      expect(invoke).toHaveBeenCalledWith("hide_popover_peek")

      await act(() => vi.advanceTimersByTimeAsync(150))
      expect(invoke).not.toHaveBeenCalledWith("show_popover_peek", expect.anything())
    } finally {
      vi.useRealTimers()
    }
  })

  it("conceals an active provider preview before expanding the limits bar", async () => {
    render(<PopoverView />)

    const trigger = await screen.findByRole("button", { name: "Codex at 40 percent" })
    fireEvent.mouseEnter(trigger)
    fireEvent.click(screen.getByRole("button", { name: "Expand usage limits" }))

    expect(invoke).toHaveBeenCalledWith("hide_popover_peek")
    expect(screen.getByRole("button", { name: "Collapse usage limits" })).toBeInTheDocument()
  })

  it("shows cached limits and sessions while the provider refresh is still running", async () => {
    let finishRefresh: (() => void) | null = null
    const baseInvoke = invoke.getMockImplementation()!
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command !== "refresh_live_usage") return baseInvoke(command, args)
      return new Promise((resolve) => {
        finishRefresh = () => resolve(LIVE_USAGE)
      })
    })

    render(<PopoverView />)

    expect(await screen.findByTestId("usage-limits-bar")).toBeInTheDocument()
    expect(screen.getByText("Wire the tray popover")).toBeInTheDocument()
    expect(screen.getByRole("status")).toBeInTheDocument()

    await act(async () => {
      finishRefresh?.()
      await Promise.resolve()
    })
  })

  it("shows the app version beside the title", async () => {
    render(<PopoverView />)
    await screen.findByTestId("usage-limits-bar")

    const footer = screen.getByRole("button", { name: "Open settings" }).parentElement
    expect(footer).not.toBeNull()
    expect(footer).toHaveTextContent("antiburn")
    expect(footer?.querySelector('[data-testid="usage-limits-bar"]')).toBeNull()
    const nameAndVersion = screen.getByRole("button", { name: "antiburn v0.1.0 debug" })
    expect(nameAndVersion).toHaveClass("type-caption", "text-label-secondary")
  })

  it("opens the GitHub repo when the name and version are clicked", async () => {
    render(<PopoverView />)
    await screen.findByTestId("usage-limits-bar")

    fireEvent.click(screen.getByRole("button", { name: "antiburn v0.1.0 debug" }))

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("open_github_repo"))
  })

  it("omits the debug label from a release build", async () => {
    mockCommands({ app_info: { appVersion: "0.1.0", debugBuild: false } })
    render(<PopoverView />)

    expect(await screen.findByRole("button", { name: "antiburn v0.1.0" })).toBeInTheDocument()
    expect(screen.queryByRole("button", { name: "antiburn v0.1.0 debug" })).toBeNull()
  })

  it("omits the version when app info cannot load", async () => {
    mockCommands({ app_info: new Error("unavailable") })
    render(<PopoverView />)

    await screen.findByTestId("usage-limits-bar")
    expect(screen.queryByText(/^v\d/)).toBeNull()
  })

  it("opens the full Usage view from a provider preview trigger", async () => {
    render(<PopoverView />)

    const trigger = await screen.findByRole("button", { name: "Codex at 40 percent" })
    fireEvent.mouseEnter(trigger)
    fireEvent.click(trigger)

    expect(await screen.findByRole("heading", { name: "Usage" })).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Back to activity" })).toBeInTheDocument()
    expect(invoke).toHaveBeenCalledWith("hide_popover_peek")
  })

  it("still shows a live-only pill on a fresh day with zero local spend anywhere", async () => {
    // The bug this fixes: the row used to be driven by local spend, so a
    // fresh day with none — even for a provider that never has any, like
    // Codex here — fell through to an empty-row fallback despite Codex's own
    // live reading sitting right there in `get_live_usage`.
    mockCommands({ get_provider_usage: { ...PROVIDER_USAGE, providers: [] } })
    render(<PopoverView />)

    await screen.findByText("Wire the tray popover")
    expect(screen.getByTestId("usage-limits-bar")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Codex at 40 percent" })).toBeInTheDocument()
    expect(screen.queryByText("No live limits")).not.toBeInTheDocument()
  })

  it("withholds the usage-limits bar entirely when no provider has a limit to show", async () => {
    const noLiveUsage = { providers: [], errors: [], meters: [], generatedAt: "" }
    mockCommands({ get_live_usage: noLiveUsage, refresh_live_usage: noLiveUsage })
    render(<PopoverView />)

    await screen.findByText("Wire the tray popover")
    expect(screen.queryByTestId("usage-limits-bar")).not.toBeInTheDocument()
    // The plain footer is unaffected — it never depended on usage at all.
    expect(screen.getByRole("button", { name: "antiburn v0.1.0 debug" })).toBeInTheDocument()
  })

  it("persists the usage-limits toggle through set_settings, and opens the meters", async () => {
    render(<PopoverView />)
    await screen.findByTestId("usage-limits-bar")

    fireEvent.click(screen.getByRole("button", { name: "Expand usage limits" }))

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_settings", {
        settings: expect.objectContaining({ overviewLimitsExpanded: true }),
      }),
    )
    // The pill row stays put and the meters drop below it, so the bar keeps
    // its seat in both states.
    expect(
      await screen.findByRole("button", { name: "Collapse usage limits" }),
    ).toBeInTheDocument()
    expect(screen.getByTestId("usage-limits-bar")).toBeInTheDocument()
    expect(screen.getByRole("region", { name: "Usage limits" })).toBeInTheDocument()
    expect(screen.getByRole("group", { name: "Codex" })).toBeInTheDocument()
  })

  it("shows a spinner beside the toggle while a usage refresh is in flight", async () => {
    let resolveSecondLoad: (() => void) | null = null
    let liveCalls = 0
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command === "get_live_usage") return Promise.resolve(LIVE_USAGE)
      if (command === "refresh_live_usage") {
        liveCalls += 1
        // The first refresh settles so the view reaches its resting state.
        // The second refresh stays open so the test can observe the spinner.
        if (liveCalls === 1) return Promise.resolve(LIVE_USAGE)
        return new Promise((resolve) => {
          resolveSecondLoad = () => resolve(LIVE_USAGE)
        })
      }
      switch (command) {
        case "get_settings":
          return Promise.resolve(SETTINGS)
        case "list_recent_sessions":
          return Promise.resolve([activityEntry()])
        case "get_provider_usage":
          return Promise.resolve(PROVIDER_USAGE)
        case "get_storage_health":
          return Promise.resolve(HEALTHY_STORAGE)
        case "list_repositories":
          return Promise.resolve([])
        case "set_settings":
          return Promise.resolve((args as Record<string, unknown> | undefined)?.["settings"])
        default:
          return Promise.resolve(null)
      }
    })

    render(<PopoverView />)
    await screen.findByTestId("usage-limits-bar")
    expect(screen.queryByRole("status")).not.toBeInTheDocument()

    emit("popover:shown", undefined)
    await waitFor(() => expect(resolveSecondLoad).not.toBeNull())
    expect(screen.getByRole("status")).toBeInTheDocument()

    await act(async () => {
      resolveSecondLoad?.()
      await Promise.resolve()
    })
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument())
  })

  it("refreshes usage on the shell’s popover-shown signal, independent of any scan", async () => {
    render(<PopoverView />)
    await screen.findByTestId("usage-limits-bar")

    const callsBeforeShown = invoke.mock.calls.filter(
      ([command]) => command === "refresh_live_usage",
    ).length
    const allocationCallsBeforeShown = invoke.mock.calls.filter(
      ([command]) => command === "get_session_limit_allocations",
    ).length

    // `popover:shown` carries no payload — it is a pure signal, unlike the
    // scan events, which carry a status.
    emit("popover:shown", undefined)

    await waitFor(() =>
      expect(
        invoke.mock.calls.filter(([command]) => command === "refresh_live_usage").length,
      ).toBeGreaterThan(callsBeforeShown),
    )
    await waitFor(() =>
      expect(
        invoke.mock.calls.filter(([command]) => command === "get_session_limit_allocations")
          .length,
      ).toBeGreaterThan(allocationCallsBeforeShown),
    )
    // Not riding the scan pipeline: no scan command was ever asked for.
    expect(invoke).not.toHaveBeenCalledWith("scan_now", expect.anything())
  })

  it("re-loads the open session’s analysis on the popover-shown signal and shows a spinner meanwhile", async () => {
    render(<PopoverView />)
    fireEvent.click(await screen.findByText("Wire the tray popover"))
    await screen.findByRole("button", { name: "Back" }, { timeout: 5_000 })
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument())

    const loadsBeforeShown = invoke.mock.calls.filter(
      ([command]) => command === "get_session_analysis",
    ).length

    let finishLoad: (() => void) | null = null
    const baseInvoke = invoke.getMockImplementation()!
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (command !== "get_session_analysis") return baseInvoke(command, args)
      return new Promise((resolve) => {
        finishLoad = () => resolve(ANALYTICS)
      })
    })

    emit("popover:shown", undefined)

    await waitFor(() =>
      expect(
        invoke.mock.calls.filter(([command]) => command === "get_session_analysis").length,
      ).toBe(loadsBeforeShown + 1),
    )
    // The settled analysis stays on screen; only the header spinner says a
    // newer one is on its way.
    expect(screen.getByRole("status")).toBeInTheDocument()
    expect(screen.queryByTestId("session-analysis-skeleton")).not.toBeInTheDocument()

    await act(async () => {
      finishLoad?.()
      await Promise.resolve()
    })
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument())
  })

  it("refetches the session list on the shell's popover-shown signal, as a defence against a missed scan event", async () => {
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")

    const listCallsBeforeShown = invoke.mock.calls.filter(
      ([command]) => command === "list_recent_sessions",
    ).length

    emit("popover:shown", undefined)

    await waitFor(() =>
      expect(
        invoke.mock.calls.filter(([command]) => command === "list_recent_sessions").length,
      ).toBeGreaterThan(listCallsBeforeShown),
    )
    expect(invoke).toHaveBeenCalledWith("list_recent_sessions", { windowDays: 7 })
  })

  it("never renders the first-run flow, whatever the flag says", async () => {
    // The flow has its own window now (`views/OnboardingView.tsx`), and
    // the shell sends the tray click there instead of here. A popover that
    // could still draw it would be a second, unreachable copy.
    mockCommands({ get_settings: { ...SETTINGS, onboardingCompleted: false } })
    render(<PopoverView />)

    await screen.findByText("Wire the tray popover")
    expect(
      screen.queryByRole("heading", { name: "Stop hitting your token limits." }),
    ).not.toBeInTheDocument()
  })
})

/**
 * Attention banners.
 *
 * Every case here starts from a signal the shell actually reports — a
 * repository the system refuses to open, a database that rejected a write.
 * There is no test for a speculative banner because there is no speculative
 * banner.
 */
describe("PopoverView — attention banners", () => {
  beforeEach(() => {
    invoke.mockReset()
    confirmDialog.mockReset()
    saveDialog.mockReset()
    openDialog.mockReset()
    listeners.clear()
    mockCommands()
  })

  it("says nothing when nothing is wrong", async () => {
    mockCommands({ list_repositories: [repositoryPayload()] })
    render(<PopoverView />)

    await screen.findByText("Wire the tray popover")
    expect(screen.queryByRole("status")).not.toBeInTheDocument()
  })

  it("surfaces a blocked repository, and Review opens Settings at Sources", async () => {
    mockCommands({
      list_repositories: [repositoryPayload({ status: "permission_denied" })],
    })
    render(<PopoverView />)

    const banner = await screen.findByRole("status")
    expect(banner).toHaveTextContent(/blocking antiburn from reading widgets/i)

    fireEvent.click(screen.getByRole("button", { name: "Review" }))

    // Not just "open Settings": the banner lands the reader on the pane that
    // can do something about what it reported.
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("open_settings_window", { pane: "sources" }),
    )
  })

  it("reads the repository list on first paint rather than waiting for a scan", async () => {
    // A blocked repository is precisely the case where a scan may never
    // complete, so the banner cannot depend on one finishing.
    mockCommands({
      list_repositories: [repositoryPayload({ status: "permission_denied" })],
    })
    render(<PopoverView />)

    await screen.findByRole("status")
    expect(invoke).toHaveBeenCalledWith("list_repositories")
  })

  it("stays dismissed once the reader waves it away", async () => {
    mockCommands({
      list_repositories: [repositoryPayload({ status: "permission_denied" })],
    })
    render(<PopoverView />)

    await screen.findByRole("status")
    fireEvent.click(screen.getByRole("button", { name: /^Dismiss the/ }))

    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument())

    // A scan finishing re-reads the repository list; the banner must not come
    // back from the dead because of it.
    emit("scan:finished", SCAN_STATUS)
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("list_repositories"))
    expect(screen.queryByRole("status")).not.toBeInTheDocument()
  })

  it("surfaces a storage failure with a retry that runs a scan", async () => {
    mockCommands({
      get_storage_health: {
        failing: true,
        message: "The session index could not be written: disk full",
      },
    })
    render(<PopoverView />)

    const banner = await screen.findByRole("status")
    expect(banner).toHaveTextContent(/disk full/)
    expect(banner).toHaveTextContent(/Nothing already indexed is lost/)

    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("scan_now"))
  })

  it("takes the storage banner away when the shell reports a recovery", async () => {
    mockCommands({
      get_storage_health: {
        failing: true,
        message: "The session index could not be written: disk full",
      },
    })
    render(<PopoverView />)

    await screen.findByRole("status")

    emit("storage:health", { failing: false, message: null })

    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument())
  })

  it("shows a recovered-then-failed store again, even after a dismissal", async () => {
    mockCommands({
      get_storage_health: {
        failing: true,
        message: "The session index could not be written: disk full",
      },
    })
    render(<PopoverView />)

    await screen.findByRole("status")
    fireEvent.click(screen.getByRole("button", { name: /^Dismiss the/ }))
    await waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument())

    // Recovery clears the dismissal, so the *next* failure is not silent.
    emit("storage:health", { failing: false, message: null })
    emit("storage:health", {
      failing: true,
      message: "The scan bookkeeping could not be written: database is locked",
    })

    expect(await screen.findByRole("status")).toHaveTextContent(/database is locked/)
  })
})

describe("PopoverView — window behaviour", () => {
  beforeEach(() => {
    invoke.mockReset()
    confirmDialog.mockReset()
    saveDialog.mockReset()
    openDialog.mockReset()
    listeners.clear()
    mockCommands()
  })

  it("asks the shell for each active surface height within the contract ceiling", async () => {
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_popover_height", {
        height: 700,
        animate: true,
      }),
    )

    fireEvent.click(await screen.findByText("Wire the tray popover"))
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("set_popover_height", {
        height: 700,
        animate: true,
      }),
    )

    const heights = invoke.mock.calls
      .filter(([command]) => command === "set_popover_height")
      .map(([, args]) => (args as { height: number }).height)
    expect(heights.length).toBeGreaterThan(0)
    expect(Math.max(...heights)).toBeLessThanOrEqual(780)
  })

  it("keeps Usage mounted and session rows absent until contraction completes", async () => {
    let finishContraction: (() => void) | null = null
    const baseInvoke = invoke.getMockImplementation()!
    invoke.mockImplementation((command: string, args?: unknown) => {
      if (
        command === "set_popover_height" &&
        (args as { height?: number } | undefined)?.height === 700
      ) {
        return new Promise<boolean>((resolve) => {
          finishContraction = () => resolve(true)
        })
      }
      return baseInvoke(command, args)
    })
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")

    fireEvent.click(await screen.findByRole("button", { name: "Codex at 40 percent" }))
    expect(await screen.findByRole("heading", { name: "Usage" })).toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Back to activity" }))

    expect(screen.getByRole("heading", { name: "Usage" })).toBeInTheDocument()
    expect(screen.queryByRole("region", { name: "Sessions" })).not.toBeInTheDocument()
    expect(screen.queryByText("Wire the tray popover")).not.toBeInTheDocument()

    await act(async () => {
      finishContraction?.()
      await Promise.resolve()
    })
    expect(await screen.findByText("Wire the tray popover")).toBeInTheDocument()
  })

  it("dismisses the popover on Escape", async () => {
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")

    fireEvent.keyDown(document, { key: "Escape" })

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("hide_popover"))
  })

  it("opens Settings on the platform preferences shortcut", async () => {
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")

    fireEvent.keyDown(document, { key: ",", metaKey: true })

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("open_settings_window", { pane: null }),
    )
  })

  it("moves focus to the heading of the surface that takes over", async () => {
    render(<PopoverView />)

    const activity = await screen.findByRole("button", { name: "antiburn v0.1.0 debug" })
    await waitFor(() => expect(activity).toHaveFocus())

    fireEvent.click(await screen.findByText("Wire the tray popover"))

    const detail = await screen.findByRole("heading", { name: "Session Detail" })
    await waitFor(() => expect(detail).toHaveFocus())
  })
})

describe("PopoverView — floating HUD restore", () => {
  beforeEach(() => {
    invoke.mockReset()
    confirmDialog.mockReset()
    saveDialog.mockReset()
    openDialog.mockReset()
    listeners.clear()
    mockCommands()
    platform.mac = false
    hudPreference.enabled = false
    hudPreference.overlayVisible = false
    hudPreference.popoverVisible = false
    overlayVisibilityRead.mockReset()
    overlayVisibilityRead.mockImplementation(async () => hudPreference.overlayVisible)
  })

  it("keeps the stored HUD hidden before the popover is shown", async () => {
    platform.mac = true
    hudPreference.enabled = true
    render(<PopoverView />)

    await screen.findByText("Wire the tray popover")
    expect(invoke).not.toHaveBeenCalledWith("open_overlay_window")
  })

  it("reopens the stored HUD when the hidden popover appears", async () => {
    platform.mac = true
    hudPreference.enabled = true
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")

    hudPreference.popoverVisible = true
    emit("popover:shown", null)

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("open_overlay_window"))
  })

  it("restores the stored HUD when the popover opened before its listener attached", async () => {
    platform.mac = true
    hudPreference.enabled = true
    hudPreference.popoverVisible = true
    render(<PopoverView />)

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("open_overlay_window"))
  })

  it("does not show a stored HUD that is already visible", async () => {
    platform.mac = true
    hudPreference.enabled = true
    hudPreference.overlayVisible = true
    hudPreference.popoverVisible = true
    render(<PopoverView />)

    await waitFor(() => expect(overlayVisibilityRead).toHaveBeenCalled())
    expect(invoke).not.toHaveBeenCalledWith("open_overlay_window")
  })

  it("does not restore an off preference", async () => {
    platform.mac = true
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")
    expect(invoke).not.toHaveBeenCalledWith("open_overlay_window")
  })

  it("does not restore the HUD outside macOS", async () => {
    hudPreference.enabled = true
    render(<PopoverView />)
    await screen.findByText("Wire the tray popover")
    expect(invoke).not.toHaveBeenCalledWith("open_overlay_window")
  })
})

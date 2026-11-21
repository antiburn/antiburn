import type * as IpcModule from "../../lib/ipc"
import { fireEvent, render, screen, cleanup, waitFor, act } from "@testing-library/react"
import { beforeEach, afterEach, describe, expect, it, vi } from "vitest"
import { DEFAULT_SETTINGS, type LiveUsageSummaryPayload } from "../../lib/ipc"
import { defaultUsagePins, usageCandidates, SidebarUsage } from "./SidebarUsageLimits"
const mocks = vi.hoisted(() => ({
  settings: {} as typeof DEFAULT_SETTINGS,
  failRead: false,
  failWatch: false,
  useBrowserPatch: false,
  writes: vi.fn(),
  fullWrites: vi.fn(),
  interactions: vi.fn(),
  openSettings: vi.fn(),
  refresh: vi.fn(),
  start: vi.fn(),
  handler: undefined as undefined | ((s: typeof DEFAULT_SETTINGS) => void),
}))
vi.mock("../../lib/ipc", async (original) => ({
  ...(await original<typeof IpcModule>()),
  getSettings: async () => {
    if (mocks.failRead) throw new Error("Settings read failed")
    return mocks.settings
  },
  startLiveUsage: () => mocks.start(),
  noteInteraction: (interaction: IpcModule.Interaction) => mocks.interactions(interaction),
  openSettingsWindow: (...args: Parameters<typeof IpcModule.openSettingsWindow>) =>
    mocks.openSettings(...args),
  setSettings: async (settings: typeof DEFAULT_SETTINGS) => {
    mocks.fullWrites(settings)
    mocks.settings = settings
    return settings
  },
  patchSidebarUsage: async (...args: Parameters<typeof IpcModule.patchSidebarUsage>) => {
    if (mocks.useBrowserPatch)
      return (await original<typeof IpcModule>()).patchSidebarUsage(...args)
    const [patch] = args
    mocks.writes(patch)
    mocks.settings = { ...mocks.settings, ...patch }
    return mocks.settings
  },
  onSettingsChanged: async (handler: (s: typeof DEFAULT_SETTINGS) => void) => {
    if (mocks.failWatch) throw new Error("Settings subscription failed")
    mocks.handler = handler
    return () => {}
  },
  refreshLiveUsage: async () => (await mocks.refresh()) ?? liveUsage,
}))
const liveUsage: LiveUsageSummaryPayload = {
  generatedAt: "2027-01-15T12:00:00Z",
  errors: [],
  meters: [],
  providers: [
    {
      provider: "anthropic",
      accountKey: "test-account",
      displayName: "Claude",
      support: "live",
      freshness: "fresh",
      sourceLabel: "Test",
      observedAt: "2027-01-15T12:00:00Z",
      plan: null,
      accountUuid: null,
      accountEmail: null,
      extraUsage: null,
      resetCredits: null,
      windows: ["five-hour", "seven-day"].map((id, i) => ({
        id,
        role: i === 0 ? "primaryShort" : "primaryLong",
        kind: i === 0 ? "rolling" : "weekly",
        scopeModel: null,
        usedPercent: i === 0 ? 0 : 94,
        startsAt: null,
        resetsAt: null,
        elapsedFraction: null,
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
      })),
    },
  ],
}
function Harness({
  live = liveUsage,
  onRefresh = async () => undefined,
}: {
  live?: LiveUsageSummaryPayload | null
  onRefresh?: () => Promise<LiveUsageSummaryPayload | void>
}) {
  mocks.refresh.mockImplementation(onRefresh)
  return <SidebarUsage live={live} />
}
beforeEach(() => {
  mocks.failRead = false
  mocks.failWatch = false
  mocks.useBrowserPatch = false
  mocks.writes.mockClear()
  mocks.fullWrites.mockClear()
  mocks.interactions.mockClear()
  mocks.openSettings.mockReset().mockResolvedValue(undefined)
  mocks.settings = structuredClone(DEFAULT_SETTINGS)
  mocks.refresh.mockReset()
  mocks.start.mockReset().mockImplementation(async () => {
    mocks.settings = { ...mocks.settings, liveUsageEnabled: true, liveUsageStarted: true }
    return mocks.settings
  })
  HTMLDialogElement.prototype.showModal = function () {
    this.setAttribute("open", "")
  }
  HTMLDialogElement.prototype.close = function () {
    this.removeAttribute("open")
  }
})
afterEach(cleanup)
describe("sidebar usage pins", () => {
  it("explains the opt-out and enables usage through the native command", async () => {
    mocks.settings.liveUsageEnabled = false
    render(<Harness live={{ ...liveUsage, providers: [] }} />)
    await act(async () => {})
    fireEvent.click(screen.getByRole("button", { name: "Pin usage limits" }))
    expect(screen.getByRole("heading", { name: "See your usage limits" })).toBeTruthy()
    expect(screen.queryByRole("radio", { name: "Dials" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Enable live usage" }))
    await waitFor(() => expect(mocks.start).toHaveBeenCalledTimes(1))
    await waitFor(() =>
      expect(
        screen.getByRole("heading", { name: "Your limits will appear here" }),
      ).toBeTruthy(),
    )
    expect(mocks.settings.liveUsageEnabled).toBe(true)
    expect(mocks.refresh).toHaveBeenCalledTimes(1)
    expect(mocks.interactions.mock.calls.map(([event]) => event.action)).toEqual([
      "opened",
      "enable_succeeded",
      "refresh_succeeded",
    ])
  })
  it("keeps an activation failure off and offers a retry", async () => {
    mocks.settings.liveUsageEnabled = false
    mocks.start.mockRejectedValue(new Error("settings unavailable"))
    render(<Harness live={{ ...liveUsage, providers: [] }} />)
    await act(async () => {})
    fireEvent.click(screen.getByRole("button", { name: "Pin usage limits" }))
    fireEvent.click(screen.getByRole("button", { name: "Enable live usage" }))
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Could not load"))
    expect(mocks.settings.liveUsageEnabled).toBe(false)
    expect(mocks.refresh).not.toHaveBeenCalled()
    expect(screen.getByRole("button", { name: "Enable live usage" })).toBeEnabled()
    expect(mocks.interactions.mock.calls.map(([event]) => event.action)).toEqual([
      "opened",
      "enable_failed",
    ])
  })
  it.each([false, true])("records Usage settings navigation failure=%s", async (failed) => {
    mocks.settings.liveUsageEnabled = true
    mocks.settings.liveUsageStarted = true
    mocks.openSettings.mockImplementation(
      failed ? async () => Promise.reject(new Error("settings unavailable")) : async () => {},
    )
    render(<Harness live={{ ...liveUsage, providers: [] }} />)
    await act(async () => {})
    fireEvent.click(screen.getByRole("button", { name: "Pin usage limits" }))
    fireEvent.click(screen.getByRole("button", { name: "Usage settings" }))
    await waitFor(() => expect(mocks.openSettings).toHaveBeenCalledWith("usage"))
    expect(mocks.interactions.mock.calls.map(([event]) => event.action)).toEqual([
      "opened",
      failed ? "settings_open_failed" : "settings_opened",
    ])
  })
  it("separates loading from an active empty state", async () => {
    const { rerender } = render(<Harness live={null} />)
    await act(async () => {})
    fireEvent.click(screen.getByRole("button", { name: "Pin usage limits" }))
    expect(screen.getByRole("heading", { name: "Checking your usage limits" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: "Check again" })).toBeNull()
    rerender(<Harness live={{ ...liveUsage, providers: [] }} />)
    expect(screen.getByRole("heading", { name: "Your limits will appear here" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Check again" })).toBeEnabled()
    expect(screen.getByRole("button", { name: "Usage settings" })).toBeEnabled()
  })

  it("defaults to distinct vendors and prefers Claude and Codex weekly limits", () => {
    const live = structuredClone(liveUsage)
    const claude = live.providers[0]!
    claude.windows[0]!.usedPercent = 99
    claude.windows[1]!.usedPercent = 12
    const other = (provider: string, accountKey: string) => ({
      ...structuredClone(claude),
      provider,
      accountKey,
    })
    live.providers = [
      other("google", "google"),
      other("anthropic", "claude-2"),
      claude,
      other("anthropic", "claude-3"),
      other("openai", "codex"),
      other("openai", "codex-2"),
    ]
    const pins = defaultUsagePins(live)
    expect(pins.map((pin) => pin.provider)).toEqual(["anthropic", "openai", "google"])
    expect(pins.slice(0, 2).map((pin) => pin.window)).toEqual(["seven-day", "seven-day"])
    expect(pins[2]!.window).toBe("five-hour")
  })
  it("pins both available vendors even when one has several accounts", async () => {
    const live = structuredClone(liveUsage)
    const claude = live.providers[0]!
    live.providers.push(
      ...[1, 2, 3].map((i) => ({ ...structuredClone(claude), accountKey: `claude-${i}` })),
      {
        ...structuredClone(claude),
        provider: "openai",
        displayName: "Codex",
        accountKey: "codex",
      },
    )
    render(<Harness live={live} />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).toHaveLength(2))
    expect(mocks.settings.sidebarUsagePins?.map((pin) => [pin.provider, pin.window])).toEqual([
      ["anthropic", "seven-day"],
      ["openai", "seven-day"],
    ])
  })
  it("does not substitute a short or model-scoped limit for an unavailable weekly limit", () => {
    const live = structuredClone(liveUsage)
    live.providers[0]!.windows[1]!.scopeModel = "sonnet"
    expect(defaultUsagePins(live)).toEqual([])
  })
  it("preserves a saved short-window choice", async () => {
    const pin = defaultUsagePins(liveUsage)[0]!
    mocks.settings.sidebarUsagePins = [{ ...pin, window: "five-hour", label: "5-hour limit" }]
    render(<Harness />)
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /Claude at 0 percent/ })).toBeTruthy(),
    )
    expect(mocks.settings.sidebarUsagePins[0]!.window).toBe("five-hour")
  })

  it("retains defaults across resets", async () => {
    const { rerender } = render(<Harness />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    const before = JSON.stringify(mocks.settings.sidebarUsagePins)
    const changed = structuredClone(liveUsage)
    changed.providers[0]!.windows[0]!.usedPercent = 99
    changed.providers[0]!.windows[1]!.usedPercent = 0
    rerender(<Harness live={changed} />)
    expect(JSON.stringify(mocks.settings.sidebarUsagePins)).toBe(before)
    expect(screen.getByRole("button", { name: /Claude at 0 percent/ })).toBeTruthy()
  })
  it("pins immediately and remembers explicit empty selection", async () => {
    const { unmount } = render(<Harness />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    fireEvent.click(screen.getByRole("button", { name: /Claude at 94 percent/ }))
    fireEvent.click(screen.getByRole("button", { name: /^Unpin Claude/ }))
    fireEvent.click(screen.getByRole("button", { name: /Pin Claude 5-hour/ }))
    expect(mocks.interactions.mock.calls.map(([event]) => event.action)).toEqual([
      "opened",
      "pin_removed",
      "pin_added",
    ])
    expect(document.querySelectorAll("[data-usage-limits-trigger]")).toHaveLength(
      defaultUsagePins(liveUsage).length,
    )
    for (const button of screen.getAllByRole("button", { name: /^Unpin / }))
      fireEvent.click(button)
    expect(document.querySelectorAll("[data-usage-limits-trigger][data-state]")).toHaveLength(0)
    unmount()
    render(<Harness />)
    expect(document.querySelectorAll("[data-usage-limits-trigger][data-state]")).toHaveLength(0)
    expect(screen.getByRole("button", { name: "Pin usage limits" })).toBeTruthy()
  })
  it("switches to bars, closes with the header button, and persists the display", async () => {
    const { unmount } = render(<Harness />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    fireEvent.click(screen.getByRole("button", { name: /Claude at 94 percent/ }))
    fireEvent.click(screen.getByRole("radio", { name: "Bars" }))
    expect(screen.getByRole("radio", { name: "Bars" })).toHaveAttribute("aria-checked", "true")
    fireEvent.click(screen.getByRole("button", { name: "Close usage limits" }))
    expect(mocks.interactions.mock.calls.map(([event]) => event.action)).toEqual([
      "opened",
      "appearance_bars",
      "closed",
    ])
    expect(screen.queryByRole("dialog")).toBeNull()
    unmount()
    render(<Harness />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    fireEvent.click(screen.getByRole("button", { name: /Claude at 94 percent/ }))
    expect(screen.getByRole("radio", { name: "Bars" })).toHaveAttribute("aria-checked", "true")
  })
  it("hides cached readings when the saved master switch turns off", async () => {
    render(<Harness />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    expect(screen.getByRole("button", { name: /Claude at 94 percent/ })).toBeTruthy()
    const pins = mocks.settings.sidebarUsagePins
    await act(async () => mocks.handler?.({ ...mocks.settings, liveUsageEnabled: false }))
    expect(screen.queryByRole("button", { name: /Claude at 94 percent/ })).toBeNull()
    const savedPin = screen.getByRole("button", { name: /Claude, .*unavailable/ })
    fireEvent.click(savedPin)
    expect(screen.getByRole("heading", { name: "See your usage limits" })).toBeTruthy()
    expect(mocks.settings.sidebarUsagePins).toEqual(pins)
  })
  it("keeps browser settings when a pin changes after choosing Bars", async () => {
    mocks.useBrowserPatch = true
    mocks.settings.theme = "dark"
    mocks.settings.sidebarUsagePins = defaultUsagePins(liveUsage)
    render(<Harness />)
    const trigger = await screen.findByRole("button", { name: /Claude at 94 percent/ })
    fireEvent.click(trigger)
    fireEvent.click(screen.getByRole("radio", { name: "Bars" }))
    await act(async () => {})
    fireEvent.click(screen.getByRole("button", { name: /^Unpin Claude/ }))
    await act(async () => {})
    expect(screen.getByRole("radio", { name: "Bars" })).toHaveAttribute("aria-checked", "true")
    expect(document.documentElement).toHaveAttribute("data-theme", "dark")
  })
  it("does not rebind missing accounts", async () => {
    const { rerender } = render(<Harness />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    const next = structuredClone(liveUsage)
    next.providers[0]!.accountKey = "different-account"
    rerender(<Harness live={next} />)
    expect(screen.getByRole("button", { name: /Claude, .*unavailable/ })).toBeTruthy()
  })
  it("rejects ambiguous fallback accounts", () => {
    const next = structuredClone(liveUsage)
    next.providers[0]!.accountKey = null
    next.providers.push(structuredClone(next.providers[0]!))
    expect(
      usageCandidates(next).filter(
        ({ reading }) => reading.provider === next.providers[0]!.provider,
      ),
    ).toHaveLength(0)
  })
  it("dismisses only a complete outside click and preserves selections and focus", async () => {
    render(<Harness />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    const trigger = screen.getByRole("button", { name: /Claude at 94 percent/ })
    trigger.focus()
    fireEvent.click(trigger)
    fireEvent.click(screen.getByRole("radio", { name: "Bars" }))
    const saved = JSON.stringify(mocks.settings.sidebarUsagePins)
    const dialog = screen.getByRole("dialog")
    const pointer = (target: Element, type: string) =>
      fireEvent(
        target,
        new MouseEvent(type, { bubbles: true, clientX: 10, clientY: 10, button: 0 }),
      )
    pointer(screen.getByText("Sidebar display"), "pointerdown")
    pointer(dialog, "pointerup")
    fireEvent.click(dialog, { clientX: 10, clientY: 10 })
    expect(screen.getByRole("dialog")).toBeTruthy()
    pointer(dialog, "pointerdown")
    pointer(dialog, "pointerup")
    fireEvent.click(dialog, { clientX: 10, clientY: 10 })
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(trigger).toHaveFocus()
    expect(JSON.stringify(mocks.settings.sidebarUsagePins)).toBe(saved)
    fireEvent.click(trigger)
    expect(screen.getByRole("radio", { name: "Bars" })).toHaveAttribute("aria-checked", "true")
  })
  it("guards duplicate refreshes and reports a failure without losing pins", async () => {
    let rejectRefresh!: (reason: Error) => void
    const refresh = vi.fn(
      () =>
        new Promise<void>((_, reject) => {
          rejectRefresh = reject
        }),
    )
    render(<Harness onRefresh={refresh} />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    fireEvent.click(screen.getByRole("button", { name: /Claude at 94 percent/ }))
    const button = screen.getByRole("button", { name: "Refresh usage limits" })
    fireEvent.click(button)
    fireEvent.click(button)
    expect(refresh).toHaveBeenCalledTimes(1)
    expect(button).toHaveAttribute("aria-busy", "true")
    rejectRefresh(new Error("offline"))
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("Could not refresh"),
    )
    expect(mocks.interactions.mock.calls.map(([event]) => event.action)).toEqual([
      "opened",
      "refresh_failed",
    ])
    expect(button).toHaveAttribute("aria-busy", "false")
    expect(screen.getAllByRole("button", { name: /^Unpin / })).toHaveLength(
      defaultUsagePins(liveUsage).length,
    )
  })
  it.each(["notSignedIn", "desktopOnly"] as const)(
    "shows %s as setup guidance after checking usage",
    async (detail) => {
      const result: LiveUsageSummaryPayload = {
        ...liveUsage,
        providers: [],
        errors: [
          {
            source: "test",
            provider: "anthropic",
            displayName: "Claude",
            category: "authentication",
            detail,
          },
        ],
      }
      render(<Harness live={result} onRefresh={async () => result} />)
      await act(async () => {})
      fireEvent.click(screen.getByRole("button", { name: "Pin usage limits" }))
      expect(screen.getByRole("heading", { name: "Your limits will appear here" })).toBeTruthy()
      fireEvent.click(screen.getByRole("button", { name: "Check again" }))
      await waitFor(() =>
        expect(screen.getByRole("status")).toHaveTextContent("Usage limits checked."),
      )
      expect(mocks.interactions.mock.calls.map(([event]) => event.action)).toEqual([
        "opened",
        "refresh_succeeded",
      ])
    },
  )
  it.each(["signInRequired", "cliMissing"] as const)(
    "reports %s as a failed refresh of an existing login",
    async (detail) => {
      const result: LiveUsageSummaryPayload = {
        ...liveUsage,
        providers: [],
        errors: [
          {
            source: "test",
            provider: "anthropic",
            displayName: "Claude",
            category: "authentication",
            detail,
          },
        ],
      }
      render(<Harness live={result} onRefresh={async () => result} />)
      await act(async () => {})
      fireEvent.click(screen.getByRole("button", { name: "Pin usage limits" }))
      expect(
        screen.getByRole("heading", { name: "Your limits couldn’t be updated" }),
      ).toBeTruthy()
      fireEvent.click(screen.getByRole("button", { name: "Check again" }))
      await waitFor(() =>
        expect(screen.getByRole("status")).toHaveTextContent(
          "Could not refresh usage limits. Try again.",
        ),
      )
      expect(mocks.interactions.mock.calls.map(([event]) => event.action)).toEqual([
        "opened",
        "refresh_failed",
      ])
    },
  )
  it.each([
    { provider: "openai", displayName: "Codex", freshness: "fresh", partial: true },
    { provider: "openai", displayName: "Codex", freshness: "stale", partial: false },
    { provider: "anthropic", displayName: "Claude", freshness: "fresh", partial: false },
  ] as const)(
    "reports partial=$partial when $provider fails and the Claude reading is $freshness",
    async ({ provider, displayName, freshness, partial }) => {
      const result: LiveUsageSummaryPayload = {
        ...liveUsage,
        providers: [{ ...liveUsage.providers[0]!, freshness }],
        errors: [{ source: "test", provider, displayName, category: "unavailable" }],
      }
      render(<Harness live={result} onRefresh={async () => result} />)
      await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
      fireEvent.click(screen.getByRole("button", { name: /Claude at 94 percent/ }))
      fireEvent.click(screen.getByRole("button", { name: "Refresh usage limits" }))
      await waitFor(() =>
        expect(screen.getByRole("status")).toHaveTextContent(
          partial
            ? "Some usage limits couldn’t be refreshed."
            : "Could not refresh usage limits. Try again.",
        ),
      )
      expect(mocks.interactions.mock.calls.map(([event]) => event.action)).toEqual([
        "opened",
        partial ? "refresh_succeeded" : "refresh_failed",
      ])
    },
  )
  it("restores focus after Escape", async () => {
    render(<Harness />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    const trigger = screen.getByRole("button", { name: /Claude at 94 percent/ })
    trigger.focus()
    fireEvent.click(trigger)
    expect(screen.getByRole("dialog")).toHaveAttribute("aria-modal", "true")
    fireEvent(
      screen.getByRole("dialog"),
      new Event("cancel", { bubbles: true, cancelable: true }),
    )
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(trigger).toHaveFocus()
  })
  it("keeps scoped limits distinct and restores focus when the opener disappears", async () => {
    const live = structuredClone(liveUsage)
    live.providers[0]!.windows[0]!.scopeModel = "sonnet"
    live.providers[0]!.windows.push({ ...live.providers[0]!.windows[0]!, scopeModel: "opus" })
    render(<Harness live={live} />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    const trigger = screen.getByRole("button", { name: /Claude at 94 percent/ })
    trigger.focus()
    fireEvent.click(trigger)
    for (const button of screen.getAllByRole("button", { name: /^Pin Claude/ }))
      fireEvent.click(button)
    expect(mocks.settings.sidebarUsagePins).toHaveLength(3)
    expect(new Set(mocks.settings.sidebarUsagePins?.map((pin) => pin.scope)).size).toBe(3)
    for (const button of screen.getAllByRole("button", { name: /^Unpin / }))
      fireEvent.click(button)
    fireEvent.click(screen.getByRole("button", { name: "Close usage limits" }))
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Pin usage limits" })).toHaveFocus(),
    )
  })
  it("renders only occupied rows for four pins and reports refresh success", async () => {
    const live = structuredClone(liveUsage)
    for (let i = 1; i < 4; i++)
      live.providers.push({
        ...structuredClone(live.providers[0]!),
        accountKey: `account-${i}`,
      })
    mocks.settings.sidebarUsagePins = live.providers.map(
      (provider) => defaultUsagePins({ ...live, providers: [provider] })[0]!,
    )
    render(<Harness live={live} />)
    await waitFor(() =>
      expect(document.querySelectorAll("[data-usage-limits-trigger]")).toHaveLength(4),
    )
    const grid = document.querySelector(".grid-cols-3")!
    expect(grid.children).toHaveLength(4)
    fireEvent.click(document.querySelector("[data-usage-limits-trigger]")!)
    fireEvent.click(screen.getByRole("radio", { name: "Bars" }))
    expect(document.querySelector(".grid-cols-2")?.children).toHaveLength(4)
    fireEvent.click(screen.getByRole("button", { name: "Refresh usage limits" }))
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("Usage limits refreshed."),
    )
  })
  it("waits for settings before selecting defaults and keeps provider updates pinned", async () => {
    mocks.settings.sidebarUsagePins = []
    const { rerender } = render(<Harness />)
    await act(async () => {})
    expect(mocks.settings.sidebarUsagePins).toEqual([])
    rerender(<Harness live={{ ...liveUsage, generatedAt: "2027-01-16T12:00:00Z" }} />)
    expect(document.querySelectorAll("[data-usage-limits-trigger]")).toHaveLength(1)
    expect(screen.getByRole("button", { name: "Pin usage limits" })).toBeTruthy()
  })
  it("does not start another provider request after closing and reopening during refresh", async () => {
    let complete!: () => void
    const refresh = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          complete = resolve
        }),
    )
    render(<Harness onRefresh={refresh} />)
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    fireEvent.click(screen.getByRole("button", { name: /Claude at 94 percent/ }))
    fireEvent.click(screen.getByRole("button", { name: "Refresh usage limits" }))
    fireEvent.click(screen.getByRole("button", { name: "Close usage limits" }))
    fireEvent.click(screen.getByRole("button", { name: /Claude at 94 percent/ }))
    fireEvent.click(screen.getByRole("button", { name: "Refresh usage limits" }))
    expect(refresh).toHaveBeenCalledTimes(1)
    complete()
    await waitFor(() =>
      expect(screen.getByRole("status")).toHaveTextContent("Usage limits refreshed."),
    )
  })
})

it("does not save fallback preferences after a failed settings read and recovers from an event", async () => {
  mocks.settings.analyticsEnabled = false
  mocks.settings.theme = "dark"
  mocks.failRead = true
  render(<Harness />)
  await act(async () => {})
  expect(mocks.writes).not.toHaveBeenCalled()
  expect(mocks.settings.analyticsEnabled).toBe(false)
  expect(mocks.settings.theme).toBe("dark")
  await act(async () => {
    mocks.handler?.(mocks.settings)
  })
  await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
  expect(mocks.writes).toHaveBeenCalledOnce()
  expect(mocks.settings.analyticsEnabled).toBe(false)
  expect(mocks.settings.theme).toBe("dark")
})

it.each([false, true])(
  "retries a failed settings read on focus when the subscription failure is %s",
  async (failWatch) => {
    mocks.failWatch = failWatch
    mocks.settings.analyticsEnabled = false
    mocks.settings.theme = "dark"
    mocks.failRead = true
    render(<Harness />)
    await act(async () => {})
    expect(mocks.writes).not.toHaveBeenCalled()
    mocks.failRead = false
    await act(async () => {
      window.dispatchEvent(new Event("focus"))
    })
    await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
    expect(mocks.writes).toHaveBeenCalledOnce()
    expect(mocks.settings.analyticsEnabled).toBe(false)
    expect(mocks.settings.theme).toBe("dark")
  },
)

it("patches sidebar preferences without replacing concurrent Settings changes", async () => {
  const { rerender } = render(<Harness live={null} />)
  await act(async () => {})
  // Hold the broadcast to keep the main window snapshot stale.
  mocks.settings = { ...mocks.settings, notificationsEnabled: false, analyticsEnabled: false }
  rerender(<Harness />)
  await waitFor(() => expect(mocks.settings.sidebarUsagePins).not.toBeNull())
  expect(mocks.settings.notificationsEnabled).toBe(false)
  expect(mocks.settings.analyticsEnabled).toBe(false)
  expect(mocks.fullWrites).not.toHaveBeenCalled()
  expect(Object.keys(mocks.writes.mock.calls[0]![0])).toEqual(["sidebarUsagePins"])
})

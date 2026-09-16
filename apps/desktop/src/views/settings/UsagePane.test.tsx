import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../../lib/ipc"
import type {
  AppSettings,
  LiveUsageMeterPayload,
  LiveUsageSourceErrorPayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from "../../lib/ipc"
import { UsagePane } from "./UsagePane"

const getLiveUsage = vi.hoisted(() => vi.fn())
const refreshLiveUsage = vi.hoisted(() => vi.fn())
const onLiveUsageChanged = vi.hoisted(() => vi.fn(async () => () => {}))

const platform = vi.hoisted(() => ({ mac: false }))
vi.mock("../../lib/platform", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  return { ...actual, isMacOS: () => platform.mac }
})

const openOverlayWindow = vi.hoisted(() => vi.fn(async () => {}))
const hideOverlayWindow = vi.hoisted(() => vi.fn(async () => {}))
const setFloatingHudEnabled = vi.hoisted(() => vi.fn())
const hudVisibility = vi.hoisted(() => ({
  visible: false,
  listeners: new Set<() => void>(),
}))
vi.mock("../../lib/overlayWindow", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  class HudVisibilitySession {
    getSnapshot = () => hudVisibility.visible
    subscribe = (listener: () => void) => {
      hudVisibility.listeners.add(listener)
      return () => hudVisibility.listeners.delete(listener)
    }
    set = (visible: boolean) => {
      emitHudVisibility(visible)
      setFloatingHudEnabled(visible)
      void (visible ? openOverlayWindow() : hideOverlayWindow())
    }
  }
  return {
    ...actual,
    HudVisibilitySession,
    openOverlayWindow,
    hideOverlayWindow,
    setFloatingHudEnabled,
  }
})

vi.mock("../../lib/ipc", async () => {
  const actual = await vi.importActual<typeof Ipc>("../../lib/ipc")
  return {
    ...actual,
    getLiveUsage,
    refreshLiveUsage,
    onLiveUsageChanged,
  }
})

const SETTINGS = { liveUsageEnabled: false } as unknown as AppSettings

function summary(overrides: Partial<LiveUsageSummaryPayload> = {}): LiveUsageSummaryPayload {
  return { providers: [], errors: [], meters: [], generatedAt: "", ...overrides }
}

function pane(settings: Partial<AppSettings> = {}, update = vi.fn()) {
  render(
    <UsagePane settings={{ ...SETTINGS, ...settings } as AppSettings} update={update} loaded />,
  )
  return update
}

function emitHudVisibility(visible: boolean) {
  hudVisibility.visible = visible
  for (const listener of hudVisibility.listeners) listener()
}

describe("UsagePane", () => {
  beforeEach(() => {
    getLiveUsage.mockReset()
    getLiveUsage.mockResolvedValue(summary())
    refreshLiveUsage.mockReset()
    refreshLiveUsage.mockResolvedValue(summary())
    onLiveUsageChanged.mockClear()
    platform.mac = false
  })

  it("names both consequences of the one switch", async () => {
    // A switch with two effects has to say both, or turning it off for one
    // reason surprises the reader with the other: it makes readings possible
    // at all *and* it lets milestone notifications fire.
    pane()
    const row = screen.getByText("Keep my plan limits current").closest("div")!
    expect(row).toHaveTextContent(/every five minutes in the background/i)
    expect(row).toHaveTextContent(/more often while visible/i)
    expect(row).toHaveTextContent(/that.s your own connection, made as you/i)
    expect(row).toHaveTextContent(/no antiburn server is involved/i)
    expect(row).toHaveTextContent(/milestone notifications/i)
  })

  it("says what happens with the switch off, rather than leaving it implied", async () => {
    pane()
    expect(screen.getByText("With this off").closest("div")).toHaveTextContent(
      /makes none of these requests and shows no plan limits/i,
    )
  })

  it("keeps asking while on screen and stops when it leaves", async () => {
    vi.useFakeTimers()
    try {
      const { unmount } = render(
        <UsagePane settings={SETTINGS as AppSettings} update={vi.fn()} loaded />,
      )
      // The subscribe path awaits the event listener before its first ask.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0)
      })
      expect(refreshLiveUsage).toHaveBeenCalledTimes(1)
      await act(async () => {
        await vi.advanceTimersByTimeAsync(60_000)
      })
      expect(refreshLiveUsage).toHaveBeenCalledTimes(2)
      unmount()
      await act(async () => {
        await vi.advanceTimersByTimeAsync(120_000)
      })
      expect(refreshLiveUsage).toHaveBeenCalledTimes(2)
    } finally {
      vi.useRealTimers()
    }
  })

  it("writes the preference through when the switch moves", async () => {
    const update = pane()
    fireEvent.click(screen.getByRole("switch", { name: /keep my plan limits current/i }))
    expect(update).toHaveBeenCalledWith({ liveUsageEnabled: true })
    await waitFor(() => expect(refreshLiveUsage).toHaveBeenCalled())
  })

  it("says the switches show meters and that sign-in happens in the tool", () => {
    pane()
    expect(screen.getByRole("heading", { name: "Track Limits for" })).toBeInTheDocument()
    expect(
      screen.getByText("You need to sign in inside each tool to track its limits."),
    ).toBeInTheDocument()
  })

  it.each<{ meter: LiveUsageMeterPayload; note: string }>([
    {
      meter: {
        provider: "google",
        displayName: "Google",
        shown: true,
        detection: "notInstalled",
      },
      note: "Couldn't find Antigravity or Antigravity usage on this computer.",
    },
    {
      meter: {
        provider: "anthropic",
        displayName: "Claude",
        shown: true,
        detection: "installedNotSignedIn",
      },
      note: "Found Claude Code, but it isn't signed in.",
    },
    {
      meter: {
        provider: "anthropic",
        displayName: "Claude",
        shown: true,
        detection: "signedIn",
      },
      note: "Signed in.",
    },
    {
      meter: { provider: "anthropic", displayName: "Claude", shown: true },
      note: "Not checked yet.",
    },
  ])(
    "explains $meter.provider detection $meter.detection without a reading",
    async ({ meter, note }) => {
      getLiveUsage.mockResolvedValue(summary({ meters: [meter] }))
      pane({ liveUsageEnabled: true })
      expect(await screen.findByText(note)).toBeInTheDocument()
      expect(
        screen.getByRole("switch", { name: `Show ${meter.displayName} meter` }),
      ).toBeChecked()
    },
  )

  it("names the tool a found login came from", async () => {
    getLiveUsage.mockResolvedValue(
      summary({
        meters: [
          {
            provider: "anthropic",
            displayName: "Claude",
            shown: true,
            detection: "signedIn",
            carrier: "pi",
            carrierLabel: "Pi",
          },
        ],
      }),
    )
    pane({ liveUsageEnabled: true })
    expect(await screen.findByText("Signed in through Pi.")).toBeInTheDocument()
  })

  it.each<{ error: LiveUsageSourceErrorPayload; note: string }>([
    {
      error: {
        source: "claude-usage-fetch",
        provider: "anthropic",
        displayName: "Claude",
        category: "unavailable",
        detail: "keychainUnreadable",
      },
      note: "Couldn't read Claude Code's login from the Keychain. If a prompt appears, choose Always Allow.",
    },
    {
      error: {
        source: "antigravity-usage-fetch",
        provider: "google",
        displayName: "Google",
        category: "authentication",
        detail: "refreshUnsupported",
      },
      note: "Antigravity's login has expired. Sign in inside Antigravity again.",
    },
  ])("shows $error.detail guidance before the detection note", async ({ error, note }) => {
    getLiveUsage.mockResolvedValue(
      summary({
        meters: [
          {
            provider: error.provider,
            displayName: error.displayName,
            shown: true,
            detection: "signedIn",
          },
        ],
        errors: [error],
      }),
    )
    pane({ liveUsageEnabled: true })
    expect(await screen.findByText(note)).toBeInTheDocument()
    expect(screen.queryByText(/^Signed in/)).not.toBeInTheDocument()
  })

  it("keeps the off-switch guidance and disables provider switches despite detection", async () => {
    getLiveUsage.mockResolvedValue(
      summary({
        meters: [
          { provider: "anthropic", displayName: "Claude", shown: true, detection: "signedIn" },
        ],
      }),
    )
    pane({ liveUsageEnabled: false })
    const label = await screen.findByText("Claude")
    expect(label.closest("div")).toHaveTextContent(
      "Turn the switch above back on to ask for current plan limits.",
    )
    expect(screen.getByRole("switch", { name: "Show Claude meter" })).toBeDisabled()
    expect(screen.queryByText(/^Signed in/)).not.toBeInTheDocument()
  })

  it("always offers the Google meter without a live reading", async () => {
    getLiveUsage.mockResolvedValue(summary())
    const update = pane({ liveUsageEnabled: true })
    await waitFor(() => expect(screen.getByText("Google")).toBeInTheDocument())
    const toggle = screen.getByRole("switch", { name: "Show Google meter" })
    expect(toggle).toBeChecked()
    expect(screen.getByText("Not checked yet.")).toBeInTheDocument()

    fireEvent.click(toggle)

    expect(update).toHaveBeenCalledWith({ liveUsageHiddenProviders: ["google"] })
  })

  it("turns each failure into something a reader could act on", async () => {
    getLiveUsage.mockResolvedValue(
      summary({
        errors: [
          {
            source: "claude-usage-fetch",
            provider: "anthropic",
            displayName: "Claude",
            category: "authentication",
          },
        ],
      }),
    )
    pane()
    await waitFor(() =>
      expect(
        screen.getByText("Claude sign-in expired. Sign in again, then retry."),
      ).toBeInTheDocument(),
    )
    // And it is not reported as "nothing found", which would send the reader
    // to use their coding tool when the problem is that they are signed out of it.
    expect(screen.getByText("Google")).toBeInTheDocument()
  })

  it("lists what each source can currently prove", async () => {
    getLiveUsage.mockResolvedValue(
      summary({
        providers: [
          {
            provider: "anthropic",
            accountKey: null,
            displayName: "Anthropic",
            support: "live",
            freshness: "fresh",
            sourceLabel: "Asked Claude directly",
            observedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
            windows: [],
            extraUsage: null,
            resetCredits: null,
            plan: null,
            accountUuid: null,
            accountEmail: null,
          },
        ],
      }),
    )
    pane()
    await waitFor(() => expect(screen.getByText("Anthropic")).toBeInTheDocument())
    expect(screen.getByText("Signed in · 0 limits tracked 5m ago")).toBeInTheDocument()
    expect(screen.queryByText(/Asked Claude directly/)).not.toBeInTheDocument()
  })

  it("counts only the limits the reader can see", async () => {
    // Codex on a Pro plan: one weekly primary window plus supplemental
    // per-feature windows the HUD hides until they show usage. The count
    // must match the bars, not the payload.
    const window = (overrides: Partial<LiveUsageWindowPayload>): LiveUsageWindowPayload => ({
      id: "weekly",
      role: "primaryLong",
      kind: "weekly",
      scopeModel: null,
      usedPercent: 17,
      startsAt: null,
      resetsAt: "2027-01-15T14:30:00Z",
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
      ...overrides,
    })
    getLiveUsage.mockResolvedValue(
      summary({
        providers: [
          {
            provider: "openai",
            accountKey: null,
            displayName: "Codex",
            support: "live",
            freshness: "fresh",
            sourceLabel: "Asked Codex directly",
            observedAt: new Date(Date.now() - 21_000).toISOString(),
            windows: [
              window({}),
              window({
                id: "weekly-code-review",
                role: "supplemental",
                scopeModel: "code-review",
                usedPercent: 0,
                hasNonzeroUsageInCurrentPeriod: false,
              }),
              window({
                id: "weekly-something",
                role: "supplemental",
                scopeModel: "something",
                usedPercent: 0,
                hasNonzeroUsageInCurrentPeriod: false,
              }),
            ],
            extraUsage: null,
            resetCredits: null,
            plan: null,
            accountUuid: null,
            accountEmail: null,
          },
        ],
      }),
    )
    pane()
    await waitFor(() => expect(screen.getByText("Codex")).toBeInTheDocument())
    expect(screen.getByText("Signed in · 1 limit tracked 21s ago")).toBeInTheDocument()
  })

  it("lists every provider it can meter, with nothing to report yet", async () => {
    // The roster, not the readings. A reader who has signed into neither tool
    // still sees what antiburn is able to meter.
    getLiveUsage.mockResolvedValue(
      summary({
        meters: [
          { provider: "anthropic", displayName: "Claude", shown: true },
          { provider: "openai", displayName: "Codex", shown: true },
        ],
      }),
    )
    pane({ liveUsageEnabled: true })
    await waitFor(() => expect(screen.getByText("Claude")).toBeInTheDocument())
    expect(screen.getByText("Codex")).toBeInTheDocument()
    expect(screen.getByText("Google")).toBeInTheDocument()
    const switches = screen.getAllByRole("switch", { name: /meter$/ })
    expect(switches.map((entry) => entry.getAttribute("aria-label"))).toEqual([
      "Show Claude meter",
      "Show Google meter",
      "Show Codex meter",
    ])
  })

  it("keeps a hidden provider's row, so the switch can be found again", async () => {
    // The regression the roster exists to prevent: hiding a meter stops the
    // request, so the provider reports nothing and a list built from readings
    // would lose the only control that turns it back on.
    getLiveUsage.mockResolvedValue(
      summary({
        meters: [{ provider: "openai", displayName: "Codex", shown: false }],
      }),
    )
    pane({ liveUsageEnabled: true, liveUsageHiddenProviders: ["openai"] })
    await waitFor(() => expect(screen.getByText("Codex")).toBeInTheDocument())
    expect(screen.getByRole("switch", { name: "Show Codex meter" })).not.toBeChecked()
  })

  it("names both consequences of hiding one provider", async () => {
    // Same rule as the master switch above it: a switch that stops the request
    // also stops that provider's milestones, and has to say so.
    getLiveUsage.mockResolvedValue(
      summary({
        meters: [{ provider: "openai", displayName: "Codex", shown: false }],
      }),
    )
    pane({ liveUsageEnabled: true, liveUsageHiddenProviders: ["openai"] })
    await waitFor(() => expect(screen.getByText("Codex")).toBeInTheDocument())
    const row = screen.getByText("Codex").closest("div")!
    expect(row).toHaveTextContent(/does not ask Codex for usage/i)
    expect(row).toHaveTextContent(/milestone notifications do not fire/i)
  })

  it("writes the hidden set when a meter switch moves", async () => {
    getLiveUsage.mockResolvedValue(
      summary({
        meters: [
          { provider: "anthropic", displayName: "Claude", shown: true },
          { provider: "openai", displayName: "Codex", shown: true },
        ],
      }),
    )
    const update = pane({ liveUsageEnabled: true, liveUsageHiddenProviders: ["anthropic"] })
    await waitFor(() => expect(screen.getByText("Codex")).toBeInTheDocument())

    fireEvent.click(screen.getByRole("switch", { name: "Show Codex meter" }))
    // The provider already hidden stays hidden: one switch moves one meter.
    expect(update).toHaveBeenCalledWith({
      liveUsageHiddenProviders: ["anthropic", "openai"],
    })

    fireEvent.click(screen.getByRole("switch", { name: "Show Claude meter" }))
    expect(update).toHaveBeenCalledWith({ liveUsageHiddenProviders: [] })
    await waitFor(() => expect(refreshLiveUsage).toHaveBeenCalled())
  })

  it("keeps Google available when the shell request fails", async () => {
    getLiveUsage.mockRejectedValue(new Error("no shell"))
    pane()
    await waitFor(() => expect(screen.getByText("Google")).toBeInTheDocument())
  })
})

describe("UsagePane — the grace period", () => {
  const GENERATED_AT = "2027-01-15T12:00:00Z"

  function withGracedReading(observedAt: string) {
    return summary({
      generatedAt: GENERATED_AT,
      providers: [
        {
          provider: "anthropic",
          accountKey: null,
          displayName: "Anthropic",
          support: "live",
          freshness: "fresh",
          sourceLabel: "Asked Claude directly",
          observedAt,
          windows: [],
          extraUsage: null,
          resetCredits: null,
          plan: null,
          accountUuid: null,
          accountEmail: null,
        },
      ],
      errors: [
        {
          source: "claude-usage-fetch",
          provider: "anthropic",
          displayName: "Claude",
          category: "rateLimited",
        },
      ],
    })
  }

  beforeEach(() => {
    getLiveUsage.mockReset()
    refreshLiveUsage.mockReset()
    refreshLiveUsage.mockResolvedValue(summary())
  })

  it("keeps the last reading beside a failed check, however old", async () => {
    // A rate limit is a provider answering: the sign-in worked. The row keeps
    // the figure from the earlier check, that check's own time, and the reason.
    for (const observedAt of ["2027-01-15T11:56:00Z", "2027-01-15T11:49:00Z"]) {
      getLiveUsage.mockResolvedValue(withGracedReading(observedAt))
      const { unmount } = render(
        <UsagePane
          settings={{ ...SETTINGS, liveUsageEnabled: true }}
          update={vi.fn()}
          loaded
        />,
      )
      await waitFor(() => expect(screen.getByText("Anthropic")).toBeInTheDocument())
      expect(
        screen.getByText(/^Signed in · 0 limits tracked .* · rate limited$/),
      ).toBeInTheDocument()
      expect(screen.queryByText(/Wait, then retry/)).not.toBeInTheDocument()
      unmount()
    }
  })

  it("says signed in for a rate limit with no reading yet", async () => {
    getLiveUsage.mockResolvedValue(
      summary({
        generatedAt: GENERATED_AT,
        providers: [],
        errors: [
          {
            source: "claude-usage-fetch",
            provider: "anthropic",
            displayName: "Claude",
            category: "rateLimited",
          },
        ],
        meters: [{ provider: "anthropic", displayName: "Claude", shown: true }],
      }),
    )
    pane({ liveUsageEnabled: true })
    await waitFor(() => expect(screen.getByText("Claude")).toBeInTheDocument())
    expect(screen.getByText("Signed in · rate limited · retrying")).toBeInTheDocument()
  })

  it("keeps the reading at the grace boundary too", async () => {
    // Exactly 10 minutes before `GENERATED_AT` — LIVE_USAGE_GRACE_MS itself.
    getLiveUsage.mockResolvedValue(withGracedReading("2027-01-15T11:50:00Z"))
    pane()
    await waitFor(() => expect(screen.getByText("Anthropic")).toBeInTheDocument())
    expect(screen.getByText(/^Signed in · 0 limits tracked/)).toBeInTheDocument()
  })
})

describe("UsagePane — floating HUD", () => {
  beforeEach(() => {
    getLiveUsage.mockReset()
    getLiveUsage.mockResolvedValue(summary())
    refreshLiveUsage.mockResolvedValue(summary())
    platform.mac = true
    hudVisibility.visible = false
    hudVisibility.listeners.clear()
    setFloatingHudEnabled.mockClear()
    openOverlayWindow.mockClear()
    hideOverlayWindow.mockClear()
  })

  it("offers the HUD only on macOS", () => {
    platform.mac = false
    pane()
    expect(screen.queryByText("Floating HUD")).not.toBeInTheDocument()
  })

  it("opens the HUD and stores the preference", () => {
    pane()
    fireEvent.click(screen.getByRole("switch", { name: "Show floating usage HUD" }))
    expect(setFloatingHudEnabled).toHaveBeenCalledWith(true)
    expect(openOverlayWindow).toHaveBeenCalled()
  })

  it("reads the preference and hides the HUD", () => {
    hudVisibility.visible = true
    pane()
    const toggle = screen.getByRole("switch", { name: "Show floating usage HUD" })
    expect(toggle).toBeChecked()
    fireEvent.click(toggle)
    expect(setFloatingHudEnabled).toHaveBeenCalledWith(false)
    expect(hideOverlayWindow).toHaveBeenCalled()
  })

  it("turns off when the native HUD closes", () => {
    hudVisibility.visible = true
    pane()
    const toggle = screen.getByRole("switch", { name: "Show floating usage HUD" })
    expect(toggle).toBeChecked()

    act(() => emitHudVisibility(false))

    expect(toggle).not.toBeChecked()
  })
})

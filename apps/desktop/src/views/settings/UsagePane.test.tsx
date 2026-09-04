import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type * as Ipc from "../../lib/ipc"
import type { AppSettings, LiveUsageSummaryPayload } from "../../lib/ipc"
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
  return { ...actual, getLiveUsage, refreshLiveUsage, onLiveUsageChanged }
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

  it("writes the preference through when the switch moves", async () => {
    const update = pane()
    fireEvent.click(screen.getByRole("switch", { name: /keep my plan limits current/i }))
    expect(update).toHaveBeenCalledWith({ liveUsageEnabled: true })
    await waitFor(() => expect(refreshLiveUsage).toHaveBeenCalled())
  })

  it("always offers the Google meter without a live reading", async () => {
    getLiveUsage.mockResolvedValue(summary())
    const update = pane({ liveUsageEnabled: true })
    await waitFor(() => expect(screen.getByText("Google")).toBeInTheDocument())
    const toggle = screen.getByRole("switch", { name: "Show Google meter" })
    expect(toggle).toBeChecked()
    expect(screen.getByText(/No readings yet\. Sign in with Google/)).toBeInTheDocument()

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
      expect(screen.getByText(/sign in again with your coding tool/i)).toBeInTheDocument(),
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
    expect(screen.getByText(/Asked Claude directly/)).toBeInTheDocument()
    expect(screen.getByText("Live 5m ago")).toBeInTheDocument()
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

  it("replaces the failure note with a grace note while the reading is within its window", async () => {
    // 4 minutes before `GENERATED_AT`.
    getLiveUsage.mockResolvedValue(withGracedReading("2027-01-15T11:56:00Z"))
    pane()
    await waitFor(() => expect(screen.getByText("Anthropic")).toBeInTheDocument())
    expect(
      screen.getByText(
        "Asked Claude directly. 0 limits reported. Claude rate limited the last check; reading from 4 min ago.",
      ),
    ).toBeInTheDocument()
    expect(screen.queryByText(/Wait, then retry/)).not.toBeInTheDocument()
  })

  it("drops the reading and falls back to the plain failure note once past the grace", async () => {
    // 11 minutes before `GENERATED_AT`.
    getLiveUsage.mockResolvedValue(withGracedReading("2027-01-15T11:49:00Z"))
    pane()
    await waitFor(() => expect(screen.getByText("Anthropic")).toBeInTheDocument())
    expect(
      screen.getByText(/rate limited usage checks\. Wait, then retry\./),
    ).toBeInTheDocument()
    expect(screen.queryByText(/Asked Claude directly/)).not.toBeInTheDocument()
  })

  it("reads exactly the grace boundary as still shown", async () => {
    // Exactly 10 minutes before `GENERATED_AT` — LIVE_USAGE_GRACE_MS itself.
    getLiveUsage.mockResolvedValue(withGracedReading("2027-01-15T11:50:00Z"))
    pane()
    await waitFor(() => expect(screen.getByText("Anthropic")).toBeInTheDocument())
    expect(screen.getByText(/Asked Claude directly/)).toBeInTheDocument()
    expect(screen.queryByText(/Wait, then retry/)).not.toBeInTheDocument()
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

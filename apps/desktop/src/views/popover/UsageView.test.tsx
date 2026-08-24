// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen, within } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type {
  LiveProviderUsagePayload,
  LiveUsageForecastPayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
  ProviderUsagePayload,
  ProviderUsageSummaryPayload,
  ProviderUsageWindowPayload,
} from "../../lib/ipc"
import { UsageView } from "./UsageView"

const platform = vi.hoisted(() => ({ mac: false }))
vi.mock("../../lib/platform", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  return { ...actual, isMacOS: () => platform.mac }
})

const hudWindow = vi.hoisted(() => ({ visible: false }))
const openOverlayWindow = vi.hoisted(() => vi.fn(async () => {}))
const hideOverlayWindow = vi.hoisted(() => vi.fn(async () => {}))
const setFloatingHudEnabled = vi.hoisted(() => vi.fn())
vi.mock("../../lib/overlayWindow", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>()
  class HudVisibilitySession {
    private listeners = new Set<() => void>()
    private visible = hudWindow.visible
    getSnapshot = () => this.visible
    subscribe = (listener: () => void) => {
      this.listeners.add(listener)
      return () => this.listeners.delete(listener)
    }
    toggle = () => {
      this.visible = !this.visible
      setFloatingHudEnabled(this.visible)
      void (this.visible ? openOverlayWindow() : hideOverlayWindow())
      for (const listener of this.listeners) listener()
    }
  }
  return { ...actual, HudVisibilitySession }
})

function usageWindow(
  overrides: Partial<ProviderUsageWindowPayload> = {},
): ProviderUsageWindowPayload {
  return {
    tokensIn: 0,
    tokensOut: 0,
    cacheRead: 0,
    estimatedUsd: null,
    sessionCount: 0,
    ...overrides,
  }
}

/** Anthropic, busy today; OpenAI, busy only earlier in the week. */
const ANTHROPIC: ProviderUsagePayload = {
  provider: "anthropic",
  displayName: "Anthropic",
  state: "estimated",
  staleness: "fresh",
  windows: {
    today: usageWindow({ estimatedUsd: 2.5, tokensIn: 1_000, sessionCount: 1 }),
    week: usageWindow({ estimatedUsd: 8, tokensIn: 4_000, sessionCount: 4 }),
    month: usageWindow({ estimatedUsd: 8, tokensIn: 4_000, sessionCount: 4 }),
  },
  lastActivityAt: new Date().toISOString(),
}

const OPENAI: ProviderUsagePayload = {
  provider: "openai",
  displayName: "OpenAI",
  state: "observed",
  staleness: "stale",
  windows: {
    today: usageWindow(),
    week: usageWindow({ tokensIn: 20_000, sessionCount: 2 }),
    month: usageWindow({ tokensIn: 20_000, sessionCount: 2 }),
  },
  lastActivityAt: new Date(Date.now() - 3 * 86_400_000).toISOString(),
}

function summary(
  overrides: Partial<ProviderUsageSummaryPayload> = {},
): ProviderUsageSummaryPayload {
  return {
    providers: [ANTHROPIC, OPENAI],
    generatedAt: "2027-01-15T08:00:00Z",
    ...overrides,
  }
}

describe("UsageView", () => {
  it("sections current work first: used-today providers under Recently used", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    expect(screen.getByRole("heading", { name: "Usage" })).toBeInTheDocument()

    const recent = within(screen.getByRole("region", { name: "Recently used" }))
    expect(recent.getByText("Anthropic")).toBeInTheDocument()
    expect(recent.getByText("Used today")).toBeInTheDocument()

    const rest = within(screen.getByRole("region", { name: "All detected" }))
    expect(rest.getByText("OpenAI")).toBeInTheDocument()
    expect(rest.queryByText("Used today")).not.toBeInTheDocument()
  })

  it("shows every window on one card, with sessions beside each figure", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    const card = screen.getByText("Anthropic").closest("li")
    expect(card).not.toBeNull()
    expect(within(card!).getByText("Today")).toBeInTheDocument()
    expect(within(card!).getByText("Last 7 days")).toBeInTheDocument()
    expect(within(card!).getByText("This month")).toBeInTheDocument()
    // "$2.50" appears twice by design: the Today's-spend metric row and the
    // Today window row describe the same day.
    expect(within(card!).getAllByText("$2.50")).toHaveLength(2)
    expect(within(card!).getAllByText("$8.00")).toHaveLength(2)
    expect(within(card!).getByText("1 session")).toBeInTheDocument()
  })

  it("derives the metric block from the reader’s own windows", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    const card = screen.getByText("Anthropic").closest("li")
    expect(within(card!).getByText("Today's spend")).toBeInTheDocument()
    expect(within(card!).getByText("Today's tokens")).toBeInTheDocument()
    // 1,000 today vs (4,000 − 1,000)/6 = 500 per day → 2.0× and rising.
    expect(within(card!).getByText(/Picking up · 2\.0×/)).toBeInTheDocument()
  })

  it("marks an unpriced provider observed and shows its tokens instead of a dollar zero", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    const card = screen.getByText("OpenAI").closest("li")
    expect(card).not.toBeNull()
    expect(within(card!).getByText("Observed")).toBeInTheDocument()
    expect(within(card!).getAllByText("20.0k").length).toBeGreaterThan(0)
    expect(within(card!).getByText(/Last used 3d ago/)).toBeInTheDocument()
    // Nothing today against a real weekly baseline reads as easing off.
    expect(within(card!).getByText(/Easing · <0\.1×/)).toBeInTheDocument()
  })

  it("explains how local spend estimates are calculated", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    expect(screen.getByText(/window of its most recent activity/)).toBeInTheDocument()
    expect(screen.getByText(/Not a bill/)).toBeInTheDocument()
  })

  it("is honest when there is nothing to show", () => {
    render(<UsageView summary={summary({ providers: [] })} onBack={vi.fn()} />)

    expect(screen.getByText("No local evidence yet")).toBeInTheDocument()
  })

  it("goes back to the activity list", () => {
    const onBack = vi.fn()
    render(<UsageView summary={summary()} onBack={onBack} />)

    fireEvent.click(screen.getByRole("button", { name: "Back to activity" }))
    expect(onBack).toHaveBeenCalledTimes(1)
  })
})

describe("UsageWindowRows shares", () => {
  it("fills each bar with the window’s share of the provider’s own month", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    const card = screen.getByText("Anthropic").closest("li")!
    // Fixture: today $2.50 of this month's $8.00 → 31% (rounded).
    expect(within(card).getByTestId("usage-share-today")).toHaveStyle({ width: "31%" })
    expect(within(card).getByTestId("usage-share-month")).toHaveStyle({ width: "100%" })
  })
})

/* -------------------------------------------------------------------------
 * The layered limit half
 * ---------------------------------------------------------------------- */

const NOW = Date.parse("2027-01-15T12:00:00Z")

/** No forecast by default: sparse history is the resting state of a source
    that only moves when an agent runs. */
export const NO_FORECAST: LiveUsageForecastPayload = {
  unavailableReason: "sparseHistory",
  confidence: null,
  consumptionRate: null,
  paceRatio: null,
  paceTrend: null,
  runwayAt: null,
  usedToday: null,
}

function liveWindow(overrides: Partial<LiveUsageWindowPayload> = {}): LiveUsageWindowPayload {
  return {
    id: "five-hour",
    role: "primaryShort",
    kind: "rolling",
    scopeModel: null,
    usedPercent: 81,
    startsAt: "2027-01-15T09:30:00Z",
    resetsAt: "2027-01-15T14:30:00Z",
    hasNonzeroUsageInCurrentPeriod: false,
    forecast: NO_FORECAST,
    ...overrides,
  }
}

function liveProvider(): LiveProviderUsagePayload {
  return {
    provider: "anthropic",
    displayName: "Anthropic",
    support: "live",
    freshness: "fresh",
    sourceLabel: "Asked Claude directly",
    observedAt: new Date(Date.now() - 5 * 60_000).toISOString(),
    windows: [
      liveWindow(),
      liveWindow({
        id: "seven-day",
        role: "primaryLong",
        kind: "weekly",
        usedPercent: 65,
        startsAt: "2027-01-12T18:00:00Z",
        resetsAt: "2027-01-19T18:00:00Z",
      }),
    ],
    extraUsage: null,
    resetCredits: null,
  }
}

function live(overrides: Partial<LiveUsageSummaryPayload> = {}): LiveUsageSummaryPayload {
  return {
    providers: [liveProvider()],
    errors: [],
    generatedAt: "2027-01-15T12:00:00Z",
    ...overrides,
  }
}

describe("UsageView — plan limits layered over local estimates", () => {
  it("puts the provider’s limits above the spend estimates on the same card", () => {
    render(<UsageView summary={summary()} live={live()} now={NOW} onBack={vi.fn()} />)

    const card = screen.getByText("Anthropic").closest("li")!
    const limits = within(card).getByRole("region", { name: "Anthropic plan limits" })
    expect(within(limits).getByText("5-hour limit")).toBeInTheDocument()
    expect(within(limits).getByText("81%")).toBeInTheDocument()
    expect(within(limits).getByText("Weekly limit")).toBeInTheDocument()
    expect(within(limits).getByText(/resets 2:30pm/)).toBeInTheDocument()

    // The estimate half is still there and unchanged: a reader who connects a
    // source gains the limits, they do not trade one surface for the other.
    expect(within(card).getByText("Last 7 days")).toBeInTheDocument()
    expect(within(card).getByTestId("usage-share-today")).toBeInTheDocument()

    // And the limits come first in the document, not after.
    expect(
      limits.compareDocumentPosition(within(card).getByText("Last 7 days")) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
  })

  it("shows manual reset credits with the Codex command that uses one", () => {
    const codex = live({
      providers: [
        {
          ...liveProvider(),
          provider: "openai",
          displayName: "Codex",
          resetCredits: { availableCount: 1 },
        },
      ],
    })
    render(<UsageView summary={summary()} live={codex} now={NOW} onBack={vi.fn()} />)

    const card = screen.getByText("OpenAI").closest("li")!
    expect(within(card).getByText("1 usage limit reset available.")).toBeInTheDocument()
    expect(within(card).getByText("/usage")).toBeInTheDocument()
    expect(within(card).getByText(/in Codex to use one/)).toBeInTheDocument()
  })

  it("marks how far through the period the clock has travelled", () => {
    render(<UsageView summary={summary()} live={live()} now={NOW} onBack={vi.fn()} />)

    // 09:30 → 14:30 with the clock at 12:00 is half the period gone against
    // 81% of the allowance: the whole point of showing both.
    const bar = screen.getByRole("progressbar", { name: "5-hour limit" })
    expect(bar).toHaveAttribute("aria-valuenow", "81")
    expect(bar).toHaveAttribute("aria-valuetext", "81% used; 50% of the period elapsed")
    expect(screen.getByTestId("live-usage-elapsed-five-hour")).toHaveStyle({
      left: "50%",
    })
  })

  it("renders an unknown percentage as indeterminate rather than empty", () => {
    const unknown = live()
    unknown.providers = [{ ...liveProvider(), windows: [liveWindow({ usedPercent: null })] }]
    render(<UsageView summary={summary()} live={unknown} now={NOW} onBack={vi.fn()} />)

    expect(screen.getByText("Unknown")).toBeInTheDocument()
    const bar = screen.getByRole("progressbar", { name: "5-hour limit" })
    // No `aria-valuenow` at all: a progressbar without one is announced as
    // indeterminate, which is the truth. A zero would be a claim.
    expect(bar).not.toHaveAttribute("aria-valuenow")
    expect(screen.queryByTestId("live-usage-fill-five-hour")).not.toBeInTheDocument()
  })

  it("marks a weekly window from its kind alone, with no stated start", () => {
    // Seven days is what "weekly" means, the same way five hours is what
    // "five-hour" means — this is arithmetic on the window's own identity,
    // not a guess.
    const bounded = live({
      providers: [
        {
          ...liveProvider(),
          windows: [
            liveWindow({
              id: "seven-day",
              role: "primaryLong",
              kind: "weekly",
              startsAt: null,
              resetsAt: "2027-01-19T18:00:00Z",
            }),
          ],
        },
      ],
    })
    render(<UsageView summary={summary()} live={bounded} now={NOW} onBack={vi.fn()} />)

    // 2027-01-12T18:00 → 2027-01-19T18:00, clock at 2027-01-15T12:00: 66 of
    // 168 hours into a seven-day period.
    expect(screen.getByRole("progressbar", { name: "Weekly limit" })).toHaveAttribute(
      "aria-valuetext",
      "81% used; 39% of the period elapsed",
    )
    expect(screen.getByTestId("live-usage-elapsed-seven-day")).toBeInTheDocument()
  })

  it("omits the marker for a window whose period is not stated or implied", () => {
    // A monthly window with no start: a month's length genuinely varies, so
    // "thirty days before the reset" would be a guess dressed as a
    // measurement — unlike weekly or daily, monthly gets no marker.
    const unbounded = live({
      providers: [
        {
          ...liveProvider(),
          windows: [
            liveWindow({
              id: "monthly-window",
              role: "other",
              kind: "monthly",
              startsAt: null,
            }),
          ],
        },
      ],
    })
    render(<UsageView summary={summary()} live={unbounded} now={NOW} onBack={vi.fn()} />)

    expect(screen.queryByTestId("live-usage-elapsed-monthly-window")).not.toBeInTheDocument()
    expect(screen.getByRole("progressbar", { name: "Monthly limit" })).toHaveAttribute(
      "aria-valuetext",
      "81% used",
    )
  })

  it("says a reading is stale without removing it", () => {
    render(
      <UsageView
        summary={summary()}
        live={live({ providers: [{ ...liveProvider(), freshness: "stale" }] })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(screen.getByText(/may have moved since/)).toBeInTheDocument()
    // Still the best figures anyone has, so they stay on screen.
    expect(screen.getByText("81%")).toBeInTheDocument()
  })

  it("shows nothing at all where no source could prove a limit", () => {
    render(<UsageView summary={summary()} live={live()} now={NOW} onBack={vi.fn()} />)

    // Anthropic has a live reading; OpenAI does not, and gets no empty frame.
    const openai = screen.getByText("OpenAI").closest("li")!
    expect(
      within(openai).queryByRole("region", { name: /plan limits/ }),
    ).not.toBeInTheDocument()
    expect(within(openai).getByText("Last 7 days")).toBeInTheDocument()
  })

  it("falls back to the estimate surface alone when no live payload is given", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    expect(screen.queryByRole("region", { name: /plan limits/ })).not.toBeInTheDocument()
    // One per provider card, and both still there.
    expect(screen.getAllByText("Last 7 days")).toHaveLength(2)
  })

  it("banners a signed-out source, and only that failure", () => {
    render(
      <UsageView
        summary={summary()}
        live={live({
          errors: [
            {
              source: "claude-usage-fetch",
              provider: "anthropic",
              displayName: "Claude",
              category: "authentication",
            },
          ],
        })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )
    expect(screen.getByRole("status")).toHaveTextContent(/sign in again/i)
  })

  it("separates the two halves in the footer, so neither claim covers the other", () => {
    render(<UsageView summary={summary()} live={live()} now={NOW} onBack={vi.fn()} />)

    expect(screen.getByText(/Spend figures are local estimates/)).toBeInTheDocument()
    expect(screen.getByText(/Plan limits are your provider.s own figures/)).toBeInTheDocument()
  })
})

describe("UsageView — what history says about a limit", () => {
  const withForecast = (overrides: Partial<LiveUsageForecastPayload>) =>
    live({
      providers: [
        {
          ...liveProvider(),
          windows: [
            liveWindow({
              usedPercent: 60,
              resetsAt: "2027-01-15T14:00:00Z",
              forecast: { ...NO_FORECAST, unavailableReason: null, ...overrides },
            }),
          ],
        },
      ],
    })

  it("reports pace, trend and runway when the series supports them", () => {
    render(
      <UsageView
        summary={summary()}
        live={withForecast({
          confidence: "high",
          consumptionRate: 12.5,
          paceRatio: 1.25,
          paceTrend: 1.4,
          runwayAt: "2027-01-15T13:12:00Z",
        })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const card = screen.getByText("Anthropic").closest("li")!
    expect(within(card).getByText("Running hot · 1.3× · 12.5%/hour")).toBeInTheDocument()
    expect(within(card).getByText("Picking up · 1.4×")).toBeInTheDocument()
    expect(within(card).getByText("Runs out in 1h 12m")).toBeInTheDocument()
  })

  it("keeps the rows and gives the reason when the series does not", () => {
    render(<UsageView summary={summary()} live={live()} now={NOW} onBack={vi.fn()} />)

    const card = screen.getByText("Anthropic").closest("li")!
    const rows = within(card).getByRole("group", { name: /pace$/ })
    expect(within(rows).getByText("Pace")).toBeInTheDocument()
    expect(within(rows).getByText("Runway")).toBeInTheDocument()
    // Three questions still asked; the answer is why we cannot answer them.
    expect(within(rows).getAllByText("Not enough history")).toHaveLength(3)
  })

  it("distinguishes a window that just reset from one with no history at all", () => {
    render(
      <UsageView
        summary={summary()}
        live={withForecast({ unavailableReason: "transition" })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )
    // The numbers are fine and simply too new — a different message from
    // "come back later", and a different one again from "go use your agent".
    expect(screen.getAllByText("Just reset").length).toBeGreaterThan(0)
  })
})

describe("UsageView — HUD pop-out", () => {
  beforeEach(() => {
    platform.mac = true
    hudWindow.visible = false
    openOverlayWindow.mockClear()
    hideOverlayWindow.mockClear()
    setFloatingHudEnabled.mockClear()
  })

  it("offers the pop-out only on macOS", () => {
    platform.mac = false
    render(<UsageView summary={summary()} onBack={vi.fn()} />)
    expect(
      screen.queryByRole("button", { name: /floating usage hud/i }),
    ).not.toBeInTheDocument()
  })

  it("opens the HUD and records the preference", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)
    const button = screen.getByRole("button", { name: "Show the floating usage HUD" })
    fireEvent.click(button)
    expect(openOverlayWindow).toHaveBeenCalled()
    expect(setFloatingHudEnabled).toHaveBeenCalledWith(true)
    expect(button).toHaveAttribute("aria-pressed", "true")
  })

  it("reflects a visible HUD and hides it on the second press", () => {
    hudWindow.visible = true
    render(<UsageView summary={summary()} onBack={vi.fn()} />)
    const button = screen.getByRole("button", { name: "Hide the floating usage HUD" })
    fireEvent.click(button)
    expect(hideOverlayWindow).toHaveBeenCalled()
    expect(setFloatingHudEnabled).toHaveBeenCalledWith(false)
    expect(button).toHaveAttribute("aria-pressed", "false")
  })
})

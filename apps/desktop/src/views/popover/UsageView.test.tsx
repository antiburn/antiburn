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
    costComplete: true,
    sessionCount: 0,
    ...overrides,
  }
}

/** Anthropic, busy today; OpenAI, busy only earlier in the week. */
const ANTHROPIC: ProviderUsagePayload = {
  provider: "anthropic",
  accountKey: null,
  displayName: "Anthropic",
  state: "estimated",
  staleness: "fresh",
  windows: {
    today: usageWindow({ estimatedUsd: 2.5, tokensIn: 1_000, sessionCount: 1 }),
    week: usageWindow({ estimatedUsd: 8, tokensIn: 4_000, sessionCount: 4 }),
    monthToDate: usageWindow({ estimatedUsd: 8, tokensIn: 4_000, sessionCount: 4 }),
    last30Days: usageWindow({ estimatedUsd: 8, tokensIn: 4_000, sessionCount: 4 }),
  },
  agents: [],
  lastActivityAt: new Date().toISOString(),
}

const OPENAI: ProviderUsagePayload = {
  provider: "openai",
  accountKey: null,
  displayName: "OpenAI",
  state: "observed",
  staleness: "stale",
  windows: {
    today: usageWindow(),
    week: usageWindow({ tokensIn: 20_000, sessionCount: 2 }),
    monthToDate: usageWindow({ tokensIn: 20_000, sessionCount: 2 }),
    last30Days: usageWindow({ tokensIn: 20_000, sessionCount: 2 }),
  },
  agents: [],
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
    expect(recent.getByRole("heading", { name: "Recently used" })).toBeInTheDocument()
    expect(recent.getByText("Anthropic")).toBeInTheDocument()
    expect(recent.getByText("Used today")).toBeInTheDocument()

    const rest = within(screen.getByRole("region", { name: "All detected" }))
    expect(rest.getByRole("heading", { name: "All detected" })).toBeInTheDocument()
    expect(rest.getByText("OpenAI")).toBeInTheDocument()
    expect(rest.queryByText("Used today")).not.toBeInTheDocument()
  })

  it("puts Unattributed last and starts its card collapsed", () => {
    const unattributed: ProviderUsagePayload = {
      ...ANTHROPIC,
      provider: "unknown",
      displayName: "Unattributed",
      windows: {
        ...ANTHROPIC.windows,
        today: usageWindow({ tokensIn: 50_000, sessionCount: 3 }),
      },
    }
    render(
      <UsageView
        summary={summary({ providers: [unattributed, ANTHROPIC, OPENAI] })}
        onBack={vi.fn()}
      />,
    )

    const cards = [...document.querySelectorAll<HTMLElement>("[data-provider-card]")]
    expect(cards.map((card) => card.dataset.providerCard)).toEqual([
      "anthropic",
      "openai",
      "unknown",
    ])
    const toggle = screen.getByRole("button", { name: "Expand Unattributed usage" })
    expect(toggle).toHaveAttribute("aria-expanded", "false")
    const bodyId = toggle.getAttribute("aria-controls")!
    expect(document.getElementById(bodyId)).toBeInTheDocument()
    expect(within(cards[2]!).getByText("Trend")).not.toBeVisible()

    fireEvent.click(toggle)

    expect(toggle).toHaveAttribute("aria-expanded", "true")
    expect(toggle).toHaveAccessibleName("Collapse Unattributed usage")
    expect(within(cards[2]!).getByText("Trend")).toBeInTheDocument()
  })

  it("hides embedded section headings but keeps their accessible regions", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} embedded />)

    expect(screen.queryByRole("heading", { name: "Usage" })).not.toBeInTheDocument()

    const recent = within(screen.getByRole("region", { name: "Recently used" }))
    expect(recent.queryByRole("heading", { name: "Recently used" })).not.toBeInTheDocument()
    expect(recent.getByText("Anthropic")).toBeInTheDocument()

    const rest = within(screen.getByRole("region", { name: "All detected" }))
    expect(rest.queryByRole("heading", { name: "All detected" })).not.toBeInTheDocument()
    expect(rest.getByText("OpenAI")).toBeInTheDocument()
  })

  it("shows every window on one card, with sessions beside each figure", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    const card = screen.getByText("Anthropic").closest("li")
    expect(card).not.toBeNull()
    expect(within(card!).getByText("Today")).toBeInTheDocument()
    expect(within(card!).getByText("This week")).toBeInTheDocument()
    expect(within(card!).getByText("Last 30 days")).toBeInTheDocument()
    expect(within(card!).getAllByText("$2.50 · 1.0k")).toHaveLength(1)
    expect(within(card!).getAllByText("$8.00 · 4.0k")).toHaveLength(2)
    expect(within(card!).getByText("1 session")).toBeInTheDocument()
  })

  it("keeps only the local spend trend above the windows", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    const card = screen.getByText("Anthropic").closest("li")
    expect(within(card!).queryByText("Today's spend")).not.toBeInTheDocument()
    expect(within(card!).queryByText("Today's tokens")).not.toBeInTheDocument()
    // 1,000 today vs (4,000 − 1,000)/6 = 500 per day → 2.0× and rising.
    expect(within(card!).getByText(/Picking up · 2\.0×/)).toBeInTheDocument()
  })

  it("marks an unpriced provider observed and shows its tokens instead of a dollar zero", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    const card = screen.getByText("OpenAI").closest("li")
    expect(card).not.toBeNull()
    expect(within(card!).queryByText("Observed")).not.toBeInTheDocument()
    expect(within(card!).getAllByText("20.0k").length).toBeGreaterThan(0)
    expect(within(card!).getByText(/Last used 3d ago/)).toBeInTheDocument()
    // Nothing today against a real weekly baseline reads as easing off.
    expect(within(card!).getByText(/Easing · <0\.1×/)).toBeInTheDocument()
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
    expect(within(card).getByTestId("usage-share-last30Days")).toHaveStyle({ width: "100%" })
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

function liveProvider(
  overrides: Partial<LiveProviderUsagePayload> = {},
): LiveProviderUsagePayload {
  return {
    provider: "anthropic",
    accountKey: null,
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
    plan: null,
    accountUuid: null,
    accountEmail: null,
    ...overrides,
  }
}

function live(overrides: Partial<LiveUsageSummaryPayload> = {}): LiveUsageSummaryPayload {
  return {
    providers: [liveProvider()],
    errors: [],
    meters: [],
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
    expect(within(card).getByText("This week")).toBeInTheDocument()
    expect(within(card).getByTestId("usage-share-today")).toBeInTheDocument()

    // And the limits come first in the document, not after.
    expect(
      limits.compareDocumentPosition(within(card).getByText("This week")) &
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

  it("does not give Google users the Codex reset command", () => {
    const antigravity = live({
      providers: [
        liveProvider({
          provider: "google",
          displayName: "Google",
          sourceLabel: "Asked Antigravity directly",
          resetCredits: { availableCount: 2 },
        }),
      ],
    })
    render(
      <UsageView
        summary={summary({ providers: [] })}
        live={antigravity}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(screen.getByText("2 usage limit resets available.")).toBeInTheDocument()
    expect(screen.getByText(/Use Google to apply one/)).toBeInTheDocument()
    expect(screen.queryByText("/usage")).not.toBeInTheDocument()
  })

  it("joins Google live limits to a local Google estimate", () => {
    const google: ProviderUsagePayload = {
      ...ANTHROPIC,
      provider: "google",
      displayName: "Google",
    }
    const antigravity = live({
      providers: [
        liveProvider({
          provider: "google",
          displayName: "Google",
          sourceLabel: "Asked Antigravity directly",
          windows: [
            liveWindow({
              id: "antigravity-gemini-5h",
              scopeModel: "Gemini",
            }),
          ],
        }),
      ],
    })
    render(
      <UsageView
        summary={summary({ providers: [google] })}
        live={antigravity}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const card = screen.getByText("Google").closest("li")!
    expect(screen.getAllByRole("listitem")).toHaveLength(1)
    expect(within(card).getByText("Gemini 5-hour limit")).toBeInTheDocument()
    expect(within(card).getByText("This week")).toBeInTheDocument()
    expect(within(card).getByText(/Asked Antigravity directly · Live/)).toBeInTheDocument()
  })

  it("creates a Google card when Antigravity has no local estimate", () => {
    const antigravity = live({
      providers: [
        liveProvider({
          provider: "google",
          displayName: "Google",
          sourceLabel: "Asked Antigravity directly",
          plan: { name: "Google AI Pro", tier: "pro-tier" },
          windows: [
            liveWindow({ id: "antigravity-gemini-5h", scopeModel: "Gemini" }),
            liveWindow({
              id: "antigravity-gemini-weekly",
              role: "primaryLong",
              kind: "weekly",
              scopeModel: "Gemini",
            }),
            liveWindow({
              id: "antigravity-claude-gpt-5h",
              role: "supplemental",
              scopeModel: "Claude + GPT",
            }),
            liveWindow({
              id: "antigravity-claude-gpt-weekly",
              role: "supplemental",
              kind: "weekly",
              scopeModel: "Claude + GPT",
            }),
            liveWindow({
              id: "weekly-gemini-3-pro",
              role: "supplemental",
              kind: "weekly",
              scopeModel: "Gemini 3 Pro",
              usedPercent: 0,
            }),
            liveWindow({
              id: "weekly-gemini-3-flash",
              role: "supplemental",
              kind: "weekly",
              scopeModel: "Gemini 3 Flash",
              usedPercent: null,
            }),
            liveWindow({
              id: "weekly-claude-sonnet",
              role: "supplemental",
              kind: "weekly",
              scopeModel: "Claude Sonnet",
              usedPercent: 0,
            }),
          ],
          extraUsage: {
            enabled: true,
            usedPercent: null,
            used: 250,
            remaining: 750,
            limit: 1_000,
            currency: null,
          },
        }),
      ],
    })
    render(
      <UsageView
        summary={summary({ providers: [] })}
        live={antigravity}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const card = screen.getByText("Google", { selector: "h3" }).closest("li")!
    expect(within(card).getByRole("region", { name: "Google plan limits" })).toBeInTheDocument()
    expect(within(card).getByRole("heading", { level: 3 })).toHaveTextContent(
      "Google · Google AI Pro",
    )
    expect(within(card).getByText("AI credits: 750 remaining")).toBeInTheDocument()
    expect(within(card).queryByText("This week")).not.toBeInTheDocument()
    expect(card.querySelector('[data-provider-icon="google"]')).toBeInTheDocument()

    for (const name of [
      "Gemini 5-hour limit",
      "Gemini weekly limit",
      "Claude and GPT 5-hour limit",
      "Claude and GPT weekly limit",
    ]) {
      expect(within(card).getByRole("progressbar", { name })).toBeInTheDocument()
    }
    expect(within(card).queryByText("Gemini 3 Pro weekly limit")).not.toBeInTheDocument()
    expect(within(card).queryByText("Claude Sonnet weekly limit")).not.toBeInTheDocument()
  })

  it("collapses unassigned usage when multiple accounts exist", () => {
    const google: ProviderUsagePayload = {
      ...ANTHROPIC,
      provider: "google",
      displayName: "Google",
    }
    const first = liveProvider({
      provider: "google",
      accountKey: "account-b",
      displayName: "Google",
      sourceLabel: "Asked Antigravity directly",
      windows: [liveWindow({ id: "antigravity-gemini-5h", scopeModel: "Gemini" })],
    })
    const second = {
      ...first,
      accountKey: "account-a",
      windows: [
        liveWindow({
          id: "antigravity-gemini-weekly",
          role: "primaryLong",
          kind: "weekly",
          scopeModel: "Gemini",
        }),
      ],
    }
    render(
      <UsageView
        summary={summary({ providers: [google] })}
        live={live({ providers: [first, second] })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const card = document.querySelector<HTMLElement>('[data-provider-card="google"]')!
    expect(screen.getAllByRole("listitem")).toHaveLength(1)
    expect(
      within(card).getByRole("region", { name: "Google Account 1 plan limits" }),
    ).toHaveTextContent("Gemini 5-hour limit")
    expect(
      within(card).getByRole("region", { name: "Google Account 2 plan limits" }),
    ).toHaveTextContent("Gemini weekly limit")
    const unassigned = within(card).getByRole("button", { name: "Unassigned account" })
    expect(unassigned).toHaveAttribute("aria-expanded", "false")
    expect(within(card).queryByText("This week")).not.toBeInTheDocument()

    fireEvent.click(unassigned)
    expect(unassigned).toHaveAttribute("aria-expanded", "true")
    expect(within(card).getAllByText("This week")).toHaveLength(1)
  })

  it("shows unassigned sessions under the only account", () => {
    const assigned: ProviderUsagePayload = {
      ...ANTHROPIC,
      provider: "google",
      accountKey: "account-a",
      displayName: "Google",
      windows: {
        today: usageWindow({ estimatedUsd: 1, tokensIn: 500, sessionCount: 1 }),
        week: usageWindow({ estimatedUsd: 1, tokensIn: 500, sessionCount: 1 }),
        monthToDate: usageWindow({ estimatedUsd: 1, tokensIn: 500, sessionCount: 1 }),
        last30Days: usageWindow({ estimatedUsd: 1, tokensIn: 500, sessionCount: 1 }),
      },
    }
    const unassigned: ProviderUsagePayload = {
      ...ANTHROPIC,
      provider: "google",
      displayName: "Google",
    }
    const account = liveProvider({
      provider: "google",
      accountKey: "account-a",
      displayName: "Google",
    })

    render(
      <UsageView
        summary={summary({ providers: [assigned, unassigned] })}
        live={live({ providers: [account] })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const card = document.querySelector<HTMLElement>('[data-provider-card="google"]')!
    expect(within(card).queryByText("Unassigned account")).not.toBeInTheDocument()
    expect(within(card).getAllByText("$3.50 · 1.5k").length).toBeGreaterThan(0)
    expect(within(card).getByText("2 sessions")).toBeInTheDocument()
  })

  it("places identified local usage under only its matching live account", () => {
    const accountA: ProviderUsagePayload = {
      ...ANTHROPIC,
      provider: "google",
      accountKey: "account-a",
      displayName: "Google",
      windows: {
        today: usageWindow({ estimatedUsd: 1, sessionCount: 1 }),
        week: usageWindow({ estimatedUsd: 1, sessionCount: 1 }),
        monthToDate: usageWindow({ estimatedUsd: 1, sessionCount: 1 }),
        last30Days: usageWindow({ estimatedUsd: 1, sessionCount: 1 }),
      },
    }
    const accountB: ProviderUsagePayload = {
      ...accountA,
      accountKey: "account-b",
      windows: {
        today: usageWindow({ estimatedUsd: 2, sessionCount: 1 }),
        week: usageWindow({ estimatedUsd: 2, sessionCount: 1 }),
        monthToDate: usageWindow({ estimatedUsd: 2, sessionCount: 1 }),
        last30Days: usageWindow({ estimatedUsd: 2, sessionCount: 1 }),
      },
    }
    const first = liveProvider({
      provider: "google",
      accountKey: "account-b",
      displayName: "Google",
      windows: [liveWindow({ id: "account-b-window" })],
    })
    const second = liveProvider({
      provider: "google",
      accountKey: "account-a",
      displayName: "Google",
      windows: [liveWindow({ id: "account-a-window" })],
    })

    render(
      <UsageView
        summary={summary({ providers: [accountA, accountB] })}
        live={live({ providers: [first, second] })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const firstGroup = screen.getByRole("region", {
      name: "Google Account 1 plan limits",
    }).parentElement!
    const secondGroup = screen.getByRole("region", {
      name: "Google Account 2 plan limits",
    }).parentElement!
    expect(within(firstGroup).getAllByText("$2.00 · 0").length).toBeGreaterThan(0)
    expect(within(firstGroup).queryByText("$1.00 · 0")).not.toBeInTheDocument()
    expect(within(secondGroup).getAllByText("$1.00 · 0").length).toBeGreaterThan(0)
    expect(within(secondGroup).queryByText("$2.00 · 0")).not.toBeInTheDocument()
  })

  it("keeps account sections mounted through reorder, polling, and removal", () => {
    const first = liveProvider({
      provider: "google",
      accountKey: "account-a",
      displayName: "Google",
      sourceLabel: "Asked Antigravity directly",
      windows: [liveWindow({ id: "antigravity-gemini-5h", scopeModel: "Gemini" })],
    })
    const second = {
      ...first,
      accountKey: "account-b",
      windows: [
        liveWindow({
          id: "antigravity-gemini-weekly",
          role: "primaryLong",
          kind: "weekly",
          scopeModel: "Gemini",
        }),
      ],
    }
    const initial = live({ providers: [second, first] })
    const { rerender } = render(
      <UsageView
        summary={summary({ providers: [] })}
        live={initial}
        now={NOW}
        onBack={vi.fn()}
      />,
    )
    const card = document.querySelector<HTMLElement>('[data-provider-card="google"]')!
    const accountA = within(card).getByRole("region", { name: "Google Account 2 plan limits" })

    rerender(
      <UsageView
        summary={summary({ providers: [] })}
        live={live({
          providers: [
            { ...first, observedAt: "2027-01-15T12:01:00Z" },
            { ...second, observedAt: "2027-01-15T12:01:00Z" },
          ],
        })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(document.querySelector('[data-provider-card="google"]')).toBe(card)
    expect(within(card).getByRole("region", { name: "Google Account 2 plan limits" })).toBe(
      accountA,
    )

    rerender(
      <UsageView
        summary={summary({ providers: [] })}
        live={live({ providers: [{ ...first, observedAt: "2027-01-15T12:02:00Z" }] })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )
    expect(within(card).getByRole("region", { name: "Google plan limits" })).toBe(accountA)
  })

  it("keeps an unassigned live account mounted when its plan and windows change", () => {
    const initial = liveProvider({
      provider: "google",
      displayName: "Google",
      sourceLabel: "Read from Antigravity IDE",
      plan: { name: "Google AI Pro", tier: "pro-tier" },
      windows: [liveWindow({ id: "antigravity-gemini-5h" })],
    })
    const { rerender } = render(
      <UsageView
        summary={summary({ providers: [] })}
        live={live({ providers: [initial] })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )
    const account = screen.getByRole("region", { name: "Google plan limits" })

    rerender(
      <UsageView
        summary={summary({ providers: [] })}
        live={live({
          providers: [
            {
              ...initial,
              plan: { name: "Google AI Ultra", tier: "ultra-tier" },
              windows: [
                ...initial.windows,
                liveWindow({ id: "antigravity-gemini-weekly", kind: "weekly" }),
              ],
            },
          ],
        })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(screen.getByRole("region", { name: "Google plan limits" })).toBe(account)
  })

  it("names multiple local-only accounts without exposing their keys", () => {
    const accountA: ProviderUsagePayload = {
      ...ANTHROPIC,
      accountKey: "opaque-account-a",
    }
    const accountB: ProviderUsagePayload = {
      ...ANTHROPIC,
      accountKey: "opaque-account-b",
      windows: {
        ...ANTHROPIC.windows,
        today: usageWindow({ estimatedUsd: 2, sessionCount: 1 }),
      },
    }
    render(
      <UsageView
        summary={summary({ providers: [accountA, accountB] })}
        live={live({ providers: [] })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(screen.getByRole("group", { name: "Anthropic Account 1 usage" })).toBeInTheDocument()
    expect(screen.getByRole("group", { name: "Anthropic Account 2 usage" })).toBeInTheDocument()
    expect(document.body).not.toHaveTextContent("opaque-account-a")
    expect(document.body).not.toHaveTextContent("opaque-account-b")
  })

  it("shows different plans inside their account sections", () => {
    const base = liveProvider({
      provider: "google",
      displayName: "Google",
      sourceLabel: "Asked Antigravity directly",
      windows: [liveWindow({ id: "antigravity-gemini-5h", scopeModel: "Gemini" })],
    })
    render(
      <UsageView
        summary={summary({ providers: [] })}
        live={live({
          providers: [
            {
              ...base,
              accountKey: "account-b",
              plan: { name: "Google AI Ultra", tier: "ultra-tier" },
            },
            {
              ...base,
              accountKey: "account-a",
              plan: { name: "Google AI Pro", tier: "pro-tier" },
            },
          ],
        })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(screen.getByRole("heading", { level: 3, name: "Google" })).not.toHaveTextContent(
      "Google AI",
    )
    expect(
      screen.getByRole("region", { name: "Google Account 1 plan limits" }),
    ).toHaveTextContent("Plan · Google AI Ultra")
    expect(
      screen.getByRole("region", { name: "Google Account 2 plan limits" }),
    ).toHaveTextContent("Plan · Google AI Pro")
  })

  it("shows local Antigravity fallback windows and their provenance at zero and unknown", () => {
    const fallback = liveProvider({
      provider: "google",
      displayName: "Google",
      sourceLabel: "Read from the Antigravity CLI",
      windows: [
        liveWindow({
          id: "antigravity-model-gemini-3-pro",
          role: "supplemental",
          scopeModel: "Gemini 3 Pro",
          usedPercent: 0,
        }),
        liveWindow({
          id: "antigravity-model-claude-sonnet",
          role: "supplemental",
          scopeModel: "Claude Sonnet",
          usedPercent: null,
        }),
      ],
    })
    render(
      <UsageView
        summary={summary({ providers: [] })}
        live={live({ providers: [fallback] })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(screen.getByText(/Read from the Antigravity CLI · Live/)).toBeInTheDocument()
    expect(screen.getByRole("progressbar", { name: "Gemini 3 Pro limit" })).toHaveAttribute(
      "aria-valuenow",
      "0",
    )
    expect(
      screen.getByRole("progressbar", { name: "Claude Sonnet limit" }),
    ).not.toHaveAttribute("aria-valuenow")
  })

  it("shows a credits-only Google snapshot with provenance", () => {
    const credits = liveProvider({
      provider: "google",
      displayName: "Google",
      sourceLabel: "Asked Antigravity directly",
      windows: [],
      extraUsage: {
        enabled: true,
        usedPercent: null,
        used: null,
        remaining: null,
        limit: 1_000,
        currency: null,
      },
    })
    render(
      <UsageView
        summary={summary({ providers: [] })}
        live={live({ providers: [credits] })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(screen.getByText("AI credits: 1,000 total")).toBeInTheDocument()
    expect(screen.getByText(/Asked Antigravity directly · Live/)).toBeInTheDocument()
  })

  it("shows the plan as a muted suffix on the provider card heading", () => {
    // Anthropic is the only provider in "Recently used" for this fixture, so
    // its region carries exactly one provider card and one h3.
    const pro = live({
      providers: [{ ...liveProvider(), plan: { name: "pro", tier: null } }],
    })
    render(<UsageView summary={summary()} live={pro} now={NOW} onBack={vi.fn()} />)

    const recent = within(screen.getByRole("region", { name: "Recently used" }))
    const heading = recent.getByRole("heading", { level: 3 })
    expect(heading).toHaveTextContent("Anthropic · Pro")
    const suffix = within(heading).getByText("· Pro")
    expect(suffix.className).toContain("text-label-secondary")
  })

  it("omits the separator and suffix entirely when the source reports no plan", () => {
    // liveProvider()'s default carries no plan. The heading stays the bare
    // provider name, with no trailing separator left dangling.
    render(<UsageView summary={summary()} live={live()} now={NOW} onBack={vi.fn()} />)

    const recent = within(screen.getByRole("region", { name: "Recently used" }))
    const heading = recent.getByRole("heading", { level: 3 })
    expect(heading).toHaveTextContent("Anthropic")
    expect(heading.textContent).not.toContain("·")
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
    expect(within(openai).getByText("This week")).toBeInTheDocument()
  })

  it("falls back to the estimate surface alone when no live payload is given", () => {
    render(<UsageView summary={summary()} onBack={vi.fn()} />)

    expect(screen.queryByRole("region", { name: /plan limits/ })).not.toBeInTheDocument()
    // One per provider card, and both remain without live limits.
    expect(screen.getAllByText("This week")).toHaveLength(2)
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

  it("preserves the legacy provider-less authentication banner", () => {
    render(
      <UsageView
        summary={summary({ providers: [] })}
        live={live({
          providers: [],
          errors: [
            {
              source: "legacy-auth",
              provider: "",
              displayName: "",
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

  it("shows rate-limit, schema, and unavailable live failures", () => {
    render(
      <UsageView
        summary={summary({ providers: [] })}
        live={live({
          providers: [],
          errors: [
            {
              source: "google-rate-limit",
              provider: "google",
              displayName: "Antigravity",
              category: "rateLimited",
            },
            {
              source: "claude-schema",
              provider: "anthropic",
              displayName: "Claude",
              category: "schema",
            },
            {
              source: "codex-unavailable",
              provider: "openai",
              displayName: "Codex",
              category: "unavailable",
            },
          ],
        })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const statuses = screen.getAllByRole("status")
    expect(statuses).toHaveLength(3)
    expect(
      screen.getByText("Google rate limited usage checks. Wait, then retry."),
    ).toBeInTheDocument()
    expect(
      screen.getByText("Claude usage changed. Update antiburn, then retry."),
    ).toBeInTheDocument()
    expect(
      screen.getByText("Codex usage is unavailable. Check your connection, then retry."),
    ).toBeInTheDocument()
  })
})

describe("UsageView — the grace period", () => {
  function withGracedReading(observedAt: string) {
    return live({
      providers: [{ ...liveProvider(), observedAt }],
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

  it("shows the reading and a grace note instead of the orange failure note", () => {
    // 4 minutes before `live()`'s generatedAt of 12:00:00Z.
    render(
      <UsageView
        summary={summary()}
        live={withGracedReading("2027-01-15T11:56:00Z")}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const card = screen.getByText("Anthropic").closest("li")!
    expect(
      within(card).getByRole("region", { name: "Anthropic plan limits" }),
    ).toBeInTheDocument()
    expect(
      within(card).getByText("Claude rate limited the last check; reading from 4 min ago."),
    ).toBeInTheDocument()
    expect(within(card).queryByRole("status")).not.toBeInTheDocument()
  })

  it("hides a reading past its grace period and keeps the orange failure note", () => {
    // 11 minutes before `live()`'s generatedAt of 12:00:00Z.
    render(
      <UsageView
        summary={summary()}
        live={withGracedReading("2027-01-15T11:49:00Z")}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const card = screen.getByText("Anthropic").closest("li")!
    expect(
      within(card).queryByRole("region", { name: "Anthropic plan limits" }),
    ).not.toBeInTheDocument()
    expect(within(card).getByRole("status")).toHaveTextContent(
      "Claude rate limited usage checks. Wait, then retry.",
    )
  })

  it("reads exactly the grace boundary as still shown", () => {
    // Exactly 10 minutes before `live()`'s generatedAt — LIVE_USAGE_GRACE_MS
    // itself.
    render(
      <UsageView
        summary={summary()}
        live={withGracedReading("2027-01-15T11:50:00Z")}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    const card = screen.getByText("Anthropic").closest("li")!
    expect(
      within(card).getByRole("region", { name: "Anthropic plan limits" }),
    ).toBeInTheDocument()
    expect(within(card).queryByRole("status")).not.toBeInTheDocument()
  })

  it("changes nothing about a live reading with no error", () => {
    render(<UsageView summary={summary()} live={live()} now={NOW} onBack={vi.fn()} />)

    const card = screen.getByText("Anthropic").closest("li")!
    expect(
      within(card).getByRole("region", { name: "Anthropic plan limits" }),
    ).toBeInTheDocument()
    expect(within(card).queryByRole("status")).not.toBeInTheDocument()
    expect(within(card).queryByText(/rate limited/)).not.toBeInTheDocument()
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

  it("reports pace and runway when the series supports them", () => {
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
    expect(within(card).queryByText("Picking up · 1.4×")).not.toBeInTheDocument()
    expect(within(card).getByText("Runs out in 1h 12m")).toBeInTheDocument()
  })

  it("keeps the rows and gives the reason when the series does not", () => {
    render(<UsageView summary={summary()} live={live()} now={NOW} onBack={vi.fn()} />)

    const card = screen.getByText("Anthropic").closest("li")!
    const rows = within(card).getByRole("group", { name: /pace$/ })
    expect(within(rows).getByText("Pace")).toBeInTheDocument()
    expect(within(rows).getByText("Runway")).toBeInTheDocument()
    // Two questions still asked; the answer is why we cannot answer them.
    expect(within(rows).getAllByText("Not enough history")).toHaveLength(2)
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

describe("UsageView — every meter turned off", () => {
  it("names the reader's own choice instead of leaving the limits missing", () => {
    render(
      <UsageView
        summary={summary()}
        live={live({
          providers: [],
          meters: [
            { provider: "anthropic", displayName: "Claude", shown: false },
            { provider: "openai", displayName: "Codex", shown: false },
          ],
        })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(screen.getByText(/No meter selected\./)).toBeInTheDocument()
    // The estimate half survives: hiding a meter hides a provider's own
    // figures, not what this machine measured.
    expect(screen.getByText("Anthropic")).toBeInTheDocument()
  })

  it("says nothing while one meter is still shown", () => {
    render(
      <UsageView
        summary={summary()}
        live={live({
          meters: [
            { provider: "anthropic", displayName: "Claude", shown: true },
            { provider: "openai", displayName: "Codex", shown: false },
          ],
        })}
        now={NOW}
        onBack={vi.fn()}
      />,
    )

    expect(screen.queryByText(/No meter selected\./)).not.toBeInTheDocument()
  })
})

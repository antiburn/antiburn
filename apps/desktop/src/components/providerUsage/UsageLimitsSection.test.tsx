// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import type {
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
  ProviderUsagePayload,
  ProviderUsageWindowPayload,
} from "../../lib/ipc"
import { UsageLimitsSection } from "./UsageLimitsSection"

const FORECAST = {
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
    id: "seven-day",
    role: "primaryLong",
    kind: "weekly",
    scopeModel: null,
    usedPercent: 42,
    startsAt: null,
    resetsAt: null,
    hasNonzeroUsageInCurrentPeriod: false,
    forecast: FORECAST,
    ...overrides,
  }
}

function liveProvider(
  overrides: Partial<LiveProviderUsagePayload> = {},
): LiveProviderUsagePayload {
  return {
    provider: "anthropic",
    displayName: "Claude",
    support: "live",
    freshness: "fresh",
    sourceLabel: "Asked Claude directly",
    observedAt: new Date(Date.now() - 3 * 60_000).toISOString(),
    windows: [liveWindow()],
    extraUsage: null,
    ...overrides,
  }
}

function liveSummary(
  providers: readonly LiveProviderUsagePayload[] = [liveProvider()],
): LiveUsageSummaryPayload {
  return { providers: [...providers], errors: [], generatedAt: new Date().toISOString() }
}

function spendWindow(
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

function spendProvider(overrides: Partial<ProviderUsagePayload> = {}): ProviderUsagePayload {
  const today = spendWindow({ estimatedUsd: 1.25, tokensIn: 1_000, sessionCount: 1 })
  return {
    provider: "anthropic",
    displayName: "Claude",
    state: "estimated",
    staleness: "fresh",
    windows: { today, week: today, month: today },
    lastActivityAt: new Date().toISOString(),
    ...overrides,
  }
}

function section(over: Partial<Parameters<typeof UsageLimitsSection>[0]> = {}) {
  return render(
    <UsageLimitsSection
      providers={[spendProvider()]}
      live={liveSummary()}
      expanded={true}
      onToggleExpanded={vi.fn()}
      refreshing={false}
      onViewAll={vi.fn()}
      {...over}
    />,
  )
}

describe("UsageLimitsSection — visibility", () => {
  it("renders nothing when no provider has live limit data", () => {
    const { container } = section({ live: liveSummary([]) })
    expect(container).toBeEmptyDOMElement()
  })

  it("renders nothing when a provider reports but states no windows worth showing", () => {
    const { container } = section({ live: liveSummary([liveProvider({ windows: [] })]) })
    expect(container).toBeEmptyDOMElement()
  })
})

describe("UsageLimitsSection — expanded", () => {
  it("shows one subsection per provider with limit data, each with its own rows", () => {
    section({
      live: liveSummary([
        liveProvider({ provider: "anthropic", displayName: "Claude" }),
        liveProvider({
          provider: "openai",
          displayName: "Codex",
          windows: [liveWindow({ id: "five-hour", role: "primaryShort", usedPercent: 10 })],
        }),
      ]),
    })

    expect(screen.getByText("Claude")).toBeInTheDocument()
    expect(screen.getByText("Codex")).toBeInTheDocument()
    expect(screen.getAllByRole("progressbar").length).toBe(2)
  })

  it("carries a freshness note next to each provider's name", () => {
    section()
    expect(screen.getByText("Live 3m ago")).toBeInTheDocument()
  })

  it("does not show pace, trend, runway, or spend — only the limit rows", () => {
    section()
    expect(screen.queryByText(/pace/i)).not.toBeInTheDocument()
    expect(screen.queryByText(/runway/i)).not.toBeInTheDocument()
    expect(screen.queryByText(/trend/i)).not.toBeInTheDocument()
  })

  it("omits the chip row entirely while expanded", () => {
    section()
    expect(screen.queryByTestId("provider-usage-chips")).not.toBeInTheDocument()
  })

  it("overlays the toggle on the corner rather than spending a row on it", () => {
    const { container } = section()
    const toggle = screen.getByRole("button", { name: "Collapse usage limits" })
    // Overlaid, not its own flow row: the toggle's row lives outside the
    // document's normal layout, so it costs no vertical space of its own.
    expect(toggle.parentElement).toHaveClass("absolute")
    expect(container.querySelector('[data-testid="usage-limits-section"]')).toHaveClass(
      "relative",
    )
  })

  it("reserves right-hand space in only the first subsection's header, clear of the overlay", () => {
    section({
      live: liveSummary([
        liveProvider({ provider: "anthropic", displayName: "Claude" }),
        liveProvider({
          provider: "openai",
          displayName: "Codex",
          windows: [liveWindow({ id: "five-hour", role: "primaryShort", usedPercent: 10 })],
        }),
      ]),
    })

    const firstHeader = screen.getByText("Claude").parentElement
    const secondHeader = screen.getByText("Codex").parentElement
    expect(firstHeader).toHaveClass("pr-8")
    expect(secondHeader).not.toHaveClass("pr-8")
  })
})

describe("UsageLimitsSection — collapsed", () => {
  it("shows the same chip row the bottom bar used to carry", () => {
    section({ expanded: false })
    expect(screen.getByTestId("provider-usage-chips")).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Claude, $1.25 today, estimated, weekly limit 42%" }),
    ).toBeInTheDocument()
  })

  it("does not show the per-provider rows while collapsed", () => {
    section({ expanded: false })
    expect(screen.queryByRole("progressbar")).not.toBeInTheDocument()
  })
})

describe("UsageLimitsSection — toggle and spinner", () => {
  it("toggles through the chevron button, in either state", () => {
    const onToggleExpanded = vi.fn()
    const { rerender } = section({ expanded: true, onToggleExpanded })

    fireEvent.click(screen.getByRole("button", { name: "Collapse usage limits" }))
    expect(onToggleExpanded).toHaveBeenCalledTimes(1)

    rerender(
      <UsageLimitsSection
        providers={[spendProvider()]}
        live={liveSummary()}
        expanded={false}
        onToggleExpanded={onToggleExpanded}
        refreshing={false}
        onViewAll={vi.fn()}
      />,
    )
    fireEvent.click(screen.getByRole("button", { name: "Expand usage limits" }))
    expect(onToggleExpanded).toHaveBeenCalledTimes(2)
  })

  it("shows a spinner while a refresh is in flight, and hides it once settled", () => {
    const { rerender } = section({ refreshing: true })
    expect(screen.getByRole("status")).toBeInTheDocument()

    rerender(
      <UsageLimitsSection
        providers={[spendProvider()]}
        live={liveSummary()}
        expanded={true}
        onToggleExpanded={vi.fn()}
        refreshing={false}
        onViewAll={vi.fn()}
      />,
    )
    expect(screen.queryByRole("status")).not.toBeInTheDocument()
  })
})

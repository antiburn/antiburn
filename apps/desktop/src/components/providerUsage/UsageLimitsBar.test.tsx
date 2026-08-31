import { fireEvent, render, screen, within } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import type {
  LiveProviderUsagePayload,
  LiveUsageSourceErrorPayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from "../../lib/ipc"
import { UsageLimitsBar } from "./UsageLimitsBar"

const FORECAST = {
  unavailableReason: "sparseHistory",
  confidence: null,
  consumptionRate: null,
  paceRatio: null,
  paceTrend: null,
  runwayAt: null,
  usedToday: null,
}

/** The snapshot instant every fixture is measured from. */
const GENERATED_AT = "2027-01-15T12:00:00Z"

function liveWindow(overrides: Partial<LiveUsageWindowPayload> = {}): LiveUsageWindowPayload {
  return {
    id: "five-hour",
    role: "primaryShort",
    kind: "rolling",
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
    observedAt: "2027-01-15T11:57:00Z",
    windows: [liveWindow()],
    extraUsage: null,
    resetCredits: null,
    plan: null,
    ...overrides,
  }
}

function sourceError(
  overrides: Partial<LiveUsageSourceErrorPayload> = {},
): LiveUsageSourceErrorPayload {
  return {
    source: "claude-usage-fetch",
    provider: "anthropic",
    displayName: "Claude",
    category: "rateLimited",
    ...overrides,
  }
}

function liveSummary(
  overrides: Partial<LiveUsageSummaryPayload> = {},
): LiveUsageSummaryPayload {
  return {
    providers: [liveProvider()],
    errors: [],
    meters: [],
    generatedAt: GENERATED_AT,
    ...overrides,
  }
}

function bar(over: Partial<Parameters<typeof UsageLimitsBar>[0]> = {}) {
  return render(
    <UsageLimitsBar
      live={liveSummary()}
      expanded={false}
      onToggleExpanded={vi.fn()}
      refreshing={false}
      onViewAll={vi.fn()}
      {...over}
    />,
  )
}

/** The filled segments of the one meter on screen. */
function filledSegments(): number {
  const meter = screen.getByRole("region", { name: "Usage limits" })
  const segments = meter.querySelectorAll("span.rounded-full")
  return Array.from(segments).filter((node) => node.className.includes("bg-brand-tint")).length
}

describe("UsageLimitsBar — the ring row", () => {
  it("states each provider's worst window as an accessible percentage", () => {
    bar()
    expect(screen.getByRole("button", { name: "Claude at 42 percent" })).toBeInTheDocument()
  })

  it("opens the usage view from a provider pill", () => {
    const onViewAll = vi.fn()
    bar({ onViewAll })
    fireEvent.click(screen.getByRole("button", { name: "Claude at 42 percent" }))
    expect(onViewAll).toHaveBeenCalledOnce()
  })

  it("renders a controlled companion activation", () => {
    bar({
      activeProvider: { provider: "anthropic", activation: "hovered" },
    })

    const trigger = screen.getByRole("button", { name: "Claude at 42 percent" })
    expect(trigger).toHaveAttribute("data-state", "hovered")
    expect(trigger).toHaveClass("data-[state=hovered]:bg-brand-tint/[0.08]")
  })

  it("says so rather than claiming zero when a provider states no figure", () => {
    bar({
      live: liveSummary({
        providers: [liveProvider({ windows: [liveWindow({ usedPercent: null })] })],
      }),
    })
    expect(screen.getByRole("button", { name: "Claude, no stated figure" })).toBeInTheDocument()
  })

  it("reports a refresh in flight without hiding the readings it already has", () => {
    bar({ refreshing: true })
    expect(screen.getByRole("status")).toHaveTextContent("Refreshing usage limits")
    expect(screen.getByRole("button", { name: "Claude at 42 percent" })).toBeInTheDocument()
  })

  it("shows the percentage beside the ring", () => {
    bar()
    const seat = screen.getByRole("button", { name: "Claude at 42 percent" })
    expect(within(seat).getByText("42%")).toBeInTheDocument()
    expect(seat).toHaveAttribute("title", "Claude — 42%")
  })

  it("shows a dash beside the ring when the provider states no figure", () => {
    bar({
      live: liveSummary({
        providers: [liveProvider({ windows: [liveWindow({ usedPercent: null })] })],
      }),
    })
    const seat = screen.getByRole("button", { name: "Claude, no stated figure" })
    expect(within(seat).getByText("—")).toBeInTheDocument()
  })
})

describe("UsageLimitsBar — the disclosure", () => {
  it("marks itself pressed only while the meters are open", () => {
    const { rerender } = bar()
    const collapsed = screen.getByRole("button", { name: "Expand usage limits" })
    expect(collapsed).toHaveAttribute("aria-pressed", "false")
    expect(screen.queryByRole("region", { name: "Usage limits" })).not.toBeInTheDocument()

    rerender(
      <UsageLimitsBar
        live={liveSummary()}
        expanded
        onToggleExpanded={vi.fn()}
        refreshing={false}
        onViewAll={vi.fn()}
      />,
    )
    const open = screen.getByRole("button", { name: "Collapse usage limits" })
    expect(open).toHaveAttribute("aria-pressed", "true")
    expect(open).toHaveAttribute("aria-controls", expect.any(String))
    expect(screen.getByRole("region", { name: "Usage limits" })).toBeInTheDocument()
  })

  it("drops the ring row and moves itself into the meters while open", () => {
    // The point of the open state is the space the row gives back. The
    // meters restate every ring, so keeping both spends the popover's height
    // on one reading twice.
    bar({ expanded: true })
    expect(
      screen.queryByRole("button", { name: "Claude at 42 percent" }),
    ).not.toBeInTheDocument()
    const region = screen.getByRole("region", { name: "Usage limits" })
    expect(region).toContainElement(
      screen.getByRole("button", { name: "Collapse usage limits" }),
    )
  })

  it("carries the refresh spinner with it into the open meters", () => {
    bar({ expanded: true, refreshing: true })
    const region = screen.getByRole("region", { name: "Usage limits" })
    expect(region).toContainElement(screen.getByRole("status"))
  })

  it("keeps the settings gear's treatment in both states", () => {
    // One treatment open and closed, at the reader's request, and the same
    // one the footer's gear uses: the meters below say whether it is open,
    // and an orange glyph competed with the orange arcs and segments around
    // it. `PopoverView.tsx` holds the gear this matches.
    const { rerender } = bar()
    expect(screen.getByRole("button", { name: "Expand usage limits" }).className).toContain(
      "text-label-secondary",
    )

    rerender(
      <UsageLimitsBar
        live={liveSummary()}
        expanded
        onToggleExpanded={vi.fn()}
        refreshing={false}
        onViewAll={vi.fn()}
      />,
    )
    const open = screen.getByRole("button", { name: "Collapse usage limits" })
    expect(open.className).toContain("text-label-secondary")
    expect(open.className).not.toContain("text-brand")
  })

  it("names its provider group so two providers stay tellable apart", () => {
    bar({
      live: liveSummary({
        providers: [liveProvider(), liveProvider({ provider: "openai", displayName: "Codex" })],
      }),
      expanded: true,
    })
    // Both providers report a window with the same label, so the eyebrow is
    // the only thing separating the two meters.
    const claude = screen.getByRole("group", { name: "Claude" })
    const codex = screen.getByRole("group", { name: "Codex" })
    expect(within(claude).getByText("5-hour limit")).toBeInTheDocument()
    expect(within(codex).getByText("5-hour limit")).toBeInTheDocument()
  })

  it("shows the plan as a muted suffix on the provider heading", () => {
    bar({
      live: liveSummary({
        providers: [liveProvider({ plan: { name: "max", tier: "default_claude_max_5x" } })],
      }),
      expanded: true,
    })
    // The group's own aria-label carries the plan for a reader with no
    // sight of the heading, and the heading itself carries it visibly.
    const group = screen.getByRole("group", { name: "Claude, Max 5x plan" })
    const heading = within(group).getByRole("heading")
    expect(heading).toHaveTextContent("Claude · Max 5x")
    const suffix = within(heading).getByText("· Max 5x")
    expect(suffix.className).toContain("text-label-secondary")
  })

  it("omits the separator and suffix entirely when the source reports no plan", () => {
    // The default fixture carries no plan. The heading stays the bare
    // provider name, with no trailing separator left dangling.
    bar({ live: liveSummary({ providers: [liveProvider({ plan: null })] }), expanded: true })
    const group = screen.getByRole("group", { name: "Claude" })
    const heading = within(group).getByRole("heading")
    expect(heading).toHaveTextContent("Claude")
    expect(heading.textContent).not.toContain("·")
  })
})

describe("UsageLimitsBar — the meter", () => {
  it("fills its segments in proportion to the stated percentage", () => {
    bar({
      live: liveSummary({
        providers: [liveProvider({ windows: [liveWindow({ usedPercent: 50 })] })],
      }),
      expanded: true,
    })
    // 32 segments, half of them consumed.
    expect(filledSegments()).toBe(16)
  })

  it("fills nothing for a window with no stated figure", () => {
    // Not a meter at zero, which would state a figure nobody supplied — an
    // empty meter at half strength, which reads as no reading.
    bar({
      live: liveSummary({
        providers: [liveProvider({ windows: [liveWindow({ usedPercent: null })] })],
      }),
      expanded: true,
    })
    expect(filledSegments()).toBe(0)
    expect(screen.getByRole("group", { name: "Claude" })).toHaveTextContent("—")
  })

  it("puts the elapsed notch where the window's own clock has reached", () => {
    // A five-hour window that opened three hours before the snapshot: 60%
    // elapsed against 42% used, which is the comparison the notch exists for.
    bar({
      live: liveSummary({
        providers: [
          liveProvider({
            windows: [
              liveWindow({
                startsAt: "2027-01-15T09:00:00Z",
                resetsAt: "2027-01-15T14:00:00Z",
              }),
            ],
          }),
        ],
      }),
      expanded: true,
    })
    expect(screen.getByTestId("segmented-meter-notch")).toHaveStyle({ left: "60%" })
  })

  it("draws no notch when the window states neither end", () => {
    // A notch drawn from an assumed period would be a claim the provider
    // never made.
    bar({ expanded: true })
    expect(screen.queryByTestId("segmented-meter-notch")).not.toBeInTheDocument()
  })

  it("keeps the reset time in the accessibility tree while it is hidden", () => {
    // The label fades on hover rather than unmounting, so the row keeps its
    // space and a screen-reader user keeps the reset.
    bar({
      live: liveSummary({
        providers: [
          liveProvider({ windows: [liveWindow({ resetsAt: "2027-01-15T14:00:00Z" })] }),
        ],
      }),
      expanded: true,
    })
    expect(screen.getByText("resets 2pm")).toBeInTheDocument()
  })
})

describe("UsageLimitsBar — degraded state", () => {
  it("renders nothing when there are no providers and no errors", () => {
    bar({ live: liveSummary({ providers: [] }) })
    expect(screen.queryByTestId("usage-limits-bar")).not.toBeInTheDocument()
  })

  it("keeps a failed provider on the bar instead of dropping it", () => {
    // The cold-start failure: the first fetch 429s with nothing cached, so
    // the error is the provider's only trace. The bar must not read as "your
    // Claude usage vanished".
    bar({ live: liveSummary({ providers: [], errors: [sourceError()] }) })
    const seat = screen.getByTestId("usage-limits-unavailable")
    expect(seat).toHaveAccessibleName("Claude, usage unavailable (rate limited)")
    // The row states no words any more, so the failure has to survive on
    // hover. It is also spelled out in full in the expanded listing.
    expect(seat).toHaveAttribute("title", "Claude — rate limited")
  })

  it("explains the failure in the expanded listing", () => {
    bar({
      live: liveSummary({ providers: [], errors: [sourceError()] }),
      expanded: true,
    })
    expect(screen.getByRole("group", { name: "Claude" })).toBeInTheDocument()
    expect(screen.getByText(/usage unavailable — rate limited/i)).toBeInTheDocument()
    expect(screen.getByText(/asked antiburn to slow down/i)).toBeInTheDocument()
  })

  it("shows no degraded pill while the provider still shows cached windows", () => {
    // A failed refresh after an earlier success keeps the last-good reading
    // (the cooldown's contract); the staleness treatment covers that, and a
    // second seat would report one failure twice.
    bar({ live: liveSummary({ errors: [sourceError()] }) })
    expect(screen.queryByTestId("usage-limits-unavailable")).not.toBeInTheDocument()
  })

  it("shows a working provider and a failed one side by side", () => {
    bar({
      live: liveSummary({
        providers: [liveProvider()],
        errors: [
          sourceError({
            source: "codex-usage-fetch",
            provider: "openai",
            displayName: "Codex",
            category: "unavailable",
          }),
        ],
      }),
    })
    expect(screen.getByRole("button", { name: /Claude at 42 percent/ })).toBeInTheDocument()
    expect(screen.getByTestId("usage-limits-unavailable")).toHaveAccessibleName(
      "Codex, usage unavailable (unreachable)",
    )
  })
})

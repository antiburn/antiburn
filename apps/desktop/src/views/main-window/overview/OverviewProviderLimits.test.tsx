import { render, screen, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type {
  LiveProviderUsagePayload,
  LiveUsageSourceErrorPayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from "../../../lib/ipc"
import { OverviewProviderLimits, meterSegmentsForWidth } from "./OverviewProviderLimits"

const FORECAST = {
  unavailableReason: "sparseHistory",
  confidence: null,
  consumptionRate: null,
  paceRatio: null,
  paceTrend: null,
  runwayAt: null,
  usedToday: null,
}

const GENERATED_AT = "2027-01-15T12:00:00Z"

function liveWindow(overrides: Partial<LiveUsageWindowPayload> = {}): LiveUsageWindowPayload {
  return {
    id: "five-hour",
    role: "primaryShort",
    kind: "rolling",
    scopeModel: null,
    usedPercent: 42,
    startsAt: "2027-01-15T10:00:00Z",
    resetsAt: "2027-01-15T15:00:00Z",
    hasNonzeroUsageInCurrentPeriod: true,
    forecast: FORECAST,
    ...overrides,
  }
}

function liveProvider(
  overrides: Partial<LiveProviderUsagePayload> = {},
): LiveProviderUsagePayload {
  return {
    provider: "anthropic",
    accountKey: null,
    displayName: "Claude",
    support: "live",
    freshness: "fresh",
    sourceLabel: "Asked Claude directly",
    observedAt: "2027-01-15T11:57:00Z",
    windows: [liveWindow()],
    extraUsage: null,
    resetCredits: null,
    plan: { name: "Max", tier: null },
    accountUuid: null,
    accountEmail: null,
    ...overrides,
  }
}

function sourceError(
  overrides: Partial<LiveUsageSourceErrorPayload> = {},
): LiveUsageSourceErrorPayload {
  return {
    source: "codex-usage-fetch",
    provider: "openai",
    displayName: "Codex",
    category: "authentication",
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

describe("OverviewProviderLimits", () => {
  afterEach(() => vi.restoreAllMocks())

  it("draws a thirty-two-dot meter with the notch, the figure and the reset caption", () => {
    render(<OverviewProviderLimits live={liveSummary()} />)
    const card = screen.getByRole("group", { name: /Claude/ })
    expect(card).toHaveAccessibleName("Claude, Max plan")
    expect(within(card).getByText("42%")).toBeInTheDocument()
    expect(within(card).getByTestId("segmented-meter-notch")).toHaveStyle({ left: "40%" })
    expect(within(card).getByText(/^resets /)).toBeInTheDocument()
    const dots = card.querySelectorAll(".rounded-full")
    expect(dots).toHaveLength(32)
    expect(
      Array.from(dots).filter((dot) => dot.className.includes("bg-brand-tint")),
    ).toHaveLength(13)
    expect(screen.queryByText("Live")).toBeNull()
    expect(screen.queryByText(/\$/)).toBeNull()
  })

  it("adds dots as the group grows and keeps the lit share", () => {
    expect(meterSegmentsForWidth(0)).toBe(32)
    expect(meterSegmentsForWidth(100)).toBe(16)
    expect(meterSegmentsForWidth(300)).toBe(33)
    expect(meterSegmentsForWidth(603)).toBe(67)
    vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(603)
    render(<OverviewProviderLimits live={liveSummary()} />)
    const card = screen.getByRole("group", { name: /Claude/ })
    const dots = card.querySelectorAll(".rounded-full")
    expect(dots).toHaveLength(67)
    expect(
      Array.from(dots).filter((dot) => dot.className.includes("bg-brand-tint")),
    ).toHaveLength(28)
  })

  it("dims a meter with no reading and turns the red zone on above 90%", () => {
    render(
      <OverviewProviderLimits
        live={liveSummary({
          providers: [
            liveProvider({
              windows: [
                liveWindow({ id: "five-hour", usedPercent: null, resetsAt: null }),
                liveWindow({
                  id: "seven-day",
                  role: "primaryLong",
                  kind: "weekly",
                  usedPercent: 95,
                }),
              ],
            }),
          ],
        })}
      />,
    )
    const card = screen.getByRole("group", { name: /Claude/ })
    expect(within(card).getByText("—")).toBeInTheDocument()
    expect(card.querySelectorAll(".rounded-full.opacity-50")).toHaveLength(32)
    expect(card.querySelectorAll(".bg-system-red-tint").length).toBeGreaterThan(0)
  })

  it("seats a failed provider with its action and marks stale readings", () => {
    render(
      <OverviewProviderLimits
        live={liveSummary({
          providers: [liveProvider({ freshness: "stale" })],
          errors: [sourceError()],
        })}
      />,
    )
    expect(screen.getByRole("group", { name: "Codex" })).toHaveTextContent(
      "Codex sign-in expired. Sign in again, then retry.",
    )
    expect(screen.getByText("Stale")).toHaveClass("text-system-orange")
  })

  it("shows one quiet line when no provider reports anything", () => {
    render(<OverviewProviderLimits live={liveSummary({ providers: [] })} />)
    expect(screen.getByText(/No provider limits to show/)).toBeInTheDocument()
    expect(screen.queryByText("Live")).toBeNull()
  })

  it("holds placeholders while loading", () => {
    render(<OverviewProviderLimits live={null} loading />)
    expect(screen.getByRole("region", { name: "Provider limits" })).toHaveAttribute(
      "aria-busy",
      "true",
    )
    expect(screen.queryByRole("group")).toBeNull()
  })
})

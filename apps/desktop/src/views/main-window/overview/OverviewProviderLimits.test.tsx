import { fireEvent, render, screen, waitFor, within } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type {
  LiveProviderUsagePayload,
  LiveUsageSourceErrorPayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from "../../../lib/ipc"
import { OverviewProviderLimits, meterSegmentsForWidth } from "./OverviewProviderLimits"

const invoke = vi.hoisted(() => vi.fn())
vi.mock("@tauri-apps/api/core", () => ({ invoke, isTauri: () => true }))

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
    elapsedFraction: 0.4,
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

  it.each([
    ["refreshPending", "Couldn't update Claude usage. Last updated 3 min ago."],
    ["signInRequired", "Sign in to Claude again. Last updated 3 min ago."],
  ] as const)("preserves the %s detail in the visible reading", (detail, note) => {
    render(
      <OverviewProviderLimits
        live={liveSummary({
          errors: [
            sourceError({
              provider: "anthropic",
              displayName: "Claude",
              detail,
            }),
          ],
        })}
      />,
    )
    expect(screen.getByText(note)).toBeInTheDocument()
  })

  it.each([
    ["refreshPending", "Couldn't update Claude usage. Try again shortly."],
    ["cliMissing", "Couldn't update Claude usage. Open Claude Code to check your sign-in."],
  ] as const)("keeps the %s detail when Claude has no reading to show", (detail, note) => {
    render(
      <OverviewProviderLimits
        live={liveSummary({
          providers: [],
          errors: [
            sourceError({
              provider: "anthropic",
              displayName: "Claude",
              detail,
            }),
          ],
        })}
      />,
    )
    const card = screen.getByRole("group", { name: "Claude" })
    expect(card).toHaveTextContent(note)
    expect(card).not.toHaveTextContent("sign-in expired")
  })

  it("shows the sign-in action for a failed provider", () => {
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
  })

  it("names profile accounts, rules between every group, and retries a failed one", async () => {
    invoke.mockResolvedValue(null)
    const { container } = render(
      <OverviewProviderLimits
        live={liveSummary({
          providers: [
            liveProvider({ accountKey: "personal", accountLabel: "Claude" }),
            liveProvider({ accountKey: "work", accountLabel: "Claude Work" }),
          ],
          errors: [
            sourceError({
              source: "claude-usage-fetch",
              provider: "anthropic",
              displayName: "Claude Side",
              category: "rateLimited",
              accountLabel: "Claude Side",
            }),
          ],
        })}
      />,
    )

    expect(screen.getByRole("group", { name: "Claude Work, Max plan" })).toBeInTheDocument()
    expect(screen.getByRole("group", { name: "Claude Side" })).toBeInTheDocument()
    expect(container.querySelectorAll(".bg-separator")).toHaveLength(2)

    fireEvent.click(screen.getByRole("button", { name: "Retry Claude Side limits" }))
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("refresh_live_usage", expect.anything()),
    )
  })

  it("says what a retry found", async () => {
    const failed = sourceError({
      provider: "anthropic",
      displayName: "Claude Side",
      category: "rateLimited",
      accountLabel: "Claude Side",
    })
    invoke.mockImplementation(async (command: string) =>
      command === "refresh_live_usage" ? liveSummary({ errors: [failed] }) : null,
    )
    render(<OverviewProviderLimits live={liveSummary({ errors: [failed] })} />)

    fireEvent.click(screen.getByRole("button", { name: "Retry Claude Side limits" }))

    expect(await screen.findByRole("status")).toHaveTextContent(/Tried again at .*: /)
    expect(invoke).toHaveBeenCalledWith("refresh_live_usage", {
      utcOffsetMinutes: expect.any(Number),
      retry: true,
    })
  })

  it("says when a retry could not run", async () => {
    const failed = sourceError({
      provider: "anthropic",
      displayName: "Claude Side",
      category: "rateLimited",
      accountLabel: "Claude Side",
    })
    invoke.mockImplementation(async (command: string) => {
      if (command === "refresh_live_usage") throw "shell unavailable"
      return null
    })
    render(<OverviewProviderLimits live={liveSummary({ errors: [failed] })} />)

    fireEvent.click(screen.getByRole("button", { name: "Retry Claude Side limits" }))

    expect(await screen.findByRole("status")).toHaveTextContent("Could not check. Try again.")
  })

  it("names the time a rate limit lifts and holds Retry until then", () => {
    render(
      <OverviewProviderLimits
        live={liveSummary({
          errors: [
            sourceError({
              provider: "anthropic",
              displayName: "Claude Side",
              category: "rateLimited",
              accountLabel: "Claude Side",
              retryAt: "2027-01-15T12:10:00Z",
            }),
          ],
        })}
      />,
    )

    expect(screen.getByRole("status")).toHaveTextContent(/^Try again after .+\.$/)
    expect(screen.getByRole("button", { name: "Retry Claude Side limits" })).toBeDisabled()
  })

  it("enables Retry once the provider's wait has passed", () => {
    render(
      <OverviewProviderLimits
        live={liveSummary({
          errors: [
            sourceError({
              provider: "anthropic",
              displayName: "Claude Side",
              category: "rateLimited",
              accountLabel: "Claude Side",
              retryAt: "2027-01-15T11:50:00Z",
            }),
          ],
        })}
      />,
    )

    expect(screen.queryByRole("status")).not.toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Retry Claude Side limits" })).toBeEnabled()
  })

  it("shows one quiet line when no provider reports anything", () => {
    render(<OverviewProviderLimits live={liveSummary({ providers: [] })} />)
    expect(screen.getByText(/No providers set up for limits yet/)).toBeInTheDocument()
    expect(screen.queryByText("Live")).toBeNull()
  })

  it("holds placeholders while loading", () => {
    const { container } = render(<OverviewProviderLimits live={null} loading />)
    expect(screen.queryByText(/No providers set up/)).toBeNull()
    expect(container.querySelector(".animate-pulse")).not.toBeNull()
    expect(screen.getByRole("region", { name: "Provider limits" })).toHaveAttribute(
      "aria-busy",
      "true",
    )
    expect(screen.queryByRole("group")).toBeNull()
  })

  it("says there is nothing to show when the read answers with nothing", () => {
    // A failed read leaves no summary and stops the loading state. The panel
    // must answer, because a permanent skeleton states a read in progress.
    render(<OverviewProviderLimits live={null} loading={false} />)
    expect(screen.getByText(/No providers set up for limits yet/)).toBeInTheDocument()
    expect(screen.getByRole("region", { name: "Provider limits" })).not.toHaveAttribute(
      "aria-busy",
    )
  })
})

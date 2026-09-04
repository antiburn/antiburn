import { siClaude } from "simple-icons"
import { describe, expect, it } from "vitest"

import type {
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  LiveUsageWindowPayload,
} from "./ipc"
import { deriveUsageBars, providerBarColor, resetsIn, resetsLabel } from "./usageBars"

const NOW = Date.parse("2026-08-18T02:00:00Z")

function usageWindow(overrides: Partial<LiveUsageWindowPayload> = {}): LiveUsageWindowPayload {
  return {
    id: "five-hour",
    role: "primaryShort",
    kind: "rolling",
    scopeModel: null,
    usedPercent: 40,
    startsAt: null,
    resetsAt: new Date(NOW + 2 * 3_600_000).toISOString(),
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
  }
}

function provider(overrides: Partial<LiveProviderUsagePayload> = {}): LiveProviderUsagePayload {
  return {
    provider: "anthropic",
    accountKey: null,
    displayName: "Anthropic",
    support: "live",
    freshness: "fresh",
    sourceLabel: "cached usage",
    observedAt: new Date(NOW).toISOString(),
    windows: [usageWindow()],
    extraUsage: null,
    resetCredits: null,
    plan: null,
    ...overrides,
  }
}

function summary(providers: LiveProviderUsagePayload[]): LiveUsageSummaryPayload {
  return { providers, errors: [], meters: [], generatedAt: new Date(NOW).toISOString() }
}

describe("deriveUsageBars", () => {
  it("returns nothing for a missing or empty snapshot", () => {
    expect(deriveUsageBars(null)).toEqual([])
    expect(deriveUsageBars(summary([]))).toEqual([])
  })

  it("orders primary windows first and omits a single provider prefix", () => {
    const bars = deriveUsageBars(
      summary([
        provider({
          windows: [
            usageWindow({
              id: "seven-day",
              role: "primaryLong",
              kind: "weekly",
              usedPercent: 12,
            }),
            usageWindow({ id: "five-hour", usedPercent: 81 }),
          ],
        }),
      ]),
    )
    expect(bars.map((bar) => bar.label)).toEqual(["5-hour limit", "Weekly limit"])
    expect(bars.map((bar) => bar.percent)).toEqual([81, 12])
    expect(bars[0]!.key).toBe("anthropic-five-hour")
  })

  it("prefixes labels when two providers report limits", () => {
    const bars = deriveUsageBars(
      summary([
        provider(),
        provider({ provider: "openai", displayName: "OpenAI", windows: [usageWindow()] }),
      ]),
    )
    expect(bars.map((bar) => bar.label)).toEqual([
      "Anthropic · 5-hour limit",
      "OpenAI · 5-hour limit",
    ])
  })

  it("drops windows without a percentage and untouched supplemental windows", () => {
    const hidden = usageWindow({
      id: "weekly-opus",
      role: "supplemental",
      scopeModel: "Opus",
      usedPercent: 0,
      hasNonzeroUsageInCurrentPeriod: false,
    })
    expect(
      deriveUsageBars(
        summary([provider({ windows: [usageWindow({ usedPercent: null }), hidden] })]),
      ),
    ).toEqual([])
    expect(
      deriveUsageBars(
        summary([provider({ windows: [{ ...hidden, hasNonzeroUsageInCurrentPeriod: true }] })]),
      ).map((bar) => bar.label),
    ).toEqual(["Opus weekly limit"])
  })

  it("does not return an invalid reset date", () => {
    const bars = deriveUsageBars(
      summary([provider({ windows: [usageWindow({ resetsAt: "not a date" })] })]),
    )
    expect(bars[0]!.resetsAt).toBeNull()
  })

  it("uses the provider color mapping", () => {
    const bars = deriveUsageBars(
      summary([
        provider(),
        provider({ provider: "openai", displayName: "OpenAI", windows: [usageWindow()] }),
      ]),
    )
    // Anthropic keeps Claude's own colour, and it comes from the package the
    // brand marks come from — a hex written here would be the thing the
    // design contract forbids.
    expect(bars[0]!.color).toBe(`#${siClaude.hex}`)
    expect(bars[1]!.color).toBe("var(--color-label)")
    expect(providerBarColor("cursor")).toBe("var(--color-system-indigo)")
    expect(providerBarColor("somebody-new")).toBe("var(--color-burn)")
  })

  it("measures the notch from the snapshot's own time", () => {
    // The window resets in two hours and its id states a five-hour period, so
    // three of its five hours have gone.
    const bars = deriveUsageBars(summary([provider()]))
    expect(bars[0]!.expectedFraction).toBeCloseTo(0.6, 5)
  })

  it("draws no notch when nothing states the window's period", () => {
    // A notch placed from an assumed period is a claim the provider never
    // made, so there is no notch at all.
    const bars = deriveUsageBars(
      summary([provider({ windows: [usageWindow({ id: "monthly", kind: "monthly" })] })]),
    )
    expect(bars[0]!.expectedFraction).toBeNull()
  })

  it("draws no notch when the snapshot states no usable time", () => {
    const broken = summary([provider()])
    broken.generatedAt = "not a date"
    expect(deriveUsageBars(broken)[0]!.expectedFraction).toBeNull()
  })
})

describe("reset labels", () => {
  it("handles unknown, elapsed, clock, and duration states", () => {
    expect(resetsLabel(null, NOW)).toBe("reset unknown")
    expect(resetsLabel(new Date(NOW - 1000), NOW)).toBe("resets soon")
    expect(resetsLabel(new Date(NOW + 2 * 3_600_000), NOW)).toMatch(/^resets \d{1,2}:\d{2}/)
    expect(resetsLabel(new Date(NOW + 26 * 3_600_000), NOW)).toBe("resets in 1d 2h")
  })

  it("renders minute, hour, and day durations", () => {
    expect(resetsIn(new Date(NOW + 45 * 60_000), NOW)).toBe("resets in 45m")
    expect(resetsIn(new Date(NOW + (3 * 60 + 38) * 60_000), NOW)).toBe("resets in 3h 38m")
    expect(resetsIn(new Date(NOW + (5 * 24 + 2) * 3_600_000), NOW)).toBe("resets in 5d 2h")
    expect(resetsIn(null, NOW)).toBe("reset unknown")
  })
})

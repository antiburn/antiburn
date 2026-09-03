import { describe, expect, it } from "vitest"

import type { ProviderUsagePayload, ProviderUsageWindowPayload } from "../ipc"
import {
  EMPTY_WINDOW,
  formatSpendFigure,
  formatTokenFigure,
  providerInitial,
  providerWindow,
  rankByWindow,
  sessionCountLabel,
  stalenessNote,
  usageStateDescription,
  usageStateLabel,
  usageMetricLabel,
  usageMetricRows,
  usageValueLabel,
  usageWindowLabel,
  USAGE_WINDOWS,
  windowHasEvidence,
  windowShareOfLast30Days,
  windowTokens,
} from "./providerUsage"

function usageWindow(
  overrides: Partial<ProviderUsageWindowPayload> = {},
): ProviderUsageWindowPayload {
  return { ...EMPTY_WINDOW, ...overrides }
}

function provider(overrides: Partial<ProviderUsagePayload> = {}): ProviderUsagePayload {
  return {
    provider: "anthropic",
    accountKey: null,
    displayName: "Anthropic",
    state: "estimated",
    staleness: "fresh",
    windows: {
      today: usageWindow(),
      week: usageWindow(),
      monthToDate: usageWindow(),
      last30Days: usageWindow(),
    },
    agents: [],
    lastActivityAt: new Date().toISOString(),
    ...overrides,
  }
}

describe("usage values", () => {
  it("leads with the cost when the models could be priced", () => {
    expect(usageValueLabel(usageWindow({ estimatedUsd: 1.5, tokensIn: 10 }))).toBe("$1.50")
  })

  it("shows the cost before the token count in a window bar", () => {
    expect(
      usageMetricLabel(usageWindow({ estimatedUsd: 1.5, tokensIn: 1_200, tokensOut: 300 })),
    ).toBe("$1.50 · 1.5k")
  })

  it("hides a partial cost and keeps the complete token count", () => {
    expect(
      usageMetricLabel(
        usageWindow({
          estimatedUsd: 1.5,
          costComplete: false,
          tokensIn: 1_200,
          tokensOut: 300,
        }),
      ),
    ).toBe("1.5k")
  })

  it("uses tokens for a window bar when either cost is incomplete", () => {
    const summary = provider({
      windows: {
        today: usageWindow({
          estimatedUsd: 90,
          costComplete: false,
          tokensIn: 250,
        }),
        week: usageWindow(),
        monthToDate: usageWindow(),
        last30Days: usageWindow({ estimatedUsd: 100, tokensIn: 1_000 }),
      },
    })

    expect(windowShareOfLast30Days(summary, "today")).toBe(0.25)
    expect(usageMetricRows(summary)[0]?.value).toBe("—")
  })

  it("falls back to a token count rather than a zero that was never priced", () => {
    // The distinction the whole feature turns on: "$0.00" would claim the work
    // was free, when in fact its models have no price in the catalog.
    expect(usageValueLabel(usageWindow({ tokensIn: 1_200, tokensOut: 300 }))).toBe("1.5k")
  })

  it("reads as a dash when there is nothing at all", () => {
    expect(usageValueLabel(EMPTY_WINDOW)).toBe("—")
  })

  it("keeps a priced zero distinct from an unpriced one", () => {
    expect(usageValueLabel(usageWindow({ estimatedUsd: 0 }))).toBe("$0.00")
  })

  it("counts every billable token, cache reads included", () => {
    const window = usageWindow({ tokensIn: 100, tokensOut: 20, cacheRead: 5 })
    expect(windowTokens(window)).toBe(125)
    expect(windowHasEvidence(window)).toBe(true)
    expect(windowHasEvidence(EMPTY_WINDOW)).toBe(false)
    // A priced window with no tokens is still evidence — it was measured.
    expect(windowHasEvidence(usageWindow({ estimatedUsd: 0 }))).toBe(true)
  })
})

describe("capability labels", () => {
  it("names every state, including provider-owned live readings", () => {
    expect(usageStateLabel("live")).toBe("Live")
    expect(usageStateLabel("estimated")).toBe("Estimated")
    expect(usageStateLabel("observed")).toBe("Observed")
    expect(usageStateLabel("detected")).toBe("Detected")
    expect(usageStateLabel("unknown")).toBe("No estimate")
  })

  it("describes each provider-state figure without overstating it", () => {
    expect(usageStateDescription("live")).toBe("The provider reported this usage directly.")
    expect(usageStateDescription("estimated")).toBe(
      "Estimated locally at API rates. Your provider bill may differ.",
    )
    expect(usageStateDescription("observed")).toBe(
      "Tokens were counted, but some models have no price, so the cost is a floor rather than a total.",
    )
    expect(usageStateDescription("detected")).toBe(
      "This provider was detected without any usage figures.",
    )
    expect(usageStateDescription("unknown")).toBe(
      "Sessions were attributed to this provider, but none of them carries token evidence yet.",
    )
  })

  it("never promises an allowance, a percentage, or a reset", () => {
    // The copy is the last place an invented denominator could get in, so it
    // is checked the same way the payload is.
    const copy = (["live", "estimated", "observed", "detected", "unknown"] as const)
      .flatMap((state) => [usageStateLabel(state), usageStateDescription(state)])
      .concat(USAGE_WINDOWS.map((option) => option.label))
      .concat(USAGE_WINDOWS.map((option) => usageWindowLabel(option.value)))
      .join(" ")
      .toLowerCase()
    for (const forbidden of ["percent", "allowance", "remaining", "resets", "quota", "limit"]) {
      expect(copy).not.toContain(forbidden)
    }
  })

  it("notes staleness as an observation, and only when the shell says so", () => {
    const stale = provider({
      staleness: "stale",
      lastActivityAt: new Date(Date.now() - 3 * 86_400_000).toISOString(),
    })
    expect(stalenessNote(stale)).toBe("Last used 3d ago")
    expect(stalenessNote(provider())).toBeNull()
    expect(stalenessNote(provider({ staleness: "unknown" }))).toBeNull()
  })
})

describe("provider identity", () => {
  it("uses the display name initial, so no provider artwork is needed", () => {
    expect(providerInitial("Anthropic")).toBe("A")
    expect(providerInitial("  openai")).toBe("O")
    expect(providerInitial("")).toBe("?")
  })
})

describe("provider windows and ranking", () => {
  const anthropic = provider({
    provider: "anthropic",
    displayName: "Anthropic",
    windows: {
      today: usageWindow({ estimatedUsd: 1, tokensIn: 100, sessionCount: 1 }),
      week: usageWindow({ estimatedUsd: 1, tokensIn: 100, sessionCount: 1 }),
      monthToDate: usageWindow({ estimatedUsd: 1, tokensIn: 100, sessionCount: 1 }),
      last30Days: usageWindow({ estimatedUsd: 1, tokensIn: 100, sessionCount: 1 }),
    },
  })
  const openai = provider({
    provider: "openai",
    accountKey: null,
    displayName: "OpenAI",
    windows: {
      today: usageWindow({ estimatedUsd: 4, tokensIn: 10, sessionCount: 2 }),
      week: usageWindow({ estimatedUsd: 4, tokensIn: 10, sessionCount: 2 }),
      monthToDate: usageWindow({ estimatedUsd: 4, tokensIn: 10, sessionCount: 2 }),
      last30Days: usageWindow({ estimatedUsd: 4, tokensIn: 10, sessionCount: 2 }),
    },
  })
  const unpriced = provider({
    provider: "unknown",
    displayName: "Unattributed",
    state: "observed",
    windows: {
      today: usageWindow({ tokensIn: 9_000, costComplete: false, sessionCount: 3 }),
      week: usageWindow({ tokensIn: 9_000, costComplete: false, sessionCount: 3 }),
      monthToDate: usageWindow({ tokensIn: 9_000, costComplete: false, sessionCount: 3 }),
      last30Days: usageWindow({ tokensIn: 9_000, costComplete: false, sessionCount: 3 }),
    },
  })

  it("ranks every provider by tokens when one cost is incomplete", () => {
    const ranked = rankByWindow([anthropic, unpriced, openai], "today")
    expect(ranked.map((entry) => entry.provider)).toEqual(["unknown", "anthropic", "openai"])
  })

  it("ranks by cost when every provider cost is complete", () => {
    const ranked = rankByWindow([anthropic, openai], "today")
    expect(ranked.map((entry) => entry.provider)).toEqual(["openai", "anthropic"])
  })

  it("reads a window off a provider without the caller indexing it", () => {
    expect(providerWindow(openai, "monthToDate").estimatedUsd).toBe(4)
  })
})

describe("counts", () => {
  it("pluralizes sessions", () => {
    expect(sessionCountLabel(0)).toBe("0 sessions")
    expect(sessionCountLabel(1)).toBe("1 session")
    expect(sessionCountLabel(2)).toBe("2 sessions")
  })
})

describe("popover card figures", () => {
  it("prices the spend figure by its size", () => {
    expect(formatSpendFigure(0)).toBe("$0.00")
    expect(formatSpendFigure(0.004)).toBe("<$0.01")
    expect(formatSpendFigure(41.78)).toBe("$41.78")
    expect(formatSpendFigure(99.996)).toBe("$100")
    expect(formatSpendFigure(3523.96)).toBe("$3,524")
    expect(formatSpendFigure(99_999.4)).toBe("$99,999")
    expect(formatSpendFigure(100_000)).toBe("$100k")
    expect(formatSpendFigure(123_456)).toBe("$123k")
    expect(formatSpendFigure(999_600)).toBe("$1.00M")
    expect(formatSpendFigure(1_234_567)).toBe("$1.23M")
  })

  it("scales the token figure to three significant figures", () => {
    expect(formatTokenFigure(0)).toBe("0")
    expect(formatTokenFigure(999)).toBe("999")
    expect(formatTokenFigure(1000)).toBe("1.00k")
    expect(formatTokenFigure(1250)).toBe("1.25k")
    expect(formatTokenFigure(57_000_000)).toBe("57.0M")
    expect(formatTokenFigure(4_843_800_000)).toBe("4.84B")
    expect(formatTokenFigure(999_600_000)).toBe("1.00B")
    expect(formatTokenFigure(1_500_000_000_000)).toBe("1.50T")
  })
})

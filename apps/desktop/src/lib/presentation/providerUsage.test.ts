import { describe, expect, it } from "vitest"

import type { ProviderUsagePayload, ProviderUsageWindowPayload } from "../ipc"
import {
  EMPTY_WINDOW,
  providerInitial,
  providerWindow,
  rankByWindow,
  sessionCountLabel,
  stalenessNote,
  usageStateDescription,
  usageStateLabel,
  usageValueLabel,
  usageWindowLabel,
  USAGE_WINDOWS,
  windowHasEvidence,
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
    windows: { today: usageWindow(), week: usageWindow(), month: usageWindow() },
    agents: [],
    lastActivityAt: new Date().toISOString(),
    ...overrides,
  }
}

describe("usage values", () => {
  it("leads with the cost when the models could be priced", () => {
    expect(usageValueLabel(usageWindow({ estimatedUsd: 1.5, tokensIn: 10 }))).toBe("$1.50")
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
      month: usageWindow({ estimatedUsd: 1, tokensIn: 100, sessionCount: 1 }),
    },
  })
  const openai = provider({
    provider: "openai",
    accountKey: null,
    displayName: "OpenAI",
    windows: {
      today: usageWindow({ estimatedUsd: 4, tokensIn: 10, sessionCount: 2 }),
      week: usageWindow({ estimatedUsd: 4, tokensIn: 10, sessionCount: 2 }),
      month: usageWindow({ estimatedUsd: 4, tokensIn: 10, sessionCount: 2 }),
    },
  })
  const unpriced = provider({
    provider: "unknown",
    displayName: "Unattributed",
    state: "observed",
    windows: {
      today: usageWindow({ tokensIn: 9_000, sessionCount: 3 }),
      week: usageWindow({ tokensIn: 9_000, sessionCount: 3 }),
      month: usageWindow({ tokensIn: 9_000, sessionCount: 3 }),
    },
  })

  it("ranks by cost first and by tokens where there is no cost", () => {
    const ranked = rankByWindow([anthropic, unpriced, openai], "today")
    expect(ranked.map((entry) => entry.provider)).toEqual(["openai", "anthropic", "unknown"])
  })

  it("reads a window off a provider without the caller indexing it", () => {
    expect(providerWindow(openai, "month").estimatedUsd).toBe(4)
  })
})

describe("counts", () => {
  it("pluralizes sessions", () => {
    expect(sessionCountLabel(0)).toBe("0 sessions")
    expect(sessionCountLabel(1)).toBe("1 session")
    expect(sessionCountLabel(2)).toBe("2 sessions")
  })
})

import { describe, expect, it } from "vitest"

import type { InitialContextBreakdown, SessionBucket } from "../types/session"
import {
  axisScale,
  contextTokenSeries,
  timeAxisTicks,
  costBreakdownRows,
  costFigureLabel,
  costOutlierThreshold,
  formatCost,
  formatDuration,
  formatTime,
  formatTokenBand,
  formatTokensShort,
  HIGH_COST_FLOOR_USD,
  HIGH_COST_MEDIAN_MULTIPLE,
  HIGH_COST_MIN_SAMPLE,
  median,
  modeChangeMarkers,
  skillMcpOriginLabel,
  skillMcpStatusLabel,
  skillMcpUsage,
} from "./sessionAnalysis"

function bucket(over: Partial<SessionBucket> = {}): SessionBucket {
  return {
    tokensIn: 0,
    tokensOut: 0,
    subagentTokens: 0,
    contextTokens: 0,
    isCompactionBoundary: false,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    rewriteTokens: 0,
    isCacheRehydration: false,
    isCacheRoutingMiss: false,
    secsSincePriorTurn: null,
    subagentLaunches: 0,
    userPrompts: 0,
    lastTool: null,
    model: null,
    thinkingMode: null,
    speed: null,
    hasThinking: false,
    compactionTrigger: null,
    compactionPreTokens: null,
    compactionPostTokens: null,
    ...over,
  }
}

describe("contextTokenSeries", () => {
  it("keeps every bucket at its measured progress and holds the context level across empty ones", () => {
    const buckets = [
      bucket(),
      bucket({ tokensIn: 100, tokensOut: 10, subagentTokens: 25, contextTokens: 50_000 }),
      bucket(),
      bucket({ tokensIn: 300, tokensOut: 30, contextTokens: 150_000 }),
      bucket({ tokensIn: 500, tokensOut: 50, contextTokens: 200_000 }),
      bucket(),
    ]
    const series = contextTokenSeries(buckets)
    expect(series).toHaveLength(6)
    expect(series.map((p) => p.progress)).toEqual([0, 20, 40, 60, 80, 100])
    expect(series.map((p) => p.tokensIn)).toEqual([0, 100, 0, 300, 500, 0])
    expect(series.map((p) => p.subagentTokens)).toEqual([0, 25, 0, 0, 0, 0])
    expect(series.map((p) => p.contextTokens)).toEqual([
      0, 50_000, 50_000, 150_000, 200_000, 200_000,
    ])
  })

  it("fills a compaction bucket and the empty buckets after it with the next level", () => {
    const buckets = [
      bucket({ contextTokens: 180_000 }),
      bucket({ contextTokens: 0, isCompactionBoundary: true }),
      bucket(),
      bucket({ contextTokens: 20_000 }),
      bucket(),
    ]
    const series = contextTokenSeries(buckets)
    expect(series.map((p) => p.contextTokens)).toEqual([
      180_000, 20_000, 20_000, 20_000, 20_000,
    ])
  })

  it("leaves a trailing compaction at zero when nothing follows it", () => {
    const buckets = [
      bucket({ contextTokens: 180_000 }),
      bucket({ contextTokens: 0, isCompactionBoundary: true }),
      bucket(),
    ]
    expect(contextTokenSeries(buckets).map((p) => p.contextTokens)).toEqual([180_000, 0, 0])
  })

  it("carries the compaction-boundary flag onto the matching point", () => {
    const buckets = [
      bucket({ contextTokens: 180_000 }),
      bucket({ contextTokens: 0, isCompactionBoundary: true }),
      bucket({ contextTokens: 20_000 }),
    ]
    const series = contextTokenSeries(buckets)
    expect(series.map((p) => p.isCompactionBoundary)).toEqual([false, true, false])
  })

  it("carries cache totals, rewrite totals, and rehydration onto each point", () => {
    const buckets = [
      bucket({ cacheReadTokens: 5_000, cacheWriteTokens: 200, rewriteTokens: 100 }),
      bucket({
        cacheReadTokens: 0,
        cacheWriteTokens: 25_000,
        rewriteTokens: 24_000,
        isCacheRehydration: true,
        cacheRehydration: {
          contextTokens: 30_000,
          stillCachedTokens: 5_000,
          rewrittenTokens: 24_000,
          growthTokens: 1_000,
        },
      }),
    ]
    const series = contextTokenSeries(buckets)
    expect(
      series.map((p) => [
        p.cacheReadTokens,
        p.cacheWriteTokens,
        p.rewriteTokens,
        p.isCacheRehydration,
        p.cacheRehydration,
      ]),
    ).toEqual([
      [5_000, 200, 100, false, null],
      [
        0,
        25_000,
        24_000,
        true,
        {
          contextTokens: 30_000,
          stillCachedTokens: 5_000,
          rewrittenTokens: 24_000,
          growthTokens: 1_000,
        },
      ],
    ])
  })

  it("carries the routing-miss flag onto each point", () => {
    const buckets = [
      bucket({ isCacheRoutingMiss: false }),
      bucket({ cacheWriteTokens: 25_000, isCacheRoutingMiss: true }),
    ]
    const series = contextTokenSeries(buckets)
    expect(series.map((p) => p.isCacheRoutingMiss)).toEqual([false, true])
  })

  it("carries the prior-turn gap only on its own bucket", () => {
    const series = contextTokenSeries([
      bucket(),
      bucket({ contextTokens: 25_000, secsSincePriorTurn: 9_300 }),
      bucket(),
    ])

    expect(series.map((point) => point.secsSincePriorTurn)).toEqual([null, 9_300, null])
  })

  it("describes the gap a call-less bucket sits in", () => {
    const series = contextTokenSeries([
      bucket({ contextTokens: 20_000, tokensIn: 100, lastTool: "Bash" }),
      bucket({ contextTokens: 0 }),
      bucket({ contextTokens: 0 }),
      bucket({ contextTokens: 25_000, tokensIn: 100, secsSincePriorTurn: 130 }),
    ])

    expect(series.map((point) => point.betweenCalls)).toEqual([
      null,
      { secs: 130, tool: "Bash", userPrompt: false },
      { secs: 130, tool: "Bash", userPrompt: false },
      null,
    ])
  })

  it("marks a gap that a user prompt ends", () => {
    const series = contextTokenSeries([
      bucket({ contextTokens: 20_000, tokensIn: 100, lastTool: "Bash" }),
      bucket({ contextTokens: 0 }),
      bucket({ contextTokens: 25_000, tokensIn: 100, secsSincePriorTurn: 600, userPrompts: 1 }),
    ])

    expect(series[1]!.betweenCalls).toEqual({ secs: 600, tool: "Bash", userPrompt: true })
  })

  it("forward-fills model, thinking mode, and speed across buckets with no value", () => {
    const buckets = [
      bucket({ model: "claude-opus-4-6", thinkingMode: "high", speed: "standard" }),
      bucket(),
      bucket({ model: "claude-fable-5" }),
      bucket({ speed: "fast" }),
    ]
    const series = contextTokenSeries(buckets)
    expect(series.map((p) => p.model)).toEqual([
      "claude-opus-4-6",
      "claude-opus-4-6",
      "claude-fable-5",
      "claude-fable-5",
    ])
    expect(series.map((p) => p.thinkingMode)).toEqual(["high", "high", "high", "high"])
    expect(series.map((p) => p.speed)).toEqual(["standard", "standard", "standard", "fast"])
  })

  it("leaves model, thinking mode, and speed null before the first observed value", () => {
    const buckets = [bucket(), bucket({ model: "claude-opus-4-6" })]
    const series = contextTokenSeries(buckets)
    expect(series[0]!.model).toBeNull()
    expect(series[0]!.thinkingMode).toBeNull()
    expect(series[0]!.speed).toBeNull()
    expect(series[1]!.model).toBe("claude-opus-4-6")
  })

  it("carries hasThinking per-bucket, unfilled", () => {
    const buckets = [bucket({ hasThinking: true }), bucket()]
    const series = contextTokenSeries(buckets)
    expect(series.map((p) => p.hasThinking)).toEqual([true, false])
  })

  it("carries the compaction trigger and sizes through per-bucket, unfilled", () => {
    const buckets = [
      bucket({
        isCompactionBoundary: true,
        compactionTrigger: "manual",
        compactionPreTokens: 196_000,
        compactionPostTokens: 11_000,
      }),
      bucket(),
    ]
    const series = contextTokenSeries(buckets)
    expect(series[0]!.compactionTrigger).toBe("manual")
    expect(series[0]!.compactionPreTokens).toBe(196_000)
    expect(series[0]!.compactionPostTokens).toBe(11_000)
    expect(series[1]!.compactionTrigger).toBeNull()
    expect(series[1]!.compactionPreTokens).toBeNull()
    expect(series[1]!.compactionPostTokens).toBeNull()
  })
})

describe("modeChangeMarkers", () => {
  it("emits no marker for the starting mode, only for later changes", () => {
    const buckets = [
      bucket({ model: "claude-opus-4-6", thinkingMode: "high", speed: "standard" }),
      bucket({ model: "claude-opus-4-6", thinkingMode: "high", speed: "standard" }),
      bucket({ model: "claude-fable-5", thinkingMode: "high", speed: "standard" }),
    ]
    const markers = modeChangeMarkers(contextTokenSeries(buckets))
    expect(markers).toEqual([{ index: 2, label: "opus-4-6 → fable-5" }])
  })

  it("labels an effort change and a speed change separately", () => {
    const buckets = [
      bucket({ thinkingMode: "high", speed: "standard" }),
      bucket({ thinkingMode: "low" }),
      bucket({ speed: "fast" }),
    ]
    const markers = modeChangeMarkers(contextTokenSeries(buckets))
    expect(markers).toEqual([
      { index: 1, label: "effort low" },
      { index: 2, label: "fast" },
    ])
  })

  it("joins several changes in the same bucket with a middle dot", () => {
    const buckets = [
      bucket({ model: "claude-opus-4-6", thinkingMode: "high", speed: "standard" }),
      bucket({ model: "claude-fable-5", thinkingMode: "low", speed: "fast" }),
    ]
    const markers = modeChangeMarkers(contextTokenSeries(buckets))
    expect(markers).toEqual([{ index: 1, label: "opus-4-6 → fable-5 · effort low · fast" }])
  })

  it("still marks fast speed at the very start, unlike model or effort", () => {
    const buckets = [bucket({ model: "claude-opus-4-6", thinkingMode: "high", speed: "fast" })]
    const markers = modeChangeMarkers(contextTokenSeries(buckets))
    expect(markers).toEqual([{ index: 0, label: "fast" }])
  })

  it("emits nothing when the starting speed is standard", () => {
    const buckets = [bucket({ model: "claude-opus-4-6", speed: "standard" })]
    const markers = modeChangeMarkers(contextTokenSeries(buckets))
    expect(markers).toEqual([])
  })

  it("emits nothing when no bucket carries a mode signal", () => {
    const buckets = [bucket(), bucket(), bucket()]
    const markers = modeChangeMarkers(contextTokenSeries(buckets))
    expect(markers).toEqual([])
  })
})

describe("axisScale", () => {
  it("scales the ceiling to the peak with headroom, not to the cap", () => {
    expect(axisScale(130_000, 1_000_000, 5)).toEqual({
      ceiling: 150_000,
      ticks: [50_000, 100_000],
    })
  })

  it("never exceeds the cap", () => {
    expect(axisScale(195_000, 200_000, 5)).toEqual({
      ceiling: 200_000,
      ticks: [50_000, 100_000, 150_000],
    })
  })

  it("uses coarse steps for a deep session", () => {
    expect(axisScale(900_000, 1_000_000, 5)).toEqual({
      ceiling: 1_000_000,
      ticks: [200_000, 400_000, 600_000, 800_000],
    })
  })

  it("uses fine steps for a shallow session", () => {
    expect(axisScale(18_000, 1_000_000, 5)).toEqual({
      ceiling: 20_000,
      ticks: [5_000, 10_000, 15_000],
    })
  })
})

describe("formatTokenBand", () => {
  it("drops a trailing .0 that formatCompact would show", () => {
    expect(formatTokenBand(200_000)).toBe("200k")
    expect(formatTokenBand(50_000)).toBe("50k")
    expect(formatTokenBand(1_000_000)).toBe("1M")
  })

  it("keeps a real decimal", () => {
    expect(formatTokenBand(1_500_000)).toBe("1.5M")
  })
})

describe("formatTokensShort", () => {
  it("shows a plain integer under 1000", () => {
    expect(formatTokensShort(0)).toBe("0")
    expect(formatTokensShort(950)).toBe("950")
    expect(formatTokensShort(999)).toBe("999")
  })

  it("shows one decimal in the thousands and millions when the scaled value is under 10", () => {
    expect(formatTokensShort(1_200)).toBe("1.2k")
    expect(formatTokensShort(2_100_000)).toBe("2.1M")
    expect(formatTokensShort(2_100_000_000)).toBe("2.1B")
  })

  it("drops the decimal at or above 10 in the chosen unit", () => {
    expect(formatTokensShort(14_000)).toBe("14k")
    expect(formatTokensShort(143_000)).toBe("143k")
    expect(formatTokensShort(143_000_000)).toBe("143M")
  })

  it("drops a trailing .0", () => {
    expect(formatTokensShort(1_000)).toBe("1k")
    expect(formatTokensShort(1_000_000)).toBe("1M")
    expect(formatTokensShort(1_000_000_000)).toBe("1B")
  })

  it("rolls a value that would round to 1000 in its unit over to the next unit", () => {
    expect(formatTokensShort(999_600)).toBe("1M")
    expect(formatTokensShort(999_600_000)).toBe("1B")
  })

  it("never exceeds three digits before the unit suffix", () => {
    for (const n of [950, 1_200, 14_000, 143_000, 2_100_000, 143_000_000, 2_100_000_000]) {
      const text = formatTokensShort(n).replace(/[kMB]$/, "")
      expect(text.replace(".", "").length).toBeLessThanOrEqual(3)
    }
  })
})

describe("formatTime", () => {
  it("formats whole seconds as a zero-padded clock offset", () => {
    expect(formatTime(0)).toBe("00:00:00")
    expect(formatTime(899)).toBe("00:14:59")
    expect(formatTime(900)).toBe("00:15:00")
    expect(formatTime(3661)).toBe("01:01:01")
  })
})

describe("formatDuration", () => {
  it("carries the minute remainder into the hour instead of printing 60m", () => {
    expect(formatDuration(7170)).toBe("2h")
    expect(formatDuration(3570)).toBe("1h")
  })

  it("shows seconds under a minute instead of collapsing to 0m", () => {
    expect(formatDuration(0)).toBe("0s")
    expect(formatDuration(30)).toBe("30s")
    expect(formatDuration(59)).toBe("59s")
  })

  it("formats hours and minutes compactly", () => {
    expect(formatDuration(60)).toBe("1m")
    expect(formatDuration(3600)).toBe("1h")
    expect(formatDuration(3660)).toBe("1h 1m")
    expect(formatDuration(8220)).toBe("2h 17m")
  })
})

describe("formatCost", () => {
  it("always renders two decimals", () => {
    expect(formatCost(0.003)).toBe("<$0.01")
    expect(formatCost(0.42)).toBe("$0.42")
    expect(formatCost(2.4)).toBe("$2.40")
    expect(formatCost(20.7)).toBe("$20.70")
    expect(formatCost(240)).toBe("$240.00")
  })

  it("renders an exact zero as $0.00, not <$0.01", () => {
    // A structurally-zero component must read as a true zero, so the breakdown
    // rows still sum to the total.
    expect(formatCost(0)).toBe("$0.00")
  })

  it("clamps non-finite or negative (malformed) input to $0.00", () => {
    expect(formatCost(Number.NaN)).toBe("$0.00")
    expect(formatCost(Number.POSITIVE_INFINITY)).toBe("$0.00")
    expect(formatCost(-1)).toBe("$0.00")
  })
})

describe("costBreakdownRows", () => {
  it("maps the four billable components in display order", () => {
    const rows = costBreakdownRows({
      inputUsd: 0.3,
      outputUsd: 0.8,
      cacheReadUsd: 1.1,
      cacheWriteUsd: 0.2,
    })
    expect(rows.map((r) => r.label)).toEqual(["Input", "Output", "Cache read", "Cache write"])
    expect(rows.map((r) => r.usd)).toEqual([0.3, 0.8, 1.1, 0.2])
    // A caller with USD only (the activity list's payload) leaves tokens unset.
    expect(rows.map((r) => r.tokens)).toEqual([undefined, undefined, undefined, undefined])
  })

  it("carries each component's token count alongside its USD, when given one", () => {
    const rows = costBreakdownRows({
      inputUsd: 0.3,
      outputUsd: 0.8,
      cacheReadUsd: 1.1,
      cacheWriteUsd: 0.2,
      inputTokens: 1_000,
      outputTokens: 2_000,
      cacheReadTokens: 3_000,
      cacheWriteTokens: 4_000,
    })
    expect(rows.map((r) => r.tokens)).toEqual([1_000, 2_000, 3_000, 4_000])
  })
})

describe("costFigureLabel", () => {
  it("labels a live figure a projection and a settled one an estimate", () => {
    expect(costFigureLabel(true)).toBe("Projected cost")
    expect(costFigureLabel(false)).toBe("Estimated cost")
  })
})

describe("skillMcpUsage", () => {
  const breakdown = (sources: InitialContextBreakdown["sources"]): InitialContextBreakdown => ({
    sources,
  })

  it("builds one row per named skill/MCP source, sorted by tokens descending", () => {
    const ic = breakdown([
      { source: "skill_instructions", sourceName: "research", tokenCount: 500, useCount: 3 },
      { source: "mcp_instructions", sourceName: "figma", tokenCount: 900, useCount: 0 },
      { source: "skill_instructions", sourceName: "imagegen", tokenCount: 200, useCount: 1 },
    ])
    const usage = skillMcpUsage(ic)
    expect(usage.totalTokens).toBe(1600)
    expect(usage.rows.map((r) => [r.kind, r.name])).toEqual([
      ["mcp", "figma"],
      ["skill", "research"],
      ["skill", "imagegen"],
    ])
  })

  it("computes the wasted-token total from rows the session never used", () => {
    const ic = breakdown([
      { source: "skill_instructions", sourceName: "a", tokenCount: 300, useCount: 1 },
      { source: "mcp_instructions", sourceName: "b", tokenCount: 100, useCount: 0 },
    ])
    const usage = skillMcpUsage(ic)
    expect(usage.wastedTokens).toBe(100)
  })

  it("treats a missing useCount as unused", () => {
    const ic = breakdown([{ source: "skill_instructions", sourceName: "a", tokenCount: 300 }])
    const usage = skillMcpUsage(ic)
    expect(usage.rows.map((r) => r.useCount)).toEqual([0])
    expect(usage.wastedTokens).toBe(300)
  })

  it("carries the origin through, treating a missing one as unknown", () => {
    const ic = breakdown([
      { source: "skill_instructions", sourceName: "a", tokenCount: 300, origin: "user" },
      { source: "mcp_instructions", sourceName: "b", tokenCount: 100 },
    ])
    const usage = skillMcpUsage(ic)
    expect(usage.rows.map((r) => r.origin)).toEqual(["user", "unknown"])
  })

  it("ignores unnamed sources", () => {
    const ic = breakdown([
      { source: "skill_instructions", sourceName: null, tokenCount: 900 },
      { source: "mcp_instructions", sourceName: null, tokenCount: 900 },
    ])
    const usage = skillMcpUsage(ic)
    expect(usage.totalTokens).toBe(0)
    expect(usage.rows).toEqual([])
  })

  it("reports zero totals when there is nothing to attribute", () => {
    const usage = skillMcpUsage(breakdown([]))
    expect(usage.wastedTokens).toBe(0)
    expect(usage.totalTokens).toBe(0)
    expect(usage.rows).toEqual([])
  })

  it("maps a builtin_tool source to a tool row and carries deferred through", () => {
    const ic = breakdown([
      { source: "skill_instructions", sourceName: "research", tokenCount: 500, useCount: 1 },
      {
        source: "builtin_tool",
        sourceName: "Bash",
        tokenCount: 300,
        useCount: 2,
        origin: "bundled",
      },
      {
        source: "builtin_tool",
        sourceName: "Read",
        tokenCount: 20,
        useCount: 0,
        origin: "bundled",
        deferred: true,
      },
    ])
    const usage = skillMcpUsage(ic)
    expect(usage.rows.map((r) => [r.kind, r.name, r.deferred])).toEqual([
      ["skill", "research", undefined],
      ["tool", "Bash", undefined],
      ["tool", "Read", true],
    ])
  })

  it("counts a deferred unused tool's estimate toward wasted tokens", () => {
    const ic = breakdown([
      {
        source: "builtin_tool",
        sourceName: "Read",
        tokenCount: 20,
        useCount: 0,
        origin: "bundled",
        deferred: true,
      },
    ])
    const usage = skillMcpUsage(ic)
    expect(usage.wastedTokens).toBe(20)
  })

  it("breaks a token-count tie by name", () => {
    const ic = breakdown([
      { source: "skill_instructions", sourceName: "zeta", tokenCount: 100 },
      { source: "mcp_instructions", sourceName: "alpha", tokenCount: 100 },
    ])
    const usage = skillMcpUsage(ic)
    expect(usage.rows.map((r) => r.name)).toEqual(["alpha", "zeta"])
  })
})

describe("skillMcpStatusLabel", () => {
  it("labels an unused row, a single use, and a repeated use", () => {
    expect(skillMcpStatusLabel({ useCount: 0 })).toBe("Unused")
    expect(skillMcpStatusLabel({ useCount: 1 })).toBe("Used")
    expect(skillMcpStatusLabel({ useCount: 3 })).toBe("Used ×3")
  })

  it("labels an unused deferred tool 'Deferred', but a used one by its use count", () => {
    expect(skillMcpStatusLabel({ useCount: 0, deferred: true })).toBe("Deferred")
    expect(skillMcpStatusLabel({ useCount: 2, deferred: true })).toBe("Used ×2")
  })
})

describe("skillMcpOriginLabel", () => {
  it("labels every known origin, and reports null for unknown", () => {
    expect(skillMcpOriginLabel("bundled")).toBe("Bundled")
    expect(skillMcpOriginLabel("plugin")).toBe("Plugin")
    expect(skillMcpOriginLabel("user")).toBe("User")
    expect(skillMcpOriginLabel("project")).toBe("Project")
    expect(skillMcpOriginLabel("unknown")).toBeNull()
  })
})

describe("median / costOutlierThreshold", () => {
  it("returns the middle value, averaging the two middles for an even list", () => {
    expect(median([3, 1, 2])).toBe(2)
    expect(median([1, 2, 3, 4])).toBe(2.5)
    expect(median([])).toBe(0)
  })

  it("does not mutate its input", () => {
    const xs = [3, 1, 2]
    median(xs)
    expect(xs).toEqual([3, 1, 2])
  })

  it("returns null below the minimum sample size", () => {
    expect(costOutlierThreshold(Array(HIGH_COST_MIN_SAMPLE - 1).fill(5))).toBeNull()
  })

  it("uses the median multiple once the median clears the floor", () => {
    expect(costOutlierThreshold(Array(HIGH_COST_MIN_SAMPLE).fill(2))).toBe(
      HIGH_COST_MEDIAN_MULTIPLE * 2,
    )
  })

  it("falls back to the absolute floor for a cheap cohort", () => {
    expect(costOutlierThreshold(Array(HIGH_COST_MIN_SAMPLE).fill(0.1))).toBe(
      HIGH_COST_FLOOR_USD,
    )
  })
})

describe("timeAxisTicks", () => {
  it("uses the coarsest step that keeps the mark count under the limit", () => {
    // 2h 10m of active time over 181 buckets: 30m steps give four marks.
    const ticks = timeAxisTicks(7800, 181, 6)
    expect(ticks.map((t) => t.label)).toEqual(["30m", "1h", "1h 30m", "2h"])
    expect(ticks[1]!.index).toBeCloseTo((3600 / 7800) * 180, 5)
  })

  it("returns nothing for an empty or instant session", () => {
    expect(timeAxisTicks(0, 181, 6)).toEqual([])
    expect(timeAxisTicks(600, 1, 6)).toEqual([])
  })

  it("never places a mark at or past the end", () => {
    const ticks = timeAxisTicks(3600, 181, 6)
    expect(ticks.map((t) => t.label)).toEqual(["10m", "20m", "30m", "40m", "50m"])
  })
})

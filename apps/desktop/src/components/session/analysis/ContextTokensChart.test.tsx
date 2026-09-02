import { cleanup, render, screen } from "@testing-library/react"
import {
  cloneElement,
  isValidElement,
  type ComponentProps,
  type CSSProperties,
  type ReactElement,
  type ReactNode,
} from "react"
import type * as Recharts from "recharts"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { ContextTokenPoint } from "../../../lib/presentation/sessionAnalysis"
import type { SessionBucket } from "../../../lib/types/session"
import { ContextTokensChart, ContextTokensTooltip } from "./ContextTokensChart"

afterEach(cleanup)

// jsdom never measures a real layout size, so recharts' `ResponsiveContainer`
// normally waits forever for a non-zero size from `ResizeObserver` before it
// renders its children. Stand in a version that hands the chart a fixed
// width/height directly (the same props `ResponsiveContainer` would measure
// and pass down), so the chart — and its `ReferenceLine` marks — renders into
// the DOM for the tests below.
vi.mock("recharts", async (importOriginal) => {
  const actual = await importOriginal<typeof Recharts>()
  return {
    ...actual,
    Area: (props: ComponentProps<typeof actual.Area>) => (
      <g
        data-animation-active={String(props.isAnimationActive)}
        data-animation-begin={String(props.animationBegin)}
      >
        <actual.Area {...props} />
      </g>
    ),
    ResponsiveContainer: ({
      children,
      className,
      style,
    }: {
      children: ReactNode
      className?: string
      style?: CSSProperties
    }) => (
      <div
        className={`recharts-responsive-container ${className ?? ""}`}
        style={{ ...style, width: 600, height: 160 }}
      >
        {isValidElement(children)
          ? cloneElement(children as ReactElement<{ width?: number; height?: number }>, {
              width: 600,
              height: 160,
            })
          : children}
      </div>
    ),
  }
})

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

describe("ContextTokensChart", () => {
  it("plays the first bucket set in as a sequence and animates a replacement set", () => {
    const initialBuckets = [
      bucket({ contextTokens: 100_000 }),
      bucket({ contextTokens: 120_000 }),
    ]
    const { container, rerender } = render(
      <ContextTokensChart buckets={initialBuckets} contextWindow={200_000} />,
    )

    // Every area animates, and the token layers start after the context
    // fill, so the chart draws itself in order.
    expect(container.querySelectorAll('g[data-animation-active="true"]')).toHaveLength(4)
    const wrapper = container.querySelector<HTMLElement>(".recharts-responsive-container")
    expect(wrapper?.style.getPropertyValue("--chart-mark-delay")).not.toBe("0ms")
    const begins = [...container.querySelectorAll("g[data-animation-begin]")].map((node) =>
      Number(node.getAttribute("data-animation-begin")),
    )
    expect(begins[0]).toBe(0)
    expect(begins.slice(1).every((begin) => begin > 0)).toBe(true)
    expect(begins).toEqual([...begins].sort((a, b) => a - b))

    rerender(
      <ContextTokensChart
        buckets={[...initialBuckets, bucket({ contextTokens: 140_000 })]}
        contextWindow={200_000}
      />,
    )

    // A live update arrives all at once: a staggered replay would read as
    // the panel redrawing itself.
    expect(container.querySelectorAll('g[data-animation-active="true"]')).toHaveLength(4)
    expect(wrapper?.style.getPropertyValue("--chart-mark-delay")).toBe("0ms")
    expect(
      [...container.querySelectorAll("g[data-animation-begin]")].every(
        (node) => node.getAttribute("data-animation-begin") === "0",
      ),
    ).toBe(true)
  })

  it("positions a cache-rehydration bar between the cached prefix and context growth", () => {
    const buckets = [
      bucket({ contextTokens: 200_000 }),
      bucket({
        contextTokens: 160_000,
        cacheReadTokens: 50_000,
        cacheWriteTokens: 110_000,
        rewriteTokens: 100_000,
        isCacheRehydration: true,
        cacheRehydration: {
          contextTokens: 160_000,
          stillCachedTokens: 50_000,
          rewrittenTokens: 100_000,
          growthTokens: 10_000,
        },
      }),
      bucket({ contextTokens: 200_000 }),
    ]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} />,
    )

    const bar = container.querySelector('line[stroke="var(--color-context-rewrite)"]')
    // The mark carries the entrance class, so it fades in after the areas
    // have finished growing.
    expect(container.querySelector(".animate-chart-mark")).toBeTruthy()
    expect(bar).not.toBeNull()
    expect(Number(bar?.getAttribute("stroke-width"))).toBeGreaterThanOrEqual(4)
    // The bar spans the bottom quarter of the plot only: 50k of a 200k peak.
    const y1 = Number(bar?.getAttribute("y1"))
    const y2 = Number(bar?.getAttribute("y2"))
    const top = Math.min(y1, y2)
    const bottom = Math.max(y1, y2)
    // The cached 50k keeps the bar off the baseline. The 10k growth keeps it below 160k.
    expect(bottom).toBeLessThan(160 * 0.85)
    expect(top).toBeGreaterThan(160 * 0.15)
    expect(bottom - top).toBeGreaterThan(160 / 3)
    expect(screen.getByText("rehydration")).toBeInTheDocument()
  })

  it("draws no cache-rehydration marker when no bucket is flagged", () => {
    const buckets = [bucket({ contextTokens: 100_000 }), bucket({ contextTokens: 120_000 })]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} />,
    )

    const rehydrationLines = container.querySelectorAll(
      'line[stroke="var(--color-context-rewrite)"]',
    )
    expect(rehydrationLines.length).toBe(0)
  })

  it("draws no compaction line and keeps the rehydration marker solid", () => {
    const buckets = [
      bucket({ contextTokens: 100_000, isCompactionBoundary: true }),
      bucket({
        contextTokens: 100_000,
        cacheWriteTokens: 90_000,
        rewriteTokens: 90_000,
        isCacheRehydration: true,
      }),
    ]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} />,
    )

    const compactionLine = container.querySelector('line[stroke="var(--color-label-tertiary)"]')
    const rehydrationLine = container.querySelector(
      'line[stroke="var(--color-context-rewrite)"]',
    )
    expect(compactionLine).toBeNull()
    expect(rehydrationLine?.getAttribute("stroke-dasharray")).toBeFalsy()
  })

  it("draws a mode-change marker with no line, only its label", () => {
    const buckets = [
      bucket({ contextTokens: 100_000, model: "claude-opus-4-6" }),
      bucket({ contextTokens: 100_000, model: "claude-fable-5" }),
    ]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} />,
    )

    expect(screen.getByText("opus-4-6 → fable-5")).toBeInTheDocument()
    expect(container.querySelectorAll('line[stroke="none"]').length).toBeGreaterThan(0)
  })

  it("draws no mode-change marker when no bucket carries a mode signal", () => {
    const buckets = [bucket({ contextTokens: 100_000 }), bucket({ contextTokens: 120_000 })]
    render(<ContextTokensChart buckets={buckets} contextWindow={200_000} />)

    expect(screen.queryAllByText(/→|effort |^fast$/)).toEqual([])
  })

  it("draws a provider cache miss as a thin unlabeled rewrite", () => {
    const buckets = [
      bucket({ contextTokens: 200_000 }),
      bucket({
        contextTokens: 50_000,
        cacheWriteTokens: 40_000,
        rewriteTokens: 40_000,
        isCacheRoutingMiss: true,
        cacheRehydration: {
          contextTokens: 50_000,
          stillCachedTokens: 10_000,
          rewrittenTokens: 40_000,
          growthTokens: 0,
        },
      }),
      bucket({ contextTokens: 200_000 }),
    ]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} />,
    )

    const bar = container.querySelector('line[stroke="var(--color-context-rewrite)"]')
    expect(bar).not.toBeNull()
    expect(bar?.getAttribute("stroke-opacity")).toBe("0.4")
    expect(bar?.getAttribute("stroke-width")).toBe("2")
    expect(screen.queryByText(/miss|rewrite/)).not.toBeInTheDocument()
  })

  it("draws every text label after the plot layers, so none sits under an area", () => {
    const buckets = [
      bucket({ contextTokens: 113_000, tokensIn: 4_000 }),
      bucket({ contextTokens: 150_000, rewriteTokens: 90_000, tokensIn: 9_000 }),
      bucket({ contextTokens: 160_000, tokensIn: 4_000 }),
    ]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} activeSecs={600} />,
    )

    // SVG paints in document order, so a label is only legible over the areas
    // when its element comes after them. Compare positions in one flat list.
    const nodes = Array.from(container.querySelectorAll("*"))
    const areas = nodes.filter((node) => node.classList.contains("recharts-area"))
    expect(areas.length).toBeGreaterThan(0)
    const lastArea = nodes.indexOf(areas[areas.length - 1]!)

    const labels = Array.from(container.querySelectorAll("text"))
    expect(labels.length).toBeGreaterThan(0)
    for (const label of labels) {
      expect(nodes.indexOf(label)).toBeGreaterThan(lastArea)
    }
  })

  it("draws consecutive material rewrites without cache-event flags", () => {
    const buckets = [
      bucket({ contextTokens: 113_000 }),
      bucket({ contextTokens: 114_400, rewriteTokens: 106_088 }),
      bucket({ contextTokens: 115_800, rewriteTokens: 107_488 }),
      bucket({ contextTokens: 117_200, rewriteTokens: 5_512 }),
    ]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={258_400} />,
    )

    expect(
      container.querySelectorAll('line[stroke="var(--color-context-rewrite)"]'),
    ).toHaveLength(2)
    for (const line of container.querySelectorAll(
      'line[stroke="var(--color-context-critical)"]',
    )) {
      expect(line.getAttribute("stroke-width")).toBe("2")
    }
    expect(screen.queryByText("rewrite")).not.toBeInTheDocument()
  })

  it("shares one label between cache events that sit close on a long session", () => {
    // Two rehydrations two slices apart, in a 30-slice session: the bars would
    // overlap their labels, so only the first bar carries one.
    const marked = (index: number) => index === 10 || index === 12
    const buckets = Array.from({ length: 30 }, (_, index) =>
      bucket({
        contextTokens: 100_000,
        rewriteTokens: marked(index) ? 90_000 : 0,
        isCacheRehydration: marked(index),
        ...(marked(index)
          ? {
              cacheRehydration: {
                contextTokens: 100_000,
                stillCachedTokens: 10_000,
                rewrittenTokens: 90_000,
                growthTokens: 0,
              },
            }
          : {}),
      }),
    )
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={258_400} />,
    )

    expect(
      container.querySelectorAll('line[stroke="var(--color-context-rewrite)"]'),
    ).toHaveLength(2)
    expect(screen.getAllByText("rehydration")).toHaveLength(1)
  })

  it("draws sub-agent tokens as a third series on the token axis", () => {
    const buckets = [bucket({ tokensIn: 100, tokensOut: 20 }), bucket({ subagentTokens: 500 })]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} />,
    )

    // The series carries its color as a solid fill, not a gradient.
    expect(container.querySelector('linearGradient[id$="-subagentTokens"]')).toBeNull()
    expect(container.querySelector('path[fill="var(--color-token-subagent)"]')).not.toBeNull()
  })
})

function point(over: Partial<ContextTokenPoint> = {}): ContextTokenPoint {
  return {
    index: 0,
    progress: 50,
    contextTokens: 100_000,
    tokensIn: 1_000,
    tokensOut: 200,
    subagentTokens: 0,
    isCompactionBoundary: false,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    rewriteTokens: 0,
    isCacheRehydration: false,
    cacheRehydration: null,
    isCacheRoutingMiss: false,
    secsSincePriorTurn: null,
    subagentLaunches: 0,
    lastTool: null,
    betweenCalls: null,
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

describe("ContextTokensTooltip", () => {
  it("names the tool call and gap length for a bucket with no model call", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              tokensIn: 0,
              tokensOut: 0,
              betweenCalls: { secs: 130, tool: "Bash", userPrompt: false },
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("During Bash call · 2m")).toBeInTheDocument()
    expect(screen.queryByText("Tokens")).not.toBeInTheDocument()
    expect(screen.queryByText(/^Parent in/)).not.toBeInTheDocument()
  })

  it("names the drawn width of a long gap ended by a user prompt", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              tokensIn: 0,
              tokensOut: 0,
              betweenCalls: { secs: 1_500, tool: null, userPrompt: true },
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Waiting for user · 25m (5m shown)")).toBeInTheDocument()
  })

  it("keeps a long gap after a tool call attributed to the tool", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              tokensIn: 0,
              tokensOut: 0,
              betweenCalls: { secs: 1_500, tool: "Task", userPrompt: false },
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("During Task call · 25m (5m shown)")).toBeInTheDocument()
  })

  it("says the model waited for the user when a prompt ends a short gap", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              tokensIn: 0,
              tokensOut: 0,
              betweenCalls: { secs: 45, tool: "Bash", userPrompt: true },
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Waiting for user · 45s")).toBeInTheDocument()
  })

  it("falls back to a plain gap line when no tool is recorded", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              tokensIn: 0,
              tokensOut: 0,
              betweenCalls: { secs: null, tool: null, userPrompt: false },
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Between model calls")).toBeInTheDocument()
  })

  it("shows elapsed active time at the hovered bucket", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        activeSecs={7_200}
        bucketCount={5}
        payload={[{ payload: point({ index: 1, progress: 25 }) }]}
      />,
    )

    expect(screen.getByText("30m into session")).toBeInTheDocument()
    expect(screen.queryByText("25% through")).not.toBeInTheDocument()
  })

  it("explains a cache rehydration as the current context composition", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              contextTokens: 63_888,
              tokensIn: 37_537,
              cacheReadTokens: 26_351,
              cacheWriteTokens: 37_535,
              rewriteTokens: 35_397,
              isCacheRehydration: true,
              cacheRehydration: {
                contextTokens: 63_888,
                stillCachedTokens: 26_351,
                rewrittenTokens: 35_397,
                growthTokens: 2_140,
                userInactiveSecs: 9_295,
              },
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Cache rehydration · 63.9k context")).toBeInTheDocument()
    expect(screen.getByText("Context · 63.9k (32%)")).toBeInTheDocument()
    expect(screen.getByText("Still cached · 26.4k")).toBeInTheDocument()
    expect(screen.getByText("Old context rewritten · 35.4k")).toBeInTheDocument()
    expect(screen.getByText("Context growth · 2.1k")).toBeInTheDocument()
    expect(screen.getByText("User inactive · 2h 35m")).toBeInTheDocument()
    expect(screen.queryByText(/^Cache read/)).not.toBeInTheDocument()
    expect(screen.queryByText(/^Cache write/)).not.toBeInTheDocument()
    expect(screen.queryByText(/^Context rewrite/)).not.toBeInTheDocument()
    expect(screen.queryByText("Tokens")).not.toBeInTheDocument()
    expect(screen.queryByText(/^Parent in/)).not.toBeInTheDocument()
  })

  it("names a provider cache miss by its rewritten old context", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              isCacheRoutingMiss: true,
              cacheWriteTokens: 0,
              tokensIn: 42_296,
              cacheRehydration: {
                contextTokens: 64_696,
                stillCachedTokens: 22_400,
                rewrittenTokens: 41_724,
                growthTokens: 572,
              },
            }),
          },
        ]}
      />,
    )

    expect(
      screen.getByText("Provider cache miss · 41.7k old context uncached"),
    ).toBeInTheDocument()
  })

  it("names a derived rewrite separately from a reported cache write", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              rewriteTokens: 106_088,
              cacheWriteTokens: 0,
              tokensIn: 107_488,
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Context rewrite · 106.1k re-sent")).toBeInTheDocument()
    expect(screen.queryByText(/^Cache write/)).not.toBeInTheDocument()
  })

  it("shows the wall-clock gap since the prior parent turn", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[{ payload: point({ secsSincePriorTurn: 9_300 }) }]}
      />,
    )

    expect(screen.getByText("Since prior turn · 2h 35m")).toBeInTheDocument()
  })

  it("omits the prior-turn gap for a bucket without a timed parent turn", () => {
    render(
      <ContextTokensTooltip active contextWindow={200_000} payload={[{ payload: point() }]} />,
    )

    expect(screen.queryByText(/^Since prior turn/)).not.toBeInTheDocument()
  })

  it("separates parent input, parent output, and sub-agent tokens", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({ tokensIn: 1_000, tokensOut: 200, subagentTokens: 3_000 }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Parent in · 1.0k")).toBeInTheDocument()
    expect(screen.getByText("Parent out · 200")).toBeInTheDocument()
    expect(screen.getByText("Subagents · 3.0k")).toBeInTheDocument()
  })

  it("shows the sub-agent launch count when the bucket launched one", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[{ payload: point({ subagentLaunches: 2 }) }]}
      />,
    )

    expect(screen.getByText("Subagents launched · 2")).toBeInTheDocument()
  })

  it("says nothing about sub-agents when the bucket launched none", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[{ payload: point({ subagentLaunches: 0 }) }]}
      />,
    )

    expect(screen.queryByText(/Subagents launched/)).not.toBeInTheDocument()
  })

  it("shows model, effort, speed, and thinking lines when present", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              model: "claude-opus-4-6",
              thinkingMode: "high",
              speed: "fast",
              hasThinking: true,
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Model · opus-4-6")).toBeInTheDocument()
    expect(screen.getByText("Effort · high")).toBeInTheDocument()
    expect(screen.getByText("Speed · fast")).toBeInTheDocument()
    expect(screen.getByText("Thinking")).toBeInTheDocument()
  })

  it("omits mode lines that match the session baseline", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        baseline={{
          model: "claude-opus-4-6",
          thinkingMode: "high",
          speed: "fast",
          hasThinking: true,
        }}
        payload={[
          {
            payload: point({
              model: "claude-opus-4-6",
              thinkingMode: "max",
              speed: "fast",
              hasThinking: true,
            }),
          },
        ]}
      />,
    )

    expect(screen.queryByText(/^Model/)).not.toBeInTheDocument()
    expect(screen.getByText("Effort · max")).toBeInTheDocument()
    expect(screen.queryByText(/^Speed/)).not.toBeInTheDocument()
    expect(screen.queryByText("Thinking")).not.toBeInTheDocument()
  })

  it("omits model, effort, speed, and thinking lines when absent", () => {
    render(
      <ContextTokensTooltip active contextWindow={200_000} payload={[{ payload: point() }]} />,
    )

    expect(screen.queryByText(/^Model/)).not.toBeInTheDocument()
    expect(screen.queryByText(/^Effort/)).not.toBeInTheDocument()
    expect(screen.queryByText(/^Speed/)).not.toBeInTheDocument()
    expect(screen.queryByText("Thinking")).not.toBeInTheDocument()
  })

  it("labels a manual compaction with its before/after size", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              isCompactionBoundary: true,
              compactionTrigger: "manual",
              compactionPreTokens: 196_000,
              compactionPostTokens: 11_000,
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Compaction (manual) · 196.0k → 11.0k")).toBeInTheDocument()
  })

  it("labels an auto compaction with its before/after size", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              isCompactionBoundary: true,
              compactionTrigger: "auto",
              compactionPreTokens: 198_000,
              compactionPostTokens: 12_000,
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Compaction (auto) · 198.0k → 12.0k")).toBeInTheDocument()
  })

  it("labels a compaction with an unknown trigger plainly", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[{ payload: point({ isCompactionBoundary: true }) }]}
      />,
    )

    expect(screen.getByText("Compaction")).toBeInTheDocument()
  })

  it("shows only the before size when postTokens is missing", () => {
    render(
      <ContextTokensTooltip
        active
        contextWindow={200_000}
        payload={[
          {
            payload: point({
              isCompactionBoundary: true,
              compactionTrigger: "auto",
              compactionPreTokens: 196_000,
            }),
          },
        ]}
      />,
    )

    expect(screen.getByText("Compaction (auto) · 196.0k before")).toBeInTheDocument()
  })
})

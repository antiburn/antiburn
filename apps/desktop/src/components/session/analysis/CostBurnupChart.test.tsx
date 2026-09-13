import { act, cleanup, render, screen } from "@testing-library/react"
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

import type { CostBurnupPoint } from "../../../lib/presentation/sessionAnalysis"
import type { SessionBucket } from "../../../lib/types/session"
import { CostBurnupChart, CostBurnupTooltip } from "./CostBurnupChart"

const resizeHarness = vi.hoisted(() => ({
  onResize: undefined as ((width: number, height: number) => void) | undefined,
}))

afterEach(cleanup)

// Mirrors ContextTokensChart.test.tsx's recharts stand-in: jsdom never
// measures a real layout size, so ResponsiveContainer is replaced with a
// version that hands the chart a fixed width/height directly.
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
      onResize,
    }: {
      onResize?: (width: number, height: number) => void
      children: ReactNode
      className?: string
      style?: CSSProperties
    }) => {
      resizeHarness.onResize = onResize
      return (
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
      )
    },
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

function pricedBucket(over: Partial<SessionBucket> = {}): SessionBucket {
  return bucket({
    cost: { totalUsd: 1, inputUsd: 0.4, outputUsd: 0.3, cacheReadUsd: 0.2, cacheWriteUsd: 0.1 },
    ...over,
  })
}

describe("CostBurnupChart", () => {
  it("stacks the four components in order from the baseline up", () => {
    const buckets = [pricedBucket(), pricedBucket()]
    const { container } = render(<CostBurnupChart buckets={buckets} />)
    const areas = container.querySelectorAll("path.recharts-area-area")
    expect(areas.length).toBe(4)
    expect([...areas].map((area) => area.getAttribute("fill"))).toEqual([
      "var(--color-token-in)",
      "var(--color-token-out)",
      "var(--color-cost-cache-read)",
      "var(--color-cost-cache-write)",
    ])
  })

  it("lights every layer at rest, with no grey in the plot", () => {
    const buckets = [pricedBucket(), pricedBucket()]
    const { container } = render(<CostBurnupChart buckets={buckets} />)
    expect(container.querySelector('path[fill="var(--color-chart-rest)"]')).toBeNull()
    expect(container.querySelector('path[fill="var(--color-chart-rest-strong)"]')).toBeNull()
    expect(container.querySelector('path[fill="var(--color-chart-rest-faint)"]')).toBeNull()
    expect(container.querySelector('path[fill="var(--color-chart-rest-fainter)"]')).toBeNull()
  })

  it("lights one named layer and greys the other three", () => {
    const buckets = [pricedBucket(), pricedBucket()]
    const { container } = render(<CostBurnupChart buckets={buckets} highlight="cacheWrite" />)
    expect(container.querySelector('path[fill="var(--color-cost-cache-write)"]')).not.toBeNull()
    expect(container.querySelector('path[fill="var(--color-token-in)"]')).toBeNull()
    expect(container.querySelector('path[fill="var(--color-token-out)"]')).toBeNull()
    expect(container.querySelector('path[fill="var(--color-cost-cache-read)"]')).toBeNull()
    expect(container.querySelector('path[fill="var(--color-chart-rest)"]')).not.toBeNull()
    expect(
      container.querySelector('path[fill="var(--color-chart-rest-strong)"]'),
    ).not.toBeNull()
    expect(container.querySelector('path[fill="var(--color-chart-rest-faint)"]')).not.toBeNull()
  })

  it("draws a compaction line at the flagged index, with a label only while highlighted", () => {
    const buckets = [pricedBucket({ isCompactionBoundary: true }), pricedBucket()]
    const { rerender, container } = render(<CostBurnupChart buckets={buckets} />)
    expect(
      container.querySelector('line[stroke="var(--color-mark-compaction)"]'),
    ).not.toBeNull()
    expect(screen.queryByText("Compaction")).not.toBeInTheDocument()

    rerender(<CostBurnupChart buckets={buckets} highlight="compaction" />)
    expect(screen.getByText("Compaction")).toBeInTheDocument()
  })

  it("names a manual compaction in its label", () => {
    const buckets = [
      pricedBucket({ isCompactionBoundary: true, compactionTrigger: "manual" }),
      pricedBucket(),
    ]
    render(<CostBurnupChart buckets={buckets} highlight="compaction" />)
    expect(screen.getByText("Manual compaction")).toBeInTheDocument()
  })

  it("draws a rehydration bar at the flagged index", () => {
    const buckets = [pricedBucket({ isCacheRehydration: true }), pricedBucket()]
    const { container } = render(<CostBurnupChart buckets={buckets} highlight="rehydration" />)
    const bar = container.querySelector('line[stroke="var(--color-mark-rehydration)"]')
    expect(bar).not.toBeNull()
    expect(bar?.getAttribute("stroke-width")).toBe("7")
  })

  it("draws a sub-agent launch tick only for a bucket that launched one", () => {
    const buckets = [pricedBucket({ subagentLaunches: 2 }), pricedBucket()]
    const { container } = render(<CostBurnupChart buckets={buckets} />)
    expect(container.querySelectorAll("rect").length).toBeGreaterThanOrEqual(1)
  })

  it("draws no sub-agent launch tick when no bucket launched one", () => {
    const buckets = [pricedBucket(), pricedBucket()]
    const { container: withoutLaunch } = render(<CostBurnupChart buckets={buckets} />)
    const baselineRects = [...withoutLaunch.querySelectorAll("rect")].filter(
      (rect) => Number(rect.getAttribute("height")) === 6,
    )
    expect(baselineRects).toHaveLength(0)
  })

  it("draws every text label after the plot layers", () => {
    const buckets = [
      pricedBucket({ isCompactionBoundary: true }),
      pricedBucket({ isCacheRehydration: true }),
    ]
    const { container } = render(<CostBurnupChart buckets={buckets} highlight="compaction" />)
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

  it("updates resized geometry without replaying the entrance animation", () => {
    const buckets = [pricedBucket(), pricedBucket()]
    const { container } = render(<CostBurnupChart buckets={buckets} />)
    act(() => resizeHarness.onResize?.(600, 160))
    expect(container.querySelectorAll('g[data-animation-active="true"]')).toHaveLength(4)
    act(() => resizeHarness.onResize?.(720, 300))
    expect(container.querySelectorAll('g[data-animation-active="false"]')).toHaveLength(4)
  })

  it("renders axes with no areas for an all-unpriced series", () => {
    const buckets = [bucket(), bucket()]
    const { container } = render(<CostBurnupChart buckets={buckets} activeSecs={600} />)
    expect(container.querySelectorAll("path.recharts-area-area")).toHaveLength(0)
    expect(container.querySelector(".recharts-yAxis")).not.toBeNull()
    expect(container.querySelector(".recharts-xAxis")).not.toBeNull()
  })
})

function point(over: Partial<CostBurnupPoint> = {}): CostBurnupPoint {
  return {
    index: 0,
    progress: 50,
    inputUsd: 1,
    outputUsd: 0.5,
    cacheReadUsd: 0.2,
    cacheWriteUsd: 0.1,
    totalUsd: 1.8,
    bucketUsd: 0.3,
    isCompactionBoundary: false,
    compactionTrigger: null,
    isCacheRehydration: false,
    isCacheRoutingMiss: false,
    subagentLaunches: 0,
    secsSincePriorTurn: null,
    model: null,
    ...over,
  }
}

describe("CostBurnupTooltip", () => {
  it("shows the running total and each component's cumulative figure", () => {
    render(
      <CostBurnupTooltip
        active
        payload={[{ payload: point() }]}
        activeSecs={7_200}
        bucketCount={5}
      />,
    )
    expect(screen.getByText(/Total so far/)).toHaveTextContent("Total so far · $1.80")
    expect(screen.getByText(/^Input/)).toHaveTextContent("Input · $1.00")
    expect(screen.getByText(/^Output/)).toHaveTextContent("Output · $0.50")
    expect(screen.getByText(/^Cache read/)).toHaveTextContent("Cache read · $0.20")
    expect(screen.getByText(/^Cache write/)).toHaveTextContent("Cache write · $0.10")
  })

  it("shows the bucket's own slice when it spent anything", () => {
    render(<CostBurnupTooltip active payload={[{ payload: point({ bucketUsd: 0.3 }) }]} />)
    expect(screen.getByText(/This slice/)).toHaveTextContent("This slice · $0.30")
  })

  it("omits the slice line when the bucket spent nothing", () => {
    render(<CostBurnupTooltip active payload={[{ payload: point({ bucketUsd: 0 }) }]} />)
    expect(screen.queryByText(/This slice/)).not.toBeInTheDocument()
  })

  it("names a compaction and a manual compaction", () => {
    render(
      <CostBurnupTooltip
        active
        payload={[
          { payload: point({ isCompactionBoundary: true, compactionTrigger: "manual" }) },
        ]}
      />,
    )
    expect(screen.getByText("Manual compaction")).toBeInTheDocument()
  })

  it("names a cache rehydration", () => {
    render(
      <CostBurnupTooltip active payload={[{ payload: point({ isCacheRehydration: true }) }]} />,
    )
    expect(screen.getByText("Cache rehydration")).toBeInTheDocument()
  })

  it("names the sub-agent launch count", () => {
    render(<CostBurnupTooltip active payload={[{ payload: point({ subagentLaunches: 2 }) }]} />)
    expect(screen.getByText(/^Sub-agents launched/)).toHaveTextContent(
      "Sub-agents launched · 2",
    )
  })

  it("shows elapsed active time at the hovered bucket", () => {
    render(
      <CostBurnupTooltip
        active
        activeSecs={7_200}
        bucketCount={5}
        payload={[{ payload: point({ index: 1 }) }]}
      />,
    )
    expect(screen.getByText("30m into session")).toBeInTheDocument()
  })

  it("omits the elapsed line when active seconds are unavailable", () => {
    render(<CostBurnupTooltip active payload={[{ payload: point() }]} />)
    expect(screen.queryByText(/into session/)).not.toBeInTheDocument()
  })
})

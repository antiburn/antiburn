// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { cleanup, render, screen } from "@testing-library/react"
import { cloneElement, isValidElement, type ReactElement, type ReactNode } from "react"
import type * as Recharts from "recharts"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { ContextTokenPoint } from "../../../lib/presentation/sessionAnalytics"
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
    ResponsiveContainer: ({ children }: { children: ReactNode }) => (
      <div style={{ width: 600, height: 160 }}>
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
    contextTokens: 0,
    isCompactionBoundary: false,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    isCacheRehydration: false,
    subagentLaunches: 0,
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
  it("draws a solid warning-colored marker for a cache-rehydration bucket", () => {
    const buckets = [
      bucket({ contextTokens: 100_000 }),
      bucket({ contextTokens: 100_000, cacheWriteTokens: 90_000, isCacheRehydration: true }),
      bucket({ contextTokens: 100_000 }),
    ]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} />,
    )

    const rehydrationLines = container.querySelectorAll(
      'line[stroke="var(--color-context-warning)"]',
    )
    expect(rehydrationLines.length).toBeGreaterThan(0)
  })

  it("draws no cache-rehydration marker when no bucket is flagged", () => {
    const buckets = [bucket({ contextTokens: 100_000 }), bucket({ contextTokens: 120_000 })]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} />,
    )

    const rehydrationLines = container.querySelectorAll(
      'line[stroke="var(--color-context-warning)"]',
    )
    expect(rehydrationLines.length).toBe(0)
  })

  it("draws no compaction line and keeps the rehydration marker solid", () => {
    const buckets = [
      bucket({ contextTokens: 100_000, isCompactionBoundary: true }),
      bucket({ contextTokens: 100_000, cacheWriteTokens: 90_000, isCacheRehydration: true }),
    ]
    const { container } = render(
      <ContextTokensChart buckets={buckets} contextWindow={200_000} />,
    )

    const compactionLine = container.querySelector('line[stroke="var(--color-label-tertiary)"]')
    const rehydrationLine = container.querySelector(
      'line[stroke="var(--color-context-warning)"]',
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
})

function point(over: Partial<ContextTokenPoint> = {}): ContextTokenPoint {
  return {
    index: 0,
    progress: 50,
    contextTokens: 100_000,
    tokensIn: 1_000,
    tokensOut: 200,
    isCompactionBoundary: false,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    isCacheRehydration: false,
    subagentLaunches: 0,
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

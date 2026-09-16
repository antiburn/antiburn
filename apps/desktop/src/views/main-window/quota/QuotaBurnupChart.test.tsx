import { cleanup, render, screen } from "@testing-library/react"
import type { CSSProperties, ReactNode } from "react"
import { cloneElement, isValidElement, type ReactElement } from "react"
import type * as Recharts from "recharts"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { QuotaPeriodPayload, QuotaUsagePayload } from "../../../lib/providerUsageIpc"
import { QuotaBurnupChart } from "./QuotaBurnupChart"

// Mirrors CostBurnupChart.test.tsx's recharts stand-in: jsdom never measures
// a real layout size, so ResponsiveContainer is replaced with a version that
// hands the chart a fixed width/height directly.
vi.mock("recharts", async (importOriginal) => {
  const actual = await importOriginal<typeof Recharts>()
  return {
    ...actual,
    ResponsiveContainer: ({
      children,
    }: {
      children: ReactNode
      className?: string
      style?: CSSProperties
      onResize?: (width: number, height: number) => void
    }) => (
      <div style={{ width: 600, height: 200 }}>
        {isValidElement(children)
          ? cloneElement(children as ReactElement<{ width?: number; height?: number }>, {
              width: 600,
              height: 200,
            })
          : children}
      </div>
    ),
  }
})

const WEEK = 7 * 24 * 60 * 60
const BUCKET = 15 * 60

function period(over: Partial<QuotaPeriodPayload> = {}): QuotaPeriodPayload {
  return {
    periodId: 1,
    startsAtEpoch: 0,
    resetsAtEpoch: WEEK,
    startSource: "reported",
    resetSource: "reported",
    samples: [{ observedAtEpoch: BUCKET, usedPercent: 40, fresh: true, authoritative: true }],
    peakPercent: 40,
    contributions: [
      {
        agent: "claude",
        sessionId: "s1",
        wslDistro: null,
        bucketStartEpoch: 0,
        usd: 1,
        percent: 10,
      },
    ],
    sessions: [
      {
        agent: "claude",
        sessionId: "s1",
        wslDistro: null,
        title: "Fix bug",
        usd: 1,
        percent: 10,
      },
    ],
    unattributed: { usd: 0.2, percent: 2, sessionCount: 1 },
    estimatedPercent: 10,
    ...over,
  }
}

function usage(over: Partial<QuotaUsagePayload> = {}): QuotaUsagePayload {
  return {
    provider: "anthropic",
    accountKey: "acct",
    lane: "weekly",
    laneLabel: "Weekly",
    rangeStartEpoch: 0,
    rangeEndEpoch: WEEK,
    factor: { usdPerPercent: 1, confidence: "learned" },
    periods: [period()],
    generatedAt: "now",
    ...over,
  }
}

afterEach(cleanup)

describe("QuotaBurnupChart", () => {
  it("draws one area per top session plus other, unattributed, and the meter line", () => {
    const { container } = render(
      <QuotaBurnupChart
        usage={usage()}
        rangeStartEpoch={0}
        rangeEndEpoch={WEEK}
        nowEpoch={-1}
        highlight={null}
        onHighlight={() => undefined}
        onPin={() => undefined}
      />,
    )
    // One stacked area for the session, one for "other", one for
    // "unattributed", plus the unfilled meter area: four area paths.
    const areas = container.querySelectorAll("path.recharts-area-area")
    expect(areas.length).toBe(4)
  })

  it("draws a dashed reset line for an inferred boundary and a solid one for a reported one", () => {
    const inferred = usage({
      periods: [period({ periodId: 1, resetSource: "cadence" })],
    })
    const { container, rerender } = render(
      <QuotaBurnupChart
        usage={inferred}
        rangeStartEpoch={0}
        rangeEndEpoch={WEEK}
        nowEpoch={-1}
        highlight={null}
        onHighlight={() => undefined}
        onPin={() => undefined}
      />,
    )
    const dashedLine = container.querySelector(
      'line[stroke="var(--color-chart-rest-mark)"][stroke-dasharray]',
    )
    expect(dashedLine).not.toBeNull()

    const reported = usage({ periods: [period({ periodId: 1, resetSource: "reported" })] })
    rerender(
      <QuotaBurnupChart
        usage={reported}
        rangeStartEpoch={0}
        rangeEndEpoch={WEEK}
        nowEpoch={-1}
        highlight={null}
        onHighlight={() => undefined}
        onPin={() => undefined}
      />,
    )
    const solidLine = container.querySelector('line[stroke="var(--color-chart-rest-mark)"]')
    expect(solidLine?.getAttribute("stroke-dasharray")).toBeNull()
  })

  it("draws a now line only when now falls inside the range", () => {
    const { container: outside } = render(
      <QuotaBurnupChart
        usage={usage()}
        rangeStartEpoch={0}
        rangeEndEpoch={WEEK}
        nowEpoch={-100}
        highlight={null}
        onHighlight={() => undefined}
        onPin={() => undefined}
      />,
    )
    expect(outside.querySelector('line[stroke="var(--color-label)"]')).toBeNull()

    const { container: inside } = render(
      <QuotaBurnupChart
        usage={usage()}
        rangeStartEpoch={0}
        rangeEndEpoch={WEEK}
        nowEpoch={BUCKET}
        highlight={null}
        onHighlight={() => undefined}
        onPin={() => undefined}
      />,
    )
    expect(inside.querySelector('line[stroke="var(--color-label)"]')).not.toBeNull()
  })

  it("shows the inferred-boundary caption only when a period's reset was not reported", () => {
    render(
      <QuotaBurnupChart
        usage={usage({ periods: [period({ resetSource: "reported" })] })}
        rangeStartEpoch={0}
        rangeEndEpoch={WEEK}
        nowEpoch={-1}
        highlight={null}
        onHighlight={() => undefined}
        onPin={() => undefined}
      />,
    )
    expect(screen.queryByText(/inferred, not stated/)).not.toBeInTheDocument()

    render(
      <QuotaBurnupChart
        usage={usage({ periods: [period({ resetSource: "turnGap" })] })}
        rangeStartEpoch={0}
        rangeEndEpoch={WEEK}
        nowEpoch={-1}
        highlight={null}
        onHighlight={() => undefined}
        onPin={() => undefined}
      />,
    )
    expect(screen.getByText(/inferred, not stated/)).toBeInTheDocument()
  })

  it("shows 'No estimate yet' for the stacked layers when the lane has no factor", () => {
    render(
      <QuotaBurnupChart
        usage={usage({ factor: null })}
        rangeStartEpoch={0}
        rangeEndEpoch={WEEK}
        nowEpoch={-1}
        highlight={null}
        onHighlight={() => undefined}
        onPin={() => undefined}
      />,
    )
    expect(screen.getAllByText("No estimate yet").length).toBeGreaterThan(0)
    // Only the unfilled meter area draws; every stacked layer is withheld.
    expect(document.querySelectorAll("path.recharts-area-area")).toHaveLength(1)
  })

  it("dims every other layer when one is highlighted", () => {
    const { container } = render(
      <QuotaBurnupChart
        usage={usage()}
        rangeStartEpoch={0}
        rangeEndEpoch={WEEK}
        nowEpoch={-1}
        highlight="other"
        onHighlight={() => undefined}
        onPin={() => undefined}
      />,
    )
    expect(container.querySelector('path[fill="var(--color-quota-other)"]')).not.toBeNull()
    expect(
      container.querySelectorAll('path[fill="var(--color-chart-rest-faint)"]').length,
    ).toBeGreaterThan(0)
  })
})

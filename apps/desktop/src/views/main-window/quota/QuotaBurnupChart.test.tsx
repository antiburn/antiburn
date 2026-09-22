import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest"

import type { QuotaPeriodPayload, QuotaUsagePayload } from "../../../lib/providerUsageIpc"
import { QuotaBurnupChart, type QuotaBurnupChartProps, xAxisTicks } from "./QuotaBurnupChart"
import { rowsInWindow } from "./quotaLayout"
import { quotaBandSpecs } from "./quotaPaths"
import { quotaBurnupSeries, type QuotaSeries } from "./quotaSeries"

// jsdom does not calculate element dimensions.
const { size } = vi.hoisted(() => ({ size: { width: 600, height: 200 } }))
vi.mock("../../../lib/useElementWidth", () => ({
  useElementWidth: () => size.width,
  useElementHeight: () => size.height,
}))

const DAY = 24 * 60 * 60
const WEEK = 7 * DAY
const BUCKET = 15 * 60

function plotBounds(container: HTMLElement) {
  const rect = container.querySelector("clipPath rect")!
  const left = Number(rect.getAttribute("x"))
  const top = Number(rect.getAttribute("y"))
  const width = Number(rect.getAttribute("width"))
  const height = Number(rect.getAttribute("height"))
  return { left, top, right: left + width, bottom: top + height, width, height }
}

function period(over: Partial<QuotaPeriodPayload> = {}): QuotaPeriodPayload {
  return {
    periodId: 1,
    startsAtEpoch: 0,
    resetsAtEpoch: WEEK,
    startSource: "reported",
    resetSource: "reported",
    samples: [{ observedAtEpoch: BUCKET, usedPercent: 40, fresh: true, authoritative: true }],
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
    unattributedBuckets: [{ bucketStartEpoch: 0, usd: 0.2, percent: 2 }],
    estimatedPercent: 10,
    unexplainedBuckets: [],
    unexplainedPercent: null,
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
    periods: [period()],
    generatedAt: "now",
    ...over,
  }
}

function seriesFor(u: QuotaUsagePayload, start = 0, end = WEEK, now = WEEK + 1): QuotaSeries {
  return quotaBurnupSeries(u, start, end, now)
}

function chartProps(
  over: { usage: QuotaUsagePayload } & Partial<QuotaBurnupChartProps>,
): QuotaBurnupChartProps {
  const rangeStartEpoch = over.rangeStartEpoch ?? 0
  const rangeEndEpoch = over.rangeEndEpoch ?? WEEK
  const nowEpoch = over.nowEpoch ?? WEEK + 1
  return {
    rangeStartEpoch,
    rangeEndEpoch,
    nowEpoch,
    periods: over.periods ?? over.usage.periods,
    showPace: over.showPace ?? true,
    series: over.series ?? seriesFor(over.usage, rangeStartEpoch, rangeEndEpoch, nowEpoch),
    onHighlight: over.onHighlight ?? (() => undefined),
  }
}

afterEach(() => {
  cleanup()
  size.width = 600
  size.height = 200
})

describe("QuotaBurnupChart", () => {
  it("draws bands with usage and omits empty bands", () => {
    const u = usage()
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    const session0 = container.querySelector("path.quota-area-s0")
    expect(session0).not.toBeNull()
    expect(session0).toHaveClass("quota-area")
    expect(session0?.getAttribute("fill")).toBe("var(--color-quota-session-1)")
    expect(container.querySelector("path.quota-area-unattributed")).not.toBeNull()
    expect(container.querySelector("path.quota-area-unattributed")?.getAttribute("fill")).toBe(
      "var(--color-chart-rest-faint)",
    )
    expect(container.querySelector("path.quota-line-top")).not.toBeNull()
    expect(container.querySelectorAll("path.quota-area")).toHaveLength(2)
    expect(container.querySelector("path.quota-area-other")).toBeNull()
  })

  it("ends the stack-top line at the sum of the last row's own bands", () => {
    const u = usage()
    const series = seriesFor(u)
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u, series })} />)

    const bandKeys = quotaBandSpecs(series.topSessions).map((spec) => spec.key)
    const windowRows = rowsInWindow(series.rows, u.periods[0]!)
    const lastRow = windowRows[windowRows.length - 1]!
    const expectedTop = bandKeys.reduce((total, key) => total + (lastRow[key] ?? 0), 0)
    const expectedY =
      plotBounds(container).top +
      (1 - expectedTop / 100) * (plotBounds(container).bottom - plotBounds(container).top)

    const d = container.querySelector("path.quota-line-top")!.getAttribute("d")!
    const points = [...d.matchAll(/[ML]([\d.-]+) ([\d.-]+)/g)]
    const [, , lastY] = points[points.length - 1]!
    expect(Number(lastY)).toBeCloseTo(expectedY, 1)
  })

  it("starts a session's path at the x of its first active bucket, not at the range start", () => {
    const late = usage({
      periods: [
        period({
          contributions: [
            {
              agent: "claude",
              sessionId: "s1",
              wslDistro: null,
              bucketStartEpoch: 2 * BUCKET,
              usd: 1,
              percent: 10,
            },
          ],
        }),
      ],
    })
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: late })} />)
    const d = container.querySelector("path.quota-area-s0")!.getAttribute("d")!
    const [, leadingX] = d.match(/^M([\d.]+) /) ?? []
    expect(Number(leadingX)).toBeGreaterThan(plotBounds(container).left)
  })

  it("draws every reset line solid, in the reset token color, with no text label", () => {
    const inferred = usage({
      periods: [period({ periodId: 1, resetSource: "cadence" })],
    })
    const { container, rerender } = render(
      <QuotaBurnupChart {...chartProps({ usage: inferred })} />,
    )
    const inferredLine = container.querySelector('[data-quota-line="reset"]')
    expect(inferredLine).not.toBeNull()
    expect(inferredLine?.getAttribute("stroke-dasharray")).toBeNull()
    expect(inferredLine?.getAttribute("stroke")).toBe("var(--color-quota-reset)")
    expect(screen.queryByText("reset")).not.toBeInTheDocument()

    const reported = usage({ periods: [period({ periodId: 1, resetSource: "reported" })] })
    rerender(<QuotaBurnupChart {...chartProps({ usage: reported })} />)
    const reportedLine = container.querySelector('[data-quota-line="reset"]')
    expect(reportedLine?.getAttribute("stroke-dasharray")).toBeNull()
    expect(reportedLine?.getAttribute("stroke")).toBe("var(--color-quota-reset)")
  })

  it("stretches the open window across the whole plot, ends its pace line where now falls on the rate, labels 'now' at the right edge, and draws no reset line", () => {
    const u = usage()
    const now = 3 * DAY
    const { container } = render(
      <QuotaBurnupChart {...chartProps({ usage: u, nowEpoch: now })} />,
    )
    expect(container.querySelector('[data-quota-line="reset"]')).toBeNull()
    const nowLabel = within(container).getByText("now")
    expect(Number(nowLabel.getAttribute("x"))).toBe(plotBounds(container).right)
    expect(nowLabel.getAttribute("text-anchor")).toBe("end")
    const paceLine = container.querySelector('[data-quota-line="pace"]')!
    expect(Number(paceLine.getAttribute("x2"))).toBe(plotBounds(container).right)
    const expectedY =
      plotBounds(container).top +
      (1 - 3 / 7) * (plotBounds(container).bottom - plotBounds(container).top)
    expect(Number(paceLine.getAttribute("y2"))).toBeCloseTo(expectedY, 5)
    const d = container.querySelector("path.quota-line-top")!.getAttribute("d")!
    const points = [...d.matchAll(/[ML]([\d.-]+) ([\d.-]+)/g)]
    const [, lastX] = points[points.length - 1]!
    const windowRows = rowsInWindow(seriesFor(u, 0, WEEK, now).rows, u.periods[0]!)
    const lastTime = windowRows[windowRows.length - 1]!.t
    const bounds = plotBounds(container)
    expect(Number(lastX)).toBeCloseTo(bounds.left + (lastTime / now) * bounds.width, 1)
  })

  it("labels a closed weekly window with its start and the midnights inside it, with no 'now'", () => {
    const u = usage()
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    expect(within(container).queryByText("now")).toBeNull()
    const ticks = [...container.querySelectorAll("text")].filter((el) =>
      /^\d+ \w+$/.test(el.textContent ?? ""),
    )
    expect(ticks.length).toBeGreaterThan(1)
    expect(ticks[0]!.getAttribute("text-anchor")).toBe("start")
    expect(ticks[1]!.getAttribute("text-anchor")).toBe("middle")
  })

  it("clips every band and the top line to the one plot-area clipPath", () => {
    const u = usage()
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    const bands = container.querySelectorAll("path.quota-area-s0")
    expect(bands).toHaveLength(1)
    const band = bands[0]!

    const topLine = container.querySelector("path.quota-line-top")!
    const plotClipMatch = band.getAttribute("clip-path")!.match(/url\(#(.+)\)/)
    expect(plotClipMatch).not.toBeNull()
    expect(topLine.getAttribute("clip-path")).toBe(band.getAttribute("clip-path"))
    const plotClipPathEl = container.querySelector(`clipPath#${plotClipMatch![1]}`)
    expect(plotClipPathEl).not.toBeNull()
    const rect = plotClipPathEl!.querySelector("rect")!
    expect(Number(rect.getAttribute("width"))).toBeGreaterThan(0)
    expect(Number(rect.getAttribute("height"))).toBeGreaterThan(0)
    expect(container.querySelectorAll("clipPath")).toHaveLength(1)
  })

  it("draws one pace line per period in range, from the window's start at 0% to its reset at 100%", () => {
    const u = usage()
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    const paceLines = container.querySelectorAll('[data-quota-line="pace"]')
    expect(paceLines).toHaveLength(1)
    const paceLine = paceLines[0]!
    expect(Number(paceLine.getAttribute("x1"))).toBe(plotBounds(container).left)
    expect(Number(paceLine.getAttribute("y1"))).toBe(plotBounds(container).bottom)
    expect(Number(paceLine.getAttribute("x2"))).toBe(plotBounds(container).right)
    expect(Number(paceLine.getAttribute("y2"))).toBe(plotBounds(container).top)
    expect(paceLine.getAttribute("stroke-dasharray")).not.toBeNull()
    const clipMatch = paceLine.getAttribute("clip-path")!.match(/url\(#(.+)\)/)
    expect(clipMatch).not.toBeNull()
    expect(container.querySelector(`clipPath#${clipMatch![1]} rect`)).not.toBeNull()
  })

  it("draws no pace line when no period is in range", () => {
    const u = usage({ periods: [] })
    const { container } = render(
      <QuotaBurnupChart {...chartProps({ usage: u, periods: [] })} />,
    )
    expect(container.querySelectorAll('[data-quota-line="pace"]')).toHaveLength(0)
  })

  it("calls onHighlight once per enter and leave when the pointer enters and leaves a band", () => {
    const u = usage()
    const onHighlight = vi.fn()
    const { container } = render(
      <QuotaBurnupChart {...chartProps({ usage: u, onHighlight })} />,
    )
    const area = container.querySelector("path.quota-area-unattributed")!
    const group = area.parentElement!
    fireEvent.mouseEnter(group)
    expect(onHighlight).toHaveBeenCalledWith("unattributed")
    fireEvent.mouseLeave(group)
    expect(onHighlight).toHaveBeenCalledWith(null)
    expect(onHighlight).toHaveBeenCalledTimes(2)
  })

  it("draws nothing but the wrapper before the container is measured", () => {
    size.width = 0
    size.height = 0
    const u = usage()
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    expect(container.querySelector("svg")).toBeNull()
  })

  it("draws no pace line when showPace is off, and the bands unchanged", () => {
    const u = usage()
    const { container } = render(
      <QuotaBurnupChart {...chartProps({ usage: u, showPace: false })} />,
    )
    expect(container.querySelectorAll("path.quota-area-s0")).toHaveLength(1)
    expect(container.querySelectorAll('[data-quota-line="pace"]')).toHaveLength(0)
  })

  describe("window slots", () => {
    const AFTER_BOTH = WEEK + 5 * 60 * 60 + 1

    function twoWindows(): QuotaUsagePayload {
      const first = period({
        periodId: 1,
        startsAtEpoch: 0,
        resetsAtEpoch: WEEK,
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
      })
      const second = period({
        periodId: 2,
        startsAtEpoch: WEEK,
        resetsAtEpoch: WEEK + 5 * 60 * 60,
        contributions: [
          {
            agent: "claude",
            sessionId: "s1",
            wslDistro: null,
            bucketStartEpoch: WEEK,
            usd: 1,
            percent: 10,
          },
        ],
      })
      return usage({ periods: [first, second] })
    }

    it("gives windows of different durations equal widths with a gap", () => {
      const u = twoWindows()
      const { container } = render(
        <QuotaBurnupChart
          {...chartProps({
            usage: u,
            rangeStartEpoch: 0,
            rangeEndEpoch: WEEK + 5 * 60 * 60,
            nowEpoch: AFTER_BOTH,
          })}
        />,
      )
      const resetLines = container.querySelectorAll('[data-quota-line="reset"]')
      expect(resetLines).toHaveLength(2)
      const paceLines = [...container.querySelectorAll('[data-quota-line="pace"]')]
      expect(paceLines).toHaveLength(2)
      const [first, second] = paceLines.map((line) => ({
        left: Number(line.getAttribute("x1")),
        right: Number(line.getAttribute("x2")),
      }))
      expect(first!.right - first!.left).toBeGreaterThan(0)
      expect(first!.right - first!.left).toBeCloseTo(second!.right - second!.left)
      expect(second!.left).toBeGreaterThan(first!.right)
    })

    it("draws a pace line per slot, from the slot's own left at 0% to its right at 100%", () => {
      const u = twoWindows()
      const { container } = render(
        <QuotaBurnupChart
          {...chartProps({
            usage: u,
            rangeStartEpoch: 0,
            rangeEndEpoch: WEEK + 5 * 60 * 60,
            nowEpoch: AFTER_BOTH,
          })}
        />,
      )
      const paceLines = container.querySelectorAll('[data-quota-line="pace"]')
      expect(paceLines).toHaveLength(2)
      const resetLines = container.querySelectorAll('[data-quota-line="reset"]')
      paceLines.forEach((line, index) => {
        expect(Number(line.getAttribute("y1"))).toBe(plotBounds(container).bottom)
        expect(Number(line.getAttribute("y2"))).toBe(plotBounds(container).top)
        expect(Number(line.getAttribute("x2"))).toBeCloseTo(
          Number(resetLines[index]!.getAttribute("x1")),
          5,
        )
      })
    })

    it("keeps each band within its own window slot", () => {
      const u = twoWindows()
      const { container } = render(
        <QuotaBurnupChart
          {...chartProps({
            usage: u,
            rangeStartEpoch: 0,
            rangeEndEpoch: WEEK + 5 * 60 * 60,
            nowEpoch: AFTER_BOTH,
          })}
        />,
      )
      const paths = container.querySelectorAll("path.quota-area-s0")
      expect(paths).toHaveLength(2)
      const paceLines = container.querySelectorAll('[data-quota-line="pace"]')
      paths.forEach((path, index) => {
        const left = Number(paceLines[index]!.getAttribute("x1"))
        const right = Number(paceLines[index]!.getAttribute("x2"))
        const points = [...path.getAttribute("d")!.matchAll(/[ML]([\d.-]+) ([\d.-]+)/g)]
        expect(points.length).toBeGreaterThan(0)
        for (const [, x] of points) {
          expect(Number(x)).toBeGreaterThanOrEqual(left)
          expect(Number(x)).toBeLessThanOrEqual(right)
        }
      })
    })

    it("draws one x-axis tick label per slot", () => {
      const u = twoWindows()
      const { container } = render(
        <QuotaBurnupChart
          {...chartProps({
            usage: u,
            rangeStartEpoch: 0,
            rangeEndEpoch: WEEK + 5 * 60 * 60,
            nowEpoch: AFTER_BOTH,
          })}
        />,
      )
      const ticks = [...container.querySelectorAll("text")].filter((el) =>
        /^\d+ \w+/.test(el.textContent ?? ""),
      )
      expect(ticks.length).toBeGreaterThanOrEqual(2)
      expect(ticks[0]!.getAttribute("text-anchor")).toBe("start")
    })
  })
})

// The test TypeScript configuration does not include Node.js global types.
const nodeEnv = (
  globalThis as unknown as { process: { env: Record<string, string | undefined> } }
).process.env

describe("xAxisTicks", () => {
  let originalTz: string | undefined

  beforeAll(() => {
    originalTz = nodeEnv.TZ
    nodeEnv.TZ = "Australia/Melbourne"
  })

  afterAll(() => {
    // Delete an absent value because process.env converts undefined to a string.
    if (originalTz === undefined) delete nodeEnv.TZ
    else nodeEnv.TZ = originalTz
  })

  it("keeps every tick at local midnight across a daylight-saving change", () => {
    expect(new Date(2026, 9, 15, 12, 0, 0).getTimezoneOffset()).toBe(-660)

    const rangeStart = Math.floor(new Date(2026, 8, 15).getTime() / 1000)
    const rangeEnd = Math.floor(new Date(2026, 10, 15).getTime() / 1000)
    const ticks = xAxisTicks(rangeStart, rangeEnd)
    expect(ticks.length).toBeGreaterThan(1)
    for (const t of ticks) {
      expect(new Date(t * 1000).getHours()).toBe(0)
    }
  })
})

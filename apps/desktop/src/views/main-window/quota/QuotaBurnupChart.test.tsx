import { cleanup, fireEvent, render, screen, within } from "@testing-library/react"
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest"

import type { QuotaPeriodPayload, QuotaUsagePayload } from "../../../lib/providerUsageIpc"
import { QuotaBurnupChart, type QuotaBurnupChartProps, xAxisTicks } from "./QuotaBurnupChart"
import { rowsInWindow } from "./quotaLayout"
import { quotaBandSpecs } from "./quotaPaths"
import { quotaBurnupSeries, type QuotaSeries } from "./quotaSeries"

// jsdom never measures a real layout size, so the sizing hooks are replaced
// with a mutable stand-in the "zero size" test can shrink to 0.
const { size } = vi.hoisted(() => ({ size: { width: 600, height: 200 } }))
vi.mock("../../../lib/useElementWidth", () => ({
  useElementWidth: () => size.width,
  useElementHeight: () => size.height,
}))

const DAY = 24 * 60 * 60
const WEEK = 7 * DAY
const BUCKET = 15 * 60
// Plot geometry at the mocked 600x200 size, matching the constants in
// QuotaBurnupChart.tsx (value axis 56px, right margin 12, top margin 6,
// time axis 16px), so a test can assert against the exact plot rectangle.
const PLOT_LEFT = 56
const PLOT_TOP = 6
const PLOT_RIGHT = 588 // width 600 - right margin 12
const PLOT_BOTTOM = 184 // height 200 - time axis 16
const PLOT_WIDTH = PLOT_RIGHT - PLOT_LEFT

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

/** The chart's own series builder, so a test's fixture usage always matches
 *  what the real caller (QuotaView) would compute and hand the chart. */
function seriesFor(u: QuotaUsagePayload, start = 0, end = WEEK, now = WEEK + 1): QuotaSeries {
  return quotaBurnupSeries(u, start, end, now)
}

/** Full chart props from a fixture usage, with every field a test does not
 *  care about defaulted: `showPace` on, matching the screen's own default,
 *  and `now` just past the fixture window's reset, so the window is closed. */
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
  it("draws one path per band with usage, at full strength, and one stack-top line, with no recharts DOM", () => {
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
    // Only the top session and unattributed hold usage in this fixture:
    // "other" and "unexplained" are empty, so they draw nothing at all.
    expect(container.querySelectorAll("path.quota-area")).toHaveLength(2)
    expect(container.querySelector("path.quota-area-other")).toBeNull()
    expect(container.querySelector(".recharts-wrapper")).toBeNull()
  })

  it("ends the stack-top line at the sum of the last row's own bands", () => {
    const u = usage()
    const series = seriesFor(u)
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u, series })} />)

    const bandKeys = quotaBandSpecs(series.topSessions).map((spec) => spec.key)
    // The line draws the window's own rows: the series' last row sits at
    // the reset itself, outside the window.
    const windowRows = rowsInWindow(series.rows, u.periods[0]!)
    const lastRow = windowRows[windowRows.length - 1]!
    const expectedTop = bandKeys.reduce((total, key) => total + (lastRow[key] ?? 0), 0)
    const expectedY = PLOT_TOP + (1 - expectedTop / 100) * (PLOT_BOTTOM - PLOT_TOP)

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
    // The plot's x at the range start is exactly PLOT_LEFT; a path
    // starting later than the range's first bucket must read past it.
    expect(Number(leadingX)).toBeGreaterThan(PLOT_LEFT)
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
    expect(Number(nowLabel.getAttribute("x"))).toBe(PLOT_RIGHT)
    expect(nowLabel.getAttribute("text-anchor")).toBe("end")
    const paceLine = container.querySelector('[data-quota-line="pace"]')!
    expect(Number(paceLine.getAttribute("x2"))).toBe(PLOT_RIGHT)
    const expectedY = PLOT_TOP + (1 - 3 / 7) * (PLOT_BOTTOM - PLOT_TOP)
    expect(Number(paceLine.getAttribute("y2"))).toBeCloseTo(expectedY, 5)
    // The last row inside the window sits at its right edge, not a fraction
    // of the way across.
    const d = container.querySelector("path.quota-line-top")!.getAttribute("d")!
    const points = [...d.matchAll(/[ML]([\d.-]+) ([\d.-]+)/g)]
    const [, lastX] = points[points.length - 1]!
    expect(Number(lastX)).toBeGreaterThan(PLOT_RIGHT - 10)
  })

  it("labels a closed weekly window with its start and the midnights inside it, with no 'now'", () => {
    const u = usage()
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    expect(within(container).queryByText("now")).toBeNull()
    const ticks = [...container.querySelectorAll("text")].filter((el) =>
      /^\d+ \w+$/.test(el.textContent ?? ""),
    )
    // A week at 532px wide fits every midnight at the 64px minimum gap.
    expect(ticks.length).toBeGreaterThanOrEqual(6)
    expect(ticks[0]!.getAttribute("text-anchor")).toBe("start")
    expect(ticks[1]!.getAttribute("text-anchor")).toBe("middle")
  })

  it("has no rotated axis title, only the percent tick labels", () => {
    const u = usage()
    render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    expect(screen.queryByText("% of limit")).not.toBeInTheDocument()
    expect(screen.getByText("100%")).toBeInTheDocument()
    expect(screen.getByText("0%")).toBeInTheDocument()
  })

  it("draws four gridlines at 25/50/75/100 percent and no 100% dashed limit line", () => {
    const u = usage()
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    const gridLines = container.querySelectorAll('[data-quota-line="grid"]')
    expect(gridLines).toHaveLength(4)
    gridLines.forEach((line) => {
      expect(line.getAttribute("stroke-dasharray")).toBeNull()
      expect(line.getAttribute("stroke")).toBe("var(--color-quota-reset)")
    })
    expect(container.querySelector('[data-quota-line="limit"]')).toBeNull()
  })

  it("draws real band values from accumulated percents", () => {
    const u = usage()
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    expect(container.querySelectorAll("path.quota-area").length).toBeGreaterThan(0)
    const topLine = container.querySelector("path.quota-line-top")
    expect(topLine).not.toBeNull()
    expect(topLine!.getAttribute("d")).not.toBe("")
  })

  it("clips every band and the top line to the one plot-area clipPath", () => {
    const u = usage()
    const { container } = render(<QuotaBurnupChart {...chartProps({ usage: u })} />)
    const bands = container.querySelectorAll("path.quota-area-s0")
    expect(bands).toHaveLength(1)
    const band = bands[0]!
    expect(band).not.toHaveClass("quota-area-under-pace")

    const topLine = container.querySelector("path.quota-line-top")!
    const plotClipMatch = band.getAttribute("clip-path")!.match(/url\(#(.+)\)/)
    expect(plotClipMatch).not.toBeNull()
    expect(topLine.getAttribute("clip-path")).toBe(band.getAttribute("clip-path"))
    const plotClipPathEl = container.querySelector(`clipPath#${plotClipMatch![1]}`)
    expect(plotClipPathEl).not.toBeNull()
    const rect = plotClipPathEl!.querySelector("rect")!
    expect(Number(rect.getAttribute("x"))).toBe(PLOT_LEFT)
    expect(Number(rect.getAttribute("y"))).toBe(PLOT_TOP)
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
    expect(Number(paceLine.getAttribute("x1"))).toBe(PLOT_LEFT)
    expect(Number(paceLine.getAttribute("y1"))).toBe(PLOT_BOTTOM)
    expect(Number(paceLine.getAttribute("x2"))).toBe(PLOT_RIGHT)
    expect(Number(paceLine.getAttribute("y2"))).toBe(PLOT_TOP)
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
    expect(container.querySelector(".flex.min-h-0.flex-1.flex-col")).not.toBeNull()
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
    /** Just past the second window's reset, so both windows are closed. */
    const AFTER_BOTH = WEEK + 5 * 60 * 60 + 1

    /** Two windows: a full week, then a five-hour window starting right at
     *  its reset, so the two slots sit back to back with distinct spans. */
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

    it("gives two windows two slots with a 6px gap between them", () => {
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
      const rights = [...resetLines].map((line) => Number(line.getAttribute("x1")))
      const slotWidth = (PLOT_WIDTH - 6) / 2
      expect(rights[0]).toBeCloseTo(PLOT_LEFT + slotWidth, 5)
      expect(rights[1]).toBeCloseTo(PLOT_LEFT + slotWidth * 2 + 6, 5)
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
        expect(Number(line.getAttribute("y1"))).toBe(PLOT_BOTTOM)
        expect(Number(line.getAttribute("y2"))).toBe(PLOT_TOP)
        expect(Number(line.getAttribute("x2"))).toBeCloseTo(
          Number(resetLines[index]!.getAttribute("x1")),
          5,
        )
      })
      // The second slot's left is the first slot's right plus the gap.
      expect(Number(paceLines[1]!.getAttribute("x1"))).toBeCloseTo(
        Number(paceLines[0]!.getAttribute("x2"))! + 6,
        5,
      )
    })

    it("starts a band's path at its own slot's left, not the shared range start", () => {
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
      // One path per slot.
      expect(paths).toHaveLength(2)
      const secondSlotPath = [...paths].find((path) => {
        const d = path.getAttribute("d")!
        const [, leadingX] = d.match(/^M([\d.]+) /) ?? []
        return Number(leadingX) > PLOT_LEFT + (PLOT_WIDTH - 6) / 2
      })
      expect(secondSlotPath).toBeDefined()
    })

    it("omits rows in the gap between two windows: nothing draws past the first window's own reset inside its slot", () => {
      // A gap between the two windows: the second starts a bucket after the
      // first resets, so a naive shared scale would draw a connecting
      // segment through the gap.
      const gapped = usage({
        periods: [
          period({ periodId: 1, startsAtEpoch: 0, resetsAtEpoch: 2 * BUCKET }),
          period({ periodId: 2, startsAtEpoch: 4 * BUCKET, resetsAtEpoch: 6 * BUCKET }),
        ],
      })
      const { container } = render(
        <QuotaBurnupChart
          {...chartProps({
            usage: gapped,
            rangeStartEpoch: 0,
            rangeEndEpoch: 6 * BUCKET,
            nowEpoch: 6 * BUCKET + 1,
          })}
        />,
      )
      // The stack-top line draws one path per slot (two), each confined to
      // its own window: no single path can span both slots' rows.
      expect(container.querySelectorAll("path.quota-line-top")).toHaveLength(2)
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
      // Two slots at PLOT_WIDTH ~530px each comfortably clear the 64px
      // minimum gap, so both windows label.
      const ticks = [...container.querySelectorAll("text")].filter((el) =>
        /^\d+ \w+/.test(el.textContent ?? ""),
      )
      expect(ticks.length).toBeGreaterThanOrEqual(2)
      expect(ticks[0]!.getAttribute("text-anchor")).toBe("start")
    })
  })
})

// No @types/node in this project's tsconfig: reach `process.env` through
// `globalThis` instead of the untyped global.
const nodeEnv = (
  globalThis as unknown as { process: { env: Record<string, string | undefined> } }
).process.env

describe("xAxisTicks", () => {
  // Fix the timezone so the test is deterministic wherever it runs.
  // Melbourne moves its clocks forward in October, so a range crossing
  // that change exercises the fix in any zone that also has one.
  let originalTz: string | undefined

  beforeAll(() => {
    originalTz = nodeEnv.TZ
    nodeEnv.TZ = "Australia/Melbourne"
  })

  afterAll(() => {
    // Assigning `undefined` would store the string "undefined", so an
    // absent TZ is deleted rather than reassigned.
    if (originalTz === undefined) delete nodeEnv.TZ
    else nodeEnv.TZ = originalTz
  })

  it("keeps every tick at local midnight across a daylight-saving change", () => {
    // Confirms the TZ override took effect: Melbourne sits 11 hours ahead
    // of UTC during October daylight saving.
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

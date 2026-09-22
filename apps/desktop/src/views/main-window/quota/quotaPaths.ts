/**
 * Pure SVG path builders for the burnup chart's stacked bands and meter
 * line. No React, no recharts: every function here takes plain rows and
 * scales and returns a `d` string plus the vertex count the caller traces.
 *
 * A band's path draws a trapezoid, not a step: each retained row's own
 * cumulative height sits at its own time, and both the top and bottom
 * edges run as straight lines between rows, so a value ramps across the
 * time between rows instead of jumping in at the later row. A run breaks
 * wherever the band's own value is null or zero, and closes to a point at
 * the next row's own height there, drawing the reset drop; a run that
 * reaches the series' end has no closing point.
 */

import { QUOTA_HUE_COUNT, type QuotaSeriesRow, type QuotaTopSession } from "./quotaSeries"

export interface QuotaBandSpec {
  key: string
  className: string
  fill: string
}

interface PathResult {
  d: string
  vertices: number
}

interface Stack {
  below: number
  top: number
  /** The band's own value at this row is present and greater than zero. */
  active: boolean
}

/** Round to at most one decimal and drop a trailing `.0`, to keep `d`
 *  strings short. Never emits `-0`. */
function num(n: number): string {
  const rounded = Math.round(n * 10) / 10
  return String(rounded === 0 ? 0 : rounded)
}

function point(x: number, y: number): string {
  return `${num(x)} ${num(y)}`
}

/**
 * One edge (top or bottom) of a run, as scaled coordinate strings: one
 * point per retained row, at its own time and its own value, drawing a
 * straight line from each point to the next.
 */
function edgeCoords(
  rows: readonly QuotaSeriesRow[],
  retained: readonly number[],
  x: (t: number) => number,
  y: (v: number) => number,
  valueAt: (index: number) => number,
): string[] {
  return retained.map((k) => point(x(rows[k]!.t), y(valueAt(k))))
}

export function quotaBandPaths(
  rows: readonly QuotaSeriesRow[],
  bandKeys: readonly string[],
  x: (t: number) => number,
  y: (v: number) => number,
): PathResult[] {
  // Reuse the cumulative totals so each band does not sum all lower bands again.
  const totals = rows.map(() => 0)
  return bandKeys.map((key) => {
    const stacks = rows.map((row, index) => {
      const below = totals[index]!
      const raw = row[key]
      const top = below + (raw ?? 0)
      totals[index] = top
      return { below, top, active: raw != null && raw > 0 }
    })
    return bandPathFromStacks(rows, stacks, x, y)
  })
}

function bandPathFromStacks(
  rows: readonly QuotaSeriesRow[],
  stacks: readonly Stack[],
  x: (t: number) => number,
  y: (v: number) => number,
): PathResult {
  const subpaths: string[] = []
  let vertices = 0
  let i = 0
  while (i < rows.length) {
    if (!stacks[i]!.active) {
      i += 1
      continue
    }
    let j = i
    while (j + 1 < rows.length && stacks[j + 1]!.active) j += 1

    // Keep an interior row only where it differs from its previous or its
    // next row: a flat plateau needs just its two endpoints, but a row
    // where a plateau ends and a ramp begins is a real vertex the straight
    // edge must pass through. The run's first and last rows always stay,
    // so the shape's extent never changes.
    const retained: number[] = [i]
    for (let k = i + 1; k < j; k++) {
      const differsFromPrev =
        stacks[k]!.top !== stacks[k - 1]!.top || stacks[k]!.below !== stacks[k - 1]!.below
      const differsFromNext =
        stacks[k]!.top !== stacks[k + 1]!.top || stacks[k]!.below !== stacks[k + 1]!.below
      if (differsFromPrev || differsFromNext) retained.push(k)
    }
    if (j > i) retained.push(j)

    const topCoords = edgeCoords(rows, retained, x, y, (k) => stacks[k]!.top)
    const bottomCoords = edgeCoords(rows, retained, x, y, (k) => stacks[k]!.below)

    // The run ends before the series' end: close the shape to a point at
    // the next row's own height, drawing the reset drop. A run that
    // reaches the series' end has nothing to close to.
    const nextRowIndex = j + 1 < rows.length ? j + 1 : undefined
    if (nextRowIndex != null) {
      topCoords.push(point(x(rows[nextRowIndex]!.t), y(stacks[nextRowIndex]!.top)))
      bottomCoords.push(point(x(rows[nextRowIndex]!.t), y(stacks[nextRowIndex]!.below)))
    }

    const outline = [...topCoords, ...bottomCoords.reverse()]
    subpaths.push(`M${outline[0]}L${outline.slice(1).join("L")}Z`)
    vertices += outline.length
    i = j + 1
  }
  return { d: subpaths.join(""), vertices }
}

/**
 * The stack's own top edge: a linear polyline through the per-row sum of
 * `bandKeys`, broken into a new `M` at each row where every one of
 * `bandKeys` is null (a gap row); a row where at least one band holds a
 * value sums the rest as zero. Under the shared-meter model this sum
 * equals the provider's own meter at a row with a reading, and the
 * device's estimate everywhere else.
 */
export function quotaStackTopPath(
  rows: readonly QuotaSeriesRow[],
  bandKeys: readonly string[],
  x: (t: number) => number,
  y: (v: number) => number,
): PathResult {
  const commands: string[] = []
  let vertices = 0
  let open = false
  for (const row of rows) {
    if (bandKeys.every((key) => row[key] == null)) {
      open = false
      continue
    }
    const sum = bandKeys.reduce((total, key) => total + (row[key] ?? 0), 0)
    const coord = point(x(row.t), y(sum))
    commands.push(open ? `L${coord}` : `M${coord}`)
    vertices += 1
    open = true
  }
  return { d: commands.join(""), vertices }
}

/**
 * The chart's bands in stack order — each top session, then the shared
 * "other", "unattributed", and "unexplained" bands — with the class and
 * fill each one draws with. One source, so `quotaBandPaths`'s `bandKeys` and
 * the chart's rendered `<path>`s always agree on stack order.
 *
 * `unexplainedFill` is the caller's own hatch `<pattern>` reference
 * (`url(#...)`), so two chart instances never share one SVG pattern id; it
 * defaults to the plain token for a caller (such as a test) that has no
 * pattern to point at.
 */
export function quotaBandSpecs(
  topSessions: readonly QuotaTopSession[],
  unexplainedFill: string = "var(--color-quota-unexplained)",
): QuotaBandSpec[] {
  return [
    ...topSessions.map((session, index) => ({
      key: session.key,
      className: `quota-area quota-area-s${index}`,
      fill: `var(--color-quota-session-${(session.hue % QUOTA_HUE_COUNT) + 1})`,
    })),
    {
      key: "other",
      className: "quota-area quota-area-other",
      fill: "var(--color-chart-rest-strong)",
    },
    {
      key: "unattributed",
      className: "quota-area quota-area-unattributed",
      fill: "var(--color-chart-rest-faint)",
    },
    {
      key: "unexplained",
      className: "quota-area quota-area-unexplained",
      fill: unexplainedFill,
    },
  ]
}

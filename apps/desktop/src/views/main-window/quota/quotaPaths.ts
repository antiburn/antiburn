/**
 * Pure SVG path builders for the burnup chart's stacked bands and meter
 * line. No React, no recharts: every function here takes plain rows and
 * scales and returns a `d` string plus the vertex count the caller traces.
 *
 * A band's path must equal what recharts drew with `stackId` + a
 * `type="stepAfter"` `Area` + `connectNulls={false}`: a stepped top edge at
 * the band's own cumulative height, a stepped bottom edge at the stack
 * beneath it, and a break wherever the band's own value is null or zero.
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

/** The stack drawn beneath `bandIndex`, and the band's own top, for one
 *  row. A null value counts as zero for the stack sum. */
function stackAt(row: QuotaSeriesRow, bandKeys: readonly string[], bandIndex: number): Stack {
  let below = 0
  for (let index = 0; index < bandIndex; index++) below += row[bandKeys[index]!] ?? 0
  const raw = row[bandKeys[bandIndex]!]
  return { below, top: below + (raw ?? 0), active: raw != null && raw > 0 }
}

/**
 * One edge (top or bottom) of a run, as scaled coordinate strings: for each
 * row kept after thinning, a point at its own value, then a point at the
 * next kept row's time — or, for the run's last row, the row that ends the
 * run (`nextRowIndex`), or nothing when the run reaches the series' end.
 */
function edgeCoords(
  rows: readonly QuotaSeriesRow[],
  retained: readonly number[],
  nextRowIndex: number | undefined,
  x: (t: number) => number,
  y: (v: number) => number,
  valueAt: (index: number) => number,
): string[] {
  const coords: string[] = []
  for (let p = 0; p < retained.length; p++) {
    const k = retained[p]!
    const v = valueAt(k)
    coords.push(point(x(rows[k]!.t), y(v)))
    const nextT =
      p + 1 < retained.length
        ? rows[retained[p + 1]!]!.t
        : nextRowIndex != null
          ? rows[nextRowIndex]!.t
          : undefined
    if (nextT != null) coords.push(point(x(nextT), y(v)))
  }
  return coords
}

/**
 * One band's fill path: every maximal run of consecutive active rows becomes
 * its own `M ... Z` subpath in the returned `d`, stepped on top at the
 * band's cumulative height and on the bottom at the stack beneath it.
 */
export function quotaBandPath(
  rows: readonly QuotaSeriesRow[],
  bandKeys: readonly string[],
  bandIndex: number,
  x: (t: number) => number,
  y: (v: number) => number,
): PathResult {
  const stacks = rows.map((row) => stackAt(row, bandKeys, bandIndex))
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

    // Thin an interior row whose top and below both repeat the row before
    // it: the horizontal step already covers the plateau. The run's first
    // and last rows always stay, so the shape's extent never changes.
    const retained: number[] = [i]
    for (let k = i + 1; k < j; k++) {
      if (stacks[k]!.top !== stacks[k - 1]!.top || stacks[k]!.below !== stacks[k - 1]!.below) {
        retained.push(k)
      }
    }
    if (j > i) retained.push(j)

    const nextRowIndex = j + 1 < rows.length ? j + 1 : undefined
    const topCoords = edgeCoords(rows, retained, nextRowIndex, x, y, (k) => stacks[k]!.top)
    const bottomCoords = edgeCoords(rows, retained, nextRowIndex, x, y, (k) => stacks[k]!.below)
    const outline = [...topCoords, ...bottomCoords.reverse()]
    subpaths.push(`M${outline[0]}L${outline.slice(1).join("L")}Z`)
    vertices += outline.length
    i = j + 1
  }
  return { d: subpaths.join(""), vertices }
}

/**
 * The meter's own line: a linear polyline through every row with a
 * non-null reading, broken into a new `M` at each null so a gap in the
 * meter's own samples stays a visible gap instead of a guessed line.
 */
export function quotaMeterPath(
  rows: readonly QuotaSeriesRow[],
  x: (t: number) => number,
  y: (v: number) => number,
): PathResult {
  const commands: string[] = []
  let vertices = 0
  let open = false
  for (const row of rows) {
    if (row.meter == null) {
      open = false
      continue
    }
    const coord = point(x(row.t), y(row.meter))
    commands.push(open ? `L${coord}` : `M${coord}`)
    vertices += 1
    open = true
  }
  return { d: commands.join(""), vertices }
}

/**
 * The chart's bands in stack order — each top session, then the shared
 * "other" and "unattributed" bands — with the class and fill each one draws
 * with. One source, so `quotaBandPath`'s `bandKeys` and the chart's
 * rendered `<path>`s always agree on stack order.
 */
export function quotaBandSpecs(topSessions: readonly QuotaTopSession[]): QuotaBandSpec[] {
  return [
    ...topSessions.map((session, index) => ({
      key: session.key,
      className: `quota-area quota-area-s${index}`,
      fill: `var(--color-quota-session-${(session.hue % QUOTA_HUE_COUNT) + 1})`,
    })),
    {
      key: "other",
      className: "quota-area quota-area-other",
      fill: "var(--color-quota-other)",
    },
    {
      key: "unattributed",
      className: "quota-area quota-area-unattributed",
      fill: "var(--color-quota-unattributed)",
    },
  ]
}

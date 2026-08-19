// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Shared 2D geometry for the hypnogram ribbons.
 *
 * Both ribbons — the bucket-resampled one and the per-turn one — draw the same
 * shape: a closed step-function polygon with rounded corners. Each builds its
 * own list of rectangles (plateaus plus lane-change risers) and hands it here,
 * so the tracing lives in one place rather than being duplicated and drifting.
 */

/** A point in viewBox units. */
export type Vec = [number, number]

/** Axis-aligned rectangle in viewBox units (a ribbon plateau or riser). */
export interface Rect {
  x0: number
  x1: number
  y0: number
  y1: number
}

const EPSILON = 1e-4

/** Drop consecutive (and wrap-around) duplicate points to avoid zero-length edges. */
export function dedupe(pts: Vec[]): Vec[] {
  const out: Vec[] = []
  for (const p of pts) {
    const last = out[out.length - 1]
    if (last && Math.abs(last[0] - p[0]) < EPSILON && Math.abs(last[1] - p[1]) < EPSILON) {
      continue
    }
    out.push(p)
  }
  if (out.length > 1) {
    const first = out[0]
    const last = out[out.length - 1]
    if (
      first &&
      last &&
      Math.abs(first[0] - last[0]) < EPSILON &&
      Math.abs(first[1] - last[1]) < EPSILON
    ) {
      out.pop()
    }
  }
  return out
}

/**
 * Trace the union of axis-aligned rectangles as one closed rounded
 * step-function outline: the min-top and max-bottom of the covering rectangles
 * across each x-interval between rectangle edges.
 */
export function traceRibbon(rects: Rect[], viewW: number, cornerR: number): string {
  if (rects.length === 0) return ""

  // x breakpoints → per-interval band extent (min top / max bottom of covers).
  const breakpoints = new Set<number>([0, viewW])
  for (const r of rects) {
    breakpoints.add(Math.max(0, Math.min(viewW, r.x0)))
    breakpoints.add(Math.max(0, Math.min(viewW, r.x1)))
  }
  const bp = [...breakpoints].sort((a, b) => a - b)

  const top: Vec[] = []
  const bottom: Vec[] = []
  for (let i = 0; i < bp.length - 1; i++) {
    const a = bp[i]
    const b = bp[i + 1]
    if (a === undefined || b === undefined) continue
    const mid = (a + b) / 2
    let hi = Infinity
    let lo = -Infinity
    for (const r of rects) {
      if (mid >= r.x0 && mid <= r.x1) {
        hi = Math.min(hi, r.y0)
        lo = Math.max(lo, r.y1)
      }
    }
    if (!Number.isFinite(hi)) continue
    top.push([a, hi], [b, hi])
    bottom.push([a, lo], [b, lo])
  }

  return roundedClosedPath(dedupe([...top, ...bottom.reverse()]), cornerR)
}

/**
 * A closed polygon traced with rounded corners (quadratic), radius clamped per
 * corner so a short edge cannot round past its own midpoint.
 */
export function roundedClosedPath(pts: Vec[], r: number): string {
  const n = pts.length
  if (n < 3) return ""
  const f = (v: number) => v.toFixed(3)
  let d = ""
  for (let i = 0; i < n; i++) {
    const cur = pts[i]
    const prev = pts[(i - 1 + n) % n]
    const next = pts[(i + 1) % n]
    if (!cur || !prev || !next) continue
    const d1 = Math.hypot(prev[0] - cur[0], prev[1] - cur[1]) || 1
    const d2 = Math.hypot(next[0] - cur[0], next[1] - cur[1]) || 1
    const rad = Math.min(r, d1 / 2, d2 / 2)
    const e1: Vec = [
      cur[0] + ((prev[0] - cur[0]) / d1) * rad,
      cur[1] + ((prev[1] - cur[1]) / d1) * rad,
    ]
    const e2: Vec = [
      cur[0] + ((next[0] - cur[0]) / d2) * rad,
      cur[1] + ((next[1] - cur[1]) / d2) * rad,
    ]
    d += `${i === 0 ? "M" : "L"}${f(e1[0])} ${f(e1[1])} Q${f(cur[0])} ${f(cur[1])} ${f(e2[0])} ${f(e2[1])} `
  }
  return `${d}Z`
}

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useId, useRef, useState } from "react"

import {
  bucketDetail,
  formatCompact,
  formatDuration,
  phaseColor,
} from "../../../lib/presentation/sessionAnalytics"
import type { PhaseDistribution, SessionBucket, SessionPhase } from "../../../lib/types/session"
import {
  dedupe,
  roundedClosedPath,
  traceRibbon,
  type Rect,
  type Vec,
} from "./hypnogramGeometry"
import { GLASS_TOOLTIP_STYLE, useTooltipPosition } from "./tooltip"

/**
 * Lanes top → bottom. Disruptions sit at the top, the way "awake" does on a
 * sleep hypnogram; implementing is the deepest lane at the bottom.
 */
const LANES: readonly SessionPhase[] = [
  "disruption",
  "thinking",
  "exploring",
  "testing",
  "implementing",
]

const VIEW_H = 40
/**
 * Fixed viewBox width, roughly matching the card's aspect, so thickness and
 * rounded corners scale evenly under `preserveAspectRatio="none"` instead of
 * distorting with the bucket count.
 */
const VIEW_W = 100
const LANE_GAP = 2
const LANE_H = (VIEW_H - LANE_GAP * (LANES.length - 1)) / LANES.length

/**
 * Plateau half-thickness, mapped from how dominant the leading phase is. The
 * maximum stays under the lane pitch so adjacent bands stay separated.
 */
const HALF_MIN = 1.8
const HALF_MAX = 4.2

/** Half-width of the vertical connector that bridges two lanes. */
const RISER_HALF = 0.2
/** Corner radius for the rounded steps, clamped per corner. */
const CORNER_R = 1.4
/**
 * Disruption "wake-up" ticks: vertical marks in the top lane for any bucket
 * holding a disruption, even when it is not that bucket's dominant phase — the
 * ribbon alone only shows the plurality, so sparse errors would vanish.
 */
const TICK_HALF_MIN = 1.6
/**
 * Plateau half-width of a disruption mark: wider than the riser, so the cap
 * reads as a small flat top exactly like the ribbon's own plateaus.
 */
const TICK_PLATEAU_HALF = 0.7

const LANE_CENTER: Record<SessionPhase, number> = LANES.reduce(
  (acc, phase, i) => {
    acc[phase] = i * (LANE_H + LANE_GAP) + LANE_H / 2
    return acc
  },
  {} as Record<SessionPhase, number>,
)

function clamp01(t: number): number {
  return t < 0 ? 0 : t > 1 ? 1 : t
}

function distTotal(d: PhaseDistribution): number {
  return d.implementing + d.testing + d.exploring + d.thinking + d.disruption
}

function leadShare(d: PhaseDistribution): number {
  const total = distTotal(d)
  if (total <= 0) return 0
  return Math.max(d.implementing, d.testing, d.exploring, d.thinking, d.disruption) / total
}

/**
 * A more dominant leading phase gives a thicker plateau. The leading share
 * spans about 0.25 (an even four-way split) to 1.0 (fully dominant).
 */
function halfThick(lead: number): number {
  return HALF_MIN + (HALF_MAX - HALF_MIN) * clamp01((lead - 0.25) / 0.75)
}

interface Run {
  /** First bucket index, inclusive. */
  startB: number
  /** Last bucket index plus one, exclusive. */
  endB: number
  /** Lane center y. */
  cy: number
  /** Half-thickness, constant across the run. */
  ht: number
}

interface Tick {
  /** viewBox x at the bucket center. */
  x: number
  /** Plateau half-thickness at the disruption lane (severity). */
  ht: number
  /** y where the riser meets the ribbon band below. */
  base: number
}

export interface HypnogramProps {
  buckets: SessionBucket[]
  height?: number
  /** Session length, used to label the hover position with elapsed time. */
  durationSecs: number
  /** Model context window; 0 when unknown. */
  contextWindow: number
}

/**
 * Merge consecutive active buckets of the same dominant phase into runs. Idle
 * buckets are skipped, so the ribbon bridges gaps as one continuous band; the
 * step between two runs is placed at the midpoint of any gap between them.
 */
function buildRuns(buckets: SessionBucket[]): Run[] {
  const runs: Run[] = []
  let i = 0
  while (i < buckets.length) {
    const bucket = buckets[i]
    if (!bucket || bucket.dominantPhase == null || distTotal(bucket.distribution) <= 0) {
      i++
      continue
    }
    const phase = bucket.dominantPhase
    let j = i
    let leadSum = 0
    for (;;) {
      const next = buckets[j]
      if (!next || next.dominantPhase !== phase || distTotal(next.distribution) <= 0) break
      leadSum += leadShare(next.distribution)
      j++
    }
    runs.push({
      startB: i,
      endB: j,
      cy: LANE_CENTER[phase],
      ht: halfThick(leadSum / (j - i)),
    })
    i = j
  }
  return runs
}

/** Top edge of the ribbon band covering a bucket, or null if none does. */
function bandTopAt(runs: Run[], bucket: number): number | null {
  for (const run of runs) {
    if (bucket >= run.startB && bucket < run.endB) return run.cy - run.ht
  }
  return null
}

/**
 * One mark per bucket that carries a disruption without leading it (dominant
 * buckets already get a real ribbon connector). Each is a small plateau at the
 * disruption lane joined by a riser down to the ribbon band — the same shape
 * and fill as the ribbon's own lane traversals — so sparse disruptions surface
 * as wake-up spikes instead of floating free.
 */
function disruptionTicks(buckets: SessionBucket[], runs: Run[], n: number): Tick[] {
  const xScale = VIEW_W / n
  const halfMax = LANE_H / 2 // a fully-disruption bucket fills the lane
  const ticks: Tick[] = []
  for (let i = 0; i < buckets.length; i++) {
    const bucket = buckets[i]
    if (!bucket) continue
    const share = bucket.distribution.disruption
    if (share <= 0 || bucket.dominantPhase === "disruption") continue
    const base = bandTopAt(runs, i)
    if (base == null) continue
    ticks.push({
      x: (i + 0.5) * xScale,
      ht: TICK_HALF_MIN + (halfMax - TICK_HALF_MIN) * clamp01(share),
      base,
    })
  }
  return ticks
}

/**
 * Vertical painted extent of the ribbon at each bucket — the run band, raised
 * to the disruption lane wherever a wake-up spike is drawn. Used to hit-test
 * hover, so the tooltip only appears over the ribbon itself and not the empty
 * space around it.
 */
function bandExtents(
  buckets: SessionBucket[],
  runs: Run[],
): ({ top: number; bottom: number } | null)[] {
  const extents: ({ top: number; bottom: number } | null)[] = buckets.map(() => null)
  for (const run of runs) {
    for (let i = run.startB; i < run.endB; i++) {
      extents[i] = { top: run.cy - run.ht, bottom: run.cy + run.ht }
    }
  }
  const halfMax = LANE_H / 2
  for (let i = 0; i < buckets.length; i++) {
    const bucket = buckets[i]
    const extent = extents[i]
    if (!bucket || !extent) continue
    const share = bucket.distribution.disruption
    if (share <= 0 || bucket.dominantPhase === "disruption") continue
    const ht = TICK_HALF_MIN + (halfMax - TICK_HALF_MIN) * clamp01(share)
    extent.top = Math.min(extent.top, LANE_CENTER.disruption - ht)
  }
  return extents
}

/** Plateau-at-the-disruption-lane plus riser-to-the-ribbon outline for one mark. */
function tickPath(tick: Tick): string {
  const cy = LANE_CENTER.disruption
  const top = cy - tick.ht
  const bot = cy + tick.ht
  const pts: Vec[] = [
    [tick.x - TICK_PLATEAU_HALF, top],
    [tick.x + TICK_PLATEAU_HALF, top],
    [tick.x + TICK_PLATEAU_HALF, bot],
    [tick.x + RISER_HALF, bot],
    [tick.x + RISER_HALF, tick.base],
    [tick.x - RISER_HALF, tick.base],
    [tick.x - RISER_HALF, bot],
    [tick.x - TICK_PLATEAU_HALF, bot],
  ]
  return roundedClosedPath(dedupe(pts), CORNER_R)
}

/**
 * The ribbon outline as the union of plateau rectangles (one per run, variable
 * thickness) and full-height connector rectangles (one per transition, fixed
 * width), traced as a clean step-function outline so lane traversals are solid,
 * visible bridges.
 */
function ribbonPath(runs: Run[], n: number): string {
  if (runs.length === 0) return ""
  const xScale = VIEW_W / n
  const xOf = (b: number) => b * xScale
  const m = runs.length

  // Riser x between run k and k+1: the midpoint of any gap between them, or
  // their shared boundary when there is none.
  const rx: number[] = []
  for (let k = 0; k < m - 1; k++) {
    const a = runs[k]
    const b = runs[k + 1]
    if (!a || !b) continue
    rx.push(xOf((a.endB + b.startB) / 2))
  }
  const xL = (k: number) => (k === 0 ? 0 : (rx[k - 1] ?? 0))
  const xR = (k: number) => (k === m - 1 ? VIEW_W : (rx[k] ?? VIEW_W))

  const rects: Rect[] = []
  for (let k = 0; k < m; k++) {
    const run = runs[k]
    if (!run) continue
    rects.push({ x0: xL(k), x1: xR(k), y0: run.cy - run.ht, y1: run.cy + run.ht })
  }
  for (let k = 0; k < m - 1; k++) {
    const a = runs[k]
    const b = runs[k + 1]
    const x = rx[k]
    if (!a || !b || x === undefined) continue
    rects.push({
      x0: x - RISER_HALF,
      x1: x + RISER_HALF,
      y0: Math.min(a.cy - a.ht, b.cy - b.ht),
      y1: Math.max(a.cy + a.ht, b.cy + b.ht),
    })
  }

  return traceRibbon(rects, VIEW_W, CORNER_R)
}

/**
 * The session hypnogram: the dominant mode at each moment drawn as a single
 * continuous ribbon that holds flat at each stage lane and steps between them
 * with rounded corners — the direct analog of a sleep-stage graph. Color is
 * keyed to height, deep mode at the bottom through disruption at the top.
 *
 * This is the bucket-resampled form, which is the one that survives averaging
 * across sessions; a single session uses the lossless per-turn ribbon instead.
 */
export function Hypnogram({
  buckets,
  height = 120,
  durationSecs,
  contextWindow,
}: HypnogramProps) {
  // Compress the timeline: drop no-activity buckets and pack the active ones
  // together, so the ribbon is a continuous run of real work with no dead
  // space. Each active bucket keeps its original index and the original count,
  // so the hover tooltip still reports its true elapsed time and progress
  // through the uncompressed session.
  const origN = Math.max(buckets.length, 1)
  const active = buckets
    .map((bucket, origIndex) => ({ bucket, origIndex }))
    .filter(({ bucket }) => bucket.dominantPhase != null && distTotal(bucket.distribution) > 0)
  const activeBuckets = active.map((a) => a.bucket)
  const n = Math.max(activeBuckets.length, 1)
  const id = useId().replace(/:/g, "")
  const runs = buildRuns(activeBuckets)
  const ticks = disruptionTicks(activeBuckets, runs, n)
  const extents = bandExtents(activeBuckets, runs)

  const wrapRef = useRef<HTMLDivElement>(null)
  const tipRef = useRef<HTMLDivElement>(null)
  const [hover, setHover] = useState<{
    index: number
    clientX: number
    clientY: number
  } | null>(null)

  function onMove(event: React.MouseEvent<HTMLDivElement>) {
    const el = wrapRef.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    if (rect.width <= 0 || rect.height <= 0) return
    const xPx = event.clientX - rect.left
    const yPx = event.clientY - rect.top
    const index = Math.min(n - 1, Math.max(0, Math.floor((xPx / rect.width) * n)))
    // Only engage hover when the pointer is over the ribbon itself (with a
    // small tolerance), so empty space around the band shows no tooltip.
    const extent = extents[index]
    const yVB = (yPx / rect.height) * VIEW_H
    const PAD = 1 // viewBox units of slack, to make thin bands easy to hit
    if (!extent || yVB < extent.top - PAD || yVB > extent.bottom + PAD) {
      setHover(null)
      return
    }
    setHover({ index, clientX: event.clientX, clientY: event.clientY })
  }

  const tipPos = useTooltipPosition(hover, wrapRef, tipRef)

  const hovered = hover ? active[hover.index] : null
  const detail = hovered
    ? bucketDetail(hovered.bucket, hovered.origIndex, origN, durationSecs, contextWindow)
    : null
  const cursorX = hover ? ((hover.index + 0.5) * VIEW_W) / n : 0

  return (
    <div
      ref={wrapRef}
      className="relative"
      onMouseMove={onMove}
      onMouseLeave={() => setHover(null)}
    >
      <svg
        width="100%"
        height={height}
        viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
        preserveAspectRatio="none"
        role="img"
        aria-label="Session mode over time"
        style={{
          display: "block",
          border: "1px solid color-mix(in srgb, var(--color-separator) 50%, transparent)",
        }}
      >
        <defs>
          {/* Vertical and lane-keyed: each lane holds one flat color across its
              full band height; colors blend only in the gaps between lanes,
              where the risers pass through, so a stage band is never two-tone. */}
          <linearGradient
            id={`${id}-lane`}
            gradientUnits="userSpaceOnUse"
            x1={0}
            y1={0}
            x2={0}
            y2={VIEW_H}
          >
            {LANES.flatMap((phase, i) => {
              const top = (i * (LANE_H + LANE_GAP)) / VIEW_H
              const bottom = (i * (LANE_H + LANE_GAP) + LANE_H) / VIEW_H
              const color = phaseColor(phase)
              return [
                <stop key={`${phase}-top`} offset={top} stopColor={color} />,
                <stop key={`${phase}-bottom`} offset={bottom} stopColor={color} />,
              ]
            })}
          </linearGradient>
        </defs>

        {/* Muted dashed separators in the gaps between lanes, drawn before the
            ribbon so it always paints on top. */}
        {LANES.slice(0, -1).map((phase, i) => {
          const y = i * (LANE_H + LANE_GAP) + LANE_H + LANE_GAP / 2
          return (
            <line
              key={`lane-divider-${phase}`}
              x1={0}
              x2={VIEW_W}
              y1={y}
              y2={y}
              stroke="var(--color-separator)"
              strokeWidth={1}
              strokeDasharray="3 3"
              vectorEffect="non-scaling-stroke"
            />
          )
        })}

        <path d={ribbonPath(runs, n)} fill={`url(#${id}-lane)`} />

        {ticks.map((tick, i) => (
          <path key={`disruption-tick-${i}`} d={tickPath(tick)} fill={`url(#${id}-lane)`} />
        ))}

        {hover && (
          <line
            x1={cursorX}
            x2={cursorX}
            y1={0}
            y2={VIEW_H}
            stroke="var(--color-label-secondary)"
            strokeWidth={1}
            vectorEffect="non-scaling-stroke"
            opacity={0.6}
          />
        )}
      </svg>

      {hover && detail && (
        <div
          ref={tipRef}
          className="pointer-events-none absolute z-10 text-label"
          style={{
            ...GLASS_TOOLTIP_STYLE,
            left: tipPos?.left ?? 0,
            top: tipPos?.top ?? 0,
            visibility: tipPos ? "visible" : "hidden",
            whiteSpace: "nowrap",
            lineHeight: 1.4,
            padding: "6px 9px",
          }}
        >
          <div className="mb-1.5 text-label-secondary" style={{ fontWeight: 500 }}>
            {formatDuration(detail.elapsedSecs)} · {detail.progressPct}% through
          </div>
          {detail.empty ? (
            <div className="text-label-tertiary">No activity</div>
          ) : (
            <div className="flex flex-col gap-1">
              {detail.modes.map((mode) => (
                <div key={mode.label} className="flex items-center gap-2">
                  <span
                    className="h-2 w-2 shrink-0 rounded-full"
                    style={{ backgroundColor: mode.colorVar }}
                  />
                  <span>{mode.label}</span>
                  <span className="ml-auto pl-4 text-label-secondary tabular-nums">
                    {mode.pct}%
                  </span>
                </div>
              ))}
              <div className="mt-1 text-label-tertiary">
                {formatCompact(detail.tokensIn)} in · {formatCompact(detail.tokensOut)} out ·
                context {detail.contextPct == null ? "unavailable" : `${detail.contextPct}%`}
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  )
}

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Width-aware clustering for timeline markers overlaid on the per-turn
 * hypnogram ribbon.
 *
 * Pure, with no React: given markers placed on the same active-time axis the
 * ribbon uses, it collapses near-coincident and would-overlap dots into
 * clusters, so a quick fan-out reads as a few legible dots instead of a blob.
 *
 * Nothing here knows what a marker *represents* — only its 0..1 position. The
 * opaque `data` payload rides through untouched for the renderer to interpret,
 * which is the seam that lets one overlay serve several marker species.
 */

/**
 * viewBox width in user units. Because the ribbon renders with
 * `preserveAspectRatio="none"`, an x of 0..VIEW_W is also a percentage of the
 * chart's rendered width.
 */
export const VIEW_W = 100;

/**
 * Phase 1 of clustering, width-independent: markers within this share of the
 * chart width are the *same instant* and merge. A viewBox-percent gap; small,
 * just enough to collapse near-coincident timestamps.
 */
const MERGE_GAP_PCT = 2.5;
/**
 * Phase 2, size-aware: breathing room left between two adjacent dots' edges,
 * in px. Dots closer than `rA + rB + DOT_GAP_PX` merge so the size-scaled dots
 * never visually collide.
 */
const DOT_GAP_PX = 2;
/** Cluster-dot diameter in px: base for a lone dot, growing per extra member. */
const DOT_BASE = 22;
const DOT_STEP = 0.5;
const DOT_MAX = 24;

/**
 * The minimal shape clustering needs from a laid-out ribbon segment: its
 * painted `[x0, x1]` span and the active time it covers. The hypnogram's own
 * geometry is structurally a superset, so it satisfies this directly.
 */
export interface SegmentSpan {
  x0: number;
  x1: number;
  /** Cumulative active ms at this segment's start. */
  cumStartMs: number;
  activeMs: number;
}

/**
 * A generic timeline marker: a 0..1 position on the active-time axis plus an
 * opaque payload the chart and its marker kind interpret.
 */
export interface TimelineMarker<T> {
  progress: number;
  data: T;
}

/** A dot after clustering: a chart position and the markers merged into it. */
export interface Cluster<T> {
  /** x in 0..100 viewBox units (equivalently, a percentage of chart width). */
  x: number;
  /** Members in left-to-right order. */
  members: TimelineMarker<T>[];
}

function clamp01(value: number): number {
  return Math.min(Math.max(value, 0), 1);
}

/**
 * Map a 0..1 active-time `progress` to an x in viewBox units using the
 * laid-out geometry, so a dot lands in the *same* segment the ribbon drew
 * despite minimum-width floors and coalescing: rescale progress to the
 * geometry's own total active ms, find the containing segment, and interpolate
 * within its painted span.
 */
export function markerX(progress: number, geom: SegmentSpan[]): number {
  const last = geom[geom.length - 1];
  if (!last) return 0;
  const totalMs = last.cumStartMs + last.activeMs;
  if (totalMs <= 0) return clamp01(progress) * VIEW_W;
  const targetMs = clamp01(progress) * totalMs;
  const seg =
    geom.find((g) => targetMs >= g.cumStartMs && targetMs <= g.cumStartMs + g.activeMs) ?? last;
  const within = seg.activeMs > 0 ? (targetMs - seg.cumStartMs) / seg.activeMs : 0;
  return seg.x0 + within * (seg.x1 - seg.x0);
}

/**
 * Group markers that would overprint into clusters, in two phases:
 *
 * 1. **Width-independent** — place each marker at its {@link markerX}, sort,
 *    and single-linkage merge dots within `MERGE_GAP_PCT`, collapsing
 *    near-coincident timestamps regardless of how wide the chart renders.
 * 2. **Size-aware**, once `widthPx` is measured — dots grow with member count,
 *    so two *separate* clusters can still visually collide. Convert to px and
 *    merge any adjacent pair whose circles would overlap, re-checking the left
 *    neighbour after each merge since the merged dot is wider.
 *
 * A cluster's `x` is the member-count-weighted mean of its members' positions.
 */
export function clusterMarkers<T>(
  markers: TimelineMarker<T>[],
  geom: SegmentSpan[],
  widthPx: number,
): Cluster<T>[] {
  const points = markers
    .map((marker) => ({ x: markerX(marker.progress, geom), marker }))
    .sort((a, b) => a.x - b.x);

  const accumulated: { sumX: number; lastX: number; members: TimelineMarker<T>[] }[] = [];
  for (const point of points) {
    const last = accumulated[accumulated.length - 1];
    if (last && point.x - last.lastX <= MERGE_GAP_PCT) {
      last.members.push(point.marker);
      last.sumX += point.x;
      last.lastX = point.x;
    } else {
      accumulated.push({ sumX: point.x, lastX: point.x, members: [point.marker] });
    }
  }
  const clusters: Cluster<T>[] = accumulated.map((c) => ({
    x: c.sumX / c.members.length,
    members: c.members,
  }));

  if (widthPx > 0) {
    const toPx = (xPct: number) => (xPct / 100) * widthPx;
    for (let i = 0; i < clusters.length - 1;) {
      const a = clusters[i];
      const b = clusters[i + 1];
      if (!a || !b) break;
      const need = dotSize(a.members.length) / 2 + dotSize(b.members.length) / 2 + DOT_GAP_PX;
      if (toPx(b.x - a.x) < need) {
        const members = [...a.members, ...b.members];
        const x = (a.x * a.members.length + b.x * b.members.length) / members.length;
        clusters.splice(i, 2, { x, members });
        // The wider merged dot may now overlap its left neighbour.
        if (i > 0) i--;
      } else {
        i++;
      }
    }
  }
  return clusters;
}

/** Cluster-dot diameter in px: grows with member count, capped for legibility. */
export function dotSize(n: number): number {
  return Math.min(DOT_BASE + (n - 1) * DOT_STEP, DOT_MAX);
}

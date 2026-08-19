// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useId, useRef, useState } from 'react';

import {
  formatCompact,
  formatDuration,
  phaseColor,
  phaseLabel,
} from '../../../lib/presentation/sessionAnalytics';
import { useDarkTheme } from '../../../lib/theme';
import { useElementWidth } from '../../../lib/useElementWidth';
import type { PhaseSegment, SessionPhase } from '../../../lib/types/session';
import { traceRibbon, type Rect } from './hypnogramGeometry';
import { HypnogramMarkers, type MarkerKind, type ResolvedDot } from './HypnogramMarkers';
import { coalesce } from './HypnogramSegments.logic';
import { clusterMarkers, dotSize, VIEW_W, type TimelineMarker } from './markerCluster';
import { GLASS_TOOLTIP_STYLE, useTooltipPosition } from './tooltip';

/** Lanes top → bottom, matching the bucket hypnogram. */
const LANES: readonly SessionPhase[] = [
  'disruption',
  'thinking',
  'exploring',
  'testing',
  'implementing',
];

const VIEW_H = 40;
const LANE_GAP = 2;
const LANE_H = (VIEW_H - LANE_GAP * (LANES.length - 1)) / LANES.length;

/**
 * Each segment is exactly one mode at full intensity, so thickness is constant
 * — there is no dominance to encode. Kept under the lane pitch so lanes stay
 * visually separated.
 */
const HALF = 3.4;
/** Corner radius for the rounded steps, in viewBox units. */
const CORNER_R = 1.4;
/** Minimum painted width per segment so a brief turn still shows. */
const MIN_W = 0.6;
/** Half-width of the vertical connector bridging two lanes. */
const RISER_HALF = 0.2;
/**
 * Markers float just *below* the ribbon band of the phase that was active when
 * each landed, anchored in lane and time. A fine connector runs from the band's
 * center down to the dot; this is the gap between band edge and dot top, in px.
 */
const TETHER_PX = 5;
/**
 * Vertical de-collision for stacked marker layers: each layer beyond the first
 * is biased down by this many px, so two species at the same instant land on
 * different rows rather than overprinting. Each layer still clusters only
 * within itself.
 */
const LAYER_PITCH_PX = 26;

const LANE_CENTER: Record<SessionPhase, number> = LANES.reduce(
  (acc, phase, i) => {
    acc[phase] = i * (LANE_H + LANE_GAP) + LANE_H / 2;
    return acc;
  },
  {} as Record<SessionPhase, number>,
);

interface Geom {
  x0: number;
  x1: number;
  cy: number;
  phase: SessionPhase;
  /** Cumulative active ms at this segment's start (for the hover elapsed label). */
  cumStartMs: number;
  activeMs: number;
  tokensIn: number;
  tokensOut: number;
  contextTokens: number;
}

/** Phase of the ribbon segment covering chart-x `x`, if any. */
function phaseAtX(x: number, geom: Geom[]): SessionPhase | null {
  return geom.find((s) => x >= s.x0 && x <= s.x1)?.phase ?? null;
}

/**
 * Lay segments out across the active-time axis. Each gets a base `MIN_W` so
 * brief runs stay visible, plus a share of the remaining width proportional to
 * active time. Falls back to pure proportional widths when the floor would
 * overflow the chart.
 */
function layout(segments: PhaseSegment[]): Geom[] {
  const total = segments.reduce((sum, seg) => sum + Math.max(seg.activeMs, 0), 0) || 1;
  const n = segments.length;
  const base = n * MIN_W < VIEW_W ? MIN_W : 0;
  const remainder = VIEW_W - base * n;
  const geom: Geom[] = [];
  let x = 0;
  let cum = 0;
  for (const seg of segments) {
    const ms = Math.max(seg.activeMs, 0);
    const w = base + (ms / total) * remainder;
    geom.push({
      x0: x,
      x1: x + w,
      cy: LANE_CENTER[seg.phase],
      phase: seg.phase,
      cumStartMs: cum,
      activeMs: ms,
      tokensIn: seg.tokensIn,
      tokensOut: seg.tokensOut,
      contextTokens: seg.contextTokens,
    });
    x += w;
    cum += ms;
  }
  const last = geom[geom.length - 1];
  if (last) last.x1 = VIEW_W; // absorb float drift
  return geom;
}

/**
 * The ribbon as one rounded step-function outline: the union of plateau
 * rectangles (one per segment) and full-height connector rectangles (one per
 * lane change), traced as a single polygon so risers blend seamlessly into the
 * band.
 */
function ribbonPath(geom: Geom[]): string {
  if (geom.length === 0) return '';
  const rects: Rect[] = geom.map((g) => ({
    x0: g.x0,
    x1: g.x1,
    y0: g.cy - HALF,
    y1: g.cy + HALF,
  }));
  for (let i = 0; i < geom.length - 1; i++) {
    const current = geom[i];
    const next = geom[i + 1];
    if (!current || !next || current.cy === next.cy) continue;
    const xb = current.x1;
    rects.push({
      x0: xb - RISER_HALF,
      x1: xb + RISER_HALF,
      y0: Math.min(current.cy, next.cy) - HALF,
      y1: Math.max(current.cy, next.cy) + HALF,
    });
  }
  return traceRibbon(rects, VIEW_W, CORNER_R);
}

/**
 * One overlay layer: a set of timeline markers paired with the
 * {@link MarkerKind} that supplies their appearance and hover card. The ribbon
 * takes a list of these so several species can coexist, each on its own row.
 */
export interface MarkerLayer<T = unknown> {
  markers: TimelineMarker<T>[];
  kind: MarkerKind<T>;
}

export interface HypnogramSegmentsProps {
  segments: PhaseSegment[];
  height?: number;
  /** Total active time, used to label the hover position. */
  activeSecs: number;
  /** Model context window; 0 when unknown, which reads as "unavailable". */
  contextWindow: number;
  /**
   * Marker layers overlaid on the ribbon. Each is clustered independently and
   * biased onto its own row, so species never merge or overprint. Absent or
   * empty renders the ribbon exactly as it would be without.
   */
  layers?: MarkerLayer[];
}

/**
 * The per-turn session hypnogram: each classified turn is one mode (mutually
 * exclusive), drawn as a plateau whose width tracks active time, with lane
 * changes bridged by risers — all traced as one continuous rounded-step ribbon
 * so the connectors blend into the band. Sub-threshold turns are coalesced
 * first, so dense sessions read as sustained runs rather than as static.
 */
export function HypnogramSegments({
  segments,
  height = 120,
  activeSecs,
  contextWindow,
  layers,
}: HypnogramSegmentsProps) {
  const id = useId().replace(/:/g, '');
  const dark = useDarkTheme();
  const geom = layout(coalesce(segments));

  const wrapRef = useRef<HTMLDivElement>(null);
  const tipRef = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<{
    index: number;
    clientX: number;
    clientY: number;
  } | null>(null);
  // Which marker layer currently owns the open hover card. Lifted here so
  // sibling layers coordinate: only one card shows at a time.
  const [activeMarkerLayer, setActiveMarkerLayer] = useState<number | null>(null);
  // The chart's rendered pixel width, tracked so dots can be clustered
  // size-aware (a dot's diameter is in px, but its x is a viewBox percent).
  // 0 until measured — clustering falls back to its width-independent pass.
  const widthPx = useElementWidth(wrapRef);

  // The SVG renders at a fixed pixel height with `preserveAspectRatio="none"`,
  // so its vertical scale is constant regardless of width.
  const scaleY = height / VIEW_H;

  // Resolve one layer's markers to positioned dots. Markers are clustered so a
  // quick fan-out does not overprint, then each dot floats just below the
  // *deepest* ribbon lane within its own horizontal footprint — not merely its
  // own lane's band — so a dot near a downward step never lands on the ribbon.
  // The tether still rises to the marker's own lane, so which mode it landed in
  // stays readable.
  const resolveDots = (
    markers: TimelineMarker<unknown>[],
    layerIndex: number,
  ): ResolvedDot<unknown>[] => {
    const clusters = geom.length > 0 ? clusterMarkers(markers, geom, widthPx) : [];
    return clusters.map((cluster) => {
      const phase = phaseAtX(cluster.x, geom) ?? geom[0]?.phase ?? 'thinking';
      const size = dotSize(cluster.members.length);
      // The dot's horizontal half-extent (plus a small pad) in viewBox units,
      // so we can test which ribbon segments it would overlap.
      const halfVB = widthPx > 0 ? ((size / 2 + 3) / widthPx) * VIEW_W : 2;
      let deepestCy = LANE_CENTER[phase];
      for (const g of geom) {
        if (g.x1 >= cluster.x - halfVB && g.x0 <= cluster.x + halfVB) {
          deepestCy = Math.max(deepestCy, g.cy);
        }
      }
      return {
        xPct: cluster.x,
        size,
        bandCenter: LANE_CENTER[phase] * scaleY,
        centerY:
          (deepestCy + HALF) * scaleY + TETHER_PX + size / 2 + layerIndex * LAYER_PITCH_PX,
        members: cluster.members,
      };
    });
  };

  const resolvedLayers = (layers ?? []).map((layer, i) => ({
    kind: layer.kind,
    dots: resolveDots(layer.markers, i),
  }));
  // Dots in the deepest lane (and lower layers) spill past the chart's bottom,
  // so reserve flow space below for the lowest one across every layer.
  const reserveBottom = Math.max(
    0,
    ...resolvedLayers.flatMap((l) => l.dots).map((d) => d.centerY + d.size / 2 - height),
  );

  // Bound to the chart-area div, so it reads that element's rect and fires
  // while the cursor is over the ribbon. Marker dots overlay the chart as
  // opaque buttons, so the pointer lands on them and no phantom segment
  // tooltip shows beneath a dot.
  function onMove(event: React.MouseEvent<HTMLDivElement>) {
    const rect = event.currentTarget.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    const xVB = ((event.clientX - rect.left) / rect.width) * VIEW_W;
    const index = geom.findIndex((g) => xVB >= g.x0 && xVB <= g.x1);
    if (index < 0) {
      setHover(null);
      return;
    }
    setHover({ index, clientX: event.clientX, clientY: event.clientY });
  }

  const tipPos = useTooltipPosition(hover, wrapRef, tipRef);

  const hovered = hover ? geom[hover.index] : null;
  const lastGeom = geom[geom.length - 1];
  const totalMs = lastGeom ? lastGeom.cumStartMs + lastGeom.activeMs : 0;
  const fraction =
    hovered && totalMs > 0 ? (hovered.cumStartMs + hovered.activeMs / 2) / totalMs : 0;
  const cursorX = hovered ? (hovered.x0 + hovered.x1) / 2 : 0;

  return (
    <div
      ref={wrapRef}
      className="relative"
      style={{ paddingBottom: reserveBottom || undefined }}
    >
      {/* The chart area owns the ribbon hover. */}
      <div onMouseMove={onMove} onMouseLeave={() => setHover(null)}>
        <svg
          width="100%"
          height={height}
          viewBox={`0 0 ${VIEW_W} ${VIEW_H}`}
          preserveAspectRatio="none"
          role="img"
          aria-label="Session mode over time"
          style={{
            display: 'block',
            border: '1px solid color-mix(in srgb, var(--color-separator) 50%, transparent)',
          }}
        >
          <defs>
            {/* Vertical and lane-keyed: each lane holds one flat color across
                its full band height; colors blend only in the gaps between
                lanes, so a stage band is never two-tone. */}
            <linearGradient
              id={`${id}-lane`}
              gradientUnits="userSpaceOnUse"
              x1={0}
              y1={0}
              x2={0}
              y2={VIEW_H}
            >
              {LANES.flatMap((phase, i) => {
                const top = (i * (LANE_H + LANE_GAP)) / VIEW_H;
                const bottom = (i * (LANE_H + LANE_GAP) + LANE_H) / VIEW_H;
                const color = phaseColor(phase);
                return [
                  <stop key={`${phase}-top`} offset={top} stopColor={color} />,
                  <stop key={`${phase}-bottom`} offset={bottom} stopColor={color} />,
                ];
              })}
            </linearGradient>
          </defs>

          {LANES.slice(0, -1).map((phase, i) => {
            const y = i * (LANE_H + LANE_GAP) + LANE_H + LANE_GAP / 2;
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
            );
          })}

          <path d={ribbonPath(geom)} fill={`url(#${id}-lane)`} />

          {hovered && (
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
      </div>

      {/* Marker overlays: one per layer, sharing this wrapper so the dots and
          cards position against the same ancestor as the ribbon tooltip. */}
      {resolvedLayers.map((layer, i) => (
        <HypnogramMarkers
          key={`layer-${i}`}
          layerId={i}
          activeLayer={activeMarkerLayer}
          onActivate={setActiveMarkerLayer}
          dots={layer.dots}
          kind={layer.kind}
          dark={dark}
          wrapRef={wrapRef}
        />
      ))}

      {hovered && (
        <div
          ref={tipRef}
          className="pointer-events-none absolute z-30 text-label"
          style={{
            ...GLASS_TOOLTIP_STYLE,
            left: tipPos?.left ?? 0,
            top: tipPos?.top ?? 0,
            visibility: tipPos ? 'visible' : 'hidden',
            whiteSpace: 'nowrap',
            lineHeight: 1.4,
            padding: '6px 9px',
          }}
        >
          <div className="mb-1.5 text-label-secondary" style={{ fontWeight: 500 }}>
            {formatDuration(Math.round(fraction * activeSecs))} · {Math.round(fraction * 100)}%
            through
          </div>
          <div className="flex items-center gap-2">
            <span
              className="h-2 w-2 shrink-0 rounded-full"
              style={{ backgroundColor: phaseColor(hovered.phase) }}
            />
            <span>{phaseLabel(hovered.phase)}</span>
            <span className="ml-auto pl-4 text-label-secondary tabular-nums">
              {formatDuration(Math.round(hovered.activeMs / 1000))}
            </span>
          </div>
          <div className="mt-1 text-label-tertiary">
            {formatCompact(hovered.tokensIn)} in · {formatCompact(hovered.tokensOut)} out ·
            context{' '}
            {contextWindow > 0
              ? `${Math.min(100, Math.round((hovered.contextTokens / contextWindow) * 100))}%`
              : 'unavailable'}
          </div>
        </div>
      )}
    </div>
  );
}

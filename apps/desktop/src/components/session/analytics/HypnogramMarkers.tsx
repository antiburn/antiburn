// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useRef, useState, type ReactNode, type RefObject } from "react"

import { useHoverIntent } from "../../../lib/useHoverIntent"
import type { TimelineMarker } from "./markerCluster"
import { GLASS_TOOLTIP_STYLE, useTooltipPosition } from "./tooltip"

/**
 * How long the card stays open after the cursor leaves a dot — the bridge that
 * lets the cursor cross the gap from the dot into the interactive card.
 */
const CLOSE_DELAY_MS = 180

/**
 * A cluster resolved to pixel geometry by the hypnogram's placement pass. The
 * overlay renders from this alone: it knows nothing about lanes, phases, or
 * what a marker represents.
 */
export interface ResolvedDot<T> {
  /** Horizontal center, in viewBox percent of the chart width. */
  xPct: number
  size: number
  /** Top of the tether: the marker's own lane band center, in px. */
  bandCenter: number
  /** The dot's vertical center, in px from the chart's top edge. */
  centerY: number
  members: TimelineMarker<T>[]
}

/**
 * The visual and content contract for one marker *species*.
 *
 * The overlay owns geometry and interaction; the kind owns everything
 * domain-specific — dot colors, the glyph inside, the accessible label, the
 * lone-dot click, and the hover card's body. Defining a new kind is all it
 * takes to render a different species on the same ribbon.
 */
export interface MarkerKind<T> {
  /** Dot fill (a CSS color), varying by theme. */
  dotFill: (dark: boolean) => string
  /** Color of the fine connector from the ribbon band down to the dot. */
  tetherColor: string
  /** Color of the count or glyph drawn inside the dot. */
  textColor: string
  /** Optional text stroke for the in-dot glyph, for legibility on a tint. */
  textStroke: (dark: boolean) => string | undefined
  /** What is drawn inside the dot — a merged count, or a glyph for a lone dot. */
  dotContent: (members: TimelineMarker<T>[]) => ReactNode
  /** Accessible label for the dot button. */
  ariaLabel: (members: TimelineMarker<T>[]) => string
  /** Click action for a lone dot (a single-member cluster). */
  onLoneClick?: (member: TimelineMarker<T>) => void
  /**
   * The hover card's body. The kind owns iteration, keys, and header policy.
   * `close()` cancels the close timer and dismisses the card, so a row can
   * dismiss-then-act on click.
   */
  renderTooltip: (members: TimelineMarker<T>[], close: () => void) => ReactNode
  /** Hover-card min width in px; defaults to 150. */
  minWidth?: number
  /** Hover-card max width in px; defaults to 230. Denser cards widen it. */
  maxWidth?: number
}

export interface HypnogramMarkersProps<T> {
  dots: ResolvedDot<T>[]
  kind: MarkerKind<T>
  dark: boolean
  /**
   * The hypnogram's own positioned wrapper. Shared, not re-created, so the
   * dots' absolute coordinates and the tooltip's wrapper-local conversion
   * resolve against the same ancestor as the ribbon's own tooltip.
   */
  wrapRef: RefObject<HTMLDivElement | null>
  /** This overlay's index among sibling layers. */
  layerId: number
  /** Which layer currently owns the open card (`null` = none). */
  activeLayer: number | null
  /** Claim the open card for this layer; siblings dismiss theirs immediately. */
  onActivate: (layerId: number) => void
}

/**
 * The generic marker overlay: clustered dots floating just below the ribbon
 * band that was active when each marker landed, tethered to it by a fine
 * connector, with an interactive hover card.
 *
 * Purely presentational — all appearance and card content come from the
 * supplied {@link MarkerKind}, so one overlay serves any species. Built from
 * HTML rather than SVG so the dots stay circular under the ribbon's
 * `preserveAspectRatio="none"`.
 */
export function HypnogramMarkers<T>({
  dots,
  kind,
  dark,
  wrapRef,
  layerId,
  activeLayer,
  onActivate,
}: HypnogramMarkersProps<T>) {
  // The card mirrors the ribbon's glass tooltip but is *interactive* (its rows
  // open a target), so a short close timer keeps it up while the cursor
  // crosses the gap from the dot into the card.
  const tipRef = useRef<HTMLDivElement>(null)
  const [hover, setHover] = useState<{
    index: number
    clientX: number
    clientY: number
  } | null>(null)
  // Owns the close-delay timer, including clearing it on unmount, so this
  // component needs no timer refs or cleanup effect of its own.
  const closeIntent = useHoverIntent()

  const open = (index: number, event: React.MouseEvent) => {
    closeIntent.cancel()
    onActivate(layerId)
    setHover({ index, clientX: event.clientX, clientY: event.clientY })
  }
  const scheduleClose = () => {
    closeIntent.schedule(() => setHover(null), CLOSE_DELAY_MS)
  }
  // Dismiss the card. Deliberately ref-free so it is safe to hand to
  // `renderTooltip`, which runs during render; a pending close timer would
  // only re-set null, and unmount cleanup clears it regardless.
  const close = () => setHover(null)

  // Only one marker card across all layers. Derived, not synchronized: the
  // parent says which layer owns the card, and a layer that does not own it
  // simply does not paint one. There is no close-delay bridge here — that gap
  // exists for travelling into *our* card, not for letting two coexist — and
  // no effect, so a sibling claiming the card cannot cost a second render.
  const owned = activeLayer === layerId ? hover : null

  const tipPos = useTooltipPosition(owned, wrapRef, tipRef)
  const hovered = owned ? dots[owned.index] : null

  if (dots.length === 0) return null

  return (
    <>
      {/* Dots scale up on hover and ease back on leave, motion-safe. `scale` is
          a standalone transform property, so it composes with the dot's
          translate-based centering instead of overriding it. */}
      <style>{`
        @media (prefers-reduced-motion: no-preference) {
          .ui-marker-dot { transition: scale var(--duration-fast) ease-out; }
          .ui-marker-dot:hover { scale: 1.15; }
        }
      `}</style>

      {dots.map(({ xPct, size, bandCenter, centerY, members }, i) => {
        const lone = members.length === 1 ? members[0] : undefined
        return (
          <div key={`cluster-${i}`}>
            {/* Fine tether from the ribbon's vertical center down to the dot. */}
            <div
              className="pointer-events-none absolute -translate-x-1/2"
              style={{
                left: `${xPct}%`,
                top: bandCenter,
                height: centerY - size / 2 - bandCenter,
                width: 1.5,
                backgroundColor: kind.tetherColor,
                opacity: 0.55,
              }}
            />
            <button
              type="button"
              aria-label={kind.ariaLabel(members)}
              onClick={lone ? () => kind.onLoneClick?.(lone) : undefined}
              onMouseEnter={(event) => open(i, event)}
              onMouseLeave={scheduleClose}
              className="ui-marker-dot absolute z-20 flex -translate-x-1/2 -translate-y-1/2 cursor-pointer items-center justify-center rounded-full p-0 tabular-nums"
              style={{
                left: `${xPct}%`,
                top: centerY,
                width: size,
                height: size,
                fontSize: Math.max(8, Math.round(size * 0.5)),
                fontWeight: dark ? 600 : 400,
                WebkitTextStroke: kind.textStroke(dark),
                lineHeight: 1,
                color: kind.textColor,
                backgroundColor: kind.dotFill(dark),
              }}
            >
              {kind.dotContent(members)}
            </button>
          </div>
        )
      })}

      {/* Hover card: the same glass surface and window-clamped positioning as
          the ribbon tooltip, but interactive. */}
      {hovered && (
        <div
          ref={tipRef}
          onMouseEnter={closeIntent.cancel}
          onMouseLeave={scheduleClose}
          className="absolute z-40 text-label"
          style={{
            ...GLASS_TOOLTIP_STYLE,
            // A touch more opaque than the shared glass: marker cards sit over
            // the busy ribbon and lose legibility when it bleeds through. Keep
            // the vibrancy, just paint the translucent surface tint over a
            // low-alpha base to firm it up. Overridden here rather than in the
            // shared style so the other chart tooltips keep their frosting.
            background: dark
              ? "linear-gradient(var(--color-surface), var(--color-surface)), rgb(30 30 30 / 0.5)"
              : "linear-gradient(var(--color-surface), var(--color-surface)), rgb(255 255 255 / 0.3)",
            left: tipPos?.left ?? 0,
            top: tipPos?.top ?? 0,
            visibility: tipPos ? "visible" : "hidden",
            minWidth: kind.minWidth ?? 150,
            maxWidth: kind.maxWidth ?? 230,
            lineHeight: 1.4,
            padding: "5px 6px",
          }}
        >
          {kind.renderTooltip(hovered.members, close)}
        </div>
      )}
    </>
  )
}

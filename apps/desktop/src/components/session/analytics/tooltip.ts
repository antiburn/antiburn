// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Shared surface and positioning for the analytics chart tooltips.
 *
 * These are cursor-following hover cards, not the platform `Tooltip` — they
 * track the pointer across a chart rather than anchoring to a trigger — so they
 * carry their own surface. Keeping both here stops the several charts that use
 * them from drifting apart, which is exactly what happened when each carried
 * its own copy.
 */

import {
  useCallback,
  useRef,
  useSyncExternalStore,
  type CSSProperties,
  type RefObject,
} from "react"

/** The frosted-glass surface every analytics chart tooltip paints on. */
export const GLASS_TOOLTIP_STYLE: CSSProperties = {
  background: "color-mix(in srgb, var(--color-surface) 88%, transparent)",
  backdropFilter: "blur(6px)",
  WebkitBackdropFilter: "blur(6px)",
  border: "1px solid var(--color-separator)",
  borderRadius: 8,
  boxShadow: "0 2px 8px rgb(0 0 0 / 0.12)",
  fontSize: 11,
}

/** Padding kept between the tooltip and the window edge, in px. */
const EDGE_PADDING = 8
/** Offset from the cursor to the tooltip's nearest corner, in px. */
const CURSOR_OFFSET = 14

type TipPos = { left: number; top: number }

/**
 * Position a hover tooltip beside the cursor: prefer below-right, flip away
 * from any edge it would overflow, then hard-clamp inside the window so it can
 * never leave it.
 *
 * Computed in viewport space and converted to wrapper-local coordinates, so it
 * is robust to any transformed ancestor. Returns `null` until measured — the
 * caller hides the tip for that first frame rather than flashing it in the
 * wrong place.
 *
 * Built on `useSyncExternalStore` rather than a layout effect. `getSnapshot`
 * recomputes the position from `wrapRef`/`tipRef` on every call; `subscribe`
 * has no external channel to listen to, but its identity is keyed on `hover`
 * (a fresh object every pointer move), so whenever `hover` changes React
 * tears the subscription down and re-establishes it — and, per
 * `useSyncExternalStore`'s guaranteed post-subscribe resync (see
 * `useElementWidth`'s doc comment for the same mechanism), immediately
 * re-reads `getSnapshot` afterward. That covers both cases the old layout
 * effect covered: the tip element not existing yet on the render where it
 * first appears (`tipRef.current` is still null mid-render), and the tip's
 * content changing size (the mid-render read sees the *previous* box, but the
 * post-commit resync sees the freshly painted one) — both resolved
 * synchronously, before the browser paints, so nothing flashes at a stale
 * position.
 */
export function useTooltipPosition(
  hover: { clientX: number; clientY: number } | null,
  wrapRef: RefObject<HTMLDivElement | null>,
  tipRef: RefObject<HTMLDivElement | null>,
): TipPos | null {
  // Caches the last computed position so `getSnapshot` can return the same
  // reference when nothing actually changed — `useSyncExternalStore` treats a
  // fresh object every call as a store change on every render otherwise.
  const lastPos = useRef<TipPos | null>(null)

  const getSnapshot = useCallback((): TipPos | null => {
    const wrap = wrapRef.current
    const tip = tipRef.current
    if (!hover || !wrap || !tip) {
      lastPos.current = null
      return null
    }
    const box = tip.getBoundingClientRect()
    const wrapRect = wrap.getBoundingClientRect()
    const maxX = window.innerWidth - EDGE_PADDING - box.width
    const maxY = window.innerHeight - EDGE_PADDING - box.height

    let vx = hover.clientX + CURSOR_OFFSET
    if (vx > maxX) vx = hover.clientX - CURSOR_OFFSET - box.width
    vx = Math.min(Math.max(vx, EDGE_PADDING), Math.max(EDGE_PADDING, maxX))

    let vy = hover.clientY + CURSOR_OFFSET
    if (vy > maxY) vy = hover.clientY - CURSOR_OFFSET - box.height
    vy = Math.min(Math.max(vy, EDGE_PADDING), Math.max(EDGE_PADDING, maxY))

    const next = { left: vx - wrapRect.left, top: vy - wrapRect.top }
    const prev = lastPos.current
    if (prev && prev.left === next.left && prev.top === next.top) return prev
    lastPos.current = next
    return next
  }, [hover, wrapRef, tipRef])

  // `hover` isn't read in the body — its only role is to give `subscribe` a
  // fresh identity on every pointer move, forcing the resubscribe-driven
  // resync described above.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const subscribe = useCallback(() => () => {}, [hover])

  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
}

function getServerSnapshot(): TipPos | null {
  return null
}

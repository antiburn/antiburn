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

import { useLayoutEffect, useState, type CSSProperties, type RefObject } from 'react';

/** The frosted-glass surface every analytics chart tooltip paints on. */
export const GLASS_TOOLTIP_STYLE: CSSProperties = {
  background: 'color-mix(in srgb, var(--color-surface) 88%, transparent)',
  backdropFilter: 'blur(6px)',
  WebkitBackdropFilter: 'blur(6px)',
  border: '1px solid var(--color-separator)',
  borderRadius: 8,
  boxShadow: '0 2px 8px rgb(0 0 0 / 0.12)',
  fontSize: 11,
};

/** Padding kept between the tooltip and the window edge, in px. */
const EDGE_PADDING = 8;
/** Offset from the cursor to the tooltip's nearest corner, in px. */
const CURSOR_OFFSET = 14;

/**
 * Position a hover tooltip beside the cursor: prefer below-right, flip away
 * from any edge it would overflow, then hard-clamp inside the window so it can
 * never leave it.
 *
 * Computed in viewport space and converted to wrapper-local coordinates, so it
 * is robust to any transformed ancestor. Returns `null` until measured — the
 * caller hides the tip for that first frame rather than flashing it in the
 * wrong place.
 */
export function useTooltipPosition(
  hover: { clientX: number; clientY: number } | null,
  wrapRef: RefObject<HTMLDivElement | null>,
  tipRef: RefObject<HTMLDivElement | null>,
): { left: number; top: number } | null {
  const [tipPos, setTipPos] = useState<{ left: number; top: number } | null>(null);

  useLayoutEffect(() => {
    const wrap = wrapRef.current;
    const tip = tipRef.current;
    if (!hover || !wrap || !tip) {
      setTipPos(null);
      return;
    }
    const box = tip.getBoundingClientRect();
    const wrapRect = wrap.getBoundingClientRect();
    const maxX = window.innerWidth - EDGE_PADDING - box.width;
    const maxY = window.innerHeight - EDGE_PADDING - box.height;

    let vx = hover.clientX + CURSOR_OFFSET;
    if (vx > maxX) vx = hover.clientX - CURSOR_OFFSET - box.width;
    vx = Math.min(Math.max(vx, EDGE_PADDING), Math.max(EDGE_PADDING, maxX));

    let vy = hover.clientY + CURSOR_OFFSET;
    if (vy > maxY) vy = hover.clientY - CURSOR_OFFSET - box.height;
    vy = Math.min(Math.max(vy, EDGE_PADDING), Math.max(EDGE_PADDING, maxY));

    setTipPos({ left: vx - wrapRect.left, top: vy - wrapRect.top });
  }, [hover, wrapRef, tipRef]);

  return tipPos;
}

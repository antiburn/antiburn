// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { BrandMark } from '../../lib/brandMarks';

/**
 * How wide a mark is drawn, in the ring's 32-unit units.
 *
 * The track and arc occupy the outer 2.5 units of a radius-13 circle, leaving
 * about 23 units of clear interior. 16.8 sits inside that with room to spare —
 * a mark that fills the interior edge-to-edge reads as crowding the arc rather
 * than sitting in it.
 */
const MARK_EXTENT = 16.8;

/**
 * Centre a mark in the ring's 32-unit box at {@link MARK_EXTENT}.
 *
 * A mark's own box is whatever its source draws in — `simple-icons` uses 24,
 * other sources do not — so the scale is derived from its `viewBox` rather
 * than assumed. Non-square boxes fit on their longer edge, so nothing is
 * stretched and nothing overflows.
 */
function markTransform(mark: BrandMark): string {
  const [, , w = 24, h = 24] = mark.viewBox.split(' ').map(Number);
  const scale = MARK_EXTENT / Math.max(w, h);
  return `translate(16 16) scale(${scale}) translate(${-w / 2} ${-h / 2})`;
}

/**
 * A provider's nearest limit, as a ring.
 *
 * The footer has room for a glyph and a number, and a ring is the one shape
 * that adds "how much of it is gone" without adding a word. It is used only
 * where a provider *stated* a percentage — the estimate surfaces have no
 * denominator and must never borrow this shape, because a full ring means
 * something there that it does not mean here.
 *
 * Three states, and the difference between them is load-bearing:
 *
 * - **Determinate.** A stated percentage. A solid arc.
 * - **Indeterminate.** A provider that reports a window but no figure for it.
 *   A dashed track and no arc — visibly a ring with nothing in it rather than
 *   a ring at zero, which would be a claim.
 * - **Estimated.** A figure that was modelled rather than stated. The arc,
 *   plus a hairline that marks it as second-hand.
 */
export function UsageRing({
  percent,
  estimated = false,
  glyph,
  mark,
  size = 16,
  className = '',
}: {
  /** Consumed capacity, 0–100. `null` renders the indeterminate ring. */
  percent: number | null;
  /** Whether the figure was modelled rather than stated by the provider. */
  estimated?: boolean;
  /**
   * What sits inside the ring — the provider's brand mark where one exists,
   * otherwise its initial.
   *
   * Not decoration. Where this replaces a provider glyph, dropping it would
   * leave a chip that says how full something is without saying whose, which
   * is a worse trade than the ring is worth.
   */
  glyph?: string;
  /** A brand mark, preferred over `glyph` when supplied. */
  mark?: BrandMark | undefined;
  size?: number;
  className?: string;
}) {
  // Geometry in a fixed 32-unit box, scaled by `size`. Keeping the viewBox
  // constant means the stroke stays proportional at every call site.
  const radius = 13;
  const circumference = 2 * Math.PI * radius;
  const clamped = percent == null ? null : Math.min(100, Math.max(0, percent));

  return (
    <svg
      viewBox="0 0 32 32"
      width={size}
      height={size}
      className={className}
      aria-hidden="true"
      focusable="false"
    >
      <circle
        cx="16"
        cy="16"
        r={radius}
        fill="none"
        strokeWidth="2.5"
        stroke="currentColor"
        className={clamped == null ? 'text-label-tertiary' : 'text-separator'}
        strokeDasharray={clamped == null ? '2 2' : undefined}
      />
      {clamped != null && (
        <circle
          cx="16"
          cy="16"
          r={radius}
          fill="none"
          strokeWidth="2.5"
          strokeLinecap="round"
          stroke="var(--color-accent-fill, var(--color-accent-fill-val))"
          strokeDasharray={circumference}
          strokeDashoffset={circumference * (1 - clamped / 100)}
          // Twelve o'clock, clockwise. A ring that starts at three o'clock
          // reads as an arbitrary wedge rather than as a gauge.
          transform="rotate(-90 16 16)"
          data-testid="usage-ring-arc"
        />
      )}
      {clamped != null && estimated && (
        <circle
          cx="16"
          cy="16"
          r={radius - 3}
          fill="none"
          strokeWidth="0.75"
          stroke="currentColor"
          strokeDasharray="1 2"
          className="text-label-tertiary"
          data-testid="usage-ring-estimated"
        />
      )}
      {mark && (
        // Scaled from the mark's own box into the ring's 32-unit one and
        // centred. 0.7 fills most of the ring's clear interior — the track and
        // arc occupy the outer 2.5 units of a radius-13 circle, leaving about
        // 23 units of usable space — without the mark touching the arc. Marks
        // do not share one box (simple-icons draws at 24, other sources do
        // not), so the scale is derived from the mark rather than assumed.
        <g transform={markTransform(mark)} fill="currentColor" data-testid="usage-ring-mark">
          <path d={mark.path} />
        </g>
      )}
      {!mark && glyph && (
        <text
          x="16"
          y="16"
          textAnchor="middle"
          dominantBaseline="central"
          // Sized against the 32-unit box rather than the rendered size, so
          // the letter keeps its proportion inside the ring at any scale.
          fontSize="16"
          fontWeight="500"
          fill="currentColor"
          data-testid="usage-ring-glyph"
        >
          {glyph}
        </text>
      )}
    </svg>
  );
}

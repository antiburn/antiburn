// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

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
  size = 16,
  className = '',
}: {
  /** Consumed capacity, 0–100. `null` renders the indeterminate ring. */
  percent: number | null;
  /** Whether the figure was modelled rather than stated by the provider. */
  estimated?: boolean;
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
    </svg>
  );
}

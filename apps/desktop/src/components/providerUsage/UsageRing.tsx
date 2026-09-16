import type { CSSProperties } from "react"

import type { BrandMark } from "../../lib/brandMarks"

/**
 * How wide a mark is drawn, in the ring's 32-unit units.
 *
 * The track and arc occupy the outer 2.5 units of a radius-13 circle, leaving
 * about 23 units of clear interior. 16.8 sits inside that with room to spare —
 * a mark that fills the interior edge-to-edge reads as crowding the arc rather
 * than sitting in it.
 */
const MARK_EXTENT = 16.8

/**
 * The share of the ring the sweep's arc covers while a session is live.
 *
 * One thirty-second of a 26px ring is two pixels, which the eye misses. An
 * eighth is the smallest arc that reads at that size without covering the
 * reading.
 */
const SWEEP_FRACTION = 1 / 8

/** The colour of the ring's track. */
const TRACK_COLOR = "var(--color-surface-tertiary)"

/**
 * Centre a mark in the ring's 32-unit box at {@link MARK_EXTENT}.
 *
 * A mark's own box is whatever its source draws in — `simple-icons` uses 24,
 * other sources do not — so the scale is derived from its `viewBox` rather
 * than assumed. Non-square boxes fit on their longer edge, so nothing is
 * stretched and nothing overflows.
 */
function markTransform(mark: BrandMark): string {
  const [, , w = 24, h = 24] = mark.viewBox.split(" ").map(Number)
  const scale = MARK_EXTENT / Math.max(w, h)
  return `translate(16 16) scale(${scale}) translate(${-w / 2} ${-h / 2})`
}

/**
 * A provider's nearest limit, as a ring.
 *
 * A provider pill has room for a glyph and a number. A ring is the one shape
 * that adds "how much of it is gone" without adding a word. It is used only
 * where a provider *stated* a percentage — the estimate surfaces have no
 * denominator and must never borrow this shape, because a full ring means
 * something there that it does not mean here.
 *
 * Two states, and the difference between them is load-bearing:
 *
 * - **Determinate.** A stated percentage. A solid arc over a plain track.
 * - **Indeterminate.** A provider that reports a window but no figure for it.
 *   A dashed track and no arc — visibly a ring with nothing in it rather than
 *   a ring at zero, which would be a claim.
 *
 * `live` runs the session sweep on a determinate ring: a gleam an eighth
 * long runs from twelve o'clock to the end of the reading's arc once per
 * cycle, on the clock of the nearest `led-clock` ancestor, and fades there.
 * The track does not move. A reading under an eighth has no room for the
 * gleam, so the first eighth flashes in the brand tint instead, the way a
 * bar with nothing lit flashes its first segment. Under reduced motion the
 * arc holds the next eighth past the arc's end, the way a meter holds its
 * next segment; a full ring holds its last eighth. The indeterminate ring
 * has no share to sweep.
 */
export function UsageRing({
  percent,
  expectedFraction = null,
  glyph,
  mark,
  size = 16,
  className = "",
  live = false,
}: {
  /** Consumed capacity, 0–100. `null` renders the indeterminate ring. */
  percent: number | null
  /** The elapsed share of the displayed window, or null when its timing is unknown. */
  expectedFraction?: number | null
  /**
   * What sits inside the ring — the provider's brand mark where one exists,
   * otherwise its initial.
   *
   * Not decoration. Where this replaces a provider glyph, dropping it would
   * leave a provider pill that says how full something is without saying whose.
   */
  glyph?: string
  /** A brand mark, preferred over `glyph` when supplied. */
  mark?: BrandMark | undefined
  size?: number
  className?: string
  /** Run the sweep around the ring while a session is live. */
  live?: boolean
}) {
  // Geometry in a fixed 32-unit box, scaled by `size`. Keeping the viewBox
  // constant means the stroke stays proportional at every call site.
  const radius = 13
  const circumference = 2 * Math.PI * radius
  const clamped = percent == null ? null : Math.min(100, Math.max(0, percent))
  // Where the sweep's arc rests under reduced motion, as a share of the
  // ring: at the arc's end, pulled back so a full ring marks its last eighth
  // and not nothing.
  const restStart = clamped == null ? 0 : Math.min(clamped / 100, 1 - SWEEP_FRACTION)
  // How far the gleam's start can travel, as a share of the ring, so that its
  // end stays inside the reading's arc. Zero holds the flash at twelve.
  const sweepSpan = clamped == null ? 0 : Math.max(0, clamped / 100 - SWEEP_FRACTION)

  return (
    <svg
      viewBox="0 0 32 32"
      width={size}
      height={size}
      className={className}
      aria-hidden="true"
      focusable="false"
    >
      {clamped == null && (
        // The indeterminate state keeps its dashed track. Without it the ring
        // would vanish.
        <circle
          cx="16"
          cy="16"
          r={radius}
          fill="none"
          strokeWidth="2.5"
          stroke="currentColor"
          className="text-label-tertiary"
          strokeDasharray="2 2"
        />
      )}
      {clamped != null && (
        <>
          {/* The remainder the arc is measured against. The usage bar states
              no figure beside the ring any more, so the ring alone carries the
              reading, and an arc with no track states a length and not a
              share. The same grey as an unfilled segment on a meter. */}
          <circle
            cx="16"
            cy="16"
            r={radius}
            fill="none"
            strokeWidth="2.5"
            stroke={TRACK_COLOR}
            data-testid="usage-ring-track"
          />
          <circle
            cx="16"
            cy="16"
            r={radius}
            fill="none"
            strokeWidth="2.5"
            strokeLinecap="round"
            stroke="var(--color-brand-tint)"
            strokeDasharray={circumference}
            strokeDashoffset={circumference * (1 - clamped / 100)}
            // Twelve o'clock, clockwise. A ring that starts at three o'clock
            // reads as an arbitrary wedge rather than as a gauge.
            transform="rotate(-90 16 16)"
            data-testid="usage-ring-arc"
          />
          {live && (
            // `hud.css` turns the arc with the sweep. The arc has no
            // `transform` attribute: the stylesheet owns its rotation, and
            // reads its range and the rest angle from the properties below.
            <circle
              cx="16"
              cy="16"
              r={radius}
              fill="none"
              strokeWidth="2.5"
              strokeLinecap="round"
              className="led-sweep-ring"
              strokeDasharray={`${circumference * SWEEP_FRACTION} ${circumference}`}
              style={
                {
                  "--led-ring-span": sweepSpan,
                  "--led-ring-rest": `${-90 + restStart * 360}deg`,
                } as CSSProperties
              }
              data-led-lit={sweepSpan > 0 || undefined}
              data-testid="usage-ring-sweep"
            />
          )}
          {expectedFraction != null && Number.isFinite(expectedFraction) && (
            // The tick crosses the track and stays outside the provider mark in the 32-unit box.
            <line
              x1="16"
              y1="1.75"
              x2="16"
              y2="3.25"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              className="text-label"
              transform={`rotate(${Math.min(1, Math.max(0, expectedFraction)) * 360} 16 16)`}
              data-testid="usage-ring-notch"
            />
          )}
        </>
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
  )
}

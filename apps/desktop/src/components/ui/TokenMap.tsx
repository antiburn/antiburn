import type { TokenMapLayout } from "../../lib/tokenMap"

/** One cell in SVG units. Dots are drawn inside it with a gap. */
const UNIT = 10
const DOT_R = 3.6
const SMALL_R = 2.2
const FRAME_PAD = 2.5

/** Thin frame colours, one per blob in order. Distinct from the mode palette. */
const FRAME_COLORS = [
  "var(--color-label-tertiary)",
  "var(--color-system-red)",
  "var(--color-system-indigo-text)",
  "var(--color-system-gold-text)",
  "var(--color-system-blue)",
  "var(--color-system-green)",
]

/** Draw the token map: one dot blob per live session inside a fixed square. */
export function TokenMap({
  layout,
  className = "",
}: {
  layout: TokenMapLayout
  className?: string
}) {
  const size = layout.cells * UNIT
  return (
    <svg
      viewBox={`0 0 ${size} ${size}`}
      className={`block aspect-square w-full ${className}`.trimEnd()}
      aria-hidden="true"
      data-dot-value={layout.dotValue}
    >
      {layout.blobs.map((blob, index) => (
        <rect
          key={blob.key}
          x={blob.x * UNIT - FRAME_PAD}
          y={blob.y * UNIT - FRAME_PAD}
          width={blob.w * UNIT + FRAME_PAD * 2}
          height={blob.h * UNIT + FRAME_PAD * 2}
          rx={UNIT / 2}
          fill="none"
          stroke={FRAME_COLORS[index % FRAME_COLORS.length]}
          strokeOpacity={0.55}
          strokeWidth={0.8}
          data-session={blob.sessionId}
        />
      ))}
      {layout.dots.map((dot, index) => (
        <circle
          key={index}
          cx={dot.x * UNIT + UNIT / 2}
          cy={dot.y * UNIT + UNIT / 2}
          r={dot.small ? SMALL_R : DOT_R}
          fill={`var(--color-mode-${dot.mode})`}
          fillOpacity={dot.dim ? 0.3 : 1}
          className={dot.live ? "token-map-live" : undefined}
          data-mode={dot.mode}
        />
      ))}
    </svg>
  )
}

import { frameColor, type TokenMapLayout } from "../../lib/tokenMap"

/*
 * The map draws in HUD pixels, so a dot is the size of an LED and sits on
 * the same column. The panel content is the 176px window minus its margins,
 * border, and padding. The LED row spreads 20 6px segments across it, and
 * the VU rows sit 9px apart.
 */
const CONTENT_PX = 134
const LED_PX = 6
const SMALL_PX = 4
const COLUMN_PITCH = (CONTENT_PX - LED_PX) / 19
const ROW_PITCH = 9
const FRAME_PAD = 1.5

/** Draw the token map: one dot blob per live session on the LED grid. */
export function TokenMap({
  layout,
  className = "",
}: {
  layout: TokenMapLayout
  className?: string
}) {
  // Crop to the rows in use, so the dots stay close to the bars below.
  const rows = layout.blobs.reduce((max, blob) => Math.max(max, blob.y + blob.h), 1)
  const height = (rows - 1) * ROW_PITCH + LED_PX
  const cx = (x: number) => x * COLUMN_PITCH + LED_PX / 2
  const cy = (y: number) => y * ROW_PITCH + LED_PX / 2
  return (
    <svg
      viewBox={`0 0 ${CONTENT_PX} ${height}`}
      className={`block w-full overflow-visible ${className}`.trimEnd()}
      style={{ aspectRatio: `${CONTENT_PX} / ${height}` }}
      aria-hidden="true"
      data-dot-value={layout.dotValue}
    >
      {layout.blobs.map((blob, index) => (
        <rect
          key={blob.key}
          x={cx(blob.x) - LED_PX / 2 - FRAME_PAD}
          y={cy(blob.y) - LED_PX / 2 - FRAME_PAD}
          width={cx(blob.x + blob.w - 1) - cx(blob.x) + LED_PX + FRAME_PAD * 2}
          height={(blob.h - 1) * ROW_PITCH + LED_PX + FRAME_PAD * 2}
          // Each corner hugs the dot inside it.
          rx={LED_PX / 2 + FRAME_PAD}
          fill="none"
          stroke={frameColor(index)}
          strokeOpacity={0.55}
          strokeWidth={0.8}
          data-session={blob.sessionId}
        />
      ))}
      {layout.dots.map((dot, index) => (
        <circle
          key={index}
          cx={cx(dot.x)}
          cy={cy(dot.y)}
          r={(dot.small ? SMALL_PX : LED_PX) / 2}
          fill={`var(--color-mode-${dot.mode})`}
          fillOpacity={dot.dim ? 0.3 : 1}
          className={dot.live ? "token-map-live" : undefined}
          data-mode={dot.mode}
        />
      ))}
    </svg>
  )
}

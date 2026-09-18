import { frameColor, type TokenMapLayout } from "../../lib/tokenMap"

/*
 * The map draws in HUD pixels, so a dot is the size of an LED and sits on
 * the same column. The panel content is the 176px window minus its 8px side
 * margins and its 10px padding. The LED row spreads 20 6px segments across
 * it, and the VU rows sit 9px apart.
 */
const CONTENT_PX = 140
const LED_PX = 6
const SMALL_PX = 4
const COLUMN_PITCH = (CONTENT_PX - LED_PX) / 19
const ROW_PITCH = 9
const FRAME_PAD = 1.5

/**
 * Draw the token map: one dot blob per live session on the LED grid.
 *
 * `onHoverBlob` reports the blob under the pointer by key, or null when the
 * pointer leaves it. Each blob is one group, so a move from the frame to a
 * dot inside it is not a leave. A sub-agent's dot also reports its owner, so
 * the detail can name it.
 */
export function TokenMap({
  layout,
  className = "",
  onHoverBlob,
}: {
  layout: TokenMapLayout
  className?: string
  onHoverBlob?: (key: string | null, subagentId?: string | null) => void
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
        <g
          key={blob.key}
          data-blob={blob.key}
          onMouseEnter={onHoverBlob ? () => onHoverBlob(blob.key) : undefined}
          onMouseLeave={onHoverBlob ? () => onHoverBlob(null) : undefined}
        >
          <rect
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
            // The unfilled frame still catches the pointer inside it.
            pointerEvents="all"
            data-session={blob.sessionId}
          />
          {layout.dots
            .filter((dot) => dot.blob === index)
            .map((dot, position) => (
              <circle
                key={position}
                cx={cx(dot.x)}
                cy={cy(dot.y)}
                r={(dot.small ? SMALL_PX : LED_PX) / 2}
                fill={`var(--color-mode-${dot.mode})`}
                fillOpacity={dot.dim ? 0.3 : 1}
                className={dot.live ? "token-map-live" : undefined}
                data-mode={dot.mode}
                data-subagent={dot.owner ?? undefined}
                onMouseEnter={
                  onHoverBlob && dot.owner ? () => onHoverBlob(blob.key, dot.owner) : undefined
                }
                onMouseLeave={
                  onHoverBlob && dot.owner ? () => onHoverBlob(blob.key, null) : undefined
                }
              />
            ))}
        </g>
      ))}
    </svg>
  )
}

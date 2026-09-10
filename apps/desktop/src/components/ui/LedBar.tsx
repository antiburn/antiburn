import type { CSSProperties } from "react"

/**
 * Render an LED-style bar with fixed circular segments.
 *
 * `expectedFraction` draws the linear-use notch, as `SegmentedMeter` does in
 * the popover: a tick at how far through the window's period the clock has
 * travelled. It separates 60% used at 30% elapsed from 60% used at 90%
 * elapsed. With no fraction there is no notch.
 *
 * `live` runs the session sweep: a band that crosses every segment from the
 * left, on the clock of the nearest `led-clock` ancestor. Each segment
 * carries its index and the bar's segment count, and `hud.css` paints the
 * band on the segment from its distance to the sweep position. `row` is the
 * bar's row within its provider: each row runs 100 ms after the one above it.
 *
 * Under reduced motion the sweep stops, and the next segment to light holds
 * the brand tint instead. At zero that is the first segment, so a bar with no
 * lit segment still shows that a session is live. A full bar marks its last
 * segment. The band on a lit segment is a lighter gleam: the bar colours sit
 * too close to the brand tint for a 6px dot to show the tint above them.
 */
export function LedBar({
  split,
  segments = 40,
  className = "",
  live = false,
  row = 0,
  expectedFraction = null,
}: {
  split: Array<{ fraction: number; color: string }>
  segments?: number
  className?: string
  /** Run the sweep across the bar, for a live session. */
  live?: boolean
  /** The bar's row within its provider, for the sweep stagger. */
  row?: number
  /** Elapsed share of the window's period, 0-1, or null when unknown. */
  expectedFraction?: number | null
}) {
  const cutoffs: Array<{ upTo: number; color: string }> = []
  let accumulated = 0
  for (const span of split) {
    accumulated += Math.max(0, span.fraction)
    cutoffs.push({ upTo: accumulated, color: span.color })
  }
  const litCount = Math.min(
    segments,
    Math.round(Math.min(1, Math.max(0, accumulated)) * segments),
  )
  // A full bar has no next segment; the still mark then stays on the last one.
  const nextIndex = live ? Math.min(segments - 1, litCount) : -1
  const sweep = live
    ? ({ "--led-segments": segments, "--led-row": row } as CSSProperties)
    : undefined

  return (
    <div
      className={`relative flex w-full items-center justify-between ${className}`.trimEnd()}
      style={sweep}
      aria-hidden="true"
    >
      {Array.from({ length: segments }, (_, index) => {
        const midpoint = (index + 0.5) / segments
        const hit = cutoffs.find((cutoff) => midpoint <= cutoff.upTo)
        const style: CSSProperties = {}
        if (hit) style.backgroundColor = hit.color
        if (live) Object.assign(style, { "--led-index": index })
        return (
          <span
            key={index}
            data-led-lit={live && hit != null ? true : undefined}
            data-led-next={index === nextIndex || undefined}
            className={`relative h-1.5 w-1.5 shrink-0 rounded-full ${hit ? "" : "bg-led-off"} ${live ? "led-sweep-dot" : ""}`.trimEnd()}
            style={hit || live ? style : undefined}
          />
        )
      })}
      {expectedFraction != null && (
        <span
          data-testid="led-bar-notch"
          className="led-notch absolute -inset-y-[2px]"
          style={{ left: `${Math.min(100, Math.max(0, expectedFraction * 100))}%` }}
        />
      )}
    </div>
  )
}

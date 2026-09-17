import type { CSSProperties } from "react"

/**
 * Render an LED-style bar with fixed circular segments.
 *
 * `expectedFraction` draws the linear-use notch, as `SegmentedMeter` does in
 * the popover: a tick at how far through the window's period the clock has
 * travelled. It separates 60% used at 30% elapsed from 60% used at 90%
 * elapsed. With no fraction there is no notch.
 *
 * `live` runs the session sweep: a gleam that crosses the lit segments from
 * the left, on the clock of the nearest `led-clock` ancestor. An unlit
 * segment does not move. Each lit segment carries its index and the bar's
 * segment count, and `hud.css` paints the gleam on the segment from its
 * distance to the sweep position. `row` is the bar's row within its
 * provider: each row runs 100 ms after the one above it.
 *
 * A bar with no lit segment flashes its first segment in the brand tint as
 * the sweep passes, so a session at zero usage still shows. Under reduced
 * motion the sweep stops, and the next segment to light holds the brand tint
 * instead; a full bar marks its last segment.
 *
 * `blinkLast` blinks the last lit segment, for the HUD's spend-rate LED. The
 * segment can take its own period and colour. The period follows the spend
 * rate; the colour is the mode of the newest live turn. Without them the
 * segment blinks at the stylesheet's period in the bar's own colour.
 */
export function LedBar({
  split,
  segments = 40,
  className = "",
  style,
  live = false,
  row = 0,
  blinkLast = false,
  blinkPeriodMs = null,
  blinkColor = null,
  expectedFraction = null,
}: {
  split: Array<{ fraction: number; color: string }>
  segments?: number
  className?: string
  style?: CSSProperties | undefined
  /** Run the sweep across the bar, for a live session. */
  live?: boolean
  /** The bar's row within its provider, for the sweep stagger. */
  row?: number
  blinkLast?: boolean
  /** Milliseconds per blink cycle, or null for the stylesheet's period. */
  blinkPeriodMs?: number | null
  /** A CSS colour for the lit half of the blink, or null for the bar's colour. */
  blinkColor?: string | null
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
  const blinkIndex = blinkLast && litCount > 0 ? litCount - 1 : -1
  const barStyle: CSSProperties | undefined = live
    ? ({ ...style, "--led-segments": segments, "--led-row": row } as CSSProperties)
    : style

  return (
    <div
      className={`relative flex w-full items-center justify-between ${className}`.trimEnd()}
      style={barStyle}
      aria-hidden="true"
    >
      {Array.from({ length: segments }, (_, index) => {
        const midpoint = (index + 0.5) / segments
        const hit = cutoffs.find((cutoff) => midpoint <= cutoff.upTo)
        // The gleam runs over the lit segments. With none lit, the first
        // segment takes the sweep alone, in the brand tint.
        const sweeping = live && (hit != null || (litCount === 0 && index === 0))
        const blinking = index === blinkIndex
        const style: CSSProperties = {}
        if (hit) style.backgroundColor = hit.color
        if (sweeping) Object.assign(style, { "--led-index": index })
        // The stylesheet derives the gleam from the segment's own colour, so a
        // provider whose bar is near white still shows the sweep.
        if (sweeping && hit) Object.assign(style, { "--led-color": hit.color })
        if (hit && blinking) {
          Object.assign(style, {
            backgroundColor: blinkColor ?? hit.color,
            "--led-on": blinkColor ?? hit.color,
            ...(blinkPeriodMs != null ? { "--led-period": `${blinkPeriodMs}ms` } : {}),
          })
        }
        const classes = [
          "relative h-1.5 w-1.5 shrink-0 rounded-full",
          hit ? "led-lit" : "led-off bg-led-off",
          sweeping ? "led-sweep-dot" : "",
          blinking ? "led-blink" : "",
        ]
        return (
          <span
            key={index}
            data-led-lit={live && hit != null ? true : undefined}
            data-led-next={index === nextIndex || undefined}
            className={classes.filter(Boolean).join(" ")}
            style={hit || sweeping ? style : undefined}
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

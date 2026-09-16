import type { CSSProperties } from "react"

/**
 * Render an LED-style bar with fixed circular segments.
 *
 * `expectedFraction` draws the linear-use notch, as `SegmentedMeter` does in
 * the popover: a tick at how far through the window's period the clock has
 * travelled. It separates 60% used at 30% elapsed from 60% used at 90%
 * elapsed. With no fraction there is no notch.
 *
 * The blinking segment can take its own period and colour. The period
 * follows the spend rate; the colour is the mode of the newest live turn.
 * Without them the segment blinks at the stylesheet's period in the bar's
 * own colour.
 */
export function LedBar({
  split,
  segments = 40,
  className = "",
  style,
  blinkLast = false,
  blinkPeriodMs = null,
  blinkColor = null,
  expectedFraction = null,
}: {
  split: Array<{ fraction: number; color: string }>
  segments?: number
  className?: string
  style?: CSSProperties | undefined
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
  const blinkIndex = blinkLast && litCount > 0 ? litCount - 1 : -1

  return (
    <div
      className={`relative flex w-full items-center justify-between ${className}`.trimEnd()}
      style={style}
      aria-hidden="true"
    >
      {Array.from({ length: segments }, (_, index) => {
        const midpoint = (index + 0.5) / segments
        const hit = cutoffs.find((cutoff) => midpoint <= cutoff.upTo)
        return (
          <span
            key={index}
            className={`h-1.5 w-1.5 shrink-0 rounded-full ${hit ? "led-lit" : "led-off bg-led-off"} ${index === blinkIndex ? "led-blink" : ""}`.trimEnd()}
            style={
              hit
                ? index === blinkIndex
                  ? ({
                      backgroundColor: blinkColor ?? hit.color,
                      "--led-on": blinkColor ?? hit.color,
                      ...(blinkPeriodMs != null
                        ? { "--led-period": `${blinkPeriodMs}ms` }
                        : {}),
                    } as CSSProperties)
                  : { backgroundColor: hit.color }
                : undefined
            }
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

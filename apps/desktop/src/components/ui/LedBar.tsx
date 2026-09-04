import type { CSSProperties } from "react"

/**
 * Render an LED-style bar with fixed circular segments.
 *
 * `expectedFraction` draws the linear-use notch, as `SegmentedMeter` does in
 * the popover: a tick at how far through the window's period the clock has
 * travelled. It separates 60% used at 30% elapsed from 60% used at 90%
 * elapsed. With no fraction there is no notch.
 */
export function LedBar({
  split,
  segments = 40,
  className = "",
  blinkLast = false,
  expectedFraction = null,
}: {
  split: Array<{ fraction: number; color: string }>
  segments?: number
  className?: string
  blinkLast?: boolean
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
      aria-hidden="true"
    >
      {Array.from({ length: segments }, (_, index) => {
        const midpoint = (index + 0.5) / segments
        const hit = cutoffs.find((cutoff) => midpoint <= cutoff.upTo)
        return (
          <span
            key={index}
            className={`h-1.5 w-1.5 shrink-0 rounded-full ${hit ? "" : "bg-led-off"} ${index === blinkIndex ? "led-blink" : ""}`.trimEnd()}
            style={
              hit
                ? index === blinkIndex
                  ? ({
                      backgroundColor: hit.color,
                      "--led-on": hit.color,
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
          className="absolute -inset-y-[2px] w-[1.5px] bg-led-notch"
          style={{ left: `${Math.min(100, Math.max(0, expectedFraction * 100))}%` }}
        />
      )}
    </div>
  )
}

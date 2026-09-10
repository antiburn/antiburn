import type { CSSProperties } from "react"

import { ledBlinkStep } from "../../lib/ledBlink"

/**
 * Render an LED-style bar with fixed circular segments.
 *
 * `expectedFraction` draws the linear-use notch, as `SegmentedMeter` does in
 * the popover: a tick at how far through the window's period the clock has
 * travelled. It separates 60% used at 30% elapsed from 60% used at 90%
 * elapsed. With no fraction there is no notch.
 *
 * `blinkNext` marks a live session on the first unlit segment, the next one
 * to light, as `SegmentedMeter` does in the popover. The blink alternates
 * between the brand tint and the unlit LED colour.
 *
 * The blink must not sit on a lit segment. A lit segment already carries its
 * bar colour, and a provider colour close to the brand tint then alternates
 * with itself. The Claude bar is one such colour: it differs from the brand
 * tint by 3 degrees of hue and 1 point of lightness, which a 6px dot cannot
 * show. The segment past the reading has the contrast the blink needs.
 *
 * At zero the rule lands on the first segment, so a bar with no lit segment
 * still shows that a session is live. A full bar keeps the blink on its last
 * segment.
 *
 * `blinkStep` is the bar's row within its provider: each row turns on 250 ms
 * after the one above it, and all rows turn off together.
 */
export function LedBar({
  split,
  segments = 40,
  className = "",
  blinkNext = false,
  blinkStep = 0,
  expectedFraction = null,
}: {
  split: Array<{ fraction: number; color: string }>
  segments?: number
  className?: string
  /** Blink the next segment to light, for a live session. */
  blinkNext?: boolean
  /** The bar's row within its provider, for the blink stagger. */
  blinkStep?: number
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
  // A full bar has no next segment; the blink then stays on the last one.
  const blinkIndex = blinkNext ? Math.min(segments - 1, litCount) : -1

  return (
    <div
      className={`relative flex w-full items-center justify-between ${className}`.trimEnd()}
      aria-hidden="true"
    >
      {Array.from({ length: segments }, (_, index) => {
        const midpoint = (index + 0.5) / segments
        const hit = cutoffs.find((cutoff) => midpoint <= cutoff.upTo)
        const blinking = index === blinkIndex
        return (
          <span
            key={index}
            data-led-step={blinking ? ledBlinkStep(blinkStep) : undefined}
            className={`h-1.5 w-1.5 shrink-0 rounded-full ${hit ? "" : "bg-led-off"} ${blinking ? "led-blink" : ""}`.trimEnd()}
            style={
              blinking
                ? ({ "--led-rest": "var(--color-led-off)" } as CSSProperties)
                : hit
                  ? { backgroundColor: hit.color }
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

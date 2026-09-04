import type { CSSProperties } from "react"

/** Render an LED-style bar with fixed circular segments. */
export function LedBar({
  split,
  segments = 40,
  className = "",
  blinkLast = false,
}: {
  split: Array<{ fraction: number; color: string }>
  segments?: number
  className?: string
  blinkLast?: boolean
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
      className={`flex w-full items-center justify-between ${className}`.trimEnd()}
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
    </div>
  )
}

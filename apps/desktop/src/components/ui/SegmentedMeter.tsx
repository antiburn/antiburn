import { cn } from "../../lib/cn"

/**
 * A limit meter drawn as a row of discrete circles, filled left to right in
 * the brand orange. The segmented form is the point: a reading that arrives
 * in steps looks like an instrument, where a continuous bar looks like a
 * download.
 *
 * A `null` percent renders every segment empty at half strength. That is
 * visibly a meter with no reading, not a meter at zero, which states a figure
 * nobody supplied.
 *
 * `expectedFraction` draws the linear-use notch: a tick at how far through
 * the window's period the clock has travelled. It keeps 60% used at 30%
 * elapsed from looking the same as 60% used at 90% elapsed. With no fraction
 * there is no notch — the component never draws one from an assumption.
 */
export function SegmentedMeter({
  percent,
  expectedFraction = null,
  // 32 packs the track: at the popover's row width the dots sit about a third
  // of a dot apart.
  segments = 32,
  className = "",
}: {
  /** Consumed capacity, 0–100, or `null` for no stated figure. */
  percent: number | null
  /** Elapsed share of the window's own period, 0–1, or `null` when unknown. */
  expectedFraction?: number | null
  segments?: number
  className?: string
}) {
  const clamped = percent == null ? null : Math.min(100, Math.max(0, percent))
  const filled = clamped == null ? 0 : Math.round((clamped / 100) * segments)

  return (
    <div aria-hidden="true" className={cn("relative", className)}>
      {/* The segments span the full row, so the track ends where the figure
          column starts. This also keeps the notch honest: the notch offset
          and the track then measure the same width. */}
      <div className="flex items-center justify-between">
        {Array.from({ length: segments }, (_, index) => (
          <span
            key={index}
            className={cn(
              "h-[7px] w-[7px] shrink-0 rounded-full",
              index < filled ? "bg-brand-tint" : "bg-surface-tertiary",
              clamped == null && "opacity-50",
            )}
          />
        ))}
      </div>
      {expectedFraction != null && (
        <span
          data-testid="segmented-meter-notch"
          className="absolute -inset-y-[2px] w-[1.5px] bg-(--color-label)"
          style={{ left: `${Math.min(100, Math.max(0, expectedFraction * 100))}%` }}
        />
      )}
    </div>
  )
}

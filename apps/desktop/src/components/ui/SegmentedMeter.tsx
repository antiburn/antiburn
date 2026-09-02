import { cn } from "../../lib/cn"

/**
 * One colored zone of a meter track. The zone runs from `from` to the start
 * of the next zone, as fractions of the track.
 */
export interface MeterZone {
  /** Where the zone starts on the track, 0–1. */
  from: number
  /** Ink for a lit segment inside the zone. */
  fillClassName: string
  /** Ink for an unlit segment inside the zone. */
  trackClassName: string
}

/**
 * The three steps of the meter palette, shared by every meter: brand orange
 * while a reading is fine, yellow for warning, red for trouble. A meter
 * places these steps along its own track; it does not pick its own colors.
 */
export const METER_INK = {
  normal: { fillClassName: "bg-brand-tint", trackClassName: "bg-brand-unlit/12" },
  warning: {
    fillClassName: "bg-system-yellow-tint",
    trackClassName: "bg-system-yellow-unlit/12",
  },
  critical: { fillClassName: "bg-system-red-tint", trackClassName: "bg-system-red-unlit/12" },
} as const

/** One step of the meter palette. */
export type MeterInk = (typeof METER_INK)[keyof typeof METER_INK]

/**
 * The additive usage scale: orange from zero, yellow from 80%, red from 90%.
 * Unlit segments keep a faint tint of their zone, so the scale shows its
 * bands before the reading reaches them.
 */
export const USAGE_METER_ZONES: MeterZone[] = [
  { from: 0, ...METER_INK.normal },
  { from: 0.8, ...METER_INK.warning },
  { from: 0.9, ...METER_INK.critical },
]

/** Which end of the track the fill starts from. */
export type MeterFillFrom = "start" | "end"

/** The zone that holds the segment centered at `fraction` of the track. */
function zoneAt(zones: MeterZone[], fraction: number): MeterZone {
  let active = zones[0]!
  for (const zone of zones) {
    if (fraction >= zone.from) active = zone
  }
  return active
}

/**
 * A limit meter drawn as a row of discrete circles, filled left to right and
 * colored like a VU meter: each segment takes the color of the zone it sits
 * in, lit at full strength or unlit as a faint tint of the same hue. The
 * segmented form is the point: a reading that arrives in steps looks like an
 * instrument, where a continuous bar looks like a download.
 *
 * A `null` percent renders every segment unlit at half strength. That is
 * visibly a meter with no reading, not a meter at zero, which states a figure
 * nobody supplied.
 *
 * `fillFrom` chooses the end the fill grows from. The reading always marks
 * the same place on the track, and the zones always run left to right, so a
 * meter that fills from the right lights the track from that end down to the
 * mark. A metric where a higher reading is better reads this way: its lit run
 * shortens as the session improves, like every other meter here.
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
  zones = USAGE_METER_ZONES,
  fillFrom = "start",
}: {
  /** Consumed capacity, 0–100, or `null` for no stated figure. */
  percent: number | null
  /** Elapsed share of the window's own period, 0–1, or `null` when unknown. */
  expectedFraction?: number | null
  segments?: number
  className?: string
  /** The color zones along the track, in ascending `from` order. */
  zones?: MeterZone[]
  /** The end the fill and the zones start from. */
  fillFrom?: MeterFillFrom
}) {
  const clamped = percent == null ? null : Math.min(100, Math.max(0, percent))
  const filled = clamped == null ? 0 : Math.round((clamped / 100) * segments)

  return (
    <div aria-hidden="true" className={cn("relative", className)}>
      {/* The segments span the full row, so the track ends where the figure
          column starts. This also keeps the notch honest: the notch offset
          and the track then measure the same width. */}
      <div className="flex items-center justify-between">
        {Array.from({ length: segments }, (_, index) => {
          const zone = zoneAt(zones, (index + 0.5) / segments)
          // The reading sits at the same mark either way. The fill covers the
          // side of that mark its own end is on.
          const lit = fillFrom === "end" ? index >= filled : index < filled
          return (
            <span
              key={index}
              className={cn(
                "h-[7px] w-[7px] shrink-0 rounded-full",
                lit ? zone.fillClassName : zone.trackClassName,
                clamped == null && "opacity-50",
              )}
            />
          )
        })}
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

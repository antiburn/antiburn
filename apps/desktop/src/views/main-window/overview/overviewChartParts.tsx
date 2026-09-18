import { axisDayLabel } from "../../../lib/presentation/overviewChart"

import { SegmentFigure } from "../../../components/ui/SegmentFigure"

import "./overview.css"

/**
 * The parts both Overview day charts draw.
 *
 * The spend chart and the allowance chart show different units, so they keep
 * their own scales and their own readings. The geometry stays the same, so a
 * reader who learns one chart can read the other.
 */

/** The axis names every seventh day and the last one. */
const AXIS_LABEL_STEP = 7
/** A dated label this close to "Today" would collide with it. */
const AXIS_LABEL_CLEARANCE = 3

/** A day's tooltip opens almost at once; the pointer is already on the bar. */
export const DAY_TOOLTIP_DELAY_MS = 100

/** The fractions of the scale that carry a hairline and a figure. */
export const GUIDE_FRACTIONS = [1, 0.75, 0.5, 0.25]

/** One pill, sized as a share of the chart height. */
export function Bar({
  fraction,
  outline,
  className,
}: {
  fraction: number
  outline: boolean
  className: string
}) {
  return (
    <span
      aria-hidden="true"
      className={`overview-bar ${className}`}
      data-outline={outline ? "" : undefined}
      style={{ blockSize: `${fraction * 100}%` }}
    />
  )
}

/** Hairlines at each quarter of the scale, behind the bars. */
export function Guides() {
  return (
    <div aria-hidden="true" className="pointer-events-none absolute inset-0">
      {GUIDE_FRACTIONS.map((fraction) => (
        <div
          key={fraction}
          className="absolute inset-x-0 border-t border-separator/60"
          style={{ top: `${(1 - fraction) * 100}%` }}
        />
      ))}
    </div>
  )
}

/** The figures for the guides, in a gutter to the right of the bars. */
export function GuideLabels({ labels }: { labels: Map<number, string> }) {
  return (
    <div
      aria-hidden="true"
      className="overview-scale type-metadata relative text-label-tertiary"
    >
      {GUIDE_FRACTIONS.map((fraction) => (
        <span
          key={fraction}
          className="absolute right-0 -translate-y-1/2 whitespace-nowrap"
          style={{ top: `${(1 - fraction) * 100}%` }}
        >
          <SegmentFigure>{labels.get(fraction) ?? ""}</SegmentFigure>
        </span>
      ))}
    </div>
  )
}

/** The key for the two series, over the top-left corner of the plot. */
export function ChartLegend({ nowClassName }: { nowClassName: string }) {
  return (
    <p className="overview-legend type-caption flex items-center gap-[var(--space-md)] text-label-secondary">
      <span className="inline-flex items-center gap-[var(--space-xs)]">
        <span aria-hidden="true" className={`h-2 w-2 rounded-small ${nowClassName}`} />
        Last 30 days
      </span>
      <span className="inline-flex items-center gap-[var(--space-xs)]">
        <span aria-hidden="true" className="h-2 w-2 rounded-small bg-label-tertiary/30" />
        30 days before
      </span>
    </p>
  )
}

/** The dates under the bars. One cell for each day, named at intervals. */
export function ChartAxis({ dates }: { dates: ReadonlyArray<string> }) {
  const lastIndex = dates.length - 1
  return (
    <div
      className="overview-axis type-caption mt-[var(--space-xs)] text-label-tertiary"
      aria-hidden="true"
    >
      {dates.map((localDate, index) => (
        <span key={localDate} className="overview-axis-day">
          {index === lastIndex ? (
            <span className="overview-axis-label">Today</span>
          ) : index % AXIS_LABEL_STEP === 0 && index < lastIndex - AXIS_LABEL_CLEARANCE ? (
            <span className="overview-axis-label">{axisDayLabel(localDate)}</span>
          ) : null}
        </span>
      ))}
    </div>
  )
}

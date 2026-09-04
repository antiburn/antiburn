import { SegmentedMeter } from "../ui/SegmentedMeter"

/**
 * One tab's reading: what the tab is worth, and how healthy the content
 * behind it is.
 */
export interface TabMeter {
  /** What the figure is. The cell shows this word above the figure. */
  label: string
  /** The tab's headline figure, formatted for display. */
  figure: string
  /** The health reading, 0 to 100, or null when no figure measures it. */
  percent: number | null
  /** What the meter measures. The block states this to assistive technology. */
  meterLabel: string
}

/**
 * The reading at the head of one tab panel.
 *
 * The popover's usage bar is the model: a name, a figure, and a meter that
 * shows the standing at a glance. A review put one of these in each nav cell
 * instead, which gave every meter a third of the popover width — ten segments
 * almost touching, and a track too short to read. The meter belongs at full
 * width, beside the content it describes.
 *
 * Every tab's meter reads the same direction: a higher reading is worse. A tab
 * with nothing to measure passes a null percent, and the meter shows no
 * reading rather than a reading of zero.
 */
export function SessionTabMeter({ meter }: { meter: TabMeter }) {
  const reading =
    meter.percent == null
      ? `${meter.meterLabel}: no stated figure`
      : `${meter.meterLabel}: ${Math.round(meter.percent)} percent`
  return (
    <div className="flex flex-col gap-y-1.5 pb-3">
      <div className="flex items-baseline justify-between gap-2">
        <span className="truncate type-caption font-medium! text-label-tertiary uppercase">
          {meter.label}
        </span>
        <span className="shrink-0 type-headline tabular-nums text-label">{meter.figure}</span>
      </div>
      <SegmentedMeter percent={meter.percent} />
      <span className="sr-only">{reading}</span>
    </div>
  )
}

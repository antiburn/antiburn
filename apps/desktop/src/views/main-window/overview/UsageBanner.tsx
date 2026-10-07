import type { ReactNode } from "react"

import { ChartLegend, type ChartLegendItem } from "../../../components/ui/ChartLegend"
import { cn } from "../../../lib/cn"
import { SegmentFigure } from "../../../components/ui/SegmentFigure"

/** One date under the banner, at a fraction of its width. */
export interface UsageBannerDate {
  key: string
  text: string
  /** 0 is the left edge and 1 is the right edge. */
  at: number
}

/**
 * The frame of a Usage card banner: a chart behind the figures, not under
 * them. The plot bleeds to the card's left, top, and right edges (the card's
 * padding is --space-lg) and stops on the date row, the last row in flow. The key sits in the empty top-left corner, beside the
 * unit tabs. `hover` is the day targets, which cover the lower half of the
 * plot only, so the figures above keep their own tooltips.
 */
export function UsageBanner({
  name,
  plot,
  keyItems,
  dates,
  hover,
}: {
  name: string
  plot: ReactNode
  keyItems: readonly ChartLegendItem[]
  dates: readonly UsageBannerDate[]
  hover?: (className: string) => ReactNode
}) {
  return (
    <>
      <div
        aria-hidden="true"
        data-usage-banner={name}
        className="overview-usage-banner pointer-events-none absolute -inset-x-(--space-lg) -top-(--space-lg) bottom-(--overview-banner-dates) -z-10 overflow-hidden rounded-t-(--radius-popover)"
      >
        <div className="overview-banner-in size-full">{plot}</div>
      </div>
      {hover?.("absolute -inset-x-(--space-lg) top-1/2 bottom-(--overview-banner-dates) z-10")}
      <ChartLegend
        ariaLabel="Key"
        items={keyItems}
        className="absolute top-0 left-0 flex h-(--overview-tabs-height) items-center"
      />
      <div
        aria-hidden="true"
        className="relative order-last -mx-(--space-lg) mt-auto h-(--overview-banner-dates) type-caption text-label-tertiary"
      >
        {dates.map((date) => (
          <span
            key={date.key}
            className={cn(
              "absolute top-1/2 -translate-y-1/2 whitespace-nowrap",
              date.at >= 1
                ? "right-(--space-lg)"
                : date.at <= 0
                  ? "ps-(--space-lg)"
                  : "ps-(--space-xs)",
            )}
            style={date.at >= 1 ? undefined : { left: `${date.at * 100}%` }}
          >
            {date.text}
          </span>
        ))}
      </div>
    </>
  )
}

/** One row of a banner tooltip: a swatch, a name, a value, and a share bar. */
export interface UsageBannerTipRow {
  key: string
  label: string
  value: string
  /** A Tailwind background class for the swatch and the share bar. */
  swatch: string
  /** The share bar's fill, from 0 to 1. Leave out for no bar. */
  share?: number
}

/**
 * The tooltip for one day of a banner: the date, the day's headline figure,
 * one row for each layer with a share bar, and a quiet footnote.
 */
export function UsageBannerTip({
  title,
  figure,
  rows,
  footnote,
}: {
  title: string
  figure: string
  rows: readonly UsageBannerTipRow[]
  footnote?: string | undefined
}) {
  return (
    <div className="flex min-w-48 flex-col gap-(--space-xs) py-(--space-xs)">
      <div className="flex items-baseline justify-between gap-(--space-md)">
        <span className="type-caption text-label-secondary">{title}</span>
        <span className="type-body font-medium">
          <SegmentFigure>{figure}</SegmentFigure>
        </span>
      </div>
      {rows.map((row) => (
        <div key={row.key} className="flex flex-col gap-0.5">
          <div className="flex items-center gap-(--space-xs) type-caption">
            <span
              aria-hidden="true"
              className={cn("size-2 shrink-0 rounded-full", row.swatch)}
            />
            <span className="flex-1 text-label-secondary">{row.label}</span>
            <SegmentFigure>{row.value}</SegmentFigure>
          </div>
          {row.share != null && (
            <span aria-hidden="true" className="h-1 overflow-hidden rounded-full bg-label/10">
              <span
                className={cn("block h-full rounded-full", row.swatch)}
                style={{ width: `${Math.min(1, Math.max(0, row.share)) * 100}%` }}
              />
            </span>
          )}
        </div>
      ))}
      {footnote && <span className="type-caption text-label-tertiary">{footnote}</span>}
    </div>
  )
}

import { cn } from "../../../lib/cn"

/** A shorter caption for a stat whose full name does not fit one cell. */
const KEY_CAPTIONS: Record<string, string> = {
  "Provider cache misses": "Cache misses",
}

/**
 * A chart's key, drawn under the plot it explains.
 *
 * Each figure is a stat cell: a swatch in the color its chart layer takes
 * when it lights, the value in the label ink, and a caption under them.
 * The cells wrap into columns that share the available width.
 * The swatch carries the color. The text keeps its contrast on both surfaces.
 *
 * Pointing at a cell lights its layer in the plot above, and the cell takes
 * the hover wash. Clicking a cell pins that layer, so it stays lit when the
 * pointer leaves; clicking it again unpins it. An entry whose `series` is
 * absent counts something the chart draws no mark for, so it neither lights
 * nor pins.
 *
 * Generic over the chart's own series union, so each chart's key stays typed
 * to the layers that chart actually draws.
 */
export function ChartKey<S extends string>({
  stats,
  pinned,
  onHighlight,
  onPin,
  swatchClass,
}: {
  stats: ReadonlyArray<{
    label: string
    value: string
    series?: S
  }>
  /** The layer held lit by a click, or null. */
  pinned: S | null
  /** Names the layer under the pointer, or null when the pointer leaves. */
  onHighlight: (series: S | null) => void
  /** Toggles the pinned layer. */
  onPin: (series: S) => void
  /** The swatch class for each series, so the key doubles as the chart's legend. */
  swatchClass: Record<S, string>
}) {
  return (
    <div data-testid="chart-key" className="session-detail-key grid">
      {stats.map((stat) => {
        const series = stat.series ?? null
        const isPinned = series != null && series === pinned
        return (
          <button
            key={stat.label}
            type="button"
            aria-pressed={series != null ? isPinned : undefined}
            disabled={series == null}
            data-series={series ?? undefined}
            className={cn(
              "chart-key-stat flex min-w-0 flex-col items-start rounded-control text-left disabled:opacity-100",
              isPinned && "bg-surface-secondary",
            )}
            onMouseEnter={() => onHighlight(series)}
            onMouseLeave={() => onHighlight(null)}
            onClick={() => series != null && onPin(series)}
          >
            <span className="flex items-center gap-x-1.5 type-body font-medium text-label tabular-nums">
              <span
                aria-hidden="true"
                className={cn(
                  "size-2 shrink-0 rounded-full",
                  series != null ? swatchClass[series] : "bg-surface-tertiary",
                )}
              />
              {stat.value}
            </span>
            <span className="max-w-full truncate text-label-secondary type-callout">
              {KEY_CAPTIONS[stat.label] ?? stat.label}
            </span>
          </button>
        )
      })}
    </div>
  )
}

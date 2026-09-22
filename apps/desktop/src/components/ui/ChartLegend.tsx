import type { ReactNode } from "react"

import { cn } from "../../lib/cn"

export interface ChartLegendItem {
  key: string
  label: ReactNode
  /** Tailwind classes that color the swatch, for example `bg-token-in/55`. */
  swatch: string
  /** `line` draws a short stroke instead of a square, for a series drawn as a line. */
  shape?: "square" | "line"
}

/**
 * The key for a chart: one entry per drawn layer, in the order the layers
 * stack. It sits above the plot in flow, so it never covers data.
 */
export function ChartLegend({
  items,
  ariaLabel,
  className,
}: {
  items: readonly ChartLegendItem[]
  ariaLabel: string
  className?: string
}) {
  return (
    <div
      role="list"
      aria-label={ariaLabel}
      className={cn(
        "type-caption flex flex-wrap gap-x-(--space-md) gap-y-(--space-xs) text-label-secondary",
        className,
      )}
    >
      {items.map((item) => (
        <span
          key={item.key}
          role="listitem"
          className="inline-flex items-center gap-(--space-xs)"
        >
          <span
            aria-hidden="true"
            className={cn(
              item.shape === "line" ? "h-0.5 w-2.5 rounded-full" : "size-2 rounded-small",
              item.swatch,
            )}
          />
          {item.label}
        </span>
      ))}
    </div>
  )
}

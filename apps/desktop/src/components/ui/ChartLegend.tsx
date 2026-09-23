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
 *
 * A chart that can bring one layer forward passes `onActiveChange`. The key
 * then reports the entry under the pointer, and fades the other entries while
 * `activeKey` is set.
 */
export function ChartLegend({
  items,
  ariaLabel,
  className,
  size = "small",
  activeKey = null,
  onActiveChange,
}: {
  items: readonly ChartLegendItem[]
  ariaLabel: string
  className?: string
  /** `large` is for a key that sits alone under a chart that fills the view. */
  size?: "small" | "large"
  activeKey?: string | null
  onActiveChange?: (key: string | null) => void
}) {
  const large = size === "large"
  return (
    <div
      role="list"
      aria-label={ariaLabel}
      className={cn(
        "flex flex-wrap gap-x-(--space-md) gap-y-(--space-xs) text-label-secondary",
        large ? "type-body" : "type-caption",
        className,
      )}
    >
      {items.map((item) => (
        <span
          key={item.key}
          role="listitem"
          className={cn(
            "inline-flex items-center gap-(--space-xs)",
            onActiveChange && "transition-opacity duration-(--duration-fast)",
            activeKey && activeKey !== item.key && "opacity-40",
          )}
          onPointerEnter={onActiveChange && (() => onActiveChange(item.key))}
          onPointerLeave={onActiveChange && (() => onActiveChange(null))}
        >
          <span
            aria-hidden="true"
            className={cn(
              item.shape === "line"
                ? large
                  ? "h-0.75 w-3.5 rounded-full"
                  : "h-0.5 w-2.5 rounded-full"
                : large
                  ? "size-2.5 rounded-small"
                  : "size-2 rounded-small",
              item.swatch,
            )}
          />
          {item.label}
        </span>
      ))}
    </div>
  )
}

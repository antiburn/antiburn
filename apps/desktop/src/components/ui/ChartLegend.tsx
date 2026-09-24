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
 *
 * A chart that can keep one layer forward also passes `onPinnedChange`. Each
 * entry is then a toggle button: a click pins the entry, and a second click
 * releases it. The pinned entry shows as pressed.
 */
export function ChartLegend({
  items,
  ariaLabel,
  className,
  size = "small",
  activeKey = null,
  onActiveChange,
  pinnedKey = null,
  onPinnedChange,
}: {
  items: readonly ChartLegendItem[]
  ariaLabel: string
  className?: string
  /** `large` is for a key that sits alone under a chart that fills the view. */
  size?: "small" | "large"
  activeKey?: string | null
  onActiveChange?: (key: string | null) => void
  pinnedKey?: string | null
  onPinnedChange?: (key: string | null) => void
}) {
  const large = size === "large"
  const swatch = (item: ChartLegendItem) => (
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
  )
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
      {items.map((item) => {
        const pinned = pinnedKey === item.key
        return (
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
            {onPinnedChange ? (
              <button
                type="button"
                aria-pressed={pinned}
                className={cn(
                  "inline-flex items-center gap-(--space-xs) rounded-control px-(--space-xs) transition-colors duration-(--duration-fast) hover:text-label",
                  pinned && "bg-surface-secondary text-label",
                )}
                onFocus={onActiveChange && (() => onActiveChange(item.key))}
                onBlur={onActiveChange && (() => onActiveChange(null))}
                onClick={() => onPinnedChange(pinned ? null : item.key)}
              >
                {swatch(item)}
                {item.label}
              </button>
            ) : (
              <>
                {swatch(item)}
                {item.label}
              </>
            )}
          </span>
        )
      })}
    </div>
  )
}

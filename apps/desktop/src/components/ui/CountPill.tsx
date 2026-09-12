import type { ComponentPropsWithoutRef } from "react"

import { cn } from "../../lib/cn"

/** A muted mono pill for a small count, such as an extra-model tally or a
 *  nav item's item count. Shares its look with the trailing "+N" model-count
 *  pill on a session card. */
export function CountPill({
  count,
  className = "",
  prefix = "",
  ...rest
}: {
  count: number
  className?: string
  /** Text before the count, such as "+" for an overflow tally. */
  prefix?: string
} & Omit<ComponentPropsWithoutRef<"span">, "className" | "children">) {
  return (
    <span
      {...rest}
      className={cn(
        "inline-flex h-4 min-w-4 shrink-0 items-center justify-center rounded-full bg-surface-tertiary/40 px-1 font-mono type-metadata font-medium! tabular-nums text-label-tertiary",
        className,
      )}
    >
      {prefix}
      {count}
    </span>
  )
}

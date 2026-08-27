import type { ReactNode } from "react"

import { cn } from "../../lib/cn"

/** Grouped, divided, bordered container. Children are laid out as edge-to-edge
 *  rows separated by hairlines — pair with `Row` / `ToggleRow`. */
export function Card({
  children,
  className = "",
}: {
  children: ReactNode
  className?: string
}) {
  return (
    <div
      className={cn(
        "divide-y divide-separator overflow-hidden rounded-popover border border-separator bg-surface-card/60",
        className,
      )}
    >
      {children}
    </div>
  )
}

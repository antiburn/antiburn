import type { ReactNode } from "react"

import { cn } from "../../lib/cn"

/** Grouped rows in one bordered surface. Set divided false to use spacing alone. */
export function Card({
  children,
  className = "",
  divided = true,
}: {
  children: ReactNode
  className?: string
  divided?: boolean
}) {
  return (
    <div
      className={cn(
        "overflow-hidden rounded-popover border border-separator bg-surface-card/60",
        divided && "divide-y divide-separator",
        className,
      )}
    >
      {children}
    </div>
  )
}

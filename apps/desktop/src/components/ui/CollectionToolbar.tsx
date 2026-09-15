import type { ReactNode } from "react"
import { cn } from "../../lib/cn"

export function CollectionToolbar({
  children,
  className,
  dragRegion = false,
}: {
  children: ReactNode
  className?: string
  dragRegion?: boolean
}) {
  return (
    <div className="shrink-0 pt-2" data-tauri-drag-region={dragRegion ? "deep" : undefined}>
      <div
        data-list-display-toolbar=""
        data-tauri-drag-region={dragRegion ? "deep" : undefined}
        className={cn("mb-1 flex h-8 shrink-0 items-center px-3", className)}
      >
        {children}
      </div>
    </div>
  )
}

import type { ReactNode } from "react"
import { cn } from "../../lib/cn"

export function CollectionToolbar({
  children,
  className,
  dragRegion = false,
  topPadding = "default",
}: {
  children: ReactNode
  className?: string
  dragRegion?: boolean
  /** Use the fixed spacing token when this toolbar follows a contextual header. */
  topPadding?: "default" | "space-sm"
}) {
  return (
    <div
      className={cn("shrink-0", topPadding === "space-sm" ? "pt-[var(--space-sm)]" : "pt-2")}
      data-tauri-drag-region={dragRegion ? "deep" : undefined}
    >
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

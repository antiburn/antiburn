import type { ReactNode } from "react"

export function CollectionHeader({
  title,
  summary,
  actions,
  children,
  dragRegion = false,
}: {
  title: string
  summary?: ReactNode
  actions?: ReactNode
  children?: ReactNode
  dragRegion?: boolean
}) {
  return (
    <header
      className="shrink-0 px-3 pt-3"
      data-collection-header=""
      data-tauri-drag-region={dragRegion ? "deep" : undefined}
    >
      <div className="flex min-h-8 items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <h2 className="shrink-0 type-headline text-label">{title}</h2>
          {summary}
        </div>
        {actions}
      </div>
      {children}
      <div
        aria-hidden="true"
        className="pointer-events-none mt-[var(--space-sm)] h-px bg-separator/50"
      />
    </header>
  )
}

import type { HTMLAttributes, ReactNode } from "react"

import { cn } from "../../lib/cn"

export interface SkeletonProps extends HTMLAttributes<HTMLSpanElement> {
  /** When false the children render untouched, so a call site can wrap its
   *  real content once and flip a flag instead of branching in JSX. */
  loading?: boolean
  children?: ReactNode
}

/**
 * A pulsing placeholder for content that has not arrived yet.
 *
 * Two shapes, chosen by whether children are present:
 *
 * - **Wrapping** — the placeholder inherits the size of the content it hides,
 *   so nothing shifts when the real value lands. The children stay in the DOM
 *   purely as a sizer: every descendant is made invisible, the text color is
 *   made transparent, and the whole subtree is taken out of the accessibility
 *   tree, out of the tab order, out of hit-testing, and out of selection.
 * - **Standalone** — no children, size it with the caller's own classes
 *   (`h-3 w-32`).
 *
 * Placeholders are announced to nobody on purpose. A screen-reader user gains
 * nothing from "loading" repeated once per line; the surface that owns the
 * fetch is the right place for a single live-region message.
 */
export function Skeleton({ loading = true, children, className = "", ...rest }: SkeletonProps) {
  if (!loading) {
    return <>{children}</>
  }

  const wrapping = children !== undefined && children !== null && children !== false
  const shape = wrapping ? "inline-block text-transparent [&_*]:invisible" : "block min-h-[1ex]"

  return (
    <span
      {...rest}
      aria-hidden="true"
      tabIndex={-1}
      data-placeholder=""
      className={cn(
        "pointer-events-none animate-pulse select-none rounded-small bg-surface-tertiary",
        shape,
        className,
      )}
    >
      {children}
    </span>
  )
}

export interface SkeletonCardProps {
  /** Reserve a small square ahead of the lines, for a leading control or icon. */
  leading?: boolean
  /** One width class per placeholder line, top to bottom. */
  lines?: string[]
  className?: string
}

/**
 * A whole `Card`'s worth of placeholder: the same shell geometry as `Card`, so
 * a list swapping from placeholders to real rows keeps its rhythm.
 */
export function SkeletonCard({
  leading = false,
  lines = ["w-32", "w-44"],
  className = "",
}: SkeletonCardProps) {
  return (
    <div
      className={cn(
        "overflow-hidden rounded-popover border border-separator bg-surface-card/60 px-4 py-3",
        className,
      )}
    >
      <div className="flex items-start gap-2">
        {leading && <Skeleton className="mt-0.5 h-[15px] w-[15px] shrink-0" />}
        <div className="min-w-0 flex-1 space-y-1.5">
          {lines.map((width, index) => (
            <Skeleton key={`${width}-${index}`} className={cn("h-3", width)} />
          ))}
        </div>
      </div>
    </div>
  )
}

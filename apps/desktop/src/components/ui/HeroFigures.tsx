import type { ReactNode } from "react"

import { cn } from "../../lib/cn"

import "./hero-figures.css"

export interface HeroFigureCell {
  key: string
  /** The label above the figure. */
  label: ReactNode
  /** The headline figure. Wrap a number in `SegmentFigure`. */
  figure: ReactNode
  /** A short line under the figure. The cell omits the line when absent. */
  caption?: ReactNode
}

/**
 * A row of headline figures, one cell each, with a hairline between cells.
 * The Overview's spend totals and the Limits header share this so the two
 * screens read as one family. The caller owns the `section` and any loading
 * state around the row.
 */
export function HeroFigures({
  cells,
  className,
}: {
  cells: readonly HeroFigureCell[]
  className?: string
}) {
  return (
    <dl className={cn("hero-figures", className)}>
      {cells.map((cell) => (
        <div key={cell.key} className="hero-figures-cell min-w-0 border-separator">
          <dt className="type-callout text-label-secondary">{cell.label}</dt>
          <dd className="type-hero-figure mt-[var(--space-xs)] whitespace-nowrap font-mono text-measure">
            {cell.figure}
          </dd>
          {cell.caption != null && (
            <dd className="type-caption mt-[var(--space-xs)] whitespace-nowrap text-label-tertiary">
              {cell.caption}
            </dd>
          )}
        </div>
      ))}
    </dl>
  )
}

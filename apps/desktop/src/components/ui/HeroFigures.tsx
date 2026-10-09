import type { KeyboardEvent, ReactNode } from "react"

import { cn } from "../../lib/cn"

import { Tooltip } from "../presentation/Tooltip"

export interface HeroFigureCell {
  key: string
  /** The label above the figure. */
  label: ReactNode
  /** The headline figure. Wrap a number in `SegmentFigure`. */
  figure: ReactNode
  /** A short line under the figure. The cell omits the line when absent. */
  caption?: ReactNode
  /** How the figure is made, for a reader who doubts it. The cell takes
   *  focus so a keyboard reaches the tooltip too. */
  tooltip?: string
  /** Set with `onSelect` to make the cell a toggle. A cell with `selected`
   *  false dims; leave it unset to show every cell at full strength. */
  selected?: boolean
  onSelect?: () => void
}

/**
 * A row of headline figures, one cell each. The Overview's spend and
 * subscription totals and the Limits header share this so the screens read
 * as one family. The caller owns the `section` and any loading state around
 * the row; a loading cell passes `Skeleton` placeholders as its figure and
 * caption.
 *
 * The grid flows by column, so the cell count sets the column count and
 * every cell gets the same share of the row. Below 540px of container width
 * the cells stack. A cell after the first takes a rule on the side that
 * faces the one before it — left in the row, top in the stack — and stands
 * off it by the same space.
 *
 * A `dl`, so each cell's label is the term for its figure.
 */
export function HeroFigures({
  cells,
  className,
}: {
  cells: readonly HeroFigureCell[]
  className?: string
}) {
  return (
    <dl
      className={cn(
        "grid grid-flow-col auto-cols-[minmax(0,1fr)] gap-(--space-lg) @max-[540px]:grid-flow-row",
        className,
      )}
    >
      {cells.map((cell) => {
        // The tooltip clones its props onto this div, so with or without one
        // the cell is a direct child of the grid and the sibling rules hold.
        const selectable = cell.onSelect != null
        const body = (
          <div
            key={cell.key}
            className={cn(
              "min-w-0 border-separator not-first:border-l not-first:ps-(--space-lg) @max-[540px]:not-first:border-t @max-[540px]:not-first:border-l-0 @max-[540px]:not-first:ps-0 @max-[540px]:not-first:pt-(--space-md)",
              selectable && "cursor-pointer transition-opacity duration-fast",
              selectable && cell.selected === false && "opacity-40 hover:opacity-70",
              selectable && cell.selected !== false && "hover:opacity-80",
            )}
            tabIndex={cell.tooltip || selectable ? 0 : undefined}
            {...(selectable
              ? {
                  role: "button",
                  "aria-pressed": cell.selected ?? false,
                  onClick: cell.onSelect,
                  onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault()
                      cell.onSelect?.()
                    }
                  },
                }
              : {})}
          >
            <dt className="type-callout text-label-secondary">{cell.label}</dt>
            <dd className="type-hero-figure whitespace-nowrap font-mono text-label/80">
              {cell.figure}
            </dd>
            {cell.caption != null && (
              <dd className="type-caption whitespace-nowrap text-label-tertiary">
                {cell.caption}
              </dd>
            )}
          </div>
        )
        return cell.tooltip ? (
          <Tooltip key={cell.key} label={cell.tooltip}>
            {body}
          </Tooltip>
        ) : (
          body
        )
      })}
    </dl>
  )
}

import { Info } from "lucide-react"

import { cn } from "../../../lib/cn"
import { Tooltip } from "../../presentation/Tooltip"

export interface RowInfoProps {
  /** The name of the row this button explains. It names the button too. */
  label: string
  /** The explanation, as the body of the tooltip. */
  body: string
  className?: string
}

/**
 * The info button on one row of a breakdown. It stays invisible until the
 * pointer enters the row, or the button takes focus, and it opens the row's
 * explanation in a tooltip.
 *
 * The button is the affordance the surface needed. An explainer that appeared
 * somewhere else on hover left no trace of what caused it, so a reader could
 * not find it again. The button also keeps every row at one height, which an
 * accordion does not.
 *
 * The parent row must carry the `group` class, so the button follows that
 * row's hover state.
 */
export function RowInfo({ label, body, className }: RowInfoProps) {
  return (
    <Tooltip label={body} side="top" interactive delayMs={150}>
      <button
        type="button"
        aria-label={`About ${label}`}
        className={cn(
          "shrink-0 leading-none text-label-tertiary opacity-0 transition-[color,opacity] duration-[var(--duration-fast)] ease-out group-hover:opacity-100 hover:text-label-secondary focus-visible:opacity-100",
          className,
        )}
      >
        <Info size={12} aria-hidden="true" />
      </button>
    </Tooltip>
  )
}

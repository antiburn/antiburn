import { Flame } from "lucide-react"

import { cn } from "../../../lib/cn"
import {
  formatCost,
  HIGH_COST_MEDIAN_MULTIPLE,
  type CostRow,
} from "../../../lib/presentation/sessionAnalysis"
import { Tooltip } from "../../presentation/Tooltip"
import { TextRoll } from "../../ui/TextRoll"

const COST_PILL_CLASS =
  "flex items-center gap-0.5 shrink-0 px-1.5 py-px rounded-full type-caption tabular-nums font-medium leading-[13px]"

export interface SessionCostBadgeProps {
  /** The headline figure, in USD. */
  totalUsd: number
  /**
   * What the figure *is* — "Projected cost" while a session is live,
   * "Estimated cost" once it settles. Leads the tooltip and carries the
   * accessible name.
   */
  figureLabel: string
  /** Every model that contributed billable tokens, as a muted subtitle. */
  models?: string[]
  /** Unusually expensive against comparable sessions. Paints the pill solid brand orange with a flame. */
  isHighCost?: boolean
  /** Billable component rows (input / output / cache read / cache write). */
  breakdownRows?: CostRow[]
  /**
   * Extra classes for the pill — a `relative top-px` nudge to optically align
   * with adjacent baseline text in a row, say. Omit on a centered flex line.
   */
  className?: string
}

/**
 * The cost pill and its tooltip, driven entirely by the values it is given.
 *
 * Every figure is an on-device estimate from the model's per-token rates. The
 * pill leads with the number because that is the answer the reader came for;
 * the component rows underneath explain how it was reached.
 */
export function SessionCostBadge({
  totalUsd,
  figureLabel,
  models = [],
  isHighCost = false,
  breakdownRows = [],
  className = "",
}: SessionCostBadgeProps) {
  return (
    <Tooltip
      // Wide card: drop it below the pill rather than to the side, where it
      // can run off the window. Radix collision handling then shifts it
      // horizontally to stay in view.
      side="bottom"
      delayMs={150}
      label={
        <div className="space-y-1.5 text-left">
          <div>
            <div className="flex justify-between gap-4 type-caption font-medium text-label">
              <span>{figureLabel}</span>
              <span className="tabular-nums">{formatCost(totalUsd)}</span>
            </div>
            {models.length > 0 && (
              <div className="type-caption text-label-tertiary">
                {models.map((model) => (
                  <div key={model}>{model}</div>
                ))}
              </div>
            )}
            {isHighCost && (
              <div className="type-caption font-medium text-brand">
                Higher than usual — over {HIGH_COST_MEDIAN_MULTIPLE}× your typical session
              </div>
            )}
          </div>
          {breakdownRows.length > 0 && (
            <div className="space-y-0.5 border-t border-separator pt-1.5">
              {breakdownRows.map((row) => (
                <div key={row.label} className="flex justify-between gap-4 type-caption">
                  <span className="text-label-tertiary">{row.label}</span>
                  <span className="tabular-nums">{formatCost(row.usd)}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      }
    >
      <span
        // High-cost status is also a solid orange fill plus a flame, but color
        // and glyph alone fail WCAG 1.4.1 and the flame is aria-hidden — so
        // the accessible name also carries the "higher than usual" meaning.
        aria-label={
          isHighCost
            ? `${figureLabel} ${formatCost(totalUsd)}, higher than usual`
            : `${figureLabel} ${formatCost(totalUsd)}`
        }
        className={cn(
          COST_PILL_CLASS,
          isHighCost ? "bg-brand-tint text-white" : "bg-label-tertiary/15 text-label-secondary",
          className,
        )}
      >
        {isHighCost && <Flame size={11} className="shrink-0" aria-hidden="true" />}
        <TextRoll text={formatCost(totalUsd)} />
      </span>
    </Tooltip>
  )
}

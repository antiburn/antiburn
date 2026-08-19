// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Flame } from "lucide-react"

import {
  formatCost,
  HIGH_COST_MEDIAN_MULTIPLE,
  type CostRow,
} from "../../../lib/presentation/sessionAnalytics"
import { Tooltip } from "../../presentation/Tooltip"
import { TextRoll } from "../../ui/TextRoll"

/**
 * Geometry and typography of the cost pill, without its color scheme. Exported
 * so every surface that paints one stays in step through this constant rather
 * than by copying the string.
 */
export const COST_PILL_CLASS =
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
  /**
   * Unusually expensive against comparable sessions. Paints the pill red with
   * a flame, and is never hidden behind {@link revealClassName}.
   */
  isHighCost?: boolean
  /** Billable component rows (input / output / cache read / cache write). */
  breakdownRows?: CostRow[]
  /**
   * Extra classes for the pill — a `relative top-px` nudge to optically align
   * with adjacent baseline text in a row, say. Omit on a centered flex line.
   */
  className?: string
  /**
   * Hover-reveal classes applied by a list row that keeps secondary detail
   * hidden until hover or focus. A **high-cost** badge ignores this and stays
   * visible: an outlier must be surfaced even at rest.
   */
  revealClassName?: string
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
  revealClassName = "",
}: SessionCostBadgeProps) {
  const badge = (
    <Tooltip
      // Wide card: drop it below the pill rather than to the side, where it
      // can run off the window. Radix collision handling then shifts it
      // horizontally to stay in view.
      side="bottom"
      delayMs={150}
      label={
        <div className="space-y-1.5 text-left">
          <div>
            <div className="flex justify-between gap-4 type-caption font-medium text-system-gold-text">
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
              <div className="type-caption font-medium text-system-red-text">
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
        // High-cost status is also red plus a flame, but color and glyph alone
        // fail WCAG 1.4.1 and the flame is aria-hidden — so carry the "higher
        // than usual" meaning in the accessible name too.
        aria-label={
          isHighCost
            ? `${figureLabel} ${formatCost(totalUsd)}, higher than usual`
            : `${figureLabel} ${formatCost(totalUsd)}`
        }
        className={`${COST_PILL_CLASS} ${
          isHighCost
            ? "bg-system-red/15 text-system-red-text"
            : "bg-system-gold/15 text-system-gold-text"
        }${className ? ` ${className}` : ""}`}
      >
        {isHighCost && <Flame size={11} className="shrink-0" aria-hidden="true" />}
        <TextRoll text={formatCost(totalUsd)} />
      </span>
    </Tooltip>
  )

  // In a list row the badge hides until hover, to keep the resting list
  // uncluttered — but a high-cost outlier must always be surfaced, so it skips
  // the reveal wrapper. The wrapper (not the bare pill) carries the classes
  // because `:has([data-state*=open])` in them keeps the badge up while its own
  // tooltip is open, and the trigger sets that attribute on the pill, which has
  // to be a *descendant* for `:has` to match.
  if (revealClassName && !isHighCost) {
    return <span className={`flex shrink-0 ${revealClassName}`}>{badge}</span>
  }
  return badge
}

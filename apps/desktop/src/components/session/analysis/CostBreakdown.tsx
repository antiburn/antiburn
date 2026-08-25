// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import {
  costBreakdownRows,
  costFigureLabel,
  formatCost,
  formatTokensShort,
} from "../../../lib/presentation/sessionAnalysis"
import {
  resultComponentCost,
  type LocalSessionCost,
} from "../../../lib/presentation/sessionCosts"

interface CostBreakdownSplit {
  /** The orchestrator's own transcript. */
  parent: LocalSessionCost
  /** Every sub-agent it launched, together. */
  subagents: LocalSessionCost
  subagentCount: number
}

export interface CostBreakdownProps {
  /**
   * The selected result. Its headline and every component row describe one
   * subject, so the rows always sum to the total shown.
   */
  cost: LocalSessionCost
  /**
   * Parent/sub-agents split, for an inclusive orchestration result. Omit it
   * for any other subject, where there is nothing to break apart.
   */
  split?: CostBreakdownSplit | null
}

/**
 * Grid columns shared by every row: the label takes the rest of the width,
 * then tokens, USD, and percent each hold a fixed track. Fixed tracks (not
 * `auto`) keep every row's columns at the same screen position, since an
 * `auto` track's width would otherwise follow that one row's own content.
 */
const ROW_GRID = "grid grid-cols-[1fr_2.5rem_3.5rem_2.25rem] items-center gap-x-3"

const DATA_ROW_CLASS = `${ROW_GRID} rounded-control -mx-1 px-1 type-caption transition-colors duration-[var(--duration-fast)] ease-out hover:bg-surface-hover`

/**
 * Percent of `totalUsd` that `usd` accounts for, as a whole percent. A
 * positive share under half a percent reads `"<1%"` rather than rounding away
 * to `"0%"`. `"—"` stands in when the total itself is zero, where a percent
 * is undefined.
 */
function formatSharePct(usd: number, totalUsd: number): string {
  if (!(totalUsd > 0)) return "—"
  const pct = (usd / totalUsd) * 100
  if (pct <= 0) return "0%"
  if (pct < 0.5) return "<1%"
  return `${Math.round(pct)}%`
}

/** One hoverable row: a label, its token count, its USD cost, and its share of the total. */
function CostRowLine({
  label,
  usd,
  tokens,
  totalUsd,
}: {
  label: string
  usd: number
  tokens: number
  totalUsd: number
}) {
  return (
    <div className={DATA_ROW_CLASS}>
      <span className="text-label-tertiary">{label}</span>
      <span className="text-right text-label-tertiary tabular-nums">
        {formatTokensShort(tokens)}
      </span>
      <span className="pr-1.5 text-right text-label tabular-nums">{formatCost(usd)}</span>
      <span className="text-right text-label-tertiary tabular-nums">
        {formatSharePct(usd, totalUsd)}
      </span>
    </div>
  )
}

/**
 * The billable-component breakdown beneath the tokens chart.
 *
 * Every figure here is the on-device estimate for one exact subject: a split
 * only appears when the caller has parent and sub-agent results drawn from the
 * same computation as the headline, because mixing sources would produce rows
 * that do not add up. The token and percent columns share that same subject,
 * so every row's percent reads as a share of the one total shown in the footer.
 */
export function CostBreakdown({ cost, split }: CostBreakdownProps) {
  const rows = costBreakdownRows(resultComponentCost(cost))
  const totalUsd = cost.totalCostUsd

  return (
    <div className="space-y-1 border-t border-separator pt-2">
      {split && (
        <div className="mb-1 space-y-1 border-b border-separator pb-1">
          <CostRowLine
            label="Parent agent"
            usd={split.parent.totalCostUsd}
            tokens={split.parent.totalTokens}
            totalUsd={totalUsd}
          />
          <CostRowLine
            label={`${split.subagentCount} sub-agent${split.subagentCount === 1 ? "" : "s"}`}
            usd={split.subagents.totalCostUsd}
            tokens={split.subagents.totalTokens}
            totalUsd={totalUsd}
          />
        </div>
      )}
      {rows.map((row) => (
        <CostRowLine
          key={row.label}
          label={row.label}
          usd={row.usd}
          tokens={row.tokens ?? 0}
          totalUsd={totalUsd}
        />
      ))}
      <div className={`${ROW_GRID} mt-1 border-t border-separator pt-1 type-caption`}>
        <span className="text-label-tertiary">{costFigureLabel(cost.isActive)}</span>
        <span className="text-right text-label-tertiary tabular-nums">
          {formatTokensShort(cost.totalTokens)}
        </span>
        <span className="flex justify-end">
          <span className="flex shrink-0 items-center rounded-full bg-system-gold/15 px-1.5 py-px type-caption font-medium leading-[13px] text-system-gold-text tabular-nums">
            {formatCost(cost.totalCostUsd)}
          </span>
        </span>
        <span className="text-right text-label-tertiary tabular-nums">
          {formatSharePct(cost.totalCostUsd, totalUsd)}
        </span>
      </div>
    </div>
  )
}

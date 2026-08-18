// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import {
  costBreakdownRows,
  costFigureLabel,
  formatCost,
} from '../../../lib/presentation/sessionAnalytics';
import {
  resultComponentCost,
  type LocalSessionCost,
} from '../../../lib/presentation/sessionCosts';

interface CostBreakdownSplit {
  /** The orchestrator's own transcript. */
  parent: LocalSessionCost;
  /** Every sub-agent it launched, together. */
  subagents: LocalSessionCost;
  subagentCount: number;
}

export interface CostBreakdownProps {
  /**
   * The selected result. Its headline and every component row describe one
   * subject, so the rows always sum to the total shown.
   */
  cost: LocalSessionCost;
  /**
   * Parent/sub-agents split, for an inclusive orchestration result. Omit it
   * for any other subject, where there is nothing to break apart.
   */
  split?: CostBreakdownSplit | null;
}

/**
 * The billable-component breakdown beneath the tokens chart.
 *
 * Every figure here is the on-device estimate for one exact subject: a split
 * only appears when the caller has parent and sub-agent results drawn from the
 * same computation as the headline, because mixing sources would produce rows
 * that do not add up.
 */
export function CostBreakdown({ cost, split }: CostBreakdownProps) {
  const rows = costBreakdownRows(resultComponentCost(cost));

  return (
    <div className="mt-2 space-y-1 border-t border-separator pt-2">
      {split && (
        <div className="mb-1 space-y-1 border-b border-separator pb-1">
          <div className="flex items-center justify-between type-caption">
            <span className="text-label-tertiary">Parent agent</span>
            <span className="pr-1.5 text-label tabular-nums">
              {formatCost(split.parent.totalCostUsd)}
            </span>
          </div>
          <div className="flex items-center justify-between type-caption">
            <span className="text-label-tertiary">
              {split.subagentCount} sub-agent{split.subagentCount === 1 ? '' : 's'}
            </span>
            <span className="pr-1.5 text-label tabular-nums">
              {formatCost(split.subagents.totalCostUsd)}
            </span>
          </div>
        </div>
      )}
      {rows.map((row) => (
        <div key={row.label} className="flex items-center justify-between type-caption">
          <span className="text-label-tertiary">{row.label}</span>
          <span className="pr-1.5 text-label tabular-nums">{formatCost(row.usd)}</span>
        </div>
      ))}
      <div className="mt-1 flex items-center justify-between border-t border-separator pt-1 type-caption">
        <span className="text-label-tertiary">{costFigureLabel(cost.isActive)}</span>
        <span className="flex shrink-0 items-center rounded-full bg-system-gold/15 px-1.5 py-px type-caption font-medium leading-[13px] text-system-gold-text tabular-nums">
          {formatCost(cost.totalCostUsd)}
        </span>
      </div>
    </div>
  );
}

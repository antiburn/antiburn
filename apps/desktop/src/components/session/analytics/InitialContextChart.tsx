// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Info } from "lucide-react"
import { Cell, Pie, PieChart, ResponsiveContainer } from "recharts"

import {
  formatCompact,
  initialContextNamedRows,
  initialContextSlices,
  initialContextTotal,
} from "../../../lib/presentation/sessionAnalytics"
import type { InitialContextBreakdown } from "../../../lib/types/session"
import { Tooltip } from "../../presentation/Tooltip"

export interface InitialContextChartProps {
  breakdown: InitialContextBreakdown
}

/**
 * Where the context window went *before* the first response, by source, with a
 * named drill-down of the heaviest contributors underneath.
 */
export function InitialContextChart({ breakdown }: InitialContextChartProps) {
  const slices = initialContextSlices(breakdown)
  const sliceTotal = slices.reduce((acc, s) => acc + s.value, 0)
  // Never let the center read smaller than its own slices when estimates
  // overshoot the reported total.
  const total = initialContextTotal(breakdown)
  const named = initialContextNamedRows(breakdown)

  if (sliceTotal === 0) {
    return <p className="type-footnote text-label-tertiary">No initial context to attribute.</p>
  }

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-3">
        <div className="relative shrink-0" style={{ width: 96, height: 96 }}>
          <ResponsiveContainer width="100%" height="100%">
            <PieChart>
              <Pie
                data={slices}
                dataKey="value"
                nameKey="label"
                innerRadius={30}
                outerRadius={46}
                stroke="none"
                paddingAngle={2}
                isAnimationActive={false}
              >
                {slices.map((slice) => (
                  <Cell key={slice.key} fill={slice.colorVar} />
                ))}
              </Pie>
            </PieChart>
          </ResponsiveContainer>
          <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center">
            <span className="type-headline leading-none text-label">
              {formatCompact(total)}
            </span>
            <span className="type-caption text-label-tertiary">tokens</span>
          </div>
        </div>

        <div className="flex min-w-0 flex-1 flex-col gap-1">
          {slices.map((slice) => (
            <div key={slice.key} className="flex min-w-0 items-center gap-1.5">
              <span
                className="h-2 w-2 shrink-0 rounded-sm"
                style={{ backgroundColor: slice.colorVar }}
              />
              <span className="type-caption text-label-secondary">{slice.label}</span>
              <Tooltip label={slice.tip} side="top" interactive delayMs={150}>
                <button
                  type="button"
                  aria-label={`About ${slice.label}`}
                  className="shrink-0 leading-none text-label-tertiary transition-colors hover:text-label-secondary"
                >
                  <Info size={11} aria-hidden="true" />
                </button>
              </Tooltip>
              <span className="ml-auto type-caption text-label-tertiary tabular-nums">
                {formatCompact(slice.value)}
              </span>
            </div>
          ))}
        </div>
      </div>

      {named.length > 0 && (
        <div className="space-y-1 border-t border-separator pt-2">
          {named.map((row) => (
            <div key={row.key} className="flex min-w-0 items-center gap-1.5">
              <span
                className="h-2 w-2 shrink-0 rounded-sm"
                style={{ backgroundColor: row.colorVar }}
              />
              <span className="truncate type-caption text-label-secondary">{row.label}</span>
              <span className="ml-auto type-caption text-label-tertiary tabular-nums">
                {formatCompact(row.tokenCount)}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  )
}

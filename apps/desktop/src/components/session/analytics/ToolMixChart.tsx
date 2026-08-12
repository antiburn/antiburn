// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Cell, Pie, PieChart, ResponsiveContainer } from 'recharts';

import { toolMixSlices } from '../../../lib/presentation/sessionAnalytics';
import type { ActiveSessionsSummary } from '../../../lib/types/session';

export interface ToolMixChartProps {
  summary: ActiveSessionsSummary;
}

/** Tool-usage donut with a legend and a search callout. */
export function ToolMixChart({ summary }: ToolMixChartProps) {
  const slices = toolMixSlices(summary);
  const total = slices.reduce((acc, s) => acc + s.value, 0);

  if (total === 0) {
    return <p className="type-footnote text-label-tertiary">No tool calls yet.</p>;
  }

  return (
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
          <span className="type-headline leading-none text-label">{total}</span>
          <span className="type-caption text-label-tertiary">tools</span>
        </div>
      </div>

      <div className="grid min-w-0 flex-1 grid-cols-2 gap-x-3 gap-y-1">
        {slices.map((slice) => (
          <div key={slice.key} className="flex min-w-0 items-center gap-1.5">
            <span
              className="h-2 w-2 shrink-0 rounded-sm"
              style={{ backgroundColor: slice.colorVar }}
            />
            <span className="truncate type-caption text-label-secondary">{slice.label}</span>
            <span className="ml-auto type-caption text-label-tertiary tabular-nums">
              {slice.value}
            </span>
          </div>
        ))}
        {summary.grepTotal > 0 && (
          <div className="col-span-2 mt-0.5 type-caption text-label-tertiary">
            {summary.grepTotal} search{summary.grepTotal === 1 ? '' : 'es'} (grep/glob)
          </div>
        )}
      </div>
    </div>
  );
}

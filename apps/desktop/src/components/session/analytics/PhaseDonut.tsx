// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { Cell, Pie, PieChart, ResponsiveContainer } from 'recharts';

import { formatDuration, phaseBreakdown } from '../../../lib/presentation/sessionAnalytics';
import type { ActiveSessionsSummary, SessionPhase } from '../../../lib/types/session';

export interface PhaseDonutProps {
  summary: ActiveSessionsSummary;
  /** Phase currently hovered, in the donut or the rows, for cross-highlight. */
  activePhase?: SessionPhase | null;
  onHoverPhase?: (phase: SessionPhase | null) => void;
}

/** A ring of the session modes with the active time in the center. */
export function PhaseDonut({ summary, activePhase, onHoverPhase }: PhaseDonutProps) {
  const rows = phaseBreakdown(summary).filter((r) => r.fraction > 0);

  return (
    <div className="relative" style={{ height: 150 }}>
      <ResponsiveContainer width="100%" height="100%">
        <PieChart>
          <Pie
            data={rows}
            dataKey="fraction"
            nameKey="label"
            innerRadius={48}
            outerRadius={66}
            startAngle={90}
            endAngle={-270}
            stroke="none"
            paddingAngle={2}
            isAnimationActive={false}
            onMouseEnter={(_, index: number) => onHoverPhase?.(rows[index]?.key ?? null)}
            onMouseLeave={() => onHoverPhase?.(null)}
          >
            {rows.map((row) => (
              <Cell
                key={row.key}
                fill={row.colorVar}
                opacity={activePhase && activePhase !== row.key ? 0.75 : 1}
                style={{ transition: 'opacity 120ms' }}
              />
            ))}
          </Pie>
        </PieChart>
      </ResponsiveContainer>
      <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center">
        <span className="type-title-2 leading-none text-label">
          {formatDuration(summary.avgActiveSecs)}
        </span>
        <span className="mt-0.5 type-caption text-label-tertiary">active time</span>
      </div>
    </div>
  );
}

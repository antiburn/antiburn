// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useId, type ReactElement } from 'react';
import {
  Area,
  AreaChart,
  ReferenceLine,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from 'recharts';

import { contextSeries } from '../../../lib/presentation/sessionAnalytics';
import type { SessionBucket } from '../../../lib/types/session';
import { GLASS_TOOLTIP_STYLE } from './tooltip';

export interface ContextDriftChartProps {
  buckets: SessionBucket[];
  contextWindow: number;
}

/** Occupancy percentage at which the chart starts warning. */
const CONTEXT_THRESHOLD = 90;

/** Context-window occupancy over progress, marked at the warning threshold. */
export function ContextDriftChart({ buckets, contextWindow }: ContextDriftChartProps) {
  const data = contextSeries(buckets, contextWindow);
  const compactionPoints = data.filter((d) => d.isCompactionBoundary);
  const fillId = `context-fill-${useId().replace(/:/g, '')}`;

  // The fill gradient is an SVG `objectBoundingBox` gradient, so its [0,1]
  // offsets map over the *area path's* bounding box — which spans 0..peak, not
  // the fixed [0,100] Y-domain. The warning break must therefore be
  // peak-relative to land exactly on the dashed threshold line: offset f sits
  // at data value peak·(1−f), so the line is at (peak−threshold)/peak from the
  // top. A constant domain-relative split only happens to align when the peak
  // is exactly 100. Null when the session never crosses the threshold, so the
  // whole area stays calm.
  const peak = data.reduce((m, d) => Math.max(m, d.contextPct), 0);
  const thresholdSplit = peak > CONTEXT_THRESHOLD ? (peak - CONTEXT_THRESHOLD) / peak : null;

  const stops: ReactElement[] = [];
  if (thresholdSplit != null) {
    stops.push(
      <stop
        key="drift-top"
        offset={0}
        stopColor="var(--color-pattern-drifting)"
        stopOpacity={0.55}
      />,
      <stop
        key="drift-edge"
        offset={thresholdSplit}
        stopColor="var(--color-pattern-drifting)"
        stopOpacity={0.3}
      />,
    );
  }
  stops.push(
    <stop
      key="healthy-edge"
      offset={thresholdSplit ?? 0}
      stopColor="var(--color-context-fill-top)"
    />,
    <stop key="healthy-base" offset={1} stopColor="var(--color-context-fill-base)" />,
  );

  return (
    <ResponsiveContainer width="100%" height={110}>
      <AreaChart data={data} margin={{ top: 4, right: 4, bottom: 0, left: 0 }}>
        <defs>
          <linearGradient id={fillId} x1={0} y1={0} x2={0} y2={1}>
            {stops}
          </linearGradient>
        </defs>
        <XAxis dataKey="progress" hide />
        <YAxis hide domain={[0, 100]} />
        <ReferenceLine
          y={CONTEXT_THRESHOLD}
          stroke="var(--color-pattern-drifting)"
          strokeDasharray="3 3"
          strokeOpacity={0.7}
        />
        {compactionPoints.map((point, i) => (
          <ReferenceLine
            // `progress` is a rounded percentage, so two compactions can land
            // on the same value in a long session; the index keeps sibling
            // keys unique regardless.
            key={`compaction-${i}-${point.progress}`}
            x={point.progress}
            stroke="var(--color-label-tertiary)"
            strokeDasharray="2 2"
            strokeOpacity={0.8}
            label={{
              value: '⊥ Compaction',
              position: 'insideTopLeft',
              fill: 'var(--color-label-tertiary)',
              fontSize: 9,
            }}
          />
        ))}
        <Tooltip
          cursor={{ stroke: 'var(--color-separator)' }}
          contentStyle={GLASS_TOOLTIP_STYLE}
          labelFormatter={(progress) => `${progress}% through`}
          formatter={(value) => [`${Number(value)}%`, 'Context']}
        />
        <Area
          type="monotone"
          dataKey="contextPct"
          stroke="var(--color-accent)"
          strokeWidth={1.5}
          fill={`url(#${fillId})`}
          isAnimationActive={false}
        />
      </AreaChart>
    </ResponsiveContainer>
  );
}

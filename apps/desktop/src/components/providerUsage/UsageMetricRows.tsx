// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ArrowRight, Coins, Gauge, TrendingDown, TrendingUp } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import type { ProviderUsagePayload } from '../../lib/ipc';
import { paceTrend, usageMetricRows } from '../../lib/presentation/providerUsage';

/**
 * The shared metric block: today's spend, today's tokens, and the pace trend
 * against the trailing week — every figure derived from the reader's own
 * sessions. No percentage, allowance, reset, or runway appears here by
 * policy: a transcript records what was spent, never what remains.
 */
export function UsageMetricRows({ provider }: { provider: ProviderUsagePayload }) {
  const trend = paceTrend(provider);
  const trendIcon: LucideIcon =
    trend.kind === 'picking-up'
      ? TrendingUp
      : trend.kind === 'easing'
        ? TrendingDown
        : ArrowRight;
  const icons: Record<string, LucideIcon> = {
    'today-spend': Gauge,
    'today-tokens': Coins,
    trend: trendIcon,
  };

  return (
    <dl className="space-y-1.5">
      {usageMetricRows(provider).map((row) => {
        const Icon = icons[row.key] ?? Gauge;
        return (
          <div key={row.key} className="flex items-baseline justify-between gap-3">
            <dt className="flex items-center gap-1.5 type-footnote text-label-secondary">
              <Icon size={12} strokeWidth={1.75} aria-hidden="true" className="shrink-0" />
              {row.label}
            </dt>
            <dd className="type-footnote tabular-nums text-label">{row.value}</dd>
          </div>
        );
      })}
    </dl>
  );
}

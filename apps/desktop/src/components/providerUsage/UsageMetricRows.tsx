// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ArrowRight, Coins, Gauge, TrendingDown, TrendingUp } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';

import type { ProviderUsagePayload } from '../../lib/ipc';
import { paceTrend, usageMetricRows } from '../../lib/presentation/providerUsage';
import { UsageStateBadge } from './ProviderUsagePrimitives';

/**
 * The shared metric block: today's spend, today's tokens, and the spend trend
 * against the trailing week — every figure derived from the reader's own
 * sessions. No percentage, allowance, reset, or runway appears here by
 * policy: a transcript records what was spent, never what remains.
 *
 * The block carries its own heading and its own evidence badge because a card
 * can now hold two halves with different provenance. The badge used to sit in
 * the card header, where it read as describing everything below it —
 * including the provider's own stated limits, which it has nothing to do
 * with. Sitting here, over exactly the rows it describes, it says what it
 * means.
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
    <div className="space-y-1.5">
      <div className="flex items-baseline justify-between gap-2">
        <h4 className="type-caption font-medium tracking-wide uppercase text-label-tertiary">
          On this machine
        </h4>
        <UsageStateBadge state={provider.state} />
      </div>
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
    </div>
  );
}

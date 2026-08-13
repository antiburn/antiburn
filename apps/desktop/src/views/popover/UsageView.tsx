// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronLeft } from 'lucide-react';
import { useState } from 'react';

import { ProviderGlyph, UsageStateBadge } from '../../components/providerUsage';
import { ScrollPane } from '../../components/ui/ScrollPane';
import { SegmentedControl } from '../../components/ui/SegmentedControl';
import type { ProviderUsageSummaryPayload } from '../../lib/ipc';
import {
  coverageNote,
  providersForWindow,
  providerWindow,
  sessionCountLabel,
  stalenessNote,
  totalForWindow,
  usageValueLabel,
  usageWindowLabel,
  USAGE_WINDOWS,
  windowTokens,
  type UsageWindowKey,
} from '../../lib/presentation/providerUsage';
import { formatCompact } from '../../lib/presentation/sessionAnalytics';

export interface UsageViewProps {
  summary: ProviderUsageSummaryPayload;
  onBack: () => void;
}

/**
 * Every provider antiburn can attribute local work to, for one window.
 *
 * The whole surface is derived from sessions already on this machine, and the
 * footnote says so in as many words. That sentence is not decoration: a
 * per-provider spend figure is exactly the shape of a bill, and a reader who
 * assumed it *was* one would be wrong twice over — it is an estimate at API
 * rates, and it only covers work this machine can see.
 */
export function UsageView({ summary, onBack }: UsageViewProps) {
  const [window, setWindow] = useState<UsageWindowKey>('today');

  const providers = providersForWindow(summary.providers, window);
  const total = totalForWindow(summary.providers, window);
  const coverage = coverageNote(summary.coverageSince, summary.retentionDays);

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-11 shrink-0 items-center gap-1 px-2">
        <button
          type="button"
          onClick={onBack}
          aria-label="Back to activity"
          className="inline-flex h-6 shrink-0 items-center rounded-control px-1 text-label-secondary hover:bg-surface-hover"
        >
          <ChevronLeft size={15} strokeWidth={2} aria-hidden="true" />
        </button>
        <h1 className="type-headline text-label">Provider usage</h1>
      </header>

      <div className="shrink-0 px-4 pb-3">
        <SegmentedControl
          options={USAGE_WINDOWS}
          value={window}
          onChange={setWindow}
          ariaLabel="Usage window"
          equalWidth
          animatedIndicator
          className="w-full"
        />

        {/* A named region rather than a bare div: the headline figure is the
            answer the view exists to give, and a reader navigating by landmark
            should be able to reach it without walking the provider list. */}
        <section
          aria-label="Usage total"
          className="mt-3 flex items-baseline justify-between gap-3"
        >
          <div className="min-w-0">
            <p className="type-caption text-label-tertiary">{usageWindowLabel(window)}</p>
            <p className="type-title-3 tabular-nums text-label">{usageValueLabel(total)}</p>
          </div>
          <p className="type-caption text-label-tertiary">
            {sessionCountLabel(total.sessionCount)} · {formatCompact(windowTokens(total))} tokens
          </p>
        </section>
      </div>

      <ScrollPane viewportClassName="px-2 pb-2">
        {providers.length === 0 ? (
          <p className="px-2 py-6 text-center type-footnote text-label-tertiary">
            No local evidence yet
          </p>
        ) : (
          <ul className="space-y-px">
            {providers.map((provider) => {
              const usage = providerWindow(provider, window);
              const stale = stalenessNote(provider);
              return (
                <li
                  key={provider.provider}
                  className="flex items-center gap-2.5 rounded-control px-2 py-2"
                >
                  <ProviderGlyph displayName={provider.displayName} size={20} />
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-1.5">
                      <span className="truncate type-footnote text-label">
                        {provider.displayName}
                      </span>
                      <UsageStateBadge state={provider.state} />
                    </div>
                    <p className="type-caption text-label-tertiary">
                      {sessionCountLabel(usage.sessionCount)}
                      {stale ? ` · ${stale}` : ''}
                    </p>
                  </div>
                  <div className="shrink-0 text-right">
                    <p className="type-footnote tabular-nums text-label">
                      {usageValueLabel(usage)}
                    </p>
                    <p className="type-caption tabular-nums text-label-tertiary">
                      {formatCompact(windowTokens(usage))} tokens
                    </p>
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </ScrollPane>

      <footer className="shrink-0 space-y-1 border-t border-separator px-4 py-2.5">
        <p className="type-caption text-label-tertiary">
          Local estimates, priced on this device from the sessions antiburn found here. Not a bill,
          and not your provider&rsquo;s own figure — work done on another machine is not counted.
        </p>
        <p className="type-caption text-label-tertiary">
          Each session counts in the window of its most recent activity.
          {coverage ? ` ${coverage}` : ''}
        </p>
      </footer>
    </div>
  );
}

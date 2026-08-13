// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronRight } from 'lucide-react';

import type { ProviderUsagePayload } from '../../lib/ipc';
import {
  providerWindow,
  stalenessNote,
  tokenRows,
  updatedNote,
  usageStateDescription,
} from '../../lib/presentation/providerUsage';
import { UsageMetricRows } from './UsageMetricRows';
import { UsageWindowRows } from './UsageWindowRows';
import { ProviderGlyph, UsageStateBadge } from './ProviderUsagePrimitives';

export interface ProviderUsageDetailProps {
  provider: ProviderUsagePayload;
  /** Id of the element naming this panel, for the caller's `aria-labelledby`. */
  headingId?: string;
  /** Rendered as an "All provider usage" affordance when supplied. */
  onViewAll?: (() => void) | undefined;
}

/**
 * One provider's three windows, its token split, and what kind of evidence
 * produced them.
 *
 * Everything on this panel is a figure the reader's own sessions produced.
 * There is no meter and no remaining balance, because a transcript records
 * spend and not allowance — the state line at the bottom says exactly that
 * rather than leaving the reader to assume a missing bar is a loading state.
 */
export function ProviderUsageDetail({
  provider,
  headingId,
  onViewAll,
}: ProviderUsageDetailProps) {
  const stale = stalenessNote(provider);
  const updated = updatedNote(provider);
  // The broadest window the app can see, so the token split describes as much
  // history as exists rather than only the current day.
  const month = providerWindow(provider, 'month');

  return (
    <div className="space-y-3">
      <div className="flex items-start gap-2">
        {/* The same glyph the chip carries, so the panel reads as that chip
            opened rather than as an unrelated card. */}
        <ProviderGlyph displayName={provider.displayName} size={18} className="mt-px" />
        <div className="min-w-0 flex-1">
          <h2 id={headingId} className="type-headline truncate text-label">
            {provider.displayName}
          </h2>
          {(stale ?? updated) && (
            <p
              className={`type-caption ${stale ? 'text-system-orange' : 'text-label-tertiary'}`}
            >
              {stale ?? updated}
            </p>
          )}
        </div>
        <UsageStateBadge state={provider.state} className="mt-px" />
      </div>

      <UsageMetricRows provider={provider} />

      <UsageWindowRows provider={provider} className="border-t border-separator pt-2.5" />

      <div className="space-y-1 border-t border-separator pt-2.5">
        <p className="type-caption font-medium text-label-secondary">Tokens · this month</p>
        {tokenRows(month).map((row) => (
          <div key={row.label} className="flex items-baseline justify-between gap-3">
            <span className="type-caption text-label-tertiary">{row.label}</span>
            <span className="type-caption tabular-nums text-label-secondary">{row.value}</span>
          </div>
        ))}
      </div>

      <p className="type-caption text-label-tertiary">
        {usageStateDescription(provider.state)}
      </p>

      {onViewAll && (
        <button
          type="button"
          onClick={onViewAll}
          className="group -mx-1 flex w-[calc(100%+0.5rem)] items-center justify-between rounded-control px-1 py-1 type-footnote text-label hover:bg-surface-hover"
        >
          <span>All provider usage</span>
          <ChevronRight
            size={13}
            aria-hidden="true"
            className="text-label-tertiary group-hover:text-label-secondary"
          />
        </button>
      )}
    </div>
  );
}

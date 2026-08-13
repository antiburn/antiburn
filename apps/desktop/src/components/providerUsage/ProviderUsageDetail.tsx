// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronRight } from 'lucide-react';

import type { ProviderUsagePayload } from '../../lib/ipc';
import {
  providerWindow,
  sessionCountLabel,
  stalenessNote,
  tokenRows,
  usageStateDescription,
  usageValueLabel,
  usageWindowLabel,
  USAGE_WINDOWS,
} from '../../lib/presentation/providerUsage';
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
          {stale && <p className="type-caption text-label-tertiary">{stale}</p>}
        </div>
        <UsageStateBadge state={provider.state} className="mt-px" />
      </div>

      <dl className="space-y-1.5">
        {USAGE_WINDOWS.map(({ value }) => {
          const window = providerWindow(provider, value);
          return (
            <div key={value} className="flex items-baseline justify-between gap-3">
              <dt className="type-footnote text-label-secondary">{usageWindowLabel(value)}</dt>
              <dd className="flex items-baseline gap-2">
                <span className="type-caption text-label-tertiary">
                  {sessionCountLabel(window.sessionCount)}
                </span>
                <span className="type-footnote tabular-nums text-label">
                  {usageValueLabel(window)}
                </span>
              </dd>
            </div>
          );
        })}
      </dl>

      <div className="space-y-1 border-t border-separator pt-2.5">
        <p className="type-caption font-medium text-label-secondary">Tokens · this month</p>
        {tokenRows(month).map((row) => (
          <div key={row.label} className="flex items-baseline justify-between gap-3">
            <span className="type-caption text-label-tertiary">{row.label}</span>
            <span className="type-caption tabular-nums text-label-secondary">{row.value}</span>
          </div>
        ))}
      </div>

      <p className="type-caption text-label-tertiary">{usageStateDescription(provider.state)}</p>

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

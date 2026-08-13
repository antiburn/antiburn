// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronLeft } from 'lucide-react';

import { ProviderGlyph, UsageStateBadge } from '../../components/providerUsage';
import { UsageMetricRows } from '../../components/providerUsage/UsageMetricRows';
import { UsageWindowRows } from '../../components/providerUsage/UsageWindowRows';
import { ScrollPane } from '../../components/ui/ScrollPane';
import type { ProviderUsagePayload, ProviderUsageSummaryPayload } from '../../lib/ipc';
import {
  coverageNote,
  providerWindow,
  rankByWindow,
  stalenessNote,
  updatedNote,
  usageStateDescription,
  windowHasEvidence,
} from '../../lib/presentation/providerUsage';

export interface UsageViewProps {
  summary: ProviderUsageSummaryPayload;
  onBack: () => void;
}

/** Providers split the way a reader scans them: current work first. */
function sectioned(providers: readonly ProviderUsagePayload[]): {
  recent: ProviderUsagePayload[];
  rest: ProviderUsagePayload[];
} {
  const ranked = rankByWindow(providers, 'month');
  const recent = ranked.filter(
    (provider) =>
      windowHasEvidence(providerWindow(provider, 'today')) || provider.staleness === 'fresh',
  );
  const rest = ranked.filter((provider) => !recent.includes(provider));
  return { recent, rest };
}

/**
 * Every provider antiburn can attribute local work to.
 *
 * Two sections — recently used, then everything else detected — with one card
 * per provider: its evidence state, the shared metric block, and its three
 * windows. The whole surface is derived from sessions already on this
 * machine, and the footnote says so in as many words. That sentence is not
 * decoration: a per-provider spend figure is exactly the shape of a bill, and
 * a reader who assumed it *was* one would be wrong twice over — it is an
 * estimate at API rates, and it only covers work this machine can see.
 */
export function UsageView({ summary, onBack }: UsageViewProps) {
  const { recent, rest } = sectioned(summary.providers);
  const coverage = coverageNote(summary.coverageSince, summary.retentionDays);
  const empty = recent.length === 0 && rest.length === 0;

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
        {/* Focused by the popover when this surface takes over, so a keyboard
            or screen-reader user lands in the view rather than on <body>. */}
        <h1 data-view-heading tabIndex={-1} className="type-headline text-label outline-none">
          Usage
        </h1>
      </header>

      <ScrollPane viewportClassName="px-3 pb-2">
        {empty ? (
          <p className="px-2 py-6 text-center type-footnote text-label-tertiary">
            No local evidence yet
          </p>
        ) : (
          <>
            <UsageSection title="Recently used" providers={recent} />
            <UsageSection title="All detected" providers={rest} />
          </>
        )}
      </ScrollPane>

      <footer className="shrink-0 space-y-1 border-t border-separator px-4 py-2.5">
        <p className="type-caption text-label-tertiary">
          Local estimates, priced on this device from the sessions antiburn found here. Not a
          bill, and not your provider&rsquo;s own figure — work done on another machine is not
          counted.
        </p>
        <p className="type-caption text-label-tertiary">
          Each session counts in the window of its most recent activity.
          {coverage ? ` ${coverage}` : ''}
        </p>
      </footer>
    </div>
  );
}

function UsageSection({
  title,
  providers,
}: {
  title: string;
  providers: readonly ProviderUsagePayload[];
}) {
  if (providers.length === 0) return null;
  return (
    <section aria-label={title} className="pt-2 first:pt-0">
      <h2 className="px-1 pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary">
        {title}
      </h2>
      <ul className="space-y-2">
        {providers.map((provider) => (
          <ProviderCard key={provider.provider} provider={provider} />
        ))}
      </ul>
    </section>
  );
}

function ProviderCard({ provider }: { provider: ProviderUsagePayload }) {
  const stale = stalenessNote(provider);
  const updated = updatedNote(provider);
  const usedToday = windowHasEvidence(providerWindow(provider, 'today'));

  return (
    <li className="space-y-2.5 rounded-control bg-surface-card px-3 py-2.5">
      <div className="flex items-start gap-2">
        <ProviderGlyph displayName={provider.displayName} size={18} className="mt-px" />
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-1.5">
            <h3 className="truncate type-footnote font-medium text-label">
              {provider.displayName}
            </h3>
            {usedToday && (
              <span className="shrink-0 rounded-full bg-system-green/15 px-1.5 py-px type-caption text-system-green">
                Used today
              </span>
            )}
          </div>
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

      <UsageWindowRows provider={provider} className="border-t border-separator pt-2" />

      <p className="type-caption text-label-tertiary">
        {usageStateDescription(provider.state)}
      </p>
    </li>
  );
}

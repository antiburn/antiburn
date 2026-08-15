// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { ChevronLeft } from 'lucide-react';

import { ProviderGlyph, UsageStateBadge } from '../../components/providerUsage';
import { LiveUsageDetail } from '../../components/providerUsage/LiveUsageDetail';
import { UsageMetricRows } from '../../components/providerUsage/UsageMetricRows';
import { UsageWindowRows } from '../../components/providerUsage/UsageWindowRows';
import { ScrollPane } from '../../components/ui/ScrollPane';
import type {
  LiveProviderUsagePayload,
  LiveUsageSummaryPayload,
  ProviderUsagePayload,
  ProviderUsageSummaryPayload,
} from '../../lib/ipc';
import { EMPTY_LIVE_USAGE } from '../../lib/ipc';
import { liveAuthNote, liveForProvider } from '../../lib/presentation/liveUsage';
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
  /** The provider's own limit figures, when a source could prove any. */
  live?: LiveUsageSummaryPayload;
  /**
   * The instant countdowns and elapsed markers are measured from. Defaults to
   * when the shell collected the snapshot, which is both pure — a render must
   * not read the clock — and more truthful than reading it here would be: the
   * countdown then agrees with the reading it sits under, instead of drifting
   * a little further from it on every re-render.
   */
  now?: number;
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
 * per provider. Each card carries up to two things, in this order and never
 * the other way round:
 *
 * 1. **The provider's own limits**, when a source could prove them: real
 *    meters against a real allowance, dated with when the provider said it.
 * 2. **What this machine spent**, always: an on-device estimate at API rates
 *    over three calendar windows, whose bars are shares of the reader's own
 *    month and not a meter against anything.
 *
 * The order is deliberate and so is the fact that the second half never
 * moves. A reader who connects a source should find the limits added above
 * what they already knew, and a reader whose source goes quiet should lose
 * the top half and keep the bottom — never a view that reshuffles because a
 * file on disk went stale. The two halves also travel over separate IPC
 * commands, so the estimate payload's "no percentage, no allowance, no reset"
 * guarantee survives this feature intact.
 */
export function UsageView({ summary, live = EMPTY_LIVE_USAGE, now, onBack }: UsageViewProps) {
  // `|| 0` rather than a fallback clock: with no snapshot there is no live
  // section to render, so nothing consumes this.
  const at = now ?? (Date.parse(live.generatedAt) || 0);
  const { recent, rest } = sectioned(summary.providers);
  const coverage = coverageNote(summary.coverageSince, summary.retentionDays);
  const empty = recent.length === 0 && rest.length === 0;
  const authNote = liveAuthNote(live);

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
        {authNote && (
          <p
            role="status"
            className="mb-2 rounded-control bg-system-orange/10 px-3 py-2 type-caption text-system-orange"
          >
            {authNote}
          </p>
        )}
        {empty ? (
          <p className="px-2 py-6 text-center type-footnote text-label-tertiary">
            No local evidence yet
          </p>
        ) : (
          <>
            <UsageSection title="Recently used" providers={recent} live={live} now={at} />
            <UsageSection title="All detected" providers={rest} live={live} now={at} />
          </>
        )}
      </ScrollPane>

      <footer className="shrink-0 space-y-1 border-t border-separator px-4 py-2.5">
        <p className="type-caption text-label-tertiary">
          Spend figures are local estimates, priced on this device from the sessions antiburn
          found here. Not a bill, and not your provider&rsquo;s own figure — work done on
          another machine is not counted.
        </p>
        <p className="type-caption text-label-tertiary">
          Plan limits are your provider&rsquo;s own figures, read from what your agent last
          cached on this machine. antiburn fetches nothing; a limit is only as current as the
          moment shown beside it.
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
  live,
  now,
}: {
  title: string;
  providers: readonly ProviderUsagePayload[];
  live: LiveUsageSummaryPayload;
  now: number;
}) {
  if (providers.length === 0) return null;
  return (
    <section aria-label={title} className="pt-2 first:pt-0">
      <h2 className="px-1 pb-1 type-caption font-medium tracking-wide uppercase text-label-tertiary">
        {title}
      </h2>
      <ul className="space-y-2">
        {providers.map((provider) => (
          <ProviderCard
            key={provider.provider}
            provider={provider}
            live={liveForProvider(live, provider.provider)}
            now={now}
          />
        ))}
      </ul>
    </section>
  );
}

function ProviderCard({
  provider,
  live,
  now,
}: {
  provider: ProviderUsagePayload;
  live: LiveProviderUsagePayload | null;
  now: number;
}) {
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

      {live && <LiveUsageDetail live={live} now={now} />}

      <UsageMetricRows provider={provider} />

      <UsageWindowRows provider={provider} className="border-t border-separator pt-2" />

      <p className="type-caption text-label-tertiary">
        {usageStateDescription(provider.state)}
      </p>
    </li>
  );
}

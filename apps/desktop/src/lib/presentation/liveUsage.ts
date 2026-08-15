// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Shaping and wording for provider-reported usage limits.
 *
 * The sibling module `providerUsage.ts` shapes the *estimate* surface, and its
 * whole job is to stop a spend figure reading as a budget. This module has the
 * opposite problem: these numbers really are a budget, so the work is stopping
 * them from reading as more current than they are.
 *
 * Two rules follow from that, and everything below is one of them:
 *
 * 1. **Say when it was true.** Every reading carries the moment the provider
 *    stated it, and a reading past its age budget says so in as many words
 *    rather than quietly ageing on screen.
 * 2. **Never fill a gap.** A window with no percentage renders as unknown, not
 *    as zero; a window with no reset says the reset is unavailable rather than
 *    guessing one from the window's name.
 *
 * Nothing here fetches, and nothing here can invent a number — the payload is
 * the provider's own statement or null.
 */

import type {
  LiveProviderUsagePayload,
  LiveUsageFreshness,
  LiveUsageSummaryPayload,
  LiveUsageSupport,
  LiveUsageWindowPayload,
} from '../ipc';
import { relativeTime } from './relativeTime';

/** The one-word capability label for a live reading. */
export function liveSupportLabel(support: LiveUsageSupport): string {
  switch (support) {
    case 'live':
      return 'Live';
    case 'estimated':
      return 'Estimated';
    case 'observed':
      return 'Activity only';
    default:
      return 'Unavailable';
  }
}

/** What that label means, in a sentence. */
export function liveSupportDescription(support: LiveUsageSupport): string {
  switch (support) {
    case 'live':
      return 'Your provider stated these figures.';
    case 'estimated':
      return 'Modelled on this device rather than stated by your provider.';
    case 'observed':
      return 'Activity is known, but no trustworthy allowance is.';
    default:
      return 'An account was found without any usage figures.';
  }
}

/**
 * The full name of one window.
 *
 * A model-scoped weekly limit is named after its model, because that is the
 * only thing distinguishing it from the account-wide weekly limit sitting
 * directly above it.
 */
export function liveWindowLabel(window: LiveUsageWindowPayload): string {
  if (window.scopeModel) return `${window.scopeModel} weekly limit`;
  if (window.role === 'primaryShort') return '5-hour limit';
  if (window.role === 'primaryLong') return 'Weekly limit';
  if (window.kind === 'daily') return 'Daily limit';
  if (window.kind === 'monthly' || window.kind === 'billingCycle') return 'Monthly limit';
  return 'Usage limit';
}

/**
 * `"81% used"`, or that we do not know.
 *
 * Deliberately not `"0% used"` when the figure is missing: an empty meter is a
 * claim, and this one would be a claim nobody made.
 */
export function liveWindowValueLabel(window: LiveUsageWindowPayload): string {
  if (window.usedPercent == null) return 'Unknown';
  return `${Math.round(window.usedPercent)}% used`;
}

/**
 * The length of a window whose own name states it, in milliseconds.
 *
 * This is arithmetic, not inference. A window identified as `five-hour` has a
 * five-hour period by definition, so its start is five hours before its reset
 * — that is not a fact we are missing, it is one the id already gave us.
 * Anything whose duration is *not* stated by its identity is absent from this
 * table on purpose: a weekly window's boundary is the provider's own, and
 * "seven days before the reset" would be a guess dressed as a measurement.
 */
const IMPLIED_PERIOD_MS: Readonly<Record<string, number>> = {
  'five-hour': 5 * 3_600_000,
};

/**
 * How far into the window's own period the clock has travelled, 0–1, or null
 * when it cannot be known.
 *
 * This is what the marker on each bar means, and it is the single most useful
 * thing on the surface: 60% used at 30% elapsed and 60% used at 90% elapsed
 * are opposite situations, and the percentage alone cannot tell them apart.
 *
 * It needs both ends of the window. The provider states the reset but usually
 * not the start, so the start comes from the window's own stated duration
 * where it has one — see `IMPLIED_PERIOD_MS`. A window with neither a stated
 * start nor an implied duration gets no marker at all, rather than one drawn
 * from an assumed period.
 */
export function liveWindowElapsed(window: LiveUsageWindowPayload, now: number): number | null {
  if (!window.resetsAt) return null;
  const end = new Date(window.resetsAt).getTime();
  if (Number.isNaN(end)) return null;

  const stated = window.startsAt ? new Date(window.startsAt).getTime() : Number.NaN;
  const implied = IMPLIED_PERIOD_MS[window.id];
  const start = Number.isNaN(stated) ? (implied == null ? Number.NaN : end - implied) : stated;
  if (Number.isNaN(start) || end <= start) return null;

  return Math.min(1, Math.max(0, (now - start) / (end - start)));
}

/**
 * `"Resets in 3h 12m"` for something imminent, `"Resets Tue 15:00"` further
 * out, and an honest admission when the provider did not say.
 *
 * The switch is at a day because "resets in 76h" is not a sentence anyone
 * reads as a time, and a weekday and clock time is.
 */
export function liveResetLabel(window: LiveUsageWindowPayload, now: number): string {
  if (!window.resetsAt) return 'Reset unavailable';
  const at = new Date(window.resetsAt).getTime();
  if (Number.isNaN(at)) return 'Reset unavailable';
  const remaining = at - now;
  // Already past, and the provider has not published the next one yet. Not an
  // error: a rolling window resets on the provider's clock, not ours.
  if (remaining <= 0) return 'Reset pending';
  if (remaining < 86_400_000) {
    const hours = Math.floor(remaining / 3_600_000);
    const minutes = Math.floor((remaining % 3_600_000) / 60_000);
    return hours > 0 ? `Resets in ${hours}h ${minutes}m` : `Resets in ${minutes}m`;
  }
  const date = new Date(at);
  const day = date.toLocaleDateString(undefined, { weekday: 'short' });
  const time = date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  return `Resets ${day} ${time}`;
}

/**
 * The provenance line: what kind of reading this is, and how old.
 *
 * One line rather than two because they are one thought — a live figure from
 * four hours ago and a modelled figure from just now are both worth doubting,
 * for different reasons, and the reader should weigh them together.
 */
export function liveSourceNote(provider: LiveProviderUsagePayload): string {
  const support = liveSupportLabel(provider.support);
  if (!provider.observedAt) return support;
  return `${support} · stated ${relativeTime(provider.observedAt)}`;
}

/**
 * Tailwind classes for the provenance line. Orange only once a reading has
 * gone stale — a fresh reading is not news.
 */
export function liveFreshnessToneClass(freshness: LiveUsageFreshness): string {
  return freshness === 'stale' ? 'text-system-orange' : 'text-label-tertiary';
}

/**
 * The one sentence a stale reading needs, or null when it is current.
 *
 * Phrased as what the app can and cannot see, not as a fault: an agent that
 * has not run since Tuesday has a Tuesday answer, and nothing is wrong.
 */
export function liveStalenessNote(provider: LiveProviderUsagePayload): string | null {
  if (provider.freshness !== 'stale') return null;
  return `These figures are from ${relativeTime(provider.observedAt)} and may have moved since.`;
}

/** Windows worth rendering, primary ones first. */
export function liveWindows(provider: LiveProviderUsagePayload): LiveUsageWindowPayload[] {
  const rank = (window: LiveUsageWindowPayload) => {
    if (window.role === 'primaryShort') return 0;
    if (window.role === 'primaryLong') return 1;
    return 2;
  };
  // A stable sort over a copy, so the provider's own order breaks ties and two
  // supplemental windows never swap places between renders.
  return [...provider.windows].sort((a, b) => rank(a) - rank(b));
}

/** The live reading for one provider id, or null when there is none. */
export function liveForProvider(
  summary: LiveUsageSummaryPayload,
  provider: string,
): LiveProviderUsagePayload | null {
  return summary.providers.find((entry) => entry.provider === provider) ?? null;
}

/**
 * The banner one failed source deserves, or null.
 *
 * Only `authentication` earns a banner: it is the one category with an action
 * behind it. A rate limit passes on its own, an unreadable file is usually a
 * missing agent, and neither is worth interrupting someone over.
 */
export function liveAuthNote(summary: LiveUsageSummaryPayload): string | null {
  const failed = summary.errors.some((error) => error.category === 'authentication');
  if (!failed) return null;
  return 'Your agent could not read your plan usage. Sign in again there, then reopen this view.';
}

/**
 * Metered spend beyond the allowance, as a row — or null when the provider
 * reports none, or reports it switched off.
 *
 * Off is deliberately silent rather than a row saying zero: a control the
 * reader has already turned off does not need reporting back to them.
 */
export function liveExtraUsageLabel(provider: LiveProviderUsagePayload): string | null {
  const extra = provider.extraUsage;
  if (!extra?.enabled) return null;
  if (extra.usedPercent != null) return `${Math.round(extra.usedPercent)}% of extra usage`;
  if (extra.used != null && extra.currency) {
    return `${extra.used.toFixed(2)} ${extra.currency} of extra usage`;
  }
  return 'Extra usage is on';
}

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

/**
 * Whether a window is a supplemental, model-scoped limit — the kind that
 * sits at 0% for most readers because it tracks a model they never touch.
 *
 * The account-wide primary windows (session, weekly-all-models) are never
 * conditional: they describe the account's overall standing and belong on
 * screen unconditionally, whatever they read.
 */
export function isConditionallyVisibleUsageWindow(window: LiveUsageWindowPayload): boolean {
  return window.role === 'supplemental' && window.scopeModel != null;
}

/**
 * Whether a window belongs on screen at all.
 *
 * A supplemental per-model weekly limit is noise beside the limits a reader
 * actually hits until they touch that model, so it stays hidden until then.
 * Once it shows non-zero usage — this reading or an earlier one in the same
 * allowance period — it stays visible for the rest of that period, even past
 * a reading that comes back with no percentage at all: a window that was
 * genuinely in use must never look like it vanished.
 */
export function isUsageWindowVisible(window: LiveUsageWindowPayload): boolean {
  return (
    !isConditionallyVisibleUsageWindow(window) ||
    (window.usedPercent ?? 0) > 0 ||
    window.hasNonzeroUsageInCurrentPeriod
  );
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
  return provider.windows.filter(isUsageWindowVisible).sort((a, b) => rank(a) - rank(b));
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

/* -------------------------------------------------------------------------
 * Derived metrics: what a window's history says about it.
 *
 * Every function below has an explicit "we cannot say" branch, and it fires
 * far more often than the others. A source that only updates when an agent
 * runs produces a sparse series by construction, so "not enough history" is
 * the resting state rather than an error — and it must never be phrased, or
 * coloured, as one.
 * ---------------------------------------------------------------------- */

/** How a window's pace reads against the pace its allowance can afford. */
export type PaceState = 'comfortable' | 'onPace' | 'runningHot' | 'atRisk';

/** Where each band starts. Below 0.8 is comfortable; above 1.5 is at risk. */
const PACE_BANDS: ReadonlyArray<{ below: number; state: PaceState }> = [
  { below: 0.8, state: 'comfortable' },
  { below: 1.1, state: 'onPace' },
  { below: 1.5, state: 'runningHot' },
];

/**
 * Which band a pace ratio falls in.
 *
 * The bands are asymmetric on purpose. 1.0 is exactly on track, so the "on
 * pace" band reaches further above it than below: a reader slightly ahead of
 * their allowance is fine, and telling them otherwise every time they have a
 * busy half hour is how a useful signal becomes one people stop reading.
 */
export function paceState(ratio: number): PaceState {
  return PACE_BANDS.find((band) => ratio < band.below)?.state ?? 'atRisk';
}

/** The word for a band. */
export function paceStateLabel(state: PaceState): string {
  switch (state) {
    case 'comfortable':
      return 'Comfortable';
    case 'onPace':
      return 'On pace';
    case 'runningHot':
      return 'Running hot';
    default:
      return 'At risk';
  }
}

/** Tailwind colour for a band. Green through red, in that order. */
export function paceStateToneClass(state: PaceState): string {
  switch (state) {
    case 'comfortable':
      return 'text-system-green';
    case 'onPace':
      return 'text-label-secondary';
    case 'runningHot':
      return 'text-system-orange';
    default:
      return 'text-system-red';
  }
}

/** Below this a trend reads as easing; above its mirror, as picking up. */
const TREND_EASING = 0.85;
const TREND_PICKING_UP = 1.15;

/** `"Picking up"` / `"Steady"` / `"Easing"`. */
export function trendLabel(ratio: number): string {
  if (ratio < TREND_EASING) return 'Easing';
  if (ratio > TREND_PICKING_UP) return 'Picking up';
  return 'Steady';
}

/**
 * Why a window has no derived figures, in a sentence — or null when it has
 * them.
 *
 * Each reason gets its own wording because each has a different implication
 * for the reader. "Not enough history" means come back later; "just reset"
 * means the numbers are fine and simply too new; "out of date" means go and
 * use the agent.
 */
export function forecastUnavailableNote(window: LiveUsageWindowPayload): string | null {
  switch (window.forecast.unavailableReason) {
    case 'sparseHistory':
      return 'Not enough history';
    case 'transition':
      return 'Just reset';
    case 'stale':
      return 'Reading is out of date';
    default:
      return null;
  }
}

/** One derived row: a label, a value, and how alarming the value is. */
export interface LiveMetricRow {
  key: string;
  label: string;
  value: string;
  toneClass: string;
}

/**
 * The derived rows for one window, in a fixed order.
 *
 * Fixed because they answer a sequence of questions — how fast, faster than
 * before?, will it last, how much was today — and a reader who learns where
 * the runway sits should find it in the same place next time. A row whose
 * figure is unavailable still appears, carrying the reason: a row that
 * vanishes takes the question with it.
 */
export function liveMetricRows(window: LiveUsageWindowPayload, now: number): LiveMetricRow[] {
  const { forecast } = window;
  const unavailable = forecastUnavailableNote(window);
  const muted = 'text-label-tertiary';

  const pace: LiveMetricRow = {
    key: 'pace',
    label: 'Pace',
    value: unavailable ?? 'Not enough history',
    toneClass: muted,
  };
  if (forecast.paceRatio != null && forecast.consumptionRate != null) {
    const state = paceState(forecast.paceRatio);
    pace.value = `${paceStateLabel(state)} · ${forecast.paceRatio.toFixed(1)}× · ${forecast.consumptionRate.toFixed(
      1,
    )}%/hour`;
    pace.toneClass = paceStateToneClass(state);
  } else if (forecast.consumptionRate != null) {
    // A rate with no reset to measure it against: still worth showing, but it
    // is not a verdict, so it stays muted.
    pace.value = `${forecast.consumptionRate.toFixed(1)}%/hour`;
  }

  const trend: LiveMetricRow = {
    key: 'trend',
    label: 'Trend',
    value: forecast.paceTrend == null ? (unavailable ?? 'Not enough history') : '',
    toneClass: muted,
  };
  if (forecast.paceTrend != null) {
    trend.value = `${trendLabel(forecast.paceTrend)} · ${forecast.paceTrend.toFixed(1)}×`;
  }

  const runway: LiveMetricRow = {
    key: 'runway',
    label: 'Runway',
    value: unavailable ?? 'Not enough history',
    toneClass: muted,
  };
  const runwayNote = runwayLabel(window, now);
  if (runwayNote) {
    runway.value = runwayNote;
    runway.toneClass =
      forecast.paceRatio != null && forecast.paceRatio > 1 ? 'text-system-orange' : muted;
  }

  const rows = [pace, trend, runway];
  if (forecast.usedToday != null) {
    rows.push({
      key: 'today',
      label: 'Today',
      value: `${forecast.usedToday.toFixed(1)} points of this window`,
      toneClass: muted,
    });
  }
  return rows;
}

/**
 * `"Hits the limit Thu 15:00"`, or that it lasts — or null when there is no
 * forecast to say either from.
 *
 * A runway past the reset is reported as lasting rather than as a date,
 * because a limit that refills before you reach it is not a deadline and
 * printing one would invent an anxiety.
 */
export function runwayLabel(window: LiveUsageWindowPayload, now: number): string | null {
  const at = window.forecast.runwayAt ? Date.parse(window.forecast.runwayAt) : Number.NaN;
  if (Number.isNaN(at)) return null;

  const reset = window.resetsAt ? Date.parse(window.resetsAt) : Number.NaN;
  if (!Number.isNaN(reset) && at >= reset) return 'Lasts past the reset';
  if (at <= now) return 'At the limit';

  const date = new Date(at);
  const remaining = at - now;
  if (remaining < 86_400_000) {
    const hours = Math.floor(remaining / 3_600_000);
    const minutes = Math.floor((remaining % 3_600_000) / 60_000);
    return hours > 0 ? `Runs out in ${hours}h ${minutes}m` : `Runs out in ${minutes}m`;
  }
  const day = date.toLocaleDateString(undefined, { weekday: 'short' });
  const time = date.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  return `Runs out ${day} ${time}`;
}

/**
 * The window a compact surface should lead with.
 *
 * By *role*, not by how full it is. The account-wide long window — the weekly
 * limit — is the one that describes the account's overall standing, so it
 * leads whenever it exists.
 *
 * The tempting alternative is "whichever is nearest its ceiling", and it is
 * wrong. A per-model weekly limit at 95% next to an account weekly at 69%
 * would take the ring, and a reader glancing at the footer would read 95% as
 * their overall position when it constrains one model. A single ring has to
 * answer "how am I doing", and only the account-wide window answers that.
 *
 * The per-model limit is not hidden — it has its own row, with its own name,
 * one hover away.
 */
export function headlineWindow(
  provider: LiveProviderUsagePayload,
): LiveUsageWindowPayload | null {
  const windows = liveWindows(provider);
  const find = (match: (window: LiveUsageWindowPayload) => boolean) => windows.find(match);

  return (
    find((window) => window.role === 'primaryLong') ??
    find((window) => window.kind === 'weekly') ??
    find((window) => window.kind === 'billingCycle' && window.scopeModel == null) ??
    // Anything account-wide that is neither the short window nor a secondary
    // limit — a monthly allowance, say.
    find(
      (window) =>
        window.role !== 'primaryShort' &&
        window.role !== 'supplemental' &&
        window.usedPercent != null,
    ) ??
    // Last resort: the short window, which is better than an empty footer.
    find((window) => window.usedPercent != null) ??
    null
  );
}

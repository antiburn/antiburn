// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Shaping and wording for local provider usage.
 *
 * The shell sends values and facts; every label, ranking, and caveat on the
 * usage surfaces is decided here. That split matters more than usual for this
 * feature: the payload deliberately has no percentage, allowance, or reset to
 * render, so the *copy* is what stops a reader reading a spend figure as a
 * budget. Each label below therefore says what the number is, not how much of
 * something is left.
 *
 * Nothing here fetches, and nothing here can invent a denominator — there is
 * none in the input to invent one from.
 */

import type {
  ProviderUsagePayload,
  ProviderUsageState,
  ProviderUsageWindowPayload,
} from '../ipc';
import { relativeTime } from './relativeTime';
import { formatCompact, formatCost } from './sessionAnalytics';

/** Which window a surface is showing. */
export type UsageWindowKey = 'today' | 'week' | 'month';

/** The window selector's options, in the order they are offered. */
export const USAGE_WINDOWS: ReadonlyArray<{ value: UsageWindowKey; label: string }> = [
  { value: 'today', label: 'Today' },
  { value: 'week', label: 'Week' },
  { value: 'month', label: 'Month' },
];

/** The full name of a window, for a place with room for it. */
export function usageWindowLabel(key: UsageWindowKey): string {
  if (key === 'today') return 'Today';
  if (key === 'week') return 'Last 7 days';
  return 'This month';
}

/** An empty window, for a provider that has none of one. */
export const EMPTY_WINDOW: ProviderUsageWindowPayload = {
  tokensIn: 0,
  tokensOut: 0,
  cacheRead: 0,
  estimatedUsd: null,
  sessionCount: 0,
};

/** One provider's totals for a window. */
export function providerWindow(
  provider: ProviderUsagePayload,
  key: UsageWindowKey,
): ProviderUsageWindowPayload {
  return provider.windows[key] ?? EMPTY_WINDOW;
}

/** Every billable token in a window — input, output, and cache reads. */
export function windowTokens(window: ProviderUsageWindowPayload): number {
  return window.tokensIn + window.tokensOut + window.cacheRead;
}

/** True when a window has anything at all to show. */
export function windowHasEvidence(window: ProviderUsageWindowPayload): boolean {
  return windowTokens(window) > 0 || window.estimatedUsd != null;
}

/**
 * What a chip or a row leads with: the cost when the models could be priced,
 * the token count when they could not, and an em dash when there is neither.
 *
 * Never a zero-dollar figure standing in for "we could not price this" — an
 * unpriced provider says how many tokens it saw instead, which is true.
 */
export function usageValueLabel(window: ProviderUsageWindowPayload): string {
  if (window.estimatedUsd != null) return formatCost(window.estimatedUsd);
  const tokens = windowTokens(window);
  if (tokens > 0) return formatCompact(tokens);
  return '—';
}

/**
 * The one-word capability label for a state.
 *
 * `unknown` reads "No estimate" rather than "Unknown", because the thing that
 * is unknown is the *cost*, not the provider.
 */
export function usageStateLabel(state: ProviderUsageState): string {
  switch (state) {
    case 'live':
      return 'Live';
    case 'estimated':
      return 'Estimated';
    case 'observed':
      return 'Observed';
    case 'detected':
      return 'Detected';
    default:
      return 'No estimate';
  }
}

/** What that label means, in a sentence. Used as tooltip and footnote copy. */
export function usageStateDescription(state: ProviderUsageState): string {
  switch (state) {
    case 'live':
      return 'The provider reported this usage directly.';
    case 'estimated':
      return 'Priced on this device from the tokens your sessions recorded.';
    case 'observed':
      return 'Tokens were counted, but some models have no price, so the cost is a floor rather than a total.';
    case 'detected':
      return 'This provider was detected without any usage figures.';
    default:
      return 'Sessions were attributed to this provider, but none of them carries token evidence yet.';
  }
}

/** Tailwind classes for a state badge. Muted by design — a badge, not an alarm. */
export function usageStateToneClass(state: ProviderUsageState): string {
  if (state === 'estimated' || state === 'live') {
    return 'bg-system-gold/15 text-system-gold-text';
  }
  if (state === 'observed' || state === 'detected') {
    return 'bg-surface-secondary text-label-secondary';
  }
  return 'bg-surface-secondary text-label-tertiary';
}

/**
 * The note shown when a provider's newest session is old, or null when it is
 * not. Phrased as an observation, never as a warning: nothing is wrong with a
 * provider you have not used since Tuesday.
 */
export function stalenessNote(provider: ProviderUsagePayload): string | null {
  if (provider.staleness !== 'stale') return null;
  return `Last used ${relativeTime(provider.lastActivityAt)}`;
}

/**
 * The provider's initial, as the chip's glyph.
 *
 * A letter rather than a logo on purpose: provider artwork is licensed
 * separately from this application, so the app ships none of it.
 */
export function providerInitial(displayName: string): string {
  const first = Array.from(displayName.trim())[0];
  return (first ?? '?').toLocaleUpperCase();
}

/**
 * Providers ranked for one window — biggest first by cost, then by tokens.
 *
 * A stable sort over a copy: the shell's own order (most recently used first)
 * decides ties, so two idle providers never swap places between renders.
 */
export function rankByWindow(
  providers: readonly ProviderUsagePayload[],
  key: UsageWindowKey,
): ProviderUsagePayload[] {
  return [...providers].sort((a, b) => {
    const left = providerWindow(a, key);
    const right = providerWindow(b, key);
    const cost = (right.estimatedUsd ?? 0) - (left.estimatedUsd ?? 0);
    if (cost !== 0) return cost;
    return windowTokens(right) - windowTokens(left);
  });
}

/** Providers with something to show in a window, biggest first. */
export function providersForWindow(
  providers: readonly ProviderUsagePayload[],
  key: UsageWindowKey,
): ProviderUsagePayload[] {
  return rankByWindow(
    providers.filter((provider) => windowHasEvidence(providerWindow(provider, key))),
    key,
  );
}

/**
 * Every provider's window added together.
 *
 * `estimatedUsd` stays null until at least one provider contributed a priced
 * figure, so a total of `$0.00` always means "priced, and it came to nothing"
 * rather than "we could not price any of this".
 */
export function totalForWindow(
  providers: readonly ProviderUsagePayload[],
  key: UsageWindowKey,
): ProviderUsageWindowPayload {
  return providers.reduce<ProviderUsageWindowPayload>((total, provider) => {
    const window = providerWindow(provider, key);
    return {
      tokensIn: total.tokensIn + window.tokensIn,
      tokensOut: total.tokensOut + window.tokensOut,
      cacheRead: total.cacheRead + window.cacheRead,
      estimatedUsd:
        window.estimatedUsd == null
          ? total.estimatedUsd
          : (total.estimatedUsd ?? 0) + window.estimatedUsd,
      sessionCount: total.sessionCount + window.sessionCount,
    };
  }, EMPTY_WINDOW);
}

/** The token rows a detail panel lists, in a fixed order. */
export function tokenRows(
  window: ProviderUsageWindowPayload,
): ReadonlyArray<{ label: string; value: string }> {
  return [
    { label: 'Input', value: formatCompact(window.tokensIn) },
    { label: 'Output', value: formatCompact(window.tokensOut) },
    { label: 'Cache read', value: formatCompact(window.cacheRead) },
  ];
}

/** `"1 session"` / `"4 sessions"`. */
export function sessionCountLabel(count: number): string {
  return `${count} ${count === 1 ? 'session' : 'sessions'}`;
}

/* -------------------------------------------------------------------------
 * Pace: today against the trailing week, from the windows the shell already
 * reports. Deliberately daily-grained — a per-hour rate would need turn-level
 * evidence this build does not collect, and a made-up denominator is exactly
 * the kind of figure these surfaces refuse to show.
 * ---------------------------------------------------------------------- */

export type PaceTrendKind = 'picking-up' | 'steady' | 'easing' | 'insufficient';

export interface PaceTrend {
  kind: PaceTrendKind;
  /** Today ÷ the prior week's daily average; null when insufficient. */
  ratio: number | null;
}

/** Below this the trend reads "Easing"; above its inverse, "Picking up". */
const PACE_STEADY_BAND = 0.85;

/**
 * Today's activity against the trailing week's daily average, by tokens.
 *
 * The week window includes today, so the baseline is the other six days. A
 * baseline with no evidence — a fresh install, or a provider first used
 * today — is `insufficient` rather than an infinite ratio.
 */
export function paceTrend(provider: ProviderUsagePayload): PaceTrend {
  const today = windowTokens(providerWindow(provider, 'today'));
  const week = windowTokens(providerWindow(provider, 'week'));
  const prior = week - today;
  if (prior <= 0) return { kind: 'insufficient', ratio: null };
  const ratio = today / (prior / 6);
  if (ratio > 1 / PACE_STEADY_BAND) return { kind: 'picking-up', ratio };
  if (ratio < PACE_STEADY_BAND) return { kind: 'easing', ratio };
  return { kind: 'steady', ratio };
}

/**
 * `"Picking up · 1.4× the 7-day average"`, or why there is no trend.
 *
 * A ratio under a tenth is floored to `<0.1×` rather than rounded. Rounding
 * gives `0.0×`, which reads as a failed computation rather than as a quiet
 * day — and a reader who has spent 28 cents against a 17-dollar average has
 * not used *nothing*, they have used very little.
 */
export function paceTrendLabel(trend: PaceTrend): string {
  if (trend.kind === 'insufficient' || trend.ratio == null) return 'Not enough history';
  const word =
    trend.kind === 'picking-up' ? 'Picking up' : trend.kind === 'easing' ? 'Easing' : 'Steady';
  const ratio = trend.ratio < 0.1 ? '<0.1' : trend.ratio.toFixed(1);
  return `${word} · ${ratio}× the 7-day average`;
}

/** `"Updated 2h ago"`, or null without a timestamp. */
export function updatedNote(provider: ProviderUsagePayload): string | null {
  if (!provider.lastActivityAt) return null;
  return `Updated ${relativeTime(provider.lastActivityAt)}`;
}

/**
 * A window's share of the same provider's month, in [0, 1] — by estimated
 * cost when the month is priced, by tokens otherwise.
 *
 * This is what the window bars are ALLOWED to mean: one span of the reader's
 * own local spend against a longer span of it. It is deliberately not a meter
 * against any allowance — that figure does not exist on this machine.
 */
export function windowShareOfMonth(
  provider: ProviderUsagePayload,
  key: UsageWindowKey,
): number {
  const window = providerWindow(provider, key);
  const month = providerWindow(provider, 'month');
  const numerator =
    month.estimatedUsd != null && month.estimatedUsd > 0
      ? (window.estimatedUsd ?? 0)
      : windowTokens(window);
  const denominator =
    month.estimatedUsd != null && month.estimatedUsd > 0
      ? month.estimatedUsd
      : windowTokens(month);
  if (denominator <= 0) return 0;
  return Math.min(1, Math.max(0, numerator / denominator));
}

/** The metric rows the detail card and the Usage view share, in order. */
export function usageMetricRows(
  provider: ProviderUsagePayload,
): ReadonlyArray<{ key: string; label: string; value: string }> {
  const today = providerWindow(provider, 'today');
  return [
    {
      key: 'today-spend',
      label: "Today's spend",
      value: today.estimatedUsd != null ? formatCost(today.estimatedUsd) : '—',
    },
    {
      key: 'today-tokens',
      label: "Today's tokens",
      value: windowTokens(today) > 0 ? formatCompact(windowTokens(today)) : '—',
    },
    // "Spend trend", not "Pace trend": a provider card can carry the plan
    // limits above these rows, and those have their own Pace and Trend
    // computed from the provider's percentages. Two rows called pace, four
    // apart, measuring different things from different evidence, is a reader
    // being asked to hold a distinction the labels refuse to make.
    { key: 'trend', label: 'Spend trend', value: paceTrendLabel(paceTrend(provider)) },
  ];
}

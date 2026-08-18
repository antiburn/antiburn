// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { LiveUsageSummaryPayload } from './ipc';
import { liveWindowLabel, liveWindows } from './presentation/liveUsage';

/** One renderable usage bar: a limit window with a known percentage, for the
 * overlay window's LED bars. */
export type UsageBarItem = {
  key: string;
  label: string;
  percent: number;
  resetsAt: Date | null;
  /** LED fill color for this provider's bars. */
  color: string;
};

/* Per-provider LED colors: burn orange stays Anthropic's; OpenAI takes the
 * label color (their black/white mark — white in dark mode, black in light);
 * the rest get distinct vivid hues. Unknown providers fall back to burn. */
const PROVIDER_COLORS: Record<string, string> = {
  anthropic: 'var(--color-burn)',
  claude: 'var(--color-burn)',
  openai: 'var(--color-label)',
  cursor: 'var(--color-system-indigo)',
  google: 'var(--color-system-blue)',
  opencode_go: 'var(--color-system-green)',
};

export function providerBarColor(provider: string): string {
  return PROVIDER_COLORS[provider.toLowerCase()] ?? 'var(--color-burn)';
}

/** A reset instant as a Date, or null when the provider did not state one
 * (or stated something unparseable — never a bar with an invalid date). */
function resetDate(resetsAt: string | null): Date | null {
  if (!resetsAt) return null;
  const date = new Date(resetsAt);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** Flatten a live-usage snapshot into renderable bars, provider-prefixed
 * when more than one provider reports limits.
 *
 * The input is this app's `getLiveUsage()` payload rather than the source
 * app's probe types — the one forced rewrite in the port
 * (docs/plans/floating-hud-port.md). Window visibility and ordering come
 * from `liveWindows`, so the HUD and the popover always agree about which
 * limits are worth showing. */
export function deriveUsageBars(response: LiveUsageSummaryPayload | null): UsageBarItem[] {
  const withBars = (response?.providers ?? [])
    .map((provider) => ({
      provider,
      windows: liveWindows(provider).filter((window) => window.usedPercent != null),
    }))
    .filter((group) => group.windows.length > 0);

  const multiProvider = withBars.length > 1;

  return withBars.flatMap((group) =>
    group.windows.map((window) => ({
      key: `${group.provider.provider}-${window.id}`,
      label: multiProvider
        ? `${group.provider.displayName} · ${liveWindowLabel(window)}`
        : liveWindowLabel(window),
      percent: window.usedPercent!,
      resetsAt: resetDate(window.resetsAt),
      color: providerBarColor(group.provider.provider),
    })),
  );
}

/** How long the reset is away before a wall-clock time stops being the
 * clearer answer. Past this you would have to reason about which day. */
const CLOCK_HORIZON_MS = 12 * 3_600_000;

/** How long until the reset, as a bare duration: `45m`, `3h 38m`, `5d 2h`. */
function durationUntil(resetsAt: Date, now: number): string {
  const minutes = Math.ceil((resetsAt.getTime() - now) / 60_000);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    const rem = minutes % 60;
    return rem > 0 ? `${hours}h ${rem}m` : `${hours}h`;
  }
  const days = Math.floor(hours / 24);
  const remHours = hours % 24;
  return remHours > 0 ? `${days}d ${remHours}h` : `${days}d`;
}

/** When the reset lands, worded the way you would say it: a clock time when
 * that is within the next twelve hours ("resets 4:12pm"), a duration beyond
 * that ("resets in 5d 2h"), since a bare time is ambiguous once a day
 * boundary is involved. */
export function resetsLabel(resetsAt: Date | null, now: number): string {
  if (!resetsAt) return 'reset unknown';
  const ms = resetsAt.getTime() - now;
  if (ms <= 0) return 'resets soon';
  if (ms > CLOCK_HORIZON_MS) return `resets in ${durationUntil(resetsAt, now)}`;
  const clock = resetsAt
    .toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
    .replace(/\s+/g, '')
    .toLowerCase();
  return `resets ${clock}`;
}

export function resetsIn(resetsAt: Date | null, now: number): string {
  if (!resetsAt) return 'reset unknown';
  const ms = resetsAt.getTime() - now;
  if (ms <= 0) return 'resets soon';
  return `resets in ${durationUntil(resetsAt, now)}`;
}

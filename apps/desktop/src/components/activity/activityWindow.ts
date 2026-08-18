// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * The visible calendar window an activity list shows.
 *
 * Calendar days, not rolling 24-hour spans: "Yesterday" means the previous
 * calendar day in the reader's own timezone, which is what a day heading has
 * to mean for the list to make sense at 00:30.
 */

function localDayOrdinal(date: Date): number {
  return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / 86_400_000;
}

/**
 * True for real timestamps inside the inclusive local-calendar-day window.
 * Future and malformed timestamps are deliberately excluded: a clock-skewed
 * record would otherwise pin itself to the top of the list forever.
 */
export function isWithinActivityDays(
  timestamp: string,
  days: number,
  now = new Date(),
): boolean {
  const at = new Date(timestamp);
  if (!Number.isFinite(at.getTime()) || at.getTime() > now.getTime()) return false;
  const age = localDayOrdinal(now) - localDayOrdinal(at);
  return age >= 0 && age < days;
}

/**
 * How many calendar days ago a timestamp falls, or null when it is in the
 * future or unparseable.
 */
export function activityDayAge(timestamp: string, now = new Date()): number | null {
  const at = new Date(timestamp);
  if (!Number.isFinite(at.getTime()) || at.getTime() > now.getTime()) return null;
  const age = localDayOrdinal(now) - localDayOrdinal(at);
  return age >= 0 ? age : null;
}

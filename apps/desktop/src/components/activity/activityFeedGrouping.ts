// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { activityDayAge, isWithinActivityDays } from './activityWindow';

/**
 * The ordering and grouping half of an activity list, as pure functions over
 * anything that carries a timestamp and a stable key.
 *
 * Generic on purpose: nothing here knows what an item describes.
 */

/** The minimum an item must carry to take a place in the list. */
export interface ActivityFeedItem {
  /** ISO timestamp the item sorts and buckets by. */
  at: string;
  /** Stable React identity — never the timestamp, which moves under live rows. */
  key: string;
}

/** One calendar-day bucket, newest day first. */
export interface ActivityDayGroup<T> {
  label: string;
  items: T[];
}

/**
 * Parse a timestamp to epoch millis for sorting. An item whose real time is
 * unknown carries an empty or otherwise unparseable timestamp; treat those as
 * oldest so they sort to the bottom deterministically, instead of producing
 * NaN comparisons that scramble the order.
 */
function atMs(at: string): number {
  const ms = new Date(at).getTime();
  return Number.isNaN(ms) ? -Infinity : ms;
}

/** Newest first, without mutating the input. */
export function newestFirst<T extends ActivityFeedItem>(items: readonly T[]): T[] {
  return [...items].sort((a, b) => {
    const x = atMs(b.at);
    const y = atMs(a.at);
    return x < y ? -1 : x > y ? 1 : 0;
  });
}

/** How a day bucket names itself. */
export function activityDayLabel(age: number): string {
  return age === 0 ? 'Today' : age === 1 ? 'Yesterday' : `${age} days ago`;
}

/**
 * Bucket items into calendar days, newest day first and newest item first
 * within each day. Anything outside the visible window — and anything future
 * or malformed — is dropped, and an empty day produces no group at all.
 *
 * The caller filters by *kind* before calling; the two filters commute, so the
 * contract stays: visible calendar window, selected kind, calendar buckets,
 * newest first.
 */
export function groupActivityByDay<T extends ActivityFeedItem>(
  items: readonly T[],
  { days, now = new Date() }: { days: number; now?: Date },
): ActivityDayGroup<T>[] {
  const inRange = items.filter((item) => isWithinActivityDays(item.at, days, now));
  const groups: ActivityDayGroup<T>[] = [];
  for (let age = 0; age < days; age += 1) {
    const dayItems = inRange.filter((item) => activityDayAge(item.at, now) === age);
    if (dayItems.length === 0) continue;
    groups.push({ label: activityDayLabel(age), items: newestFirst(dayItems) });
  }
  return groups;
}

/** How many items the grouped list will actually render. */
export function countGroupedItems<T>(groups: readonly ActivityDayGroup<T>[]): number {
  return groups.reduce((count, group) => count + group.items.length, 0);
}

// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import type { LucideIcon } from 'lucide-react';

import { relativeTime } from '../../lib/presentation/relativeTime';

/**
 * The vocabulary every row in an activity list is built from: its class names,
 * its corner markers, and how a row time is spoken.
 *
 * Feature-blind on purpose. Nothing here knows what kind of thing a row
 * describes — a row that wants to mark a problem hands a {@link RowTimeStatus}
 * to the primitives, and deriving that status stays with whoever owns the
 * concept that can fail.
 */

/** Chrome shared by every row: the hover host the reveal rules key off. */
export const ROW_CLASS = 'activity-row';
export const ROW_TITLE_CLASS = 'activity-row-title';
export const ROW_CORNER_GUTTER_CLASS = 'activity-row-corner-gutter';
export const SECONDARY_DETAIL_REVEAL_CLASS = 'activity-row-secondary-detail';
/** Applied alongside {@link ROW_CLASS} while a row's subject is still running. */
export const ROW_ACTIVE_CLASS = 'activity-row-active';
/** Applied to a title that should shimmer while its subject is running. */
export const ROW_TITLE_SHIMMER_CLASS = 'activity-row-title-shimmer';

/**
 * The row box itself: a three-column flex — glyph, content, absolute corner —
 * with the list's shared padding, minimum height, and radius.
 *
 * A constant rather than a wrapper component, because a row's root element
 * depends on whether it is interactive, and a polymorphic shell would cost more
 * than it saves.
 */
export const ROW_LAYOUT_CLASS =
  'relative flex items-start gap-3 py-2 px-2 min-h-[60px] w-full text-left rounded-md';

/**
 * The corner markers are attributes, not classes, because they carry no styles
 * of their own: they exist so the stylesheet can ask `:has()` whether this row
 * is painting a corner, and so tests can find one. A class that never styles
 * anything is a hook wearing the wrong clothes.
 */
export const ROW_CORNER_ATTR = 'data-row-corner';
export const ROW_CORNER_PINNED_ATTR = 'data-row-corner-pinned';

/**
 * How a row wants its time corner announced and marked. The corner renders
 * exactly what it is handed.
 */
export interface RowTimeStatus {
  /** What the timestamp means — "Last activity", "Edited", … */
  label: string;
  /** Problem glyph, or null for the healthy case: success gets no tick. */
  icon?: LucideIcon | null;
  /** Text tint for a problem; empty keeps the muted default. */
  tint?: string;
  /** Keep the corner visible at rest instead of revealing it on hover. */
  pinned?: boolean;
}

/**
 * What the corner *says*, as opposed to what it paints.
 *
 * The painted corner is decorative — tint alone would fail WCAG 1.4.1 and the
 * glyph is aria-hidden — so this carries the status and the time together. A
 * row hosts it one of two ways: a row with no `aria-label` computes its name
 * from its contents and can host the `sr-only` copy the corner renders, while a
 * row that *does* carry an `aria-label` replaces name-from-contents entirely
 * and must fold this string into that label instead, or it is never announced.
 */
export function rowTimeLabel(label: string, timestamp: string): string {
  return `${label} ${relativeTime(timestamp)}`;
}

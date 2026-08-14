// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * The settings window's section ids.
 *
 * They live here rather than in `SettingsView` because two other places name
 * one: the IPC wrapper that asks the shell to open a particular pane, and the
 * popover's attention banners that send a reader to it. A shared union means a
 * renamed pane is a type error at every call site instead of a link that
 * quietly lands nowhere.
 */
export const SETTINGS_PANE_IDS = [
  'general',
  'appearance',
  'sources',
  'privacy',
  'notifications',
  'about',
] as const;

export type SettingsPane = (typeof SETTINGS_PANE_IDS)[number];

/** Whether an arbitrary value — a shell payload, say — names a real pane. */
export function isSettingsPane(value: unknown): value is SettingsPane {
  return typeof value === 'string' && (SETTINGS_PANE_IDS as readonly string[]).includes(value);
}

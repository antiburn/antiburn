// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Applying the reader's theme choice.
 *
 * The token layer resolves light and dark entirely in CSS: the system
 * preference is the default, and an explicit `<html data-theme>` overrides it
 * (see `styles/tokens.css`). Choosing a theme is therefore one attribute write,
 * and choosing "system" is removing it — not writing `data-theme="system"`,
 * which no rule matches.
 *
 * Every window in the app runs the same bundle, so each applies the stored
 * choice on mount and there is nothing to broadcast between them.
 */

import type { ThemePreference } from './ipc';

/**
 * Write (or clear) the theme override on the document element.
 *
 * Safe to call before the DOM exists — server rendering and non-DOM tests
 * simply do nothing.
 */
export function applyTheme(theme: ThemePreference): void {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;
  if (theme === 'system') {
    delete root.dataset['theme'];
    return;
  }
  root.dataset['theme'] = theme;
}

/** The theme currently written on the document, or `system` when none is. */
export function currentTheme(): ThemePreference {
  if (typeof document === 'undefined') return 'system';
  const theme = document.documentElement.dataset['theme'];
  return theme === 'light' || theme === 'dark' ? theme : 'system';
}

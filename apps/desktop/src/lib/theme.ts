// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Reading the resolved theme from JavaScript.
 *
 * Styling never needs this — the token layer resolves light/dark entirely in
 * CSS. It exists for the handful of places that paint a color into a canvas or
 * an inline style and therefore cannot express "whichever the theme says", such
 * as the analytics chart overlays.
 *
 * Resolution mirrors the cascade in styles/tokens.css: an explicit
 * `<html data-theme>` override wins, and the system preference is the default.
 */

import { useSyncExternalStore } from 'react';

function darkQuery(): MediaQueryList | null {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return null;
  return window.matchMedia('(prefers-color-scheme: dark)');
}

/** True when the app is currently rendering in dark mode. */
function isDarkTheme(): boolean {
  if (typeof document !== 'undefined') {
    const theme = document.documentElement.dataset['theme'];
    if (theme === 'dark') return true;
    if (theme === 'light') return false;
  }
  return darkQuery()?.matches ?? false;
}

function subscribe(onChange: () => void): () => void {
  const query = darkQuery();
  query?.addEventListener('change', onChange);

  // The explicit override is an attribute on <html>, so watching the attribute
  // itself is the whole story — no custom event for a writer to remember to
  // fire, and no way for the two to drift apart.
  const observer =
    typeof MutationObserver === 'undefined' || typeof document === 'undefined'
      ? null
      : new MutationObserver(onChange);
  observer?.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['data-theme'],
  });

  return () => {
    query?.removeEventListener('change', onChange);
    observer?.disconnect();
  };
}

/** Subscribe to {@link isDarkTheme}. Reports `false` during server rendering. */
export function useDarkTheme(): boolean {
  return useSyncExternalStore(subscribe, isDarkTheme, () => false);
}

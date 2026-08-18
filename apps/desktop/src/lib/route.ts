// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

import { useSyncExternalStore } from 'react';

/**
 * The shell serves one bundle to every window and selects the view from the
 * URL fragment the window was opened with. There is no router: each Tauri
 * window owns exactly one route for its whole lifetime, and the fragment is
 * the only thing that distinguishes them.
 */
export type Route = 'popover' | 'settings' | 'nudge' | 'onboarding';

/** Fragment the Rust shell opens the settings window with. */
export const SETTINGS_FRAGMENT = '#/settings';

/** Fragment the nudge crate opens the notification window with. */
export const NUDGE_FRAGMENT = '#/nudge';

/**
 * Every fragment that names a window other than the popover. The popover is the
 * default rather than an entry, so an unknown fragment lands somewhere real
 * instead of rendering nothing.
 */
// A Map, not a plain object: the fragment is outside input, and an object
// index would resolve inherited names ("constructor") to functions rather
// than falling back to the popover.
const ROUTES = new Map<string, Route>([
  ['settings', 'settings'],
  ['nudge', 'nudge'],
  ['onboarding', 'onboarding'],
]);

export function routeFromHash(hash: string): Route {
  return ROUTES.get(hash.replace(/^#\/?/, '')) ?? 'popover';
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener('hashchange', onChange);
  return () => window.removeEventListener('hashchange', onChange);
}

function currentRoute(): Route {
  return routeFromHash(window.location.hash);
}

/** Reads the active route and re-renders if the fragment ever changes. */
export function useRoute(): Route {
  return useSyncExternalStore(subscribe, currentRoute, () => 'popover' as const);
}

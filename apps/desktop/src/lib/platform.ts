// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * Platform detection for the renderer.
 *
 * Styling never branches on this value in JavaScript. The platform is written
 * once to `<html data-platform>` and the few genuinely platform-specific rules
 * key off that attribute (see src/styles/platform-controls.css), which keeps
 * the branch in CSS where it can be read alongside the rule it guards.
 */

export type Platform = 'macos' | 'windows' | 'linux' | 'unknown';

/** Minimal shape of the User-Agent Client Hints API, where it exists. */
type PlatformHint = { platform?: string };

/**
 * Best-effort platform identity.
 *
 * Client Hints first because it reports a clean platform name; the User-Agent
 * string is the fallback for the webviews that do not implement them.
 */
export function detectPlatform(nav: Navigator = navigator): Platform {
  const hint = (nav as Navigator & { userAgentData?: PlatformHint }).userAgentData?.platform;
  const subject = (hint ?? nav.userAgent ?? '').toLowerCase();

  if (subject.includes('mac')) return 'macos';
  if (subject.includes('win')) return 'windows';
  if (subject.includes('linux') || subject.includes('x11')) return 'linux';
  return 'unknown';
}

/** True on macOS, for the rare case a component has to branch in TypeScript. */
export function isMacOS(): boolean {
  return detectPlatform() === 'macos';
}

/**
 * Publishes the platform to `<html data-platform>` so CSS can branch on it.
 * Call once, before the first render.
 */
export function applyPlatformAttribute(
  root: HTMLElement = document.documentElement,
  platform: Platform = detectPlatform(),
): void {
  root.setAttribute('data-platform', platform);
}

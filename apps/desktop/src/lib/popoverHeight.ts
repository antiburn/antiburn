// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * How tall the popover is for each of its surfaces.
 *
 * The window is 380px wide and never resizable, so height is the only degree of
 * freedom a view has. Four numbers rather than one fixed height: onboarding is
 * a short, centred flow that looks abandoned in a 700px window, while the
 * activity list, a session's analytics, and the usage breakdown all want every
 * pixel the contract allows.
 *
 * 700 is the ceiling the app-shell contract sets, and the shell clamps to it
 * again — these values are a request, not an instruction.
 */

/** Every surface the popover can be showing. */
export type PopoverSurface = 'onboarding' | 'activity' | 'session' | 'usage';

/** Ceiling shared with the shell (`popover::MAX_HEIGHT`). */
export const MAX_POPOVER_HEIGHT = 700;

/** Height, in logical pixels, of each surface. */
export const POPOVER_HEIGHTS: Record<PopoverSurface, number> = {
  // Three short centred screens and a source list. At full height the copy
  // floats in the middle of an empty window.
  onboarding: 520,
  activity: MAX_POPOVER_HEIGHT,
  session: MAX_POPOVER_HEIGHT,
  // The usage breakdown is a fixed header, a short list, and a footnote; it
  // runs out of content well before 700px.
  usage: 620,
};

/** The height a surface asks the shell for. */
export function popoverHeightFor(surface: PopoverSurface): number {
  return POPOVER_HEIGHTS[surface];
}

/**
 * Whether the reader has asked the system to reduce motion.
 *
 * Read here rather than in the shell: the preference is a webview media query,
 * and the same answer already drives every CSS transition in the app. Anything
 * without `matchMedia` (a test environment, an old webview) is treated as no
 * preference, which is the browser default.
 */
export function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false;
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

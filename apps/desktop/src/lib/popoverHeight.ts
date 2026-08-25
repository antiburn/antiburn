// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

/**
 * How tall the popover is for each of its surfaces.
 *
 * The window is 380px wide and never resizable, so height is the only degree of
 * freedom a view has. Three numbers rather than one fixed height: the activity
 * list and a session's analysis rest at the contract's height, and the usage
 * breakdown asks for more.
 *
 * There was a fourth, shorter than all of them, for the first-run flow. That
 * flow has its own window now (`src-tauri/src/onboarding.rs`, D-25) and is no
 * longer a popover surface at all.
 *
 * Two ceilings, not one, and the difference matters. [`DEFAULT_POPOVER_HEIGHT`]
 * is the app-shell contract's 700 — the size the window is created at and the
 * size it rests at. [`MAX_POPOVER_HEIGHT`] is 780, which only the Usage surface
 * asks for, and which exceeds the contract: that is a recorded deviation
 * (`docs/deviations.md`, D-22) rather than a quiet change. The shell clamps to
 * the same pair, so these values are a request, not an instruction.
 */

/** Every surface the popover can be showing. */
export type PopoverSurface = "activity" | "session" | "usage"

/**
 * Ceiling shared with the shell (`popover::MAX_HEIGHT`).
 *
 * Above the contract's 700. Only a surface that genuinely cannot do its job in
 * 700 should use it, and today exactly one does.
 */
export const MAX_POPOVER_HEIGHT = 780

/**
 * The contract's height, shared with the shell (`popover::DEFAULT_HEIGHT`) —
 * what the window is created at and rests at.
 */
export const DEFAULT_POPOVER_HEIGHT = 700

/** Height, in logical pixels, of each surface. */
export const POPOVER_HEIGHTS: Record<PopoverSurface, number> = {
  // Unchanged by the ceiling moving: nothing about these two got taller, and
  // growing them is a separate decision nobody has made.
  activity: DEFAULT_POPOVER_HEIGHT,
  session: DEFAULT_POPOVER_HEIGHT,
  // The one surface that outgrew the contract. It used to run out of content
  // before 700, back when a provider card was three windows and a token split.
  // A card now carries the provider's own limits above that — three rows,
  // three reset lines, and the pace block — so two vendors no longer fit, and
  // the reader ends up scrolling a surface whose whole point is one glance.
  usage: MAX_POPOVER_HEIGHT,
}

/** The height a surface asks the shell for. */
export function popoverHeightFor(surface: PopoverSurface): number {
  return POPOVER_HEIGHTS[surface]
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
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches
}

/**
 * The design system's slow duration (`--duration-slow`, `design.md`), in
 * milliseconds, for a JS-driven animation that cannot consume the CSS custom
 * property directly (a Recharts `animationDuration`, say).
 *
 * Read from the live custom property rather than a copied number, so the
 * token stays the one source of truth. Falls back to the token's documented
 * value where the property is unset, such as a test environment with no
 * stylesheet loaded.
 */
export function slowAnimationDurationMs(): number {
  const fallback = 300
  if (typeof window === "undefined" || typeof getComputedStyle !== "function") return fallback
  const raw = getComputedStyle(document.documentElement)
    .getPropertyValue("--duration-slow")
    .trim()
  const ms = raw.endsWith("ms")
    ? Number.parseFloat(raw)
    : raw.endsWith("s")
      ? Number.parseFloat(raw) * 1000
      : NaN
  return Number.isFinite(ms) ? ms : fallback
}

/**
 * How tall the popover is for each of its surfaces.
 *
 * The window is 380px wide and never resizable, so height is the only degree of
 * freedom a view has. The activity list and a session's analysis both use the
 * app-shell contract's height.
 *
 * There was a fourth, shorter than all of them, for the first-run flow. That
 * flow has its own window now (`src-tauri/src/onboarding.rs`) and is no
 * longer a popover surface at all.
 *
 * The Usage breakdown now lives only in its content-sized anchored companion.
 * The main popover therefore never grows beyond its 700px resting height.
 */

/** Every surface the popover can be showing. */
export type PopoverSurface = "activity" | "session"

/**
 * The contract's height, shared with the shell (`popover::DEFAULT_HEIGHT`) —
 * what the window is created at and rests at.
 */
export const DEFAULT_POPOVER_HEIGHT = 700

/** Height, in logical pixels, of each surface. */
export const POPOVER_HEIGHTS: Record<PopoverSurface, number> = {
  activity: DEFAULT_POPOVER_HEIGHT,
  session: DEFAULT_POPOVER_HEIGHT,
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

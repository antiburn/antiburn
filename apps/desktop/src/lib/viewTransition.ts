import { flushSync } from "react-dom"

/**
 * Run a state change inside a view transition, so elements that share a
 * `view-transition-name` before and after the change animate between their
 * two positions.
 *
 * `update` must notify React synchronously. `flushSync` makes React commit
 * the change before the browser takes the new snapshot. Without view
 * transition support, or when the reader asks for reduced motion, the change
 * applies at once.
 */
export function withViewTransition(update: () => void): void {
  if (
    typeof document === "undefined" ||
    typeof document.startViewTransition !== "function" ||
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  ) {
    update()
    return
  }
  document.startViewTransition(() => flushSync(update))
}

/*
 * Counts a figure up on screen, for the Enhance Done step. The node shows
 * the final value at once with reduced motion or without animation frames.
 */

const DURATION_MS = 1100

/** Counts `node` up from zero to `value` and returns a function that stops
 *  the count. `format` turns each step into text. */
export function countUp(
  node: HTMLElement,
  value: number,
  format: (value: number) => string,
): () => void {
  const reduced =
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  if (value <= 0 || reduced || typeof requestAnimationFrame !== "function") {
    node.textContent = format(value)
    return () => null
  }
  const start = performance.now()
  let frame = requestAnimationFrame(function step(now: number) {
    const t = Math.min((now - start) / DURATION_MS, 1)
    const eased = 1 - (1 - t) ** 3
    node.textContent = format(t < 1 ? value * eased : value)
    if (t < 1) frame = requestAnimationFrame(step)
  })
  return () => cancelAnimationFrame(frame)
}

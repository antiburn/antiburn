import { useState, type AnimationEventHandler } from "react"

/** The surfaces that have already drawn themselves in during this run. */
const drawnIn = new Set<string>()

/**
 * The entrance props for a surface that draws itself in the first time it
 * appears and looks finished every time after that. The Overview is a tab the
 * reader comes back to, and a reveal that replays on every visit stops reading
 * as an arrival and starts reading as a wait.
 *
 * Each surface keeps its own answer, so one drawing itself in does not cancel
 * another that has not appeared yet. `ready` is false while the surface still
 * stands in for its data, so the entrance belongs to the real thing rather
 * than to the placeholder.
 *
 * The key is recorded on `animationend`, not on render: a surface that never
 * plays the animation, such as an early empty-state return, must not consume
 * its one entrance and skip the reveal once real figures arrive.
 */
export function useEntranceProps(
  key: string,
  className: string,
  ready: boolean,
): { className?: string; onAnimationEnd?: AnimationEventHandler } {
  // Read once per mount. A later mount of the same surface finds the key
  // already there and renders without the class at all.
  const [first] = useState(() => !drawnIn.has(key))
  if (!first || !ready) return {}
  return {
    className,
    onAnimationEnd: (event) => {
      // A child element's own animation must not count as this entrance
      // finishing.
      if (event.target === event.currentTarget) drawnIn.add(key)
    },
  }
}

/** Forgets what has drawn in, so a test starts from a first run. */
export function resetEntrances(): void {
  drawnIn.clear()
}

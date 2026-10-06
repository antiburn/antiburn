import { useCallback, useRef, useState } from "react"

import { prefersReducedMotion, slowAnimationDurationMs } from "../../lib/popoverHeight"

function fmt(value: number): string {
  return value.toLocaleString()
}

/**
 * A whole number that counts through every value between its old and new
 * value when it changes. The count takes `--duration-slow` in total, but
 * never moves more than one step in one frame, so a large change takes
 * longer. A new value during a count continues from the number on screen.
 * The first render and reduced motion show the value immediately.
 *
 * Assistive tech reads the target value only, not the steps.
 */
export function CountUp({ value }: { value: number }) {
  const [shown, setShown] = useState(value)
  const shownRef = useRef(value)

  // React calls this ref again each time `value` changes, and calls the
  // returned cleanup first, so a new value stops the old count.
  const count = useCallback(
    (node: HTMLSpanElement | null) => {
      if (!node || shownRef.current === value) return
      if (prefersReducedMotion()) {
        shownRef.current = value
        setShown(value)
        return
      }
      const direction = Math.sign(value - shownRef.current)
      const stepMs = slowAnimationDurationMs() / Math.abs(value - shownRef.current)
      let last = performance.now()
      let frame = requestAnimationFrame(function tick(now) {
        if (now - last >= stepMs) {
          last = now
          shownRef.current += direction
          setShown(shownRef.current)
        }
        if (shownRef.current !== value) frame = requestAnimationFrame(tick)
      })
      return () => cancelAnimationFrame(frame)
    },
    [value],
  )

  return (
    <span ref={count}>
      <span className="sr-only" data-count-up-value="">
        {fmt(value)}
      </span>
      <span aria-hidden="true">{fmt(shown)}</span>
    </span>
  )
}

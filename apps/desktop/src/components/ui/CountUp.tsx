import { useCallback, useRef, useState } from "react"

import { prefersReducedMotion, slowAnimationDurationMs } from "../../lib/popoverHeight"

function fmt(value: number): string {
  return value.toLocaleString()
}

/**
 * A whole number that animates to its new value in `--duration-slow` when it
 * changes. A new value during a count continues from the number on screen.
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
      const start = shownRef.current
      let lastShown = start
      const startedAt = performance.now()
      const duration = slowAnimationDurationMs()
      let frame = requestAnimationFrame(function tick(now) {
        const progress = Math.min(1, (now - startedAt) / duration)
        shownRef.current = Math.round(start + (value - start) * progress)
        if (progress === 1) {
          shownRef.current = value
        }
        if (shownRef.current !== lastShown) {
          lastShown = shownRef.current
          setShown(shownRef.current)
        }
        if (progress < 1) frame = requestAnimationFrame(tick)
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

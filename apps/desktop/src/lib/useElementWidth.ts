import { useCallback, useSyncExternalStore, type RefObject } from "react"

function getServerSnapshot(): number {
  return 0
}

/** Shared plumbing for [[useElementWidth]] and [[useElementHeight]]: a
 *  `ResizeObserver` subscription plus a snapshot of one box property. Both
 *  exported hooks stay one-line wrappers over this, so each keeps its own
 *  name and a primitive number snapshot. `dimension` is a property name, not
 *  a closure, so it stays referentially stable across renders. */
function useElementDimension(
  ref: RefObject<HTMLElement | null>,
  dimension: "clientWidth" | "clientHeight",
): number {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const element = ref.current
      if (!element || typeof ResizeObserver === "undefined") return () => undefined
      const observer = new ResizeObserver(onChange)
      observer.observe(element)
      return () => observer.disconnect()
    },
    [ref],
  )
  const getSnapshot = useCallback(
    () => Math.round(ref.current?.[dimension] ?? 0),
    [ref, dimension],
  )
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
}

/**
 * The element's content width in whole pixels, or 0 before the first
 * measurement. `ResizeObserver` reports later width changes. The hook reads
 * the box again when `subscribe` attaches, so the first paint after mount
 * already carries the width.
 */
export function useElementWidth(ref: RefObject<HTMLElement | null>): number {
  return useElementDimension(ref, "clientWidth")
}

/** Same contract as [[useElementWidth]], for content height. Kept as a
 *  separate hook, not a `{width, height}` pair, so each snapshot stays a
 *  primitive number and `useSyncExternalStore` never re-renders a caller
 *  that reads only one dimension. */
export function useElementHeight(ref: RefObject<HTMLElement | null>): number {
  return useElementDimension(ref, "clientHeight")
}

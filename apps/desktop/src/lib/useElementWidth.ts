import { useCallback, useSyncExternalStore, type RefObject } from "react"

function getServerSnapshot(): number {
  return 0
}

/**
 * The element's content width in whole pixels, or 0 before the first
 * measurement. `ResizeObserver` reports later width changes. The hook reads
 * the box again when `subscribe` attaches, so the first paint after mount
 * already carries the width.
 */
export function useElementWidth(ref: RefObject<HTMLElement | null>): number {
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
  const getSnapshot = useCallback(() => Math.round(ref.current?.clientWidth ?? 0), [ref])
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
}
